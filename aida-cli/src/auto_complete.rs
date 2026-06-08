//! `aida queue work --auto-complete` — full lifecycle orchestrator.
//!
//! Collapses the implementer → CI → reviewer → merge → pull → build cycle
//! into one command. Where plain `aida queue work` `exec()`s `claude` and
//! replaces the aida process, the orchestrator stays alive as a parent: it
//! spawns each phase, waits, and advances. The phase-sequencing logic lives
//! here behind a [`PhaseDriver`] trait so it can be exercised end-to-end with
//! a mock; the real driver (subprocesses, CI polling, lease discovery) lives
//! in `main.rs`.
//!
//! See `docs/plans/2026-05-16-story-246-auto-complete.md`.
//! trace:STORY-246 | ai:claude

use std::time::Instant;

use colored::Colorize;

/// Which subset of the six phases an invocation runs.
/// trace:STORY-246 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutoCompleteVariant {
    /// All six phases (default — bare `--auto-complete`).
    Full,
    /// Stop after phase 2 — CI green, PR routed to the reviewer queue.
    ThroughCi,
    /// Stop after phase 4 — skip pull + build (batch the pulls later).
    ThroughMerge,
    /// Phases 1-5 — skip the build (it can happen lazily).
    SkipBuild,
}

impl AutoCompleteVariant {
    /// Parse the `--auto-complete[=MODE]` value. Bare `--auto-complete`
    /// arrives as `"full"` (clap `default_missing_value`).
    pub(crate) fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "full" => Ok(Self::Full),
            "through-ci" | "through_ci" | "throughci" => Ok(Self::ThroughCi),
            "through-merge" | "through_merge" | "throughmerge" => Ok(Self::ThroughMerge),
            "skip-build" | "skip_build" | "skipbuild" => Ok(Self::SkipBuild),
            other => Err(format!(
                "unknown --auto-complete mode `{other}` \
                 (expected: full, through-ci, through-merge, skip-build)"
            )),
        }
    }

    /// Highest phase index this variant runs through (1-6).
    pub(crate) fn last_phase(self) -> u8 {
        match self {
            Self::ThroughCi => 2,
            Self::ThroughMerge => 4,
            Self::SkipBuild => 5,
            Self::Full => 6,
        }
    }

    pub(crate) fn describe(self) -> &'static str {
        match self {
            Self::Full => "full pipeline",
            Self::ThroughCi => "through CI",
            Self::ThroughMerge => "through merge",
            Self::SkipBuild => "skip build",
        }
    }

    /// Stable machine slug for telemetry — the same spelling `parse`
    /// accepts. trace:TASK-266 | ai:claude
    pub(crate) fn slug(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::ThroughCi => "through-ci",
            Self::ThroughMerge => "through-merge",
            Self::SkipBuild => "skip-build",
        }
    }
}

/// Per-spec lifecycle short-circuit switches resolved from `lifecycle:*`
/// tags. These skip only non-integrity phases: CI wait, reviewer, and local
/// build. Merge and pull/auto-bump are deliberately not skippable.
/// trace:STORY-442 | ai:codex
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LifecycleSkip {
    pub(crate) no_ci_wait: bool,
    pub(crate) no_review: bool,
    pub(crate) no_build: bool,
}

impl LifecycleSkip {
    pub(crate) fn none() -> Self {
        Self::default()
    }

    pub(crate) fn from_tags<'a>(tags: impl IntoIterator<Item = &'a str>) -> Self {
        let mut skip = Self::none();
        for tag in tags {
            match tag.trim().to_ascii_lowercase().as_str() {
                "lifecycle:no-ci-wait" => skip.no_ci_wait = true,
                "lifecycle:no-review" => skip.no_review = true,
                "lifecycle:no-build" => skip.no_build = true,
                "lifecycle:trivial" => {
                    skip.no_ci_wait = true;
                    skip.no_review = true;
                    skip.no_build = true;
                }
                _ => {}
            }
        }
        skip
    }

    pub(crate) fn is_empty(self) -> bool {
        !self.no_ci_wait && !self.no_review && !self.no_build
    }

    /// TASK-525: the active short-circuit tokens (`no-ci-wait`, `no-review`,
    /// `no-build`) for telemetry — recorded on the auto-complete JSONL event so
    /// retro analysis can see how often fast-track tags fire and on what.
    /// Empty when no skip is active. trace:TASK-525 | ai:claude
    pub(crate) fn active_tokens(self) -> Vec<String> {
        let mut v = Vec::new();
        if self.no_ci_wait {
            v.push("no-ci-wait".to_string());
        }
        if self.no_review {
            v.push("no-review".to_string());
        }
        if self.no_build {
            v.push("no-build".to_string());
        }
        v
    }
}

/// The lifecycle short-circuit tags AIDA recognizes (STORY-442). Used both by
/// `LifecycleSkip::from_tags` and by the `aida edit`/`aida add` typo guard
/// (TASK-524) so a misspelled `lifecycle:*` tag is flagged instead of silently
/// no-op'ing. trace:TASK-524 | ai:claude
pub(crate) const RECOGNIZED_LIFECYCLE_TAGS: &[&str] = &[
    "lifecycle:no-ci-wait",
    "lifecycle:no-review",
    "lifecycle:no-build",
    "lifecycle:trivial",
];

/// TASK-524: true when `tag` is in the `lifecycle:` namespace but is NOT one of
/// the recognized short-circuit tags — i.e. a likely typo that would silently
/// have no effect. Case-insensitive, matching `from_tags`.
pub(crate) fn is_unrecognized_lifecycle_tag(tag: &str) -> bool {
    let t = tag.trim().to_ascii_lowercase();
    t.starts_with("lifecycle:") && !RECOGNIZED_LIFECYCLE_TAGS.contains(&t.as_str())
}

impl LifecycleSkip {
    pub(crate) fn banner_summary(self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        if self.no_ci_wait {
            parts.push("CI wait");
        }
        if self.no_review {
            parts.push("reviewer");
        }
        if self.no_build {
            parts.push("build");
        }
        Some(format!("skipping {}", parts.join(" + ")))
    }
}

/// Which phases an `--auto-complete --no-human` run drives headless.
///
/// `--no-human` makes the orchestrator launch a phase's Claude session with
/// `claude -p` (headless, single-turn, no Ctrl+D) instead of the interactive
/// `exec claude`. This first cut wires the *reviewer* (phase 3) only — the
/// SPIKE-7 "safe first cut". [`NoHumanMode::Both`] (the headless implementer
/// too) is the forward-compatible variant, but the headless implementer is
/// not shipped: `--no-human=both` is rejected at kickoff until STORY-276
/// wires the phase-1 headless launch. Bare `--no-human` therefore resolves
/// to [`ReviewerOnly`](Self::ReviewerOnly) — the honest default of what the
/// flag actually does today. trace:STORY-263, TASK-306 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoHumanMode {
    /// Headless reviewer (phase 3) only; the implementer (phase 1) stays
    /// interactive. Bare `--no-human` and `--no-human=reviewer-only` both
    /// resolve here. trace:TASK-306
    ReviewerOnly,
    /// Headless implementer + reviewer. The headless implementer is STORY-276
    /// — until it lands, `--no-human=both` is rejected at kickoff. The variant
    /// stays so the flag grammar is forward-compatible.
    Both,
}

impl NoHumanMode {
    /// Parse the `--no-human[=MODE]` value. Bare `--no-human` arrives as
    /// `"reviewer-only"` (clap `default_missing_value`); an empty string maps
    /// there too. trace:TASK-306 | ai:claude
    pub(crate) fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "reviewer-only" | "reviewer_only" | "revieweronly" | "reviewer" => {
                Ok(Self::ReviewerOnly)
            }
            "both" => Ok(Self::Both),
            other => Err(format!(
                "unknown --no-human mode `{other}` (expected: reviewer-only, both)"
            )),
        }
    }

    /// Stable slug — the spelling [`parse`](Self::parse) accepts, and the
    /// value the orchestrator propagates to phase children as
    /// `AIDA_NO_HUMAN_MODE` so the statusline can show the headless scope.
    /// trace:TASK-306 | ai:claude
    pub(crate) fn slug(self) -> &'static str {
        match self {
            Self::ReviewerOnly => "reviewer-only",
            Self::Both => "both",
        }
    }

    /// Does this mode *request* a headless implementer (phase 1)? True for
    /// [`Both`](Self::Both). The phase-1 headless launch is STORY-276; until
    /// it ships the kickoff gate reads this to reject `--no-human=both`.
    pub(crate) fn wants_headless_implementer(self) -> bool {
        matches!(self, Self::Both)
    }
}

/// How a `--no-human=both` drain handles an advisor *escalation* — when the
/// headless advisor judges a punted design-fork un-resolvable and kicks it
/// to a human (STORY-306).
///
/// `Blocks` is the default and the conservative choice: a confident-but-wrong
/// overnight default is worse than a paused spec, so the default escalate
/// behaviour is "pause, don't guess". `Defaults` exists for mechanical
/// batches where throughput beats per-spec correctness.
/// trace:STORY-306 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EscalateMode {
    /// Leave the spec parked (`NeedsAttention`), file the `needs-human`
    /// finding, advance the batch — this spec waits for a human. The default;
    /// the `--escalate-blocks` flag is its explicit spelling.
    Blocks,
    /// Resume the implementer told to proceed with its stated lean / the most
    /// defensible default, and file a `needs-human` finding for post-hoc
    /// review. The `--escalate-defaults` flag.
    Defaults,
}

impl EscalateMode {
    /// Resolve the mode from the `--escalate-defaults` flag — absent ⇒ the
    /// conservative [`Blocks`](Self::Blocks) default. (`--escalate-blocks` is
    /// the explicit spelling of that default; clap's `conflicts_with` keeps
    /// the two flags mutually exclusive, so a single bool resolves it.)
    /// trace:STORY-306 | ai:claude
    pub(crate) fn from_flags(escalate_defaults: bool) -> Self {
        if escalate_defaults {
            Self::Defaults
        } else {
            Self::Blocks
        }
    }
}

/// One lifecycle phase. The integer value doubles as the process exit code
/// for a failure in that phase (success is always 0).
/// trace:STORY-246 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Implementer,
    Ci,
    Reviewer,
    Merge,
    Pull,
    Build,
}

impl Phase {
    /// 1-based phase index — also the failure exit code.
    pub(crate) fn index(self) -> i32 {
        match self {
            Self::Implementer => 1,
            Self::Ci => 2,
            Self::Reviewer => 3,
            Self::Merge => 4,
            Self::Pull => 5,
            Self::Build => 6,
        }
    }

    /// Inverse of [`Phase::index`] — resolve a 1-based phase number back to
    /// its variant. Used by the `--auto-complete` telemetry views to name a
    /// logged failed-phase integer. trace:TASK-266 | ai:claude
    pub(crate) fn from_index(i: i32) -> Option<Self> {
        match i {
            1 => Some(Self::Implementer),
            2 => Some(Self::Ci),
            3 => Some(Self::Reviewer),
            4 => Some(Self::Merge),
            5 => Some(Self::Pull),
            6 => Some(Self::Build),
            _ => None,
        }
    }

    /// Stable machine slug for `--json` events and telemetry.
    pub(crate) fn slug(self) -> &'static str {
        match self {
            Self::Implementer => "implementer",
            Self::Ci => "ci",
            Self::Reviewer => "reviewer",
            Self::Merge => "merge",
            Self::Pull => "pull",
            Self::Build => "build",
        }
    }

    /// Human label for the progress lines.
    fn label(self) -> &'static str {
        match self {
            Self::Implementer => "implementer session",
            Self::Ci => "end + wait for CI",
            Self::Reviewer => "reviewer session",
            Self::Merge => "merge PR",
            Self::Pull => "pull + auto-bump",
            Self::Build => "build verify",
        }
    }
}

/// Why a phase failed — picks which recovery hint the user sees. The same
/// phase fails in several distinct ways (the orchestrator spawns subprocesses,
/// polls CI, reads verdict files), and a one-size-fits-all hint mis-routes the
/// user: a subprocess spawn ENOENT is *not* a red CI run, yet phase 2 reported
/// both as "CI is red". The driver tags each [`PhaseFailure`] with a kind so
/// [`recovery_hint`] can name the layer that actually broke.
/// trace:BUG-218 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureKind {
    /// A subprocess could not be spawned at all (ENOENT / EACCES / …). The
    /// orchestrator shells out in every phase, so this is cross-cutting — and
    /// it is a local-environment problem (PATH), never the work itself or CI.
    Spawn,
    /// A required external tool is absent from PATH (`gh`, `cargo`). A hard
    /// dependency the user must install — distinct from a transient `Spawn`.
    MissingTool,
    /// An orchestrator invariant was violated (a phase reached in a bad
    /// state). A bug in AIDA itself, not something the user caused.
    Internal,
    /// Phase 1: the implementer session exited cleanly but opened no PR.
    NoPr,
    /// Phase 2: CI ran to a terminal state and was red.
    CiRed,
    /// Phase 2: CI never reached a terminal state within the wait window.
    CiTimeout,
    /// Phase 3: the reviewer session wrote no verdict file, or an unreadable
    /// one — the review never produced a usable decision.
    NoVerdict,
    /// TASK-136: phase 1 ended *inconclusively* — the orchestrator could not
    /// confirm or deny a PR (a transient GH-API outage) even after the bounded
    /// `gh_verify_backoff_schedule` retry. In a *batch* drain this is shelved
    /// (parked for retry) rather than pausing the whole batch; the shelve
    /// carries this kind so triage knows the spec is retry-pending, not broken.
    /// trace:TASK-136 | ai:claude
    PrVerificationInconclusive,
    /// BUG-420: a headless phase tripped the no-progress / wall-clock-ceiling
    /// watchdog — it made no commit or file-change for the no-progress window
    /// (likely a degenerate echo/sleep spin) or exceeded the phase ceiling. The
    /// orchestrator killed the child and shelved the spec for a human to look
    /// at. trace:BUG-420 | ai:claude
    Watchdog,
    /// BUG-455: a phase failed because the SQLite cache (`.aida/cache.db`) was
    /// locked by another concurrent `aida` process (a sibling drain, an
    /// interactive shell, or a bulk op like a mass `archive`/`edit` sweep).
    /// The app-level cache retry loop (TASK-558) waits out most contention, but
    /// a lock held longer than the retry budget surfaces the failure here. This
    /// is transient *environment contention* — not a broken spec and not a
    /// broken install — so it is classified *shelvable*: a batch drain parks
    /// the spec and continues rather than hard-stopping the whole batch, the
    /// same way a transient GH-API blip ([`Self::PrVerificationInconclusive`])
    /// is shelved. trace:BUG-455 | ai:claude
    CacheLocked,
    /// The spawned work ran and reported failure — the phase-specific default.
    /// The hint points at the phase's normal "address it and retry" path.
    Failed,
}

impl FailureKind {
    /// Stable machine slug for `--json` failure events (failure-pattern
    /// telemetry — TASK-266 refines hints from these). trace:BUG-218 | ai:claude
    pub(crate) fn slug(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::MissingTool => "missing-tool",
            Self::Internal => "internal",
            Self::NoPr => "no-pr",
            Self::CiRed => "ci-red",
            Self::CiTimeout => "ci-timeout",
            Self::NoVerdict => "no-verdict",
            Self::PrVerificationInconclusive => "pr-verification-inconclusive",
            Self::Watchdog => "no-progress-watchdog",
            Self::CacheLocked => "cache-locked",
            Self::Failed => "failed",
        }
    }

    /// EPIC-28: should an `--auto-complete` batch drain shelve a spec
    /// on this failure kind, or stop the batch entirely?
    ///
    /// - **Shelvable** — the spec's work itself failed in a way a human
    ///   should look at (`NoPr`, `CiRed`, `CiTimeout`, `NoVerdict`,
    ///   `Failed`), or a *transient* condition that re-running clears
    ///   (`PrVerificationInconclusive`, `Watchdog`, and `CacheLocked` —
    ///   BUG-455's concurrent-`aida` SQLite-cache contention). The drain
    ///   parks the spec in `NeedsAttention` and continues to the next
    ///   batch member.
    /// - **Not shelvable** — the local environment is broken in a way
    ///   that has nothing to do with the spec: `Spawn` (no PATH for the
    ///   subprocess), `MissingTool` (no `gh` / `cargo`), `Internal`
    ///   (orchestrator bug). Parking innocent specs because the
    ///   environment is broken would be worse than stopping — every
    ///   future member would hit the same wall. Historical
    ///   `BatchDrainOutcome::Failed` stop applies.
    ///   trace:EPIC-28 | ai:claude
    pub(crate) fn is_shelvable(self) -> bool {
        matches!(
            self,
            Self::NoPr
                | Self::CiRed
                | Self::CiTimeout
                | Self::NoVerdict
                | Self::PrVerificationInconclusive
                | Self::Watchdog
                | Self::CacheLocked
                | Self::Failed
        )
    }
}

/// A reviewer's verdict, read from `.aida/review-verdicts/PR-N.json`.
/// trace:STORY-246 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    Approved,
    RequestChanges,
    Rejected,
}

impl Verdict {
    /// Parse the `verdict` field of a verdict file. Tolerant of casing and
    /// of hyphen/underscore/space spelling so a hand-written file still
    /// resolves.
    pub(crate) fn parse(s: &str) -> Option<Self> {
        let norm: String = s
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        match norm.as_str() {
            "approved" | "approve" | "lgtm" => Some(Self::Approved),
            "requestchanges" | "changesrequested" | "changes" => Some(Self::RequestChanges),
            "rejected" | "reject" => Some(Self::Rejected),
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Approved => "Approved",
            Self::RequestChanges => "RequestChanges",
            Self::Rejected => "Rejected",
        }
    }
}

/// A phase failed. `reason` is the one-line "what went wrong" shown to the
/// user; `kind` classifies it so the "what to do next" hint can name the
/// layer that broke. The hint is derived separately via [`recovery_hint`] so
/// it can be unit-tested independent of the driver.
/// trace:STORY-246 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct PhaseFailure {
    pub(crate) reason: String,
    pub(crate) kind: FailureKind,
}

impl PhaseFailure {
    /// A failure of the phase's default kind ([`FailureKind::Failed`]) — the
    /// spawned work ran and reported failure. The common case, so it keeps
    /// the terse constructor name.
    pub(crate) fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            kind: FailureKind::Failed,
        }
    }

    /// A failure of a specific [`FailureKind`] — used where the driver can
    /// tell a spawn ENOENT from a red CI run from an internal invariant, so
    /// [`recovery_hint`] routes the user to the right layer.
    /// trace:BUG-218 | ai:claude
    pub(crate) fn of(kind: FailureKind, reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            kind,
        }
    }

    /// BUG-455: upgrade a failure that is really transient SQLite cache-lock
    /// contention to [`FailureKind::CacheLocked`] (a *shelvable* kind) so a
    /// batch drain parks the spec and continues instead of hard-stopping.
    ///
    /// A concurrent `aida` process (sibling drain, interactive shell, bulk
    /// archive/edit sweep) can hold `.aida/cache.db`'s write lock past the
    /// app-level retry budget; whichever phase touched the cache then fails
    /// with a "database is locked" message. Without this, that failure
    /// inherits the phase's default kind ([`FailureKind::Failed`]) or — worse
    /// — an un-shelvable [`FailureKind::Internal`], and the whole batch stops
    /// over transient environment contention that re-running clears.
    ///
    /// Only the message text is consulted (via [`is_database_locked_message`]),
    /// not the original kind, so it catches the condition wherever a cache
    /// write surfaced it. Idempotent — a failure already classified
    /// `CacheLocked` is left as-is. trace:BUG-455 | ai:claude
    pub(crate) fn reclassify_transient(mut self) -> Self {
        if self.kind != FailureKind::CacheLocked && is_database_locked_message(&self.reason) {
            self.kind = FailureKind::CacheLocked;
        }
        self
    }
}

/// BUG-455: does a failure `reason` describe a SQLite cache-lock contention
/// (a transient "database is locked" / "database table is locked" / SQLITE_BUSY
/// surfaced through the cache layer)? Pure, case-insensitive, and fully
/// isolated so the classification rule is unit-testable without a driver.
/// trace:BUG-455 | ai:claude
pub(crate) fn is_database_locked_message(reason: &str) -> bool {
    let lower = reason.to_ascii_lowercase();
    lower.contains("database is locked") || lower.contains("database table is locked")
}

/// The verdict of the BUG-241 reconcile step: when a phase ends without the
/// artifact the orchestrator polls for (an open PR, a verdict file), did the
/// phase genuinely fail — or did the spec ship anyway, merged out-of-band by a
/// human or resolved by supersession so no code was ever needed? The
/// orchestrator asks the driver this *before declaring any phase a failure*,
/// so a phase that ended abnormally-but-successfully can never crash the batch
/// with a false "shipped 0". trace:BUG-241 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PhaseReconcile {
    /// Ground truth confirms the failure — nothing shipped. The phase failure
    /// stands and the orchestrator reports it exactly as before. This is the
    /// regression guard: the reconcile step only ever *ratifies* a real
    /// success, it never invents one.
    GenuineFailure,
    /// Ground truth shows the spec shipped despite the missing artifact.
    /// `reason` is the one-line evidence (e.g. `PR-94 is merged`) shown to the
    /// user. The orchestrator treats the run as a success and advances the
    /// batch.
    ShippedOutOfBand { reason: String },
}

/// Everything a recovery hint might need to name a concrete next command.
/// The driver fills in whatever it has discovered so far.
/// trace:STORY-246 | ai:claude
#[derive(Debug, Clone, Default)]
pub(crate) struct HintContext {
    pub(crate) spec: String,
    pub(crate) branch: Option<String>,
    pub(crate) pr_number: Option<u32>,
    pub(crate) implementer_session: Option<String>,
    pub(crate) ci_run_id: Option<String>,
    /// STORY-508/TASK-651: the project's active forge, so recovery hints name
    /// the right CLI (gh/glab) — or a forge-neutral phrasing for pure-git —
    /// when a drain phase fails. Set by the driver's `hint_context()`.
    pub(crate) forge: crate::forge::ForgeKind,
}

/// The outcome of phase 1 — the implementer session. The implementer either
/// opens a PR (the normal path), or, under a headless `--no-human=both`
/// drain, hits a design-fork it cannot safely resolve and invokes
/// `/aida-punt`, which parks the spec in `NeedsAttention`. A punt is neither a
/// failure (nothing broke) nor a ship (no PR), so it is a first-class third
/// outcome of phase 1: the orchestrator stops the pipeline cleanly and a
/// batch drain advances to the next member. trace:STORY-276 | ai:claude
///
/// BUG-257 adds a fourth outcome: *Inconclusive*. When the orchestrator can
/// neither confirm a PR nor confirm its absence — a transient GH-API
/// network error during the post-implementer PR lookup — the run halts
/// without ruling either way. The drain *pauses* (exit `0`, no
/// `failed_phase`) rather than crashing the batch with a false "no PR"
/// failure; the spec stays where it is, and the next drain retries.
/// trace:BUG-257 | ai:claude
///
/// BUG-266 extends the same outcome to the *Anthropic-API* leg: when the
/// headless implementer's `claude -p` subprocess exits non-zero because the
/// upstream model API was unreachable or overloaded (529 / 5xx / stream
/// timeout / upstream connect error), the run is inconclusive on the same
/// terms — the work the implementer did was real, the substrate just went
/// out from under it. Both legs share the `Inconclusive` variant and the
/// `finish_inconclusive` terminal path; only the `retry_hint` differs.
/// trace:BUG-266 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ImplementerOutcome {
    /// The implementer opened a PR — the pipeline continues to CI (phase 2).
    PrOpened,
    /// The implementer punted on a design-fork. `reason` is the one-line punt
    /// summary (category + detail) surfaced in the run epilogue. The punt is
    /// already durably recorded by `aida punt` (status flip + ledger).
    Punted { reason: String },
    /// BUG-257 / BUG-266: the orchestrator could not determine whether a PR
    /// was opened — either a transient GH-API network error during the
    /// post-implementer PR lookup (BUG-257) or a transient Anthropic-API
    /// outage that killed the headless implementer mid-session (BUG-266).
    /// `reason` is the one-line diagnostic surfaced in the run epilogue.
    /// `retry_hint`, when `Some`, replaces the default GH-flavored recovery
    /// hint with a leg-specific one (the BUG-266 path passes a `aida queue
    /// work <spec> --resume <session-id>` hint that recovers the exact
    /// session the API outage interrupted). `None` keeps the BUG-257
    /// default. The drain pauses, the spec is left in its current state
    /// for retry. trace:BUG-257 BUG-266 | ai:claude
    Inconclusive {
        reason: String,
        retry_hint: Option<String>,
    },
    /// BUG-250: the implementer deliberately *held* the PR — branch pushed,
    /// PR intentionally not opened, pending a manual gate (a smoke test, an
    /// out-of-band review, an operator decision). Signalled by `aida pr hold`
    /// dropping a [`crate::punt::HoldSignal`] the orchestrator reads after the
    /// session exits. This is NOT a failure (the work succeeded; the artifact
    /// was deliberately withheld) and NOT a punt (no design-fork, no
    /// NeedsAttention) — it is a clean stop. The drain halts at phase 1 with a
    /// `deliberate-hold` outcome and the correct hint (open the PR when the
    /// gate passes, then route it to the reviewer). `reason` is the operator's
    /// "why"; `branch` is the pushed branch the PR will be opened from.
    /// trace:BUG-250 | ai:claude
    Held {
        reason: Option<String>,
        branch: String,
    },
}

/// BUG-420: which watchdog tripped on a degenerate headless phase. The
/// no-progress signal (no commit + no file-change for N minutes) is the
/// precise catch for the echo/sleep filler-spin; the wall-clock ceiling is a
/// hard backstop. trace:BUG-420 | ai:claude
#[allow(dead_code)] // decision core; wired into the phase spawn-wait by slice 2
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WatchdogTrip {
    NoProgress,
    Ceiling,
}

/// Pure decision core for the BUG-420 phase watchdog. `since_progress` is the
/// time since the last observed commit/file-change in the phase's worktree;
/// `total` is wall-clock since the phase started. A limit of `0` disables that
/// check. No-progress takes precedence (it's the precise signal). The caller
/// polls the worktree and feeds these in; this stays pure + unit-testable.
/// trace:BUG-420 | ai:claude
#[allow(dead_code)] // wired into the phase spawn-wait by slice 2
pub(crate) fn watchdog_verdict(
    since_progress: std::time::Duration,
    total: std::time::Duration,
    no_progress_limit: std::time::Duration,
    ceiling: std::time::Duration,
) -> Option<WatchdogTrip> {
    if !no_progress_limit.is_zero() && since_progress >= no_progress_limit {
        return Some(WatchdogTrip::NoProgress);
    }
    if !ceiling.is_zero() && total >= ceiling {
        return Some(WatchdogTrip::Ceiling);
    }
    None
}

/// TASK-136: the GH PR-verification retry backoff schedule — `retries` waits at
/// 30s, 1m, 5m, then 15m for any beyond. A transient GH blip clears within
/// these; a persistent outage falls through to shelve-and-advance. Pure so the
/// schedule is pinned by a test. trace:TASK-136 | ai:claude
#[allow(dead_code)] // wired by slice 2
pub(crate) fn gh_verify_backoff_schedule(retries: usize) -> Vec<std::time::Duration> {
    const STEPS: [u64; 3] = [30, 60, 300];
    (0..retries)
        .map(|i| std::time::Duration::from_secs(*STEPS.get(i).unwrap_or(&900)))
        .collect()
}

/// The outcome of phase 3 — the reviewer session. The reviewer either
/// reaches a [`Verdict`] (the normal path — `Approved` continues to merge,
/// anything else stops the pipeline), or *escalates the merge decision to a
/// human*. Under `--no-human` the reviewer cannot reach a person, so when
/// the merge call turns on something it should not decide unattended
/// (uncertain zen provenance, an irreversible call) it writes its verdict
/// file with `merge: escalated-to-human` and the orchestrator stops cleanly
/// — exit `0`, no merge, no failure — leaving the PR for a human. An
/// escalation is the reviewer's honest "I will not auto-merge this": it is
/// neither a crash nor a `RequestChanges`, so it is a first-class third
/// outcome of phase 3 (BUG-241 items 4-5). trace:STORY-306 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReviewerOutcome {
    /// The reviewer reached a verdict — phase 3 proceeds on it.
    Verdict(Verdict),
    /// The reviewer escalated the *merge* decision to a human. `reason` is
    /// the one-line "why" surfaced in the run epilogue.
    EscalatedToHuman { reason: String },
}

/// Which tier escalated a decision to a human — selects the
/// [`finish_escalated`] epilogue wording. An escalated *merge* decision (the
/// reviewer would not auto-merge) and an escalated *design-fork* (the
/// headless advisor would not resolve a punt) are the same orchestration
/// outcome — exit `0`, the drain advances, a human triages — so they share
/// one terminal path, distinguished only for the epilogue wording.
/// trace:STORY-306 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EscalationKind {
    /// The reviewer escalated a merge decision (phase 3).
    MergeDecision,
    /// The headless advisor escalated a punted design-fork (phase 1 tier).
    DesignFork,
}

/// Set on an [`OrchestrationResult`] when the run ended in an escalation — a
/// first-class non-failure stop (BUG-241 items 4-5, STORY-306). Mirrors
/// STORY-276's `punt_reason`: the run exits `0` with no `failed_phase`, and
/// this is the field that distinguishes an escalation from a clean ship.
/// trace:STORY-306 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EscalationSummary {
    /// Which tier escalated — picks the epilogue wording.
    pub(crate) kind: EscalationKind,
    /// One-line "why a human is needed", surfaced in the run epilogue.
    pub(crate) reason: String,
}

/// The headless advisor's verdict on a punted design-fork (STORY-306). The
/// advisor either *resolves* the fork — it was confident enough to judge it
/// — or *escalates* it to a human. The conservative-escalation bias (resolve
/// only what is provably grounded, escalate everything else) lives in the
/// `/aida-advise` skill prompt; this enum is just the orchestrator's view of
/// the answer it wrote back. trace:STORY-306 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AdvisorOutcome {
    /// The advisor judged the fork. `answer` is the decision the implementer
    /// resumes with; `reasoning` is the audited "why" — recorded to the
    /// ledger and left as a spec comment.
    Resolved { answer: String, reasoning: String },
    /// The advisor escalated the fork to a human. `reason` is its framing of
    /// *why* it could not safely decide; `category` is the categorized
    /// escalation reason (strategy / irreversible / unrecorded-context / …).
    Escalated { reason: String, category: String },
}

/// Outcome of an [`orchestrate`] run — the process exit code plus the
/// telemetry the caller needs to log the run and, on failure, auto-draft a
/// BUG. Returning this instead of a bare `i32` keeps `orchestrate` pure: it
/// computes the result, and `handle_auto_complete` does the file I/O.
/// trace:TASK-266 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct OrchestrationResult {
    /// Process exit code: `0` on success, else the 1-based failed-phase index.
    pub(crate) exit_code: i32,
    /// The phase that failed; `None` on success.
    pub(crate) failed_phase: Option<Phase>,
    /// The failure detail; `None` on success.
    pub(crate) failure: Option<PhaseFailure>,
    /// Per-phase wall time, in run order — includes the failed phase, and
    /// stops at the variant's last phase or the first failure.
    pub(crate) phase_durations: Vec<(Phase, u128)>,
    /// Total wall time of the run, in milliseconds.
    pub(crate) total_ms: u128,
    /// STORY-276: set when phase 1 punted — the one-line punt reason. The run
    /// is *not* a failure (`exit_code` is `0`, `failed_phase` is `None`), so
    /// this is the only field that distinguishes a punt from a clean ship.
    pub(crate) punt_reason: Option<String>,
    /// BUG-245: when phase 1's PR credits a *different* spec than the
    /// dispatched id (e.g., the dispatched spec was blocked and the
    /// implementer pragmatically shipped a release blocker instead), this
    /// carries the id the PR actually credits. The batch drain records *this*
    /// id as shipped and stops with [`BatchDrainOutcome::Mismatched`] so the
    /// dispatched spec stays queued with an accurate reason rather than a
    /// false `✓ shipped`. `None` when dispatched == shipped (the common path)
    /// or when the driver could not determine an id from the PR's commits.
    /// trace:BUG-245 | ai:claude
    pub(crate) shipped_spec_id: Option<String>,
    /// STORY-306: set when the run ended in an escalation — the reviewer would
    /// not auto-merge, or the headless advisor would not resolve a punted
    /// design-fork. Like `punt_reason` this is a non-failure stop (`exit_code`
    /// `0`, `failed_phase` `None`); it carries the escalation kind + reason so
    /// the caller can log the run and a batch drain can sort the spec apart
    /// from a clean ship. trace:STORY-306 | ai:claude
    pub(crate) escalation: Option<EscalationSummary>,
    /// BUG-257: set when phase 1 ended *inconclusively* — the orchestrator
    /// could not determine whether a PR exists (a transient GH-API network
    /// error). Like `punt_reason` this is a non-failure stop (`exit_code` `0`,
    /// `failed_phase` `None`); the spec is left in its current state for the
    /// next drain to retry. A batch drain stops at the spec without claiming
    /// it shipped, punted, or failed. trace:BUG-257 | ai:claude
    pub(crate) inconclusive_reason: Option<String>,
    /// EPIC-28: set when [`finish_failure`] shelved the spec — flipped its
    /// status to `NeedsAttention` and wrote the structured `FailureReason`.
    /// A shelved run is still an exit-non-zero *failure* at the
    /// per-orchestration level (its `exit_code`/`failed_phase` are
    /// unchanged), but the *batch drain* uses this field to decide
    /// "the failure is recoverable — shelve, skip dependents, continue"
    /// vs "the failure could not be shelved — stop the batch". `None`
    /// on every non-failure path. trace:EPIC-28 | ai:claude
    pub(crate) shelved_reason: Option<aida_core::FailureReason>,
    /// BUG-250: set when phase 1 ended in a deliberate *PR-hold* — the
    /// implementer pushed the branch but intentionally did not open the PR,
    /// pending a manual gate. Like `punt_reason` / `inconclusive_reason` this
    /// is a non-failure stop (`exit_code` `0`, `failed_phase` `None`); the spec
    /// is left in its current state and the operator opens the PR when ready.
    /// Carries the one-line hold summary for the run epilogue + batch summary.
    /// `None` on every other path. trace:BUG-250 | ai:claude
    pub(crate) held_reason: Option<String>,
}

impl OrchestrationResult {
    /// STORY-265 slice 3: a `--with-plan` prelude failure terminal result.
    /// The plan phase failed before the drain's phase 1 ran, so the run exits
    /// non-zero keyed to the given `phase` (the implementer phase the drain
    /// would have entered) with every non-failure field cleared — no PR was
    /// opened, no spec shipped or punted. trace:STORY-265 | ai:claude
    pub(crate) fn failed(phase: Phase) -> Self {
        Self {
            exit_code: phase.index(),
            failed_phase: Some(phase),
            failure: None,
            phase_durations: Vec::new(),
            total_ms: 0,
            punt_reason: None,
            shipped_spec_id: None,
            escalation: None,
            inconclusive_reason: None,
            shelved_reason: None,
            held_reason: None,
        }
    }
}

/// The six phases, abstracted so the orchestrator's sequencing can be tested
/// against a mock. The real implementation spawns Claude sessions, polls CI,
/// and shells out to `gh` / `cargo`.
/// trace:STORY-246 | ai:claude
pub(crate) trait PhaseDriver {
    /// Phase 1 — run the implementer Claude session. Returns
    /// [`ImplementerOutcome::PrOpened`] once a PR is verified, or
    /// [`ImplementerOutcome::Punted`] when a headless implementer hit a
    /// design-fork and punted the spec to `NeedsAttention` (STORY-276).
    fn run_implementer(&mut self) -> Result<ImplementerOutcome, PhaseFailure>;
    /// Phase 2 — wait for CI to go terminal, then end the implementer session
    /// (which auto-queues the `Review PR-N` item for the reviewer).
    fn finish_ci(&mut self) -> Result<(), PhaseFailure>;
    /// Phase 3 — run the reviewer Claude session and read its verdict file.
    /// Returns [`ReviewerOutcome::Verdict`] on a normal review, or
    /// [`ReviewerOutcome::EscalatedToHuman`] when the reviewer wrote
    /// `merge: escalated-to-human` rather than auto-deciding the merge
    /// (STORY-306).
    fn run_reviewer(&mut self) -> Result<ReviewerOutcome, PhaseFailure>;
    /// Phase 4 — merge the PR.
    fn merge(&mut self) -> Result<(), PhaseFailure>;
    /// Phase 5 — `aida pull` (auto-bumps Done → Completed).
    fn pull(&mut self) -> Result<(), PhaseFailure>;
    /// Phase 6 — `cargo build --release`.
    fn build(&mut self) -> Result<(), PhaseFailure>;
    /// Snapshot of what the driver has discovered, for the recovery hint.
    fn hint_context(&self) -> HintContext;
    /// Phase-agnostic reality check (BUG-241). Before the orchestrator
    /// declares `phase` a failure, it asks the driver whether ground truth — a
    /// merged PR, a Completed spec — shows the work shipped anyway. Two real
    /// cases this redeems: a reviewer that escalates the merge to a human who
    /// merges out-of-band (phase 3 leaves no verdict file), and a spec already
    /// resolved by supersession (phase 1 correctly produces no PR). The
    /// default is [`PhaseReconcile::GenuineFailure`] — a driver with no way to
    /// check reality leaves every failure standing, so the reconcile step can
    /// only ever ratify a success, never invent one. trace:BUG-241 | ai:claude
    fn reconcile_failure(&mut self, _phase: Phase, _failure: &PhaseFailure) -> PhaseReconcile {
        PhaseReconcile::GenuineFailure
    }
    /// BUG-245: the spec id the PR's commits actually credit (the `(SPEC-ID)`
    /// at the end of each commit subject), or `None` when the PR cannot be
    /// inspected or its commits name no spec. The orchestrator calls this
    /// right after phase 1 confirms an open PR and, on a mismatch with the
    /// dispatched id, switches the success epilogue to credit the truth and
    /// surfaces the mismatch as an explicit anomaly. The default is `None`
    /// (a driver with no PR-introspection capability behaves exactly as
    /// before — the dispatched id is credited, matching pre-BUG-245
    /// behaviour). An implementation that finds the dispatched id among the
    /// PR's commit credits should return `Some(<dispatched>)` so no mismatch
    /// fires; only return a different id when the PR genuinely credits a
    /// different spec. trace:BUG-245 | ai:claude
    fn shipped_spec_id(&mut self) -> Option<String> {
        None
    }

    /// STORY-306 advisor tier — judge a punted design-fork. When phase 1
    /// punts, the orchestrator assembles a rich payload and spawns a headless
    /// advisor, which either resolves the fork or escalates it to a human.
    /// The default impl errors with [`FailureKind::Internal`] — a driver that
    /// supports the advisor tier overrides it; a driver that does not (a mock
    /// for a non-advisor test) never reaches it. trace:STORY-306 | ai:claude
    fn run_advisor(&mut self) -> Result<AdvisorOutcome, PhaseFailure> {
        Err(PhaseFailure::of(
            FailureKind::Internal,
            "internal: this driver does not support the advisor tier",
        ))
    }

    /// STORY-306 advisor tier — resume the punted phase-1 implementer session
    /// with `answer` (the advisor's judged decision, or, under
    /// `--escalate-defaults`, an authorization to proceed with the defensible
    /// default). Returns the implementer's outcome on resume — a PR opened,
    /// or a fresh punt (terminal: one advisor round per spec). The default
    /// impl errors; overridden by a driver that supports it.
    /// trace:STORY-306 | ai:claude
    fn resume_implementer(&mut self, _answer: &str) -> Result<ImplementerOutcome, PhaseFailure> {
        Err(PhaseFailure::of(
            FailureKind::Internal,
            "internal: this driver does not support resuming the implementer",
        ))
    }

    /// TASK-358: stamp the phase-1 implementer's session lease as
    /// `escalated_to_human` — the marker a later triage (`aida edit --status`
    /// out of `NeedsAttention`) or explicit prune
    /// (`aida session prune --escalations`) uses to know the lingering
    /// worktree is safe to remove. Called from the `--escalate-blocks` arm
    /// of [`resolve_punt_via_advisor`] before `finish_escalated` stops the
    /// run. The default impl is a no-op so test drivers without a real lease
    /// stay simple; `RealPhaseDriver` implements it.
    /// trace:TASK-358 | ai:claude
    fn mark_implementer_lease_escalated(&mut self) {}

    /// EPIC-28: park a failed spec in `NeedsAttention` with a structured
    /// `FailureReason` so a batch drain can continue past the failure
    /// rather than halting the whole batch. Called from `finish_failure`
    /// for every *shelvable* failure kind (see
    /// [`FailureKind::is_shelvable`]); pre-orchestration failures
    /// (`Spawn` / `MissingTool` / `Internal`) bypass the shelve path so
    /// a broken local environment does not park innocent specs.
    ///
    /// Returns `Ok(Some(fr))` when the shelve succeeded — the orchestrator
    /// stamps `OrchestrationResult::shelved_reason` with this value and
    /// `drain_batch` treats it as a recoverable failure. `Ok(None)` means
    /// the driver chose not to shelve (e.g. spec already terminal) and
    /// the historical "failure stops the batch" semantics apply.
    /// `Err(_)` propagates a shelve-side failure (rare); the caller logs
    /// it and falls back to the un-shelved path.
    ///
    /// The default impl is `Ok(None)` so test drivers without a real
    /// store stay simple; `RealPhaseDriver` implements it.
    /// trace:EPIC-28 | ai:claude
    fn shelve_on_failure(
        &mut self,
        _spec: &str,
        _phase: Phase,
        _failure: &PhaseFailure,
        _recovery_hint: &str,
    ) -> anyhow::Result<Option<aida_core::FailureReason>> {
        Ok(None)
    }
}

/// Build a `--json` phase-transition event line. Pure — unit-tested directly.
/// trace:STORY-246 | ai:claude
pub(crate) fn phase_event(
    phase: &str,
    status: &str,
    spec: &str,
    elapsed_ms: u128,
    exit_code: Option<i32>,
    extra: &[(&str, &str)],
) -> String {
    let mut obj = serde_json::Map::new();
    obj.insert("phase".to_string(), serde_json::Value::String(phase.into()));
    obj.insert(
        "status".to_string(),
        serde_json::Value::String(status.into()),
    );
    obj.insert("spec".to_string(), serde_json::Value::String(spec.into()));
    obj.insert(
        "elapsed_ms".to_string(),
        serde_json::Value::Number((elapsed_ms as u64).into()),
    );
    if let Some(code) = exit_code {
        obj.insert(
            "exit_code".to_string(),
            serde_json::Value::Number(code.into()),
        );
    }
    for (k, v) in extra {
        obj.insert((*k).to_string(), serde_json::Value::String((*v).into()));
    }
    serde_json::Value::Object(obj).to_string()
}

/// STORY-508/TASK-651: forge-aware "CLI not on PATH" recovery message. `action`
/// is what the missing CLI was needed for ("merge the PR"); `retry` is the
/// resume verb ("re-run"). Names the right CLI + install URL per forge; pure-git
/// (no forge CLI) gets a forge-neutral phrasing — it shouldn't normally reach a
/// MissingTool failure since it shells out to neither gh nor glab.
fn forge_cli_missing_hint(forge: crate::forge::ForgeKind, action: &str, retry: &str) -> String {
    match forge.cli_install_hint() {
        Some((name, url)) => format!(
            "`{}` is not on PATH — auto-complete needs the {} to {}. \
             Install it ({}), then {}.",
            forge.cli_name(),
            name,
            action,
            url,
            retry
        ),
        None => format!(
            "A forge CLI is needed to {action}, but this project is configured pure-git. \
             Complete the step manually, then {retry}."
        ),
    }
}

/// Pick the recovery hint for a failed phase. Pure — each branch names a
/// concrete next command, parameterised by the failure `kind` (so a spawn
/// ENOENT is never reported as red CI) and whatever the driver discovered.
/// trace:STORY-246 | ai:claude
/// trace:BUG-218 | ai:claude
pub(crate) fn recovery_hint(phase: Phase, kind: FailureKind, ctx: &HintContext) -> String {
    let spec = if ctx.spec.is_empty() {
        "<SPEC>"
    } else {
        ctx.spec.as_str()
    };
    let branch = ctx.branch.as_deref().unwrap_or("<branch>");
    let session = ctx.implementer_session.as_deref().unwrap_or("<session>");
    let pr = ctx
        .pr_number
        .map(|n| n.to_string())
        .unwrap_or_else(|| "<N>".to_string());

    // Cross-cutting kinds — the orchestrator can hit these in *any* phase, so
    // they are matched before the per-phase logic. The hint stays the same
    // shape but names the failing phase's own manual fallback command.
    match kind {
        FailureKind::Spawn => {
            let workaround = match phase {
                Phase::Implementer => format!("aida queue work {spec}"),
                Phase::Ci => format!("aida session end {session} --wait-ci"),
                Phase::Reviewer => format!("aida queue work PR-{pr}"),
                Phase::Merge => ctx
                    .forge
                    .merge_cmd(&pr)
                    .unwrap_or_else(|| format!("merge PR {pr} to your default branch")),
                Phase::Pull => "aida pull".to_string(),
                Phase::Build => "cargo build --release".to_string(),
            };
            return format!(
                "Subprocess spawn failed — this is a local environment issue, not CI. \
                 The orchestrator inherits the parent shell's PATH but may have lost it \
                 (a mid-flight `cargo build` can also invalidate its cached binary path, \
                 BUG-217). Check the command is on PATH, then finish this phase by hand: \
                 `{workaround}`"
            );
        }
        FailureKind::Internal => {
            // `aida queue rework --work` re-picks the spec up regardless of
            // its current status (Approved/InProgress/Done all resolve via
            // the rework smart-status table), so it is the one verb that
            // works whichever phase the internal error struck in.
            // trace:BUG-236 | ai:claude
            return format!(
                "This is an orchestrator bug, not something you did — please file it \
                 (`aida add --type bug --title 'auto-complete phase {} internal error'`). \
                 Meanwhile, pick the spec back up by hand: `aida queue rework {spec} --work`.",
                phase.index()
            );
        }
        // TASK-136: the GH API stayed unreachable through the retry backoff, so
        // the orchestrator could not confirm whether a PR was opened. The work
        // may well be on a branch — this is a transient-network shelve, not a
        // failed implementation. Re-run the drain once GH is reachable.
        // trace:TASK-136 | ai:claude
        FailureKind::PrVerificationInconclusive => {
            return format!(
                "GH could not be reached to confirm a PR after retrying — the spec is \
                 shelved, not failed. Re-run the drain when GH is reachable \
                 (`gh api /rate_limit` to check), then `aida queue work {spec} --auto-complete`."
            );
        }
        // BUG-420: the no-progress / ceiling watchdog killed a degenerate
        // headless phase. The partial work (if any) is on the branch; a human
        // should inspect why the session stopped making progress.
        // trace:BUG-420 | ai:claude
        FailureKind::Watchdog => {
            return format!(
                "The phase watchdog stopped a headless session that made no progress \
                 (no commit or file-change) within its window — likely a degenerate spin. \
                 Inspect the worktree and pick the spec back up by hand: \
                 `aida queue rework {spec} --work`."
            );
        }
        // BUG-455: the SQLite cache was locked by another concurrent `aida`
        // process longer than the retry budget. Transient — re-running the
        // drain once the contending process finishes clears it.
        // trace:BUG-455 | ai:claude
        FailureKind::CacheLocked => {
            return format!(
                "The local cache was locked by another `aida` process (a sibling drain, \
                 an interactive shell, or a bulk archive/edit sweep) longer than the retry \
                 window — the spec is shelved, not failed. Avoid running bulk cache writes \
                 while a drain is live, then re-run: `aida queue work {spec} --auto-complete`. \
                 If a lock looks stuck, run `aida doctor heal stale-locks`."
            );
        }
        _ => {}
    }

    // Phase- and kind-specific hints. Each phase has a `_` arm carrying its
    // [`FailureKind::Failed`] default (the spawned work ran and reported
    // failure) — the pre-BUG-218 behaviour for that phase.
    match (phase, kind) {
        (Phase::Implementer, FailureKind::NoPr) => {
            let resume = ctx
                .implementer_session
                .as_deref()
                .map(|s| format!(" {s}"))
                .unwrap_or_default();
            format!(
                "The implementer exited without opening a PR — resume the session and \
                 run `/aida-pr`: `aida queue work {spec} --resume{resume}`"
            )
        }
        (Phase::Implementer, FailureKind::MissingTool) => {
            forge_cli_missing_hint(ctx.forge, "track the PR", "re-run")
        }
        (Phase::Implementer, _) => {
            let resume = ctx
                .implementer_session
                .as_deref()
                .map(|s| format!(" {s}"))
                .unwrap_or_default();
            format!("Continue the implementer session: `aida queue work {spec} --resume{resume}`")
        }

        (Phase::Ci, FailureKind::CiRed) => {
            // STORY-508/TASK-651: forge-aware CI viewer. With a run id → view
            // it (gh run view / glab ci view); else point at the branch's
            // runs/pipeline (gh run list / glab ci status); pure-git has none.
            let branch_runs = match ctx.forge {
                crate::forge::ForgeKind::GitHub => Some(format!("gh run list --branch {branch}")),
                crate::forge::ForgeKind::GitLab => Some("glab ci status".to_string()),
                crate::forge::ForgeKind::None => None,
            };
            let view = ctx
                .ci_run_id
                .as_deref()
                .and_then(|r| ctx.forge.ci_view_cmd(r))
                .or(branch_runs)
                .unwrap_or_else(|| "check your CI dashboard".to_string());
            let run = ctx.ci_run_id.as_deref().unwrap_or("<ID>");
            format!(
                "CI failed on run {run} — view it: `{view}`. Push fixups to the same \
                 branch: `aida queue work {spec} --branch {branch} --steal`"
            )
        }
        (Phase::Ci, FailureKind::CiTimeout) => {
            // STORY-508/TASK-651: forge-aware "check CI progress" command.
            let progress = match ctx.forge {
                crate::forge::ForgeKind::GitHub => format!("`gh run list --branch {branch}`"),
                crate::forge::ForgeKind::GitLab => "`glab ci status`".to_string(),
                crate::forge::ForgeKind::None => "your CI dashboard".to_string(),
            };
            format!(
                "CI never reached a terminal state in the wait window — it may be queued or \
                 a runner is slow, CI is not red. Check progress with {progress}, then re-run \
                 auto-complete once CI settles."
            )
        }
        (Phase::Ci, _) => format!(
            "The implementer session would not end cleanly — it may have uncommitted \
             changes. Commit or discard them in the worktree, then end it: \
             `aida session end {session} --skip-ci`"
        ),

        (Phase::Reviewer, FailureKind::NoVerdict) => format!(
            "The reviewer session ended without a usable verdict — re-run the review: \
             `aida queue work PR-{pr} --steal`"
        ),
        // By phase 3 the spec is Done — the implementer opened the PR in
        // phase 1, so `aida queue work` (queued items only) would reject
        // it. `aida queue rework` is the verb that re-opens a Done spec.
        // trace:BUG-236 | ai:claude
        (Phase::Reviewer, _) => format!(
            "Address the review feedback, then push fixups: \
             `aida queue rework {spec} --work`"
        ),

        (Phase::Merge, FailureKind::MissingTool) => {
            forge_cli_missing_hint(ctx.forge, "merge the PR", "merge the PR manually")
        }
        (Phase::Merge, _) => {
            let view = ctx
                .forge
                .view_cmd(&pr)
                .map(|c| format!("`{c}`"))
                .unwrap_or_else(|| "your forge's web UI".to_string());
            format!("Investigate the merge failure: {view}")
        }

        (Phase::Pull, _) => "Classify the divergence: `aida rebase --dry-run --json`".to_string(),

        (Phase::Build, _) => {
            "Get fast feedback on the breakage: `cargo check --workspace`".to_string()
        }
    }
}

/// Format an elapsed-millisecond span as `5m 18s` / `42s`.
fn fmt_duration(ms: u128) -> String {
    let secs = ms / 1000;
    if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

fn emit_start(phase: Phase, spec: &str, json: bool, elapsed: u128) {
    if json {
        println!(
            "{}",
            phase_event(phase.slug(), "started", spec, elapsed, None, &[])
        );
    } else {
        eprintln!();
        eprintln!(
            "{} {}",
            "▶".cyan().bold(),
            format!("Phase {}/6: {}", phase.index(), phase.label()).bold()
        );
    }
}

fn emit_done(phase: Phase, spec: &str, json: bool, elapsed: u128) {
    if json {
        println!(
            "{}",
            phase_event(phase.slug(), "completed", spec, elapsed, None, &[])
        );
    } else {
        eprintln!(
            "  {} {}",
            "✓".green(),
            format!("phase {} complete", phase.index()).dimmed()
        );
    }
}

/// BUG-245: announce that phase 1's PR credits a different spec than the
/// dispatched id. The orchestrator continues phases 2-6 against the same PR
/// (those are PR-anchored, not spec-anchored) and `finish_success` will credit
/// `shipped`; the dispatched spec is left queued for its own future pickup.
/// trace:BUG-245 | ai:claude
fn emit_shipped_mismatch(dispatched: &str, shipped: &str, json: bool, elapsed: u128) {
    if json {
        println!(
            "{}",
            phase_event(
                Phase::Implementer.slug(),
                "shipped-mismatch",
                dispatched,
                elapsed,
                None,
                &[("shipped", shipped)],
            )
        );
    } else {
        eprintln!(
            "  {} phase 1 was dispatched for {} but the PR credits {} — \
             crediting {}, {} stays queued",
            "ⓘ".cyan(),
            dispatched.bold(),
            shipped.bold(),
            shipped,
            dispatched,
        );
    }
}

/// Print the success epilogue and build the success [`OrchestrationResult`].
///
/// `dispatched` is the spec the orchestrator was launched for; `credited` is
/// the id reported in the `✓ shipped` line. They differ only in the BUG-245
/// mismatch case — phase 1's PR named a different spec than the dispatched id
/// — so on a match (the common path) the caller passes `dispatched` for both
/// and the JSON event + epilogue read exactly as pre-BUG-245.
/// trace:BUG-245 | ai:claude
fn finish_success(
    dispatched: &str,
    credited: &str,
    json: bool,
    start: &Instant,
    durations: Vec<(Phase, u128)>,
) -> OrchestrationResult {
    let elapsed = start.elapsed().as_millis();
    let shipped_spec_id = (credited != dispatched).then(|| credited.to_string());
    if json {
        let mut extra: Vec<(&str, &str)> = Vec::new();
        if let Some(id) = shipped_spec_id.as_deref() {
            extra.push(("shipped", id));
        }
        println!(
            "{}",
            phase_event(
                "auto-complete",
                "success",
                dispatched,
                elapsed,
                Some(0),
                &extra
            )
        );
    } else {
        eprintln!();
        eprintln!(
            "{} {} shipped ({})",
            "✓".green().bold(),
            credited.bold(),
            fmt_duration(elapsed)
        );
    }
    OrchestrationResult {
        exit_code: 0,
        failed_phase: None,
        failure: None,
        phase_durations: durations,
        total_ms: elapsed,
        punt_reason: None,
        shipped_spec_id,
        escalation: None,
        inconclusive_reason: None,
        shelved_reason: None,
        held_reason: None,
    }
}

/// Print the punt epilogue and build a *non-failure* terminal
/// [`OrchestrationResult`] (STORY-276). A headless implementer that hits a
/// design-fork it cannot safely resolve invokes `/aida-punt`, parking the
/// spec in `NeedsAttention`; phase 1 then ends without a PR. The pipeline
/// stopping here is correct, not a failure — so the run exits `0` with no
/// `failed_phase`, and a batch drain advances to the next member (the punted
/// spec drops out of the queue on its own, `NeedsAttention` not being
/// drivable). The punt itself is already durably recorded by `aida punt`
/// (status flip + `.aida/punts.jsonl` ledger); `punt_reason` only carries it
/// into the run epilogue and telemetry. trace:STORY-276 | ai:claude
fn finish_punted(
    spec: &str,
    json: bool,
    start: &Instant,
    durations: Vec<(Phase, u128)>,
    reason: &str,
) -> OrchestrationResult {
    let elapsed = start.elapsed().as_millis();
    if json {
        println!(
            "{}",
            phase_event(
                Phase::Implementer.slug(),
                "punted",
                spec,
                elapsed,
                Some(0),
                &[("reason", reason)],
            )
        );
        println!(
            "{}",
            phase_event(
                "auto-complete",
                "punted",
                spec,
                elapsed,
                Some(0),
                &[("reason", reason)],
            )
        );
    } else {
        eprintln!();
        eprintln!(
            "{} phase 1 (implementer session) punted: {}",
            "⏸".yellow().bold(),
            reason
        );
        eprintln!(
            "  {} {}",
            "→".dimmed(),
            "the spec is parked in Needs Attention — triage it with `aida findings list`".cyan()
        );
        eprintln!();
        eprintln!(
            "{} {} punted ({}) — parked for advisor triage, drain continues",
            "⏸".yellow().bold(),
            spec.bold(),
            fmt_duration(elapsed)
        );
    }
    OrchestrationResult {
        exit_code: 0,
        failed_phase: None,
        failure: None,
        phase_durations: durations,
        total_ms: elapsed,
        punt_reason: Some(reason.to_string()),
        shipped_spec_id: None,
        escalation: None,
        inconclusive_reason: None,
        shelved_reason: None,
        held_reason: None,
    }
}

/// Print the escalation epilogue and build a *non-failure* terminal
/// [`OrchestrationResult`] (STORY-306). Two tiers escalate to a human and
/// share this path: the reviewer that will not auto-merge a PR
/// ([`EscalationKind::MergeDecision`], BUG-241 items 4-5) and the headless
/// advisor that will not resolve a punted design-fork
/// ([`EscalationKind::DesignFork`]). Both are honest stops, not failures —
/// the run exits `0` with no `failed_phase`, the PR / spec is left for a
/// human, and a batch drain advances. `escalation` is the field that
/// distinguishes the run from a clean ship; the human triages it via
/// `aida findings`. trace:STORY-306 | ai:claude
fn finish_escalated(
    spec: &str,
    json: bool,
    start: &Instant,
    durations: Vec<(Phase, u128)>,
    kind: EscalationKind,
    reason: &str,
) -> OrchestrationResult {
    let elapsed = start.elapsed().as_millis();
    // The escalation surfaced in a specific phase — phase 3 for a merge
    // decision, phase 1 (the punt) for a design-fork.
    let phase = match kind {
        EscalationKind::MergeDecision => Phase::Reviewer,
        EscalationKind::DesignFork => Phase::Implementer,
    };
    if json {
        println!(
            "{}",
            phase_event(
                phase.slug(),
                "escalated",
                spec,
                elapsed,
                Some(0),
                &[("reason", reason)],
            )
        );
        println!(
            "{}",
            phase_event(
                "auto-complete",
                "escalated",
                spec,
                elapsed,
                Some(0),
                &[("reason", reason)],
            )
        );
    } else {
        let what = match kind {
            EscalationKind::MergeDecision => "the reviewer escalated the merge decision to a human",
            EscalationKind::DesignFork => "the advisor escalated a design-fork to a human",
        };
        eprintln!();
        eprintln!("{} {}: {}", "⏸".yellow().bold(), what, reason);
        eprintln!(
            "  {} {}",
            "→".dimmed(),
            "left for a human — triage it with `aida findings list`".cyan()
        );
        eprintln!();
        eprintln!(
            "{} {} escalated ({}) — drain continues, a human decides",
            "⏸".yellow().bold(),
            spec.bold(),
            fmt_duration(elapsed)
        );
    }
    OrchestrationResult {
        exit_code: 0,
        failed_phase: None,
        failure: None,
        phase_durations: durations,
        total_ms: elapsed,
        punt_reason: None,
        shipped_spec_id: None,
        escalation: Some(EscalationSummary {
            kind,
            reason: reason.to_string(),
        }),
        inconclusive_reason: None,
        shelved_reason: None,
        held_reason: None,
    }
}

/// Print the inconclusive epilogue and build a *non-failure* terminal
/// [`OrchestrationResult`] (BUG-257). When phase 1 ends and the orchestrator's
/// PR lookup hits a transient GH-API network error, it cannot tell whether a
/// PR exists. Conflating that with "no PR opened" (the pre-BUG-257 behaviour)
/// gave the operator a wrong recovery hint ("run `/aida-pr`") and crashed the
/// batch on a network blip. The honest outcome is *Inconclusive*: the run
/// halts cleanly (exit `0`, no `failed_phase`), the spec is left in its
/// current state, the batch drain pauses at this spec (it neither shipped nor
/// failed), and the next drain retries once the API is reachable.
/// trace:BUG-257 | ai:claude
fn finish_inconclusive(
    spec: &str,
    json: bool,
    start: &Instant,
    durations: Vec<(Phase, u128)>,
    reason: &str,
    retry_hint: Option<&str>,
) -> OrchestrationResult {
    let elapsed = start.elapsed().as_millis();
    // BUG-266: per-leg recovery hint. `None` preserves BUG-257's GH-API
    // wording (the only inconclusive case at the time it shipped); BUG-266's
    // Anthropic-API path passes a `aida queue work <spec> --resume <session>`
    // hint that recovers the exact session the API outage interrupted.
    let default_hint = "transient — retry once the GH API is reachable: \
                        `gh api /rate_limit` then re-run `aida queue work --auto-complete`";
    let hint_line = retry_hint.unwrap_or(default_hint);
    if json {
        println!(
            "{}",
            phase_event(
                Phase::Implementer.slug(),
                "inconclusive",
                spec,
                elapsed,
                Some(0),
                &[("reason", reason), ("retry_hint", hint_line)],
            )
        );
        println!(
            "{}",
            phase_event(
                "auto-complete",
                "inconclusive",
                spec,
                elapsed,
                Some(0),
                &[("reason", reason), ("retry_hint", hint_line)],
            )
        );
    } else {
        eprintln!();
        eprintln!(
            "{} phase 1 (implementer session) inconclusive: {}",
            "⏸".yellow().bold(),
            reason
        );
        eprintln!("  {} {}", "→".dimmed(), hint_line.cyan());
        eprintln!();
        eprintln!(
            "{} {} inconclusive ({}) — drain paused, spec left for retry",
            "⏸".yellow().bold(),
            spec.bold(),
            fmt_duration(elapsed)
        );
    }
    OrchestrationResult {
        exit_code: 0,
        failed_phase: None,
        failure: None,
        phase_durations: durations,
        total_ms: elapsed,
        punt_reason: None,
        shipped_spec_id: None,
        escalation: None,
        inconclusive_reason: Some(reason.to_string()),
        shelved_reason: None,
        held_reason: None,
    }
}

/// TASK-136: phase 1 ended *inconclusively* (GH unreachable through the whole
/// `gh_verify_backoff_schedule` retry) inside a **batch** drain — *shelve* the
/// spec instead of pausing. Routing it through [`PhaseDriver::shelve_on_failure`]
/// (as [`FailureKind::PrVerificationInconclusive`], a shelvable kind) parks it
/// in `NeedsAttention` and stamps `shelved_reason`, so [`drain_batch`]'s
/// existing EPIC-28 shelve→advance path carries it: the spec leaves the queue
/// head, the batch continues, and a human triages the parked set via
/// `aida findings list`. The single-spec path keeps the [`finish_inconclusive`]
/// pause (reasonable when nothing is queued behind it). The epilogue says
/// *shelved, not failed* — this is a transient-network park, not a botched
/// implementation. trace:TASK-136 | ai:claude
fn finish_inconclusive_shelved(
    driver: &mut dyn PhaseDriver,
    spec: &str,
    json: bool,
    start: &Instant,
    durations: Vec<(Phase, u128)>,
    reason: &str,
) -> OrchestrationResult {
    let phase = Phase::Implementer;
    let elapsed = start.elapsed().as_millis();
    let failure = PhaseFailure::of(FailureKind::PrVerificationInconclusive, reason);
    let hint = recovery_hint(phase, failure.kind, &driver.hint_context());
    // Best-effort shelve — a shelve-side error leaves `shelved_reason` None, so
    // `drain_batch` falls back to treating the inconclusive as a hard pause
    // rather than silently swallowing the spec.
    let shelved_reason: Option<aida_core::FailureReason> =
        match driver.shelve_on_failure(spec, phase, &failure, &hint) {
            Ok(fr) => fr,
            Err(e) => {
                eprintln!(
                    "  {} could not shelve {} into Needs Attention: {} \
                     — the batch drain will treat it as a pause",
                    "ⓘ".cyan(),
                    spec,
                    e
                );
                None
            }
        };
    if json {
        println!(
            "{}",
            phase_event(
                phase.slug(),
                "inconclusive-shelved",
                spec,
                elapsed,
                Some(phase.index()),
                &[("reason", reason), ("hint", hint.as_str())],
            )
        );
        println!(
            "{}",
            phase_event(
                "auto-complete",
                "shelved",
                spec,
                elapsed,
                Some(phase.index()),
                &[("kind", failure.kind.slug())],
            )
        );
    } else {
        eprintln!();
        eprintln!(
            "{} phase 1 (implementer session) inconclusive — GH unreachable after retries: {}",
            "⏸".yellow().bold(),
            reason
        );
        eprintln!("  {} {}", "→".dimmed(), hint.cyan());
        eprintln!();
        eprintln!(
            "{} {} shelved ({}) — transient verify failure, batch advances; \
             triage with `aida findings list`",
            "⚠".yellow().bold(),
            spec.bold(),
            fmt_duration(elapsed)
        );
    }
    OrchestrationResult {
        exit_code: phase.index(),
        failed_phase: Some(phase),
        failure: Some(failure),
        phase_durations: durations,
        total_ms: elapsed,
        punt_reason: None,
        shipped_spec_id: None,
        escalation: None,
        inconclusive_reason: None,
        shelved_reason,
        held_reason: None,
    }
}

/// BUG-250: print the deliberate-PR-hold epilogue and build a clean *non-
/// failure* [`OrchestrationResult`]. When phase 1 ends with the branch pushed
/// but the PR deliberately held (the `aida pr hold` signal), the pre-BUG-250
/// behaviour mis-filed it as a phase-1 failure with a wrong recovery hint
/// ("run `/aida-pr`") — which for a deliberate hold would ship un-gated code.
/// The honest outcome is *Held*: the run halts cleanly (exit `0`, no
/// `failed_phase`), the spec is left in its current state, and the hint matches
/// the actual state — open the PR once the manual gate passes, then route it to
/// the reviewer. trace:BUG-250 | ai:claude
fn finish_held(
    spec: &str,
    json: bool,
    start: &Instant,
    durations: Vec<(Phase, u128)>,
    reason: Option<&str>,
    branch: &str,
) -> OrchestrationResult {
    let elapsed = start.elapsed().as_millis();
    let summary = match reason {
        Some(r) => format!("PR held on `{branch}` — {r}"),
        None => format!("PR held on `{branch}`"),
    };
    // The recovery hint matches the deliberate-hold state: open the PR when the
    // gate is satisfied, then hand it to the reviewer. NOT "run /aida-pr".
    let hint_line = "ready for review when you are — run your gate, then \
         `gh pr create` (or `glab mr create`) and `aida queue work PR-N --role reviewer`";
    if json {
        println!(
            "{}",
            phase_event(
                Phase::Implementer.slug(),
                "held",
                spec,
                elapsed,
                Some(0),
                &[("reason", summary.as_str()), ("hint", hint_line)],
            )
        );
        println!(
            "{}",
            phase_event(
                "auto-complete",
                "held",
                spec,
                elapsed,
                Some(0),
                &[("reason", summary.as_str())],
            )
        );
    } else {
        eprintln!();
        eprintln!(
            "{} phase 1 (implementer session) — PR deliberately held: {}",
            "⏸".yellow().bold(),
            summary
        );
        eprintln!("  {} {}", "→".dimmed(), hint_line.cyan());
        eprintln!();
        eprintln!(
            "{} {} held ({}) — branch pushed, PR held for your gate",
            "⏸".yellow().bold(),
            spec.bold(),
            fmt_duration(elapsed)
        );
    }
    OrchestrationResult {
        exit_code: 0,
        failed_phase: None,
        failure: None,
        phase_durations: durations,
        total_ms: elapsed,
        punt_reason: None,
        shipped_spec_id: None,
        escalation: None,
        inconclusive_reason: None,
        shelved_reason: None,
        held_reason: Some(summary),
    }
}

/// Print the failure epilogue (reason + recovery hint) and build the
/// failure [`OrchestrationResult`] (exit code = the phase's 1-based index).
///
/// EPIC-28: when `failure.kind.is_shelvable()`, calls
/// [`PhaseDriver::shelve_on_failure`] before building the result and
/// stamps `shelved_reason` with the structured `FailureReason` the driver
/// wrote onto the spec. A non-shelvable kind (`Spawn` / `MissingTool` /
/// `Internal`) skips the hook — those describe a broken environment, not
/// a spec-level failure, and shelving them would park innocent specs.
fn finish_failure(
    driver: &mut dyn PhaseDriver,
    phase: Phase,
    spec: &str,
    json: bool,
    start: &Instant,
    failure: &PhaseFailure,
    durations: Vec<(Phase, u128)>,
) -> OrchestrationResult {
    let code = phase.index();
    let elapsed = start.elapsed().as_millis();
    let hint = recovery_hint(phase, failure.kind, &driver.hint_context());

    // EPIC-28: shelve the spec into `NeedsAttention` so a batch drain can
    // continue past the failure. Best-effort — a shelve-side error is
    // logged but never replaces the original phase failure (we want the
    // real diagnostic on stderr, not a meta-failure about the shelving).
    // trace:EPIC-28 | ai:claude
    let shelved_reason: Option<aida_core::FailureReason> = if failure.kind.is_shelvable() {
        match driver.shelve_on_failure(spec, phase, failure, &hint) {
            Ok(fr) => fr,
            Err(e) => {
                eprintln!(
                    "  {} could not shelve {} into Needs Attention: {} \
                     — the batch drain will treat the failure as un-shelvable",
                    "ⓘ".cyan(),
                    spec,
                    e
                );
                None
            }
        }
    } else {
        None
    };

    if json {
        println!(
            "{}",
            phase_event(
                phase.slug(),
                "failed",
                spec,
                elapsed,
                Some(code),
                &[
                    ("reason", failure.reason.as_str()),
                    ("kind", failure.kind.slug()),
                    ("hint", hint.as_str()),
                ],
            )
        );
        println!(
            "{}",
            phase_event("auto-complete", "failure", spec, elapsed, Some(code), &[])
        );
    } else {
        eprintln!();
        eprintln!(
            "{} phase {} ({}) failed: {}",
            "✗".red().bold(),
            phase.index(),
            phase.label(),
            failure.reason
        );
        eprintln!("  {} {}", "→".dimmed(), hint.cyan());
        eprintln!();
        eprintln!(
            "{} {} — auto-complete failed at phase {} ({})",
            "✗".red().bold(),
            spec.bold(),
            phase.index(),
            fmt_duration(elapsed)
        );
    }
    OrchestrationResult {
        exit_code: code,
        failed_phase: Some(phase),
        failure: Some(failure.clone()),
        phase_durations: durations,
        total_ms: elapsed,
        punt_reason: None,
        shipped_spec_id: None,
        escalation: None,
        inconclusive_reason: None,
        shelved_reason,
        held_reason: None,
    }
}

/// Print the reconciled-success epilogue and build a *success*
/// [`OrchestrationResult`]. Used when the BUG-241 reconcile step finds that a
/// phase which ended without its expected artifact actually shipped — the PR
/// merged out-of-band, or the spec legitimately needed no code. The run
/// counts as a success (exit `0`, no `failed_phase`): the batch advances and
/// the spec is *not* mis-reported as un-shipped. trace:BUG-241 | ai:claude
fn finish_reconciled(
    spec: &str,
    json: bool,
    start: &Instant,
    durations: Vec<(Phase, u128)>,
    phase: Phase,
    reason: &str,
) -> OrchestrationResult {
    let elapsed = start.elapsed().as_millis();
    if json {
        println!(
            "{}",
            phase_event(
                phase.slug(),
                "reconciled",
                spec,
                elapsed,
                Some(0),
                &[("reason", reason)],
            )
        );
        println!(
            "{}",
            phase_event(
                "auto-complete",
                "success",
                spec,
                elapsed,
                Some(0),
                &[("reconciled", "true")],
            )
        );
    } else {
        eprintln!();
        eprintln!(
            "{} phase {} ({}) ended without its usual artifact — reconciled against \
             reality: {}",
            "ⓘ".cyan(),
            phase.index(),
            phase.label(),
            reason,
        );
        eprintln!(
            "{} {} shipped ({}, reconciled)",
            "✓".green().bold(),
            spec.bold(),
            fmt_duration(elapsed)
        );
    }
    OrchestrationResult {
        exit_code: 0,
        failed_phase: None,
        failure: None,
        phase_durations: durations,
        total_ms: elapsed,
        punt_reason: None,
        shipped_spec_id: None,
        escalation: None,
        inconclusive_reason: None,
        shelved_reason: None,
        held_reason: None,
    }
}

/// Resolve a phase `Err` through the BUG-241 reconcile step. The orchestrator
/// asks the driver — via [`PhaseDriver::reconcile_failure`] — whether ground
/// truth shows the spec shipped despite the missing artifact. If it did, the
/// run is a [`finish_reconciled`] success; otherwise the failure stands and
/// [`finish_failure`] reports it exactly as before. Returns the terminal
/// [`OrchestrationResult`] the caller returns immediately.
///
/// This is the phase-agnostic seam: every failure site in [`orchestrate`]
/// routes through here, so reconciliation is one principle applied at every
/// phase, not a per-phase patch. trace:BUG-241 | ai:claude
fn resolve_phase_failure(
    driver: &mut dyn PhaseDriver,
    phase: Phase,
    spec: &str,
    json: bool,
    start: &Instant,
    failure: &PhaseFailure,
    durations: Vec<(Phase, u128)>,
) -> OrchestrationResult {
    // BUG-455: a "database is locked" failure from any phase is transient
    // cache-lock contention, not a spec or environment fault — reclassify it
    // shelvable here, at the single seam every failure routes through, so the
    // drain parks the spec and continues. trace:BUG-455 | ai:claude
    let failure = failure.clone().reclassify_transient();
    let failure = &failure;
    match driver.reconcile_failure(phase, failure) {
        PhaseReconcile::ShippedOutOfBand { reason } => {
            finish_reconciled(spec, json, start, durations, phase, &reason)
        }
        PhaseReconcile::GenuineFailure => {
            finish_failure(driver, phase, spec, json, start, failure, durations)
        }
    }
}

/// The result of routing a phase-1 punt through the advisor tier
/// ([`resolve_punt_via_advisor`]). Either the drain *proceeds* — the advisor
/// resolved the fork and the resumed implementer opened a PR, so phases 2-6
/// run — or it is *terminal*: the orchestration ended inside the advisor tier
/// (escalated, re-punted, or a phase failed) and the carried result is what
/// `orchestrate` returns. trace:STORY-306 | ai:claude
enum PuntFlow {
    /// The advisor resolved the punt and the implementer resumed with a PR —
    /// `orchestrate` continues to phase 2.
    Proceed,
    /// The orchestration ended inside the advisor tier — return this result.
    Terminal(Box<OrchestrationResult>),
}

/// The instruction handed to a resumed implementer under `--escalate-defaults`
/// when the advisor escalated rather than resolved: there is no judged
/// answer, so proceed with the defensible default rather than punting again.
/// trace:STORY-306 | ai:claude
const ADVISOR_DEFAULT_PROMPT: &str =
    "No advisor judgment is available for this design-fork — the headless \
     advisor escalated it and `--escalate-defaults` is in effect. Proceed with \
     your stated lean, or, if you gave no lean, the most defensible reading of \
     the spec. Do not punt again; ship the defensible default.";

/// STORY-306 advisor tier — route a phase-1 punt through a headless advisor
/// before it reaches a human. Spawns the advisor ([`PhaseDriver::run_advisor`]);
/// on a resolve, resumes the implementer with the judged answer; on an
/// escalate, either stops clean ([`EscalateMode::Blocks`]) or resumes with the
/// defensible default ([`EscalateMode::Defaults`]). A re-punt after a resume is
/// terminal — one advisor round per spec per drain. Returns
/// [`PuntFlow::Proceed`] only when the resumed implementer opened a PR; every
/// other outcome is [`PuntFlow::Terminal`]. trace:STORY-306 | ai:claude
// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
fn resolve_punt_via_advisor(
    driver: &mut dyn PhaseDriver,
    spec: &str,
    json: bool,
    start: &Instant,
    durations: &[(Phase, u128)],
    punt_reason: &str,
    escalate_mode: EscalateMode,
    batch: bool,
) -> PuntFlow {
    if !json {
        eprintln!();
        eprintln!(
            "{} design-fork punted — escalating to a headless advisor: {}",
            "◆".cyan().bold(),
            punt_reason
        );
    }
    match driver.run_advisor() {
        Err(f) => PuntFlow::Terminal(Box::new(resolve_phase_failure(
            driver,
            Phase::Implementer,
            spec,
            json,
            start,
            &f,
            durations.to_vec(),
        ))),
        Ok(AdvisorOutcome::Resolved { answer, .. }) => {
            if !json {
                eprintln!(
                    "  {} advisor resolved the fork — resuming the implementer with the answer",
                    "✓".green()
                );
            }
            resume_after_advisor(driver, spec, json, start, durations, &answer, batch)
        }
        Ok(AdvisorOutcome::Escalated { reason, .. }) => match escalate_mode {
            EscalateMode::Blocks => {
                if !json {
                    eprintln!(
                        "  {} advisor escalated the fork to a human — drain advances, the \
                         spec waits",
                        "⏸".yellow()
                    );
                }
                // TASK-358: stamp the lease so a later triage (or explicit
                // `aida session prune --escalations`) knows the lingering
                // worktree is safe to clean. The advisor-resume path
                // (escalate-defaults below) deliberately omits this so its
                // worktree is preserved for the resume.
                // trace:TASK-358 | ai:claude
                driver.mark_implementer_lease_escalated();
                PuntFlow::Terminal(Box::new(finish_escalated(
                    spec,
                    json,
                    start,
                    durations.to_vec(),
                    EscalationKind::DesignFork,
                    &reason,
                )))
            }
            EscalateMode::Defaults => {
                if !json {
                    eprintln!(
                        "  {} advisor escalated — `--escalate-defaults`: resuming with the \
                         defensible default",
                        "⏵".yellow()
                    );
                }
                resume_after_advisor(
                    driver,
                    spec,
                    json,
                    start,
                    durations,
                    ADVISOR_DEFAULT_PROMPT,
                    batch,
                )
            }
        },
    }
}

/// The resume leg shared by the advisor's resolve path and the
/// `--escalate-defaults` path: resume the implementer with `answer`, then
/// classify the outcome. A PR opened ⇒ [`PuntFlow::Proceed`]; a re-punt is
/// terminal ([`finish_punted`] — one advisor round per spec); a failure routes
/// through the BUG-241 reconcile. trace:STORY-306 | ai:claude
fn resume_after_advisor(
    driver: &mut dyn PhaseDriver,
    spec: &str,
    json: bool,
    start: &Instant,
    durations: &[(Phase, u128)],
    answer: &str,
    batch: bool,
) -> PuntFlow {
    match driver.resume_implementer(answer) {
        Err(f) => PuntFlow::Terminal(Box::new(resolve_phase_failure(
            driver,
            Phase::Implementer,
            spec,
            json,
            start,
            &f,
            durations.to_vec(),
        ))),
        Ok(ImplementerOutcome::PrOpened) => PuntFlow::Proceed,
        // A re-punt after a resume is terminal — one advisor round per spec
        // per drain; a new fork is a fresh punt, not a conversation.
        Ok(ImplementerOutcome::Punted { reason }) => PuntFlow::Terminal(Box::new(finish_punted(
            spec,
            json,
            start,
            durations.to_vec(),
            &reason,
        ))),
        // BUG-257: an Inconclusive on the resumed implementer (a transient
        // GH-API blip during PR lookup) is terminal — the drain pauses for
        // retry, no second advisor round. BUG-266: same path when the
        // Anthropic API took out the resumed `claude -p` itself.
        Ok(ImplementerOutcome::Inconclusive { reason, retry_hint }) => {
            // TASK-136: batch → shelve-and-advance; single-spec → pause.
            if batch {
                PuntFlow::Terminal(Box::new(finish_inconclusive_shelved(
                    driver,
                    spec,
                    json,
                    start,
                    durations.to_vec(),
                    &reason,
                )))
            } else {
                PuntFlow::Terminal(Box::new(finish_inconclusive(
                    spec,
                    json,
                    start,
                    durations.to_vec(),
                    &reason,
                    retry_hint.as_deref(),
                )))
            }
        }
        // BUG-250: the resumed implementer deliberately held the PR — a clean
        // terminal stop, same as the first-pass hold (the advisor's fork is
        // resolved, the work is on a branch, the PR awaits a manual gate).
        Ok(ImplementerOutcome::Held { reason, branch }) => {
            PuntFlow::Terminal(Box::new(finish_held(
                spec,
                json,
                start,
                durations.to_vec(),
                reason.as_deref(),
                &branch,
            )))
        }
    }
}

/// TASK-133: should the orchestrator parent compensate its pre-spawn phase-1
/// status bump?
///
/// `prepare_auto_complete_phase1_status` flips Approved/Planned/Draft →
/// InProgress *before* the implementer child is spawned (BUG-369: the child
/// then treats InProgress-without-lease as "parent corroborated" rather than
/// refusing). That bump is correct only when the child goes on to acquire a
/// lease and do work. If phase 1 instead fails *without the child ever
/// recording a lease* — a spawn error, a scope-contention bail, a clean exit
/// with no commits — no work happened, yet the spec is now stranded
/// InProgress (then shelved to NeedsAttention by `finish_failure`) behind a
/// transient error. The parent must restore the pre-bump status so the spec
/// is cleanly re-queueable instead of parked in `aida findings list`.
///
/// Returns `true` only when all three hold:
/// - `bumped` — the parent actually flipped the status (an already-InProgress
///   / Planned spec the parent left untouched needs no compensation),
/// - `!lease_acquired` — the child never recorded a lease, so there is no
///   real work to triage (a lease-acquired phase-1 failure leaves commits /
///   a worktree worth keeping shelved),
/// - `failed_phase == Some(Phase::Implementer)` — phase 1 is what failed (a
///   later-phase failure means the implementer shipped a PR, so the bump was
///   legitimate; a success / punt / inconclusive is not a failure at all).
///   trace:TASK-133 | ai:claude
pub(crate) fn should_compensate_phase1_bump(
    bumped: bool,
    lease_acquired: bool,
    failed_phase: Option<Phase>,
) -> bool {
    bumped && !lease_acquired && failed_phase == Some(Phase::Implementer)
}

/// STORY-265 slice 3: the `--with-plan` PLAN PRELUDE. A `--with-plan`
/// auto-complete run does the design work in its own planning session
/// *before* the existing 6-phase drain — produces a `docs/plans/` file and
/// promotes the spec Approved → Planned — then enters the drain unchanged.
///
/// CRITICAL DESIGN: the plan phase is modelled as a PRELUDE, not a renumbered
/// [`Phase`] variant. The `Phase` enum's 1-based indices double as failure
/// exit codes everywhere (telemetry, resume reconciliation, recovery hints),
/// so renumbering it would silently re-key every existing run. Instead the
/// prelude runs ahead of phase 1 and the drain's phases keep their numbers.
///
/// Each step is the exact CLI invocation the prelude shells out to — reusing
/// slice 2's `aida queue work <SPEC> --plan-only` (the interactive/headless
/// planning session) and slice 1's `aida plan promote <SPEC>` (the
/// Approved → Planned transition). Modelling the steps as data (rather than
/// inlining the shell-outs) keeps the decision — *with-plan ⇒ plan then drain;
/// without ⇒ drain only* — a pure function the unit tests pin down in
/// isolation.
// trace:STORY-265 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlanPreludeStep {
    /// `aida queue work <spec> --plan-only` — the slice-2 planning session.
    /// `headless` appends `--no-human` when the drain runs a headless
    /// implementer (`--no-human=both`), matching how the phase-1 implementer
    /// is launched.
    // trace:STORY-265 | ai:claude
    PlanSession { spec: String, headless: bool },
    /// `aida plan promote <spec>` — the slice-1 Approved → Planned transition,
    /// fired once the plan file lands.
    // trace:STORY-265 | ai:claude
    Promote { spec: String },
}

/// Pure decision for the `--with-plan` prelude: returns the ordered prelude
/// steps to run before the drain. `with_plan == false` ⇒ no steps (drain
/// only — the historical behaviour, default unchanged per the slice-1
/// operator decision). `with_plan == true` ⇒ a plan session then a promote,
/// after which the caller enters the existing 6-phase drain unchanged.
/// `headless_implementer` mirrors the drain's phase-1 launch mode onto the
/// plan session.
// trace:STORY-265 | ai:claude
pub(crate) fn plan_prelude_steps(
    spec: &str,
    with_plan: bool,
    headless_implementer: bool,
) -> Vec<PlanPreludeStep> {
    if !with_plan {
        return Vec::new();
    }
    vec![
        PlanPreludeStep::PlanSession {
            spec: spec.to_string(),
            headless: headless_implementer,
        },
        PlanPreludeStep::Promote {
            spec: spec.to_string(),
        },
    ]
}

/// Drive the phases in order, stopping at the variant's last phase or at the
/// first failure. Returns an [`OrchestrationResult`] — the process exit code
/// (`0` on success, else the 1-based index of the phase that failed) plus
/// the per-phase timing the telemetry layer (TASK-266) records.
/// trace:STORY-246 | ai:claude
/// trace:TASK-266 | ai:claude
// TASK-552: retained as the public no-skip orchestration entry point; current
// CLI paths use the lifecycle-aware wrapper but tests/future callers need this
// stable facade.
#[allow(dead_code)]
pub(crate) fn orchestrate(
    driver: &mut dyn PhaseDriver,
    spec: &str,
    variant: AutoCompleteVariant,
    json: bool,
    escalate_mode: EscalateMode,
) -> OrchestrationResult {
    orchestrate_with_lifecycle_skip(
        driver,
        spec,
        variant,
        json,
        escalate_mode,
        LifecycleSkip::none(),
        // TASK-136: the no-skip facade is the single-spec entry point — a
        // phase-1 Inconclusive pauses for retry, it is not shelved.
        false,
    )
}

/// TASK-136: `batch` selects how a still-inconclusive phase-1 verify (GH
/// unreachable through the retry backoff) is handled. In a **batch** drain it
/// is *shelved* — parked in `NeedsAttention` so the batch loop advances past it
/// (the existing EPIC-28 shelve→advance path) rather than the whole batch
/// pausing at the head. In a **single-spec** drain it stays an Inconclusive
/// pause (the historical behaviour) — pausing one spec to retry later is
/// reasonable when there is nothing else queued behind it. trace:TASK-136
pub(crate) fn orchestrate_with_lifecycle_skip(
    driver: &mut dyn PhaseDriver,
    spec: &str,
    variant: AutoCompleteVariant,
    json: bool,
    escalate_mode: EscalateMode,
    lifecycle_skip: LifecycleSkip,
    batch: bool,
) -> OrchestrationResult {
    // Normal (non-resume) entry: start at phase 1.
    orchestrate_with_resume(
        driver,
        spec,
        variant,
        json,
        escalate_mode,
        lifecycle_skip,
        batch,
        Phase::Implementer,
    )
}

/// STORY-492: the orchestration loop with a `start_phase` — used by `--resume`
/// to re-enter a crashed drain at the first phase whose effect is *not* yet
/// present in the world (computed by `drain_resume::reconcile_resume_phase`
/// from probed git/PR/spec reality). Phases before `start_phase` are skipped
/// because their side effects already exist (the branch is pushed, the PR is
/// merged, …); the caller must have **seeded the driver** with the branch + PR
/// the skipped phases would otherwise have discovered, so the resumed phases
/// have the context they need. A normal drain passes `Phase::Implementer`
/// (skip nothing), so this is a strict superset of the historical behaviour.
///
/// SAFETY: re-entering at an earlier phase than strictly necessary is safe —
/// re-running an already-merged `merge()` is caught by the BUG-241 reconcile
/// (`detect_merged_pr` → `ShippedOutOfBand` → success), and CI/reviewer/build
/// are idempotent. The catastrophic case (double-drive) is prevented *before*
/// this is called, by the PID-liveness gate in the resume handler.
/// trace:STORY-492 | ai:claude
// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn orchestrate_with_resume(
    driver: &mut dyn PhaseDriver,
    spec: &str,
    variant: AutoCompleteVariant,
    json: bool,
    escalate_mode: EscalateMode,
    lifecycle_skip: LifecycleSkip,
    batch: bool,
    start_phase: Phase,
) -> OrchestrationResult {
    let start = Instant::now();
    // Per-phase wall time, captured as each phase runs so a failure carries
    // the timing of the phases that did complete. trace:TASK-266 | ai:claude
    let mut durations: Vec<(Phase, u128)> = Vec::new();
    if !json {
        let lifecycle_note = lifecycle_skip
            .banner_summary()
            .map(|s| format!("; {s} per lifecycle tag"))
            .unwrap_or_default();
        eprintln!();
        eprintln!(
            "{} {} {}{}",
            "🚀".bold(),
            format!("auto-complete: {spec}").bold(),
            format!("({})", variant.describe()).dimmed(),
            lifecycle_note.dimmed()
        );
    }

    // STORY-492: on a resume that re-enters past phase 1, the implementer
    // already ran in the crashed drain — the branch is pushed and the PR exists
    // (seeded into the driver by the resume handler). Skip the whole phase-1
    // block + the BUG-245 credit check; the resumed phases use the seeded
    // branch/PR. trace:STORY-492 | ai:claude
    let credited: String;
    if start_phase.index() > Phase::Implementer.index() {
        if !json {
            eprintln!(
                "  {} resumed past phase 1 (implementer) — branch + PR already exist",
                "↩".cyan()
            );
        }
        credited = spec.to_string();
    } else {
        // Phase 1 — implementer session. Outcomes: a PR was opened (run on), a
        // genuine failure (reconcile then report), or a punt. STORY-306: a punt no
        // longer stops the drain outright — it routes through the headless advisor
        // tier, which resolves the fork (the implementer resumes, the pipeline
        // continues) or escalates it (the run ends per `escalate_mode`).
        // trace:STORY-276, STORY-306 | ai:claude
        emit_start(Phase::Implementer, spec, json, start.elapsed().as_millis());
        let phase_start = Instant::now();
        match driver.run_implementer() {
            Err(f) => {
                durations.push((Phase::Implementer, phase_start.elapsed().as_millis()));
                return resolve_phase_failure(
                    driver,
                    Phase::Implementer,
                    spec,
                    json,
                    &start,
                    &f,
                    durations,
                );
            }
            Ok(ImplementerOutcome::Punted { reason }) => {
                durations.push((Phase::Implementer, phase_start.elapsed().as_millis()));
                match resolve_punt_via_advisor(
                    driver,
                    spec,
                    json,
                    &start,
                    &durations,
                    &reason,
                    escalate_mode,
                    batch,
                ) {
                    PuntFlow::Terminal(result) => return *result,
                    // The advisor resolved the fork and the implementer resumed
                    // with a PR — the pipeline continues to CI.
                    PuntFlow::Proceed => {
                        emit_done(Phase::Implementer, spec, json, start.elapsed().as_millis());
                    }
                }
            }
            Ok(ImplementerOutcome::PrOpened) => {
                durations.push((Phase::Implementer, phase_start.elapsed().as_millis()));
                emit_done(Phase::Implementer, spec, json, start.elapsed().as_millis());
            }
            // BUG-257 / BUG-266: the orchestrator could not determine whether a
            // PR was opened — either a transient GH-API blip during the PR
            // lookup (BUG-257) or a transient Anthropic-API outage that killed
            // the headless `claude -p` mid-session (BUG-266). The pipeline halts
            // cleanly — exit `0`, no `failed_phase` — and the spec is left in
            // its current state for the next drain. Distinct from a punt (no
            // design-fork was raised) and from a failure (nothing is broken).
            // trace:BUG-257 BUG-266
            Ok(ImplementerOutcome::Inconclusive { reason, retry_hint }) => {
                durations.push((Phase::Implementer, phase_start.elapsed().as_millis()));
                // TASK-136: in a batch drain, shelve-and-advance instead of pausing
                // the whole batch at this head; single-spec keeps the pause.
                if batch {
                    return finish_inconclusive_shelved(
                        driver, spec, json, &start, durations, &reason,
                    );
                }
                return finish_inconclusive(
                    spec,
                    json,
                    &start,
                    durations,
                    &reason,
                    retry_hint.as_deref(),
                );
            }
            // BUG-250: the implementer deliberately held the PR (branch pushed, PR
            // intentionally not opened, pending a manual gate). A clean non-failure
            // stop — exit `0`, no `failed_phase` — distinct from a punt (no
            // design-fork) and a failure (nothing broke). The drain halts at phase
            // 1 with the correct "open the PR when your gate passes" hint.
            // trace:BUG-250
            Ok(ImplementerOutcome::Held { reason, branch }) => {
                durations.push((Phase::Implementer, phase_start.elapsed().as_millis()));
                return finish_held(spec, json, &start, durations, reason.as_deref(), &branch);
            }
        }

        // BUG-245: before running phases 2-6 on the PR, ask the driver whose
        // SPEC-ID the PR's commits actually credit. The implementer can
        // pragmatically ship a *different* spec — e.g., the dispatched spec was
        // blocked and the implementer worked a release blocker instead. Phases
        // 2-6 (CI / reviewer / merge / pull / build) are PR-anchored, not spec-
        // anchored, so they run unchanged on whichever PR is open; the change
        // is in *attribution*: on mismatch, credit the truth in the success
        // epilogue and surface the mismatch as an explicit anomaly. A driver
        // that cannot inspect the PR returns `None`, which preserves pre-BUG-245
        // behaviour (the dispatched id is credited). trace:BUG-245 | ai:claude
        credited = driver
            .shipped_spec_id()
            .filter(|id| id != spec)
            .unwrap_or_else(|| spec.to_string());
        if credited != spec {
            emit_shipped_mismatch(spec, &credited, json, start.elapsed().as_millis());
        }
    } // end phase-1 gate (STORY-492)

    // Phase 2 — end + wait for CI. lifecycle:no-ci-wait (incl. lifecycle:trivial)
    // skips the BLOCKING wait — CI still runs remotely, the orchestrator just
    // doesn't block on it (STORY-442). Mirrors the no_review / no_build skips;
    // the variant last_phase cap below still applies either way. (Review
    // finding: this flag was parsed + bannered but never acted on.)
    if start_phase.index() > Phase::Ci.index() {
        if !json {
            eprintln!("  {} resumed past phase 2 (CI)", "↩".cyan());
        }
    } else if lifecycle_skip.no_ci_wait {
        if !json {
            eprintln!(
                "  {} skipping CI-wait per lifecycle tag (CI still runs remotely)",
                "↷".cyan()
            );
        }
    } else {
        emit_start(Phase::Ci, spec, json, start.elapsed().as_millis());
        let phase_start = Instant::now();
        if let Err(f) = driver.finish_ci() {
            durations.push((Phase::Ci, phase_start.elapsed().as_millis()));
            return resolve_phase_failure(driver, Phase::Ci, spec, json, &start, &f, durations);
        }
        durations.push((Phase::Ci, phase_start.elapsed().as_millis()));
        emit_done(Phase::Ci, spec, json, start.elapsed().as_millis());
    }
    if variant.last_phase() <= 2 {
        return finish_success(spec, &credited, json, &start, durations);
    }

    // Phase 3 — reviewer session. lifecycle:no-review skips only this model
    // phase; merge and pull still run so substrate state stays coherent.
    if start_phase.index() > Phase::Reviewer.index() {
        if !json {
            eprintln!("  {} resumed past phase 3 (reviewer)", "↩".cyan());
        }
    } else if lifecycle_skip.no_review {
        if !json {
            eprintln!("  {} skipping reviewer phase per lifecycle tag", "↷".cyan());
        }
    } else {
        emit_start(Phase::Reviewer, spec, json, start.elapsed().as_millis());
        let phase_start = Instant::now();
        match driver.run_reviewer() {
            Err(f) => {
                durations.push((Phase::Reviewer, phase_start.elapsed().as_millis()));
                return resolve_phase_failure(
                    driver,
                    Phase::Reviewer,
                    spec,
                    json,
                    &start,
                    &f,
                    durations,
                );
            }
            // STORY-306: the reviewer escalated the merge decision to a human
            // rather than auto-deciding it (uncertain zen provenance, an
            // irreversible call). A clean stop, not a failure — exit `0`, no
            // merge runs, the PR is left for a human. Distinct from a
            // non-Approved verdict (which still fails the phase below) and from a
            // crashed reviewer. trace:STORY-306 | ai:claude
            Ok(ReviewerOutcome::EscalatedToHuman { reason }) => {
                durations.push((Phase::Reviewer, phase_start.elapsed().as_millis()));
                return finish_escalated(
                    spec,
                    json,
                    &start,
                    durations,
                    EscalationKind::MergeDecision,
                    &reason,
                );
            }
            Ok(ReviewerOutcome::Verdict(verdict)) if verdict != Verdict::Approved => {
                durations.push((Phase::Reviewer, phase_start.elapsed().as_millis()));
                let f = PhaseFailure::new(format!(
                    "reviewer verdict is {} — not Approved",
                    verdict.label()
                ));
                return resolve_phase_failure(
                    driver,
                    Phase::Reviewer,
                    spec,
                    json,
                    &start,
                    &f,
                    durations,
                );
            }
            Ok(ReviewerOutcome::Verdict(_)) => {}
        }
        durations.push((Phase::Reviewer, phase_start.elapsed().as_millis()));
        emit_done(Phase::Reviewer, spec, json, start.elapsed().as_millis());
    }
    // Latent-defect guard (review finding): mirror the <=2/<=4/<=5 caps so a
    // future variant with last_phase()==3 (e.g. a "through-reviewer" mode)
    // stops here instead of falling through to merge. No current variant
    // returns 3, so this is a no-op today.
    if variant.last_phase() <= 3 {
        return finish_success(spec, &credited, json, &start, durations);
    }

    // Phase 4 — merge. STORY-492: skipped on a resume that re-enters past it —
    // the PR is already merged (the resume handler probed it), and re-running
    // merge would either no-op or get redeemed by the BUG-241 reconcile anyway,
    // but skipping is cleaner. trace:STORY-492 | ai:claude
    if start_phase.index() > Phase::Merge.index() {
        if !json {
            eprintln!(
                "  {} resumed past phase 4 (merge) — PR already merged",
                "↩".cyan()
            );
        }
    } else {
        emit_start(Phase::Merge, spec, json, start.elapsed().as_millis());
        let phase_start = Instant::now();
        if let Err(f) = driver.merge() {
            durations.push((Phase::Merge, phase_start.elapsed().as_millis()));
            return resolve_phase_failure(driver, Phase::Merge, spec, json, &start, &f, durations);
        }
        durations.push((Phase::Merge, phase_start.elapsed().as_millis()));
        emit_done(Phase::Merge, spec, json, start.elapsed().as_millis());
    }
    if variant.last_phase() <= 4 {
        return finish_success(spec, &credited, json, &start, durations);
    }

    // Phase 5 — pull + auto-bump.
    if start_phase.index() > Phase::Pull.index() {
        if !json {
            eprintln!(
                "  {} resumed past phase 5 (pull) — spec already promoted",
                "↩".cyan()
            );
        }
    } else {
        emit_start(Phase::Pull, spec, json, start.elapsed().as_millis());
        let phase_start = Instant::now();
        if let Err(f) = driver.pull() {
            durations.push((Phase::Pull, phase_start.elapsed().as_millis()));
            return resolve_phase_failure(driver, Phase::Pull, spec, json, &start, &f, durations);
        }
        durations.push((Phase::Pull, phase_start.elapsed().as_millis()));
        emit_done(Phase::Pull, spec, json, start.elapsed().as_millis());
    }
    if variant.last_phase() <= 5 {
        return finish_success(spec, &credited, json, &start, durations);
    }

    // Phase 6 — build verify. (Resume always re-runs build — its postcondition
    // is the weakest signal and the build is idempotent. trace:STORY-492)
    if lifecycle_skip.no_build {
        if !json {
            eprintln!("  {} skipping build phase per lifecycle tag", "↷".cyan());
        }
    } else {
        emit_start(Phase::Build, spec, json, start.elapsed().as_millis());
        let phase_start = Instant::now();
        if let Err(f) = driver.build() {
            durations.push((Phase::Build, phase_start.elapsed().as_millis()));
            return resolve_phase_failure(driver, Phase::Build, spec, json, &start, &f, durations);
        }
        durations.push((Phase::Build, phase_start.elapsed().as_millis()));
        emit_done(Phase::Build, spec, json, start.elapsed().as_millis());
    }

    finish_success(spec, &credited, json, &start, durations)
}

// --- Batch drain (TASK-285) -------------------------------------------------
//
// `aida queue work --batch NAME --auto-complete` chains one `orchestrate` run
// per batch member: resolve the batch head, run its full lifecycle, advance to
// the new head, repeat. The *sequencing* lives here behind a [`BatchDriver`]
// trait — the same shape as [`PhaseDriver`] — so the loop (head advance, the
// `--max` cap, the failure-stops-the-drain rule, the non-advancing-queue
// guard) is unit-tested with a mock instead of spawning real orchestrations.
// trace:TASK-285 | ai:claude

/// Why a [`drain_batch`] run stopped. trace:TASK-285 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BatchDrainOutcome {
    /// The batch is empty for the role — every queued member shipped.
    Drained,
    /// The `--max N` cap was reached with members still queued.
    MaxReached,
    /// A phase failed; the drain stopped at the spec in [`BatchDrainResult::stopped_at`].
    Failed(Phase),
    /// A run reported success but the head did not advance — stopped to
    /// avoid an infinite loop on the same spec.
    Stalled,
    /// BUG-245: phase 1's PR shipped a *different* spec than the dispatched
    /// head (the implementer pragmatically worked another spec — typically a
    /// release blocker). The drain credits the truth in `shipped`, leaves
    /// `dispatched` queued for the next pickup, and stops so the operator
    /// can inspect rather than loop on the un-advanced head. `dispatched`
    /// equals the run's `stopped_at`. trace:BUG-245 | ai:claude
    Mismatched { dispatched: String, shipped: String },
    /// BUG-257: phase 1 ended *inconclusively* — the orchestrator could not
    /// determine whether a PR was opened (transient GH-API network error).
    /// The drain pauses at this spec (it is left in its current state for
    /// retry); the next drain re-attempts once the API is reachable. Exit
    /// `0`: a network blip is not a phase failure, so the batch summary does
    /// not crash with a false "shipped 0". trace:BUG-257 | ai:claude
    Inconclusive,
    /// EPIC-28: the batch *fully drained* (no member remained pickable),
    /// but at least one member was shelved by a phase failure or skipped
    /// because its blocker had been shelved earlier in the drain. The
    /// drain did its job — failures were tolerated, dependents skipped,
    /// independents shipped — and the summary points the operator at
    /// `aida findings list` for triage. Exit code is **2** (distinct
    /// from `0` clean drain, `1` stall, and `3..=8` phase failures).
    /// trace:EPIC-28 | ai:claude
    DrainedWithShelved,
    /// BUG-250: a batch member ended in a deliberate *PR-hold* — the
    /// implementer pushed the branch but intentionally held the PR for a manual
    /// gate. Like [`Inconclusive`](Self::Inconclusive) this pauses the drain at
    /// the held spec (it is left in its current state; the operator opens the
    /// PR when the gate passes) and exits `0` — a deliberate hold is not a
    /// failure, so the batch summary must not crash with a false "shipped 0".
    /// trace:BUG-250 | ai:claude
    Held,
}

/// Outcome of a [`drain_batch`] run — what shipped, where it stopped, and the
/// process exit code (`0` drained / max-reached, else the failed phase index
/// or `1` for a stall). trace:TASK-285 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct BatchDrainResult {
    /// Spec-ids that completed their full `--auto-complete` lifecycle, in
    /// drain order.
    pub(crate) shipped: Vec<String>,
    /// STORY-276: spec-ids a headless implementer punted to `NeedsAttention`,
    /// in drain order. Kept apart from `shipped` — a punt advances the drain
    /// (the punted spec leaves the queue on its own) but it did *not* ship, so
    /// the batch summary must not claim it did.
    pub(crate) punted: Vec<String>,
    /// STORY-306: spec-ids the drain *escalated to a human*, in drain order —
    /// the reviewer would not auto-merge, or the headless advisor would not
    /// resolve a punted design-fork. Like `punted`, an escalation advances the
    /// drain but did not ship; kept apart so the batch summary is honest.
    pub(crate) escalated: Vec<String>,
    /// EPIC-28: spec-ids the drain *shelved* on a phase failure, in drain
    /// order. The orchestrator flipped each to `NeedsAttention` with a
    /// structured `FailureReason`; the drain continued past them rather
    /// than halting the whole batch. Kept apart from `shipped` / `punted`
    /// / `escalated` so the summary points the operator at the right
    /// triage path. trace:EPIC-28 | ai:claude
    pub(crate) shelved: Vec<String>,
    /// EPIC-28: members the drain skipped because they were blocked by a
    /// just-shelved (or already-`NeedsAttention`) spec. Each entry is
    /// `(display_id, reason_label)` where `reason_label` is the same
    /// `pickability_reason_label` the queue UI uses. The pure
    /// [`drain_batch`] cannot fill this — the mock has no relationship
    /// graph — so it stays empty here; the real CLI surface
    /// (`resolve_batch_members`) records skips and populates this field
    /// after the fact. trace:EPIC-28 | ai:claude
    pub(crate) skipped: Vec<(String, String)>,
    /// The spec the drain stopped on — set for `Failed` / `Stalled`, `None`
    /// for a clean `Drained` / `MaxReached`.
    pub(crate) stopped_at: Option<String>,
    /// Why the drain stopped.
    pub(crate) outcome: BatchDrainOutcome,
    /// Process exit code: `0` on a clean stop or `DrainedWithShelved`
    /// (no, EPIC-28: that one is `2`), else the failed-phase index
    /// (per STORY-246's exit codes) or `1` for a stall.
    pub(crate) exit_code: i32,
}

/// One completed segment of a multi-batch drain. trace:TASK-310 | ai:codex
#[derive(Debug, Clone)]
pub(crate) struct BatchChainStep {
    // TASK-552: retained for per-batch telemetry/rendering once the chain
    // summary grows beyond aggregate fields.
    #[allow(dead_code)]
    pub(crate) batch_name: String,
    // TASK-552: retained with `batch_name` as the stable per-step result shape
    // for future detailed multi-batch summaries.
    #[allow(dead_code)]
    pub(crate) result: BatchDrainResult,
}

/// Aggregated result for `aida queue work --batches A,B,C --auto-complete`.
/// The individual [`BatchDrainResult`] values stay available so the CLI can
/// render per-batch summaries without losing where the chain stopped.
/// trace:TASK-310 | ai:codex
#[derive(Debug, Clone)]
pub(crate) struct BatchChainDrainResult {
    // TASK-552: retained for future detailed rendering of each chained batch;
    // aggregate fields are what the current CLI consumes.
    #[allow(dead_code)]
    pub(crate) steps: Vec<BatchChainStep>,
    pub(crate) shipped: Vec<String>,
    pub(crate) punted: Vec<String>,
    pub(crate) escalated: Vec<String>,
    /// EPIC-28: spec-ids shelved across every batch in the chain, in chain
    /// order. trace:EPIC-28 | ai:claude
    pub(crate) shelved: Vec<String>,
    /// EPIC-28: dependents the chain skipped across every batch, in chain
    /// order. trace:EPIC-28 | ai:claude
    pub(crate) skipped: Vec<(String, String)>,
    pub(crate) stopped_batch: Option<String>,
    pub(crate) stopped_at: Option<String>,
    pub(crate) outcome: BatchDrainOutcome,
    pub(crate) exit_code: i32,
}

/// Drives a batch drain: yields the current batch head and runs one spec's
/// full `--auto-complete` orchestration. The real implementation re-resolves
/// the `batch:NAME` tag against the queue and calls `run_auto_complete`; the
/// mock stands in for both so the loop is testable. trace:TASK-285 | ai:claude
pub(crate) trait BatchDriver {
    /// The current batch head spec-id, or `None` when the batch is drained.
    /// Re-resolved each call — a completed spec leaves the queue, so the head
    /// advances naturally.
    fn next_head(&mut self) -> Option<String>;
    /// Run one spec's full `--auto-complete` orchestration.
    fn run_spec(&mut self, spec: &str) -> OrchestrationResult;
}

/// Drain a batch: run `orchestrate` per member until the batch is empty, the
/// `--max` cap is hit, or a phase fails un-shelvably. EPIC-28 changes the
/// failure rule: a *shelvable* phase failure (the implementer/CI/reviewer
/// part — `result.shelved_reason.is_some()`) parks the spec in
/// `NeedsAttention` and the drain *continues* to the next member, with
/// dependents skipped naturally because they show up as un-pickable to
/// the next `next_head()` call. The drain stops only when either:
/// (a) `max_failures` shelves have already happened — the environment is
///     probably broken, do not park an entire batch of innocent specs;
/// (b) an un-shelvable failure (env-level: `Spawn`/`MissingTool`/`Internal`)
///     hit — every future member would hit the same wall.
///
/// Pure sequencing; the I/O lives in the [`BatchDriver`].
/// trace:TASK-285 EPIC-28 | ai:claude
pub(crate) fn drain_batch(
    driver: &mut dyn BatchDriver,
    max: Option<usize>,
    max_failures: Option<usize>,
) -> BatchDrainResult {
    let mut shipped: Vec<String> = Vec::new();
    let mut punted: Vec<String> = Vec::new();
    let mut escalated: Vec<String> = Vec::new();
    let mut shelved: Vec<String> = Vec::new();
    let skipped: Vec<(String, String)> = Vec::new();
    loop {
        // Resolve the head first: a `--max` of exactly the batch size should
        // report `Drained` (the batch genuinely emptied), not `MaxReached`.
        let Some(head) = driver.next_head() else {
            // EPIC-28: clean drain or drained-with-shelved. The latter
            // wins whenever any failure shelved a spec, regardless of how
            // many independents shipped after — the operator still has to
            // triage the parked set.
            let outcome = if shelved.is_empty() && skipped.is_empty() {
                BatchDrainOutcome::Drained
            } else {
                BatchDrainOutcome::DrainedWithShelved
            };
            let exit_code = if matches!(outcome, BatchDrainOutcome::DrainedWithShelved) {
                2
            } else {
                0
            };
            return BatchDrainResult {
                shipped,
                punted,
                escalated,
                shelved,
                skipped,
                stopped_at: None,
                outcome,
                exit_code,
            };
        };
        // `--max` bounds how many members the drain *acts on* — shipped,
        // punted, escalated, or shelved each consumed a slot (a full phase
        // attempt).
        if let Some(limit) = max {
            if shipped.len() + punted.len() + escalated.len() + shelved.len() >= limit {
                return BatchDrainResult {
                    shipped,
                    punted,
                    escalated,
                    shelved,
                    skipped,
                    stopped_at: None,
                    outcome: BatchDrainOutcome::MaxReached,
                    exit_code: 0,
                };
            }
        }
        // A spec we already acted on resurfacing as the head means the queue
        // did not advance — its run reported success but it never left the
        // queue, so it was not really shipped. Drop it from `shipped` and
        // stop rather than loop forever on it. (A punted / escalated /
        // shelved spec leaves the queue, so it cannot resurface this way.)
        if shipped.iter().any(|s| s == &head) {
            shipped.retain(|s| s != &head);
            return BatchDrainResult {
                shipped,
                punted,
                escalated,
                shelved,
                skipped,
                stopped_at: Some(head),
                outcome: BatchDrainOutcome::Stalled,
                exit_code: 1,
            };
        }
        let result = driver.run_spec(&head);
        if result.exit_code != 0 {
            let phase = result.failed_phase.unwrap_or(Phase::Implementer);
            // EPIC-28: a shelvable failure parks the spec and the drain
            // continues — unless we have already shelved `max_failures`
            // specs (probably a broken environment) in which case fall
            // back to the historical `Failed` stop.
            // trace:EPIC-28 | ai:claude
            let over_failure_budget = max_failures
                .map(|cap| shelved.len() + 1 > cap)
                .unwrap_or(false);
            if result.shelved_reason.is_some() && !over_failure_budget {
                shelved.push(head);
                continue;
            }
            return BatchDrainResult {
                shipped,
                punted,
                escalated,
                shelved,
                skipped,
                stopped_at: Some(head),
                outcome: BatchDrainOutcome::Failed(phase),
                exit_code: result.exit_code,
            };
        }
        // BUG-257: an inconclusive run pauses the drain. The spec stays in
        // its current state — no ship, no punt, no escalate — so the next
        // drain re-attempts once the GH API is reachable. Continuing here
        // would either stall on the un-advanced head or skip the spec
        // entirely; both are worse than the explicit pause.
        // trace:BUG-257 | ai:claude
        if result.inconclusive_reason.is_some() {
            return BatchDrainResult {
                shipped,
                punted,
                escalated,
                shelved,
                skipped,
                stopped_at: Some(head),
                outcome: BatchDrainOutcome::Inconclusive,
                exit_code: 0,
            };
        }
        // BUG-250: a deliberate PR-hold pauses the drain at this spec. The
        // member did not ship, punt, or fail — it is parked on a pushed branch
        // awaiting the operator's manual gate, so stop cleanly (exit `0`) here
        // rather than falling through to the `shipped` bucket below (which
        // would falsely claim it shipped). The operator opens the PR and
        // re-runs the drain to continue. trace:BUG-250 | ai:claude
        if result.held_reason.is_some() {
            return BatchDrainResult {
                shipped,
                punted,
                escalated,
                shelved,
                skipped,
                stopped_at: Some(head),
                outcome: BatchDrainOutcome::Held,
                exit_code: 0,
            };
        }
        // BUG-245: phase 1 shipped a different spec than the dispatched
        // head. Credit the truth (the id the PR's commits name), leave the
        // dispatched head queued for its own pickup, and stop the drain —
        // the dispatched head did not advance, so a continued drain would
        // either stall on it or double-credit the actual shipped spec.
        // trace:BUG-245 | ai:claude
        if let Some(actual) = result.shipped_spec_id.clone() {
            shipped.push(actual.clone());
            return BatchDrainResult {
                shipped,
                punted,
                escalated,
                shelved,
                skipped,
                stopped_at: Some(head.clone()),
                outcome: BatchDrainOutcome::Mismatched {
                    dispatched: head,
                    shipped: actual,
                },
                exit_code: 0,
            };
        }
        // A punt and an escalation are both clean exits (`exit_code` 0) — the
        // drain advances — but the spec did not ship. Sort each accordingly so
        // the batch summary never claims a parked / escalated spec shipped.
        // trace:STORY-276, STORY-306 | ai:claude
        if result.punt_reason.is_some() {
            punted.push(head);
        } else if result.escalation.is_some() {
            escalated.push(head);
        } else {
            shipped.push(head);
        }
    }
}

/// Drain multiple named batches left-to-right. Each batch is exhausted before
/// the next starts; any non-clean stop (failed phase, stall, mismatch,
/// inconclusive) stops the chain at that batch. Empty batches are clean
/// `Drained` steps and are skipped by the caller's UI.
///
/// `max` is total work across the whole chain. Because [`drain_batch`] counts
/// shipped + punted + escalated + shelved members toward its cap, this helper
/// carries the remaining allowance into each subsequent batch.
/// `max_failures` is **per-batch** (not per-chain) — a chain `A,B,C` keeps an
/// independent failure budget for each batch. trace:TASK-310 EPIC-28
pub(crate) fn drain_batch_chain<'a, F>(
    batch_names: &[String],
    max: Option<usize>,
    max_failures: Option<usize>,
    mut make_driver: F,
) -> BatchChainDrainResult
where
    F: FnMut(&str) -> Box<dyn BatchDriver + 'a>,
{
    let mut steps = Vec::new();
    let mut shipped = Vec::new();
    let mut punted = Vec::new();
    let mut escalated = Vec::new();
    let mut shelved: Vec<String> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    let mut any_shelved_or_skipped = false;

    for batch_name in batch_names {
        let consumed = shipped.len() + punted.len() + escalated.len() + shelved.len();
        let remaining = max.map(|limit| limit.saturating_sub(consumed));
        let mut driver = make_driver(batch_name);
        let result = drain_batch(driver.as_mut(), remaining, max_failures);

        shipped.extend(result.shipped.iter().cloned());
        punted.extend(result.punted.iter().cloned());
        escalated.extend(result.escalated.iter().cloned());
        shelved.extend(result.shelved.iter().cloned());
        skipped.extend(result.skipped.iter().cloned());
        if !result.shelved.is_empty() || !result.skipped.is_empty() {
            any_shelved_or_skipped = true;
        }

        let step = BatchChainStep {
            batch_name: batch_name.clone(),
            result: result.clone(),
        };
        steps.push(step);

        match result.outcome {
            BatchDrainOutcome::Drained | BatchDrainOutcome::DrainedWithShelved => continue,
            BatchDrainOutcome::MaxReached => {
                return BatchChainDrainResult {
                    steps,
                    shipped,
                    punted,
                    escalated,
                    shelved,
                    skipped,
                    stopped_batch: Some(batch_name.clone()),
                    stopped_at: result.stopped_at,
                    outcome: BatchDrainOutcome::MaxReached,
                    exit_code: result.exit_code,
                };
            }
            BatchDrainOutcome::Failed(phase) => {
                return BatchChainDrainResult {
                    steps,
                    shipped,
                    punted,
                    escalated,
                    shelved,
                    skipped,
                    stopped_batch: Some(batch_name.clone()),
                    stopped_at: result.stopped_at,
                    outcome: BatchDrainOutcome::Failed(phase),
                    exit_code: result.exit_code,
                };
            }
            BatchDrainOutcome::Stalled => {
                return BatchChainDrainResult {
                    steps,
                    shipped,
                    punted,
                    escalated,
                    shelved,
                    skipped,
                    stopped_batch: Some(batch_name.clone()),
                    stopped_at: result.stopped_at,
                    outcome: BatchDrainOutcome::Stalled,
                    exit_code: result.exit_code,
                };
            }
            BatchDrainOutcome::Mismatched {
                dispatched,
                shipped: actual,
            } => {
                return BatchChainDrainResult {
                    steps,
                    shipped,
                    punted,
                    escalated,
                    shelved,
                    skipped,
                    stopped_batch: Some(batch_name.clone()),
                    stopped_at: result.stopped_at,
                    outcome: BatchDrainOutcome::Mismatched {
                        dispatched,
                        shipped: actual,
                    },
                    exit_code: result.exit_code,
                };
            }
            BatchDrainOutcome::Inconclusive => {
                return BatchChainDrainResult {
                    steps,
                    shipped,
                    punted,
                    escalated,
                    shelved,
                    skipped,
                    stopped_batch: Some(batch_name.clone()),
                    stopped_at: result.stopped_at,
                    outcome: BatchDrainOutcome::Inconclusive,
                    exit_code: result.exit_code,
                };
            }
            // BUG-250: a deliberate PR-hold pauses the chain at this batch,
            // exactly like an inconclusive run — the held spec awaits the
            // operator's gate, so the chain stops cleanly rather than rolling
            // on to the next batch. trace:BUG-250 | ai:claude
            BatchDrainOutcome::Held => {
                return BatchChainDrainResult {
                    steps,
                    shipped,
                    punted,
                    escalated,
                    shelved,
                    skipped,
                    stopped_batch: Some(batch_name.clone()),
                    stopped_at: result.stopped_at,
                    outcome: BatchDrainOutcome::Held,
                    exit_code: result.exit_code,
                };
            }
        }
    }

    // Every batch drained cleanly. EPIC-28: if any individual batch reported
    // a shelve/skip, the chain summary is `DrainedWithShelved` (exit 2) so the
    // operator still sees the parked set.
    let (outcome, exit_code) = if any_shelved_or_skipped {
        (BatchDrainOutcome::DrainedWithShelved, 2)
    } else {
        (BatchDrainOutcome::Drained, 0)
    };
    BatchChainDrainResult {
        steps,
        shipped,
        punted,
        escalated,
        shelved,
        skipped,
        stopped_batch: None,
        stopped_at: None,
        outcome,
        exit_code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// STORY-265 slice 3: with `--with-plan`, the prelude is a plan session
    /// THEN a promote — ordered, before the drain. Fully isolated: pure
    /// function, no driver, no filesystem. trace:STORY-265
    #[test]
    fn with_plan_prelude_is_plan_session_then_promote() {
        let steps = plan_prelude_steps("STORY-265", true, false);
        assert_eq!(
            steps,
            vec![
                PlanPreludeStep::PlanSession {
                    spec: "STORY-265".to_string(),
                    headless: false,
                },
                PlanPreludeStep::Promote {
                    spec: "STORY-265".to_string(),
                },
            ],
            "with-plan ⇒ plan session then promote, in that order"
        );
    }

    /// STORY-265 slice 3: without `--with-plan`, there is NO prelude — the
    /// drain runs alone, exactly the historical default (unchanged per the
    /// operator's slice-1 opt-in decision). trace:STORY-265
    #[test]
    fn without_with_plan_prelude_is_empty_drain_only() {
        assert!(
            plan_prelude_steps("STORY-265", false, false).is_empty(),
            "no --with-plan ⇒ drain only, no prelude steps"
        );
        // The headless flag is irrelevant when with_plan is off.
        assert!(plan_prelude_steps("STORY-265", false, true).is_empty());
    }

    /// STORY-265 slice 3: a headless drain (`--no-human=both`) runs the plan
    /// session headless too, mirroring how the phase-1 implementer launches.
    /// trace:STORY-265
    #[test]
    fn with_plan_prelude_propagates_headless_to_plan_session() {
        let steps = plan_prelude_steps("STORY-265", true, true);
        assert_eq!(
            steps[0],
            PlanPreludeStep::PlanSession {
                spec: "STORY-265".to_string(),
                headless: true,
            },
            "headless implementer ⇒ headless plan session"
        );
        // The promote step is plain — it is a status edit, never headless.
        assert_eq!(
            steps[1],
            PlanPreludeStep::Promote {
                spec: "STORY-265".to_string(),
            }
        );
    }

    /// BUG-455: a "database is locked" message is recognised (case-insensitive,
    /// both SQLite lock spellings) and nothing else trips the classifier.
    /// Pure-function test — no driver, no filesystem. trace:BUG-455
    #[test]
    fn database_locked_message_is_recognised() {
        assert!(is_database_locked_message(
            "database is locked while trying to shelve"
        ));
        assert!(is_database_locked_message("DATABASE IS LOCKED")); // case-insensitive
        assert!(is_database_locked_message(
            "Error: the database table is locked: requirements"
        ));
        // Unrelated failures must NOT be misread as a cache lock.
        assert!(!is_database_locked_message("CI run failed (red)"));
        assert!(!is_database_locked_message("no PR was opened"));
        assert!(!is_database_locked_message(""));
    }

    /// BUG-455: a transient cache-lock failure is reclassified to the shelvable
    /// `CacheLocked` kind, so a batch drain parks the spec and continues instead
    /// of hard-stopping; an unrelated failure keeps its kind. Idempotent.
    /// Pure-function test — fully isolated. trace:BUG-455
    #[test]
    fn cache_lock_failure_reclassifies_to_shelvable() {
        // A reviewer phase that surfaced a cache lock comes in with the default
        // `Failed` kind (or could even be `Internal`); either way it upgrades.
        let f = PhaseFailure::of(
            FailureKind::Internal,
            "database is locked while trying to shelve SPEC into Needs Attention",
        )
        .reclassify_transient();
        assert_eq!(f.kind, FailureKind::CacheLocked);
        assert!(
            f.kind.is_shelvable(),
            "a cache-lock failure must be shelvable so the drain continues"
        );

        // Idempotent: re-running leaves an already-CacheLocked failure alone.
        let again = f.reclassify_transient();
        assert_eq!(again.kind, FailureKind::CacheLocked);

        // An unrelated failure is untouched.
        let unrelated =
            PhaseFailure::of(FailureKind::CiRed, "CI run failed (red)").reclassify_transient();
        assert_eq!(unrelated.kind, FailureKind::CiRed);

        // A non-shelvable env failure with no lock text stays non-shelvable.
        let spawn = PhaseFailure::of(FailureKind::Spawn, "spawn ENOENT: claude not on PATH")
            .reclassify_transient();
        assert_eq!(spawn.kind, FailureKind::Spawn);
        assert!(!spawn.kind.is_shelvable());
    }

    /// BUG-420: the watchdog trips on no-progress first, ceiling as backstop;
    /// 0 disables a check. trace:BUG-420
    #[test]
    fn watchdog_verdict_no_progress_then_ceiling() {
        use std::time::Duration;
        let (np, ceil) = (Duration::from_secs(600), Duration::from_secs(2700));
        // No progress for >= limit → NoProgress.
        assert_eq!(
            watchdog_verdict(Duration::from_secs(600), Duration::from_secs(100), np, ceil),
            Some(WatchdogTrip::NoProgress)
        );
        // Progress recent but total >= ceiling → Ceiling.
        assert_eq!(
            watchdog_verdict(Duration::from_secs(10), Duration::from_secs(2700), np, ceil),
            Some(WatchdogTrip::Ceiling)
        );
        // Both within limits → None (keep running).
        assert_eq!(
            watchdog_verdict(Duration::from_secs(60), Duration::from_secs(120), np, ceil),
            None
        );
        // No-progress takes precedence when both would trip.
        assert_eq!(
            watchdog_verdict(
                Duration::from_secs(600),
                Duration::from_secs(2700),
                np,
                ceil
            ),
            Some(WatchdogTrip::NoProgress)
        );
        // Zero limits disable both checks.
        assert_eq!(
            watchdog_verdict(
                Duration::from_secs(99_999),
                Duration::from_secs(99_999),
                Duration::ZERO,
                Duration::ZERO
            ),
            None
        );
    }

    /// TASK-136: backoff schedule is 30s/1m/5m, then 15m for any beyond. trace:TASK-136
    #[test]
    fn gh_verify_backoff_schedule_steps() {
        use std::time::Duration;
        assert_eq!(
            gh_verify_backoff_schedule(3),
            vec![
                Duration::from_secs(30),
                Duration::from_secs(60),
                Duration::from_secs(300)
            ]
        );
        assert_eq!(
            gh_verify_backoff_schedule(5).last().copied(),
            Some(Duration::from_secs(900))
        );
        assert!(gh_verify_backoff_schedule(0).is_empty());
    }

    /// Mock driver: records the phases invoked, optionally fails at one, and
    /// returns a configurable verdict. Stands in for the real Claude / CI /
    /// gh / cargo machinery so the orchestrator's sequencing is tested
    /// end-to-end without spawning anything.
    struct MockPhaseDriver {
        calls: Vec<Phase>,
        fail_at: Option<Phase>,
        verdict: Verdict,
        /// BUG-241: what [`PhaseDriver::reconcile_failure`] returns. Defaults
        /// to `GenuineFailure` so every pre-BUG-241 failure test is unchanged.
        reconcile: PhaseReconcile,
        /// STORY-276: when `Some`, `run_implementer` returns
        /// [`ImplementerOutcome::Punted`] with this reason instead of opening
        /// a PR — the headless-implementer design-fork punt.
        punt: Option<String>,
        /// BUG-245: what [`PhaseDriver::shipped_spec_id`] returns. `None`
        /// (default) preserves the pre-BUG-245 behaviour — the dispatched id
        /// is credited.
        shipped_spec_id: Option<String>,
        /// STORY-306: when `Some`, `run_reviewer` returns
        /// [`ReviewerOutcome::EscalatedToHuman`] with this reason instead of a
        /// verdict — the reviewer escalated the merge decision.
        reviewer_escalates: Option<String>,
        /// STORY-306: what `run_advisor` returns; `None` ⇒ the trait default
        /// (an `Internal` error). Set by `advisor_resolves` /
        /// `advisor_escalates`.
        advisor: Option<AdvisorOutcome>,
        /// STORY-306: how many times `run_advisor` was called — a re-punt
        /// after a resume must not spawn a second advisor.
        advisor_calls: usize,
        /// STORY-306: what `resume_implementer` returns; `None` ⇒ the trait
        /// default error. Set by `resume_opens_pr` / `resume_repunts`.
        resume: Option<ImplementerOutcome>,
        /// BUG-257: when `Some`, `run_implementer` returns
        /// [`ImplementerOutcome::Inconclusive`] with this reason — the
        /// orchestrator's phase-1 PR lookup hit a transient GH-API blip and
        /// cannot tell whether a PR was opened. The drain pauses.
        inconclusive: Option<String>,
        /// BUG-250: when set, `run_implementer` returns
        /// [`ImplementerOutcome::Held`] — the implementer deliberately held the
        /// PR (branch pushed, PR withheld for a manual gate). The drain reports
        /// a clean `Held` outcome, not a phase-1 failure.
        held: Option<String>,
        /// TASK-358: how many times `mark_implementer_lease_escalated` was
        /// called. The `--escalate-blocks` path stamps it once before
        /// `finish_escalated`; the `--escalate-defaults` resume path and
        /// every non-escalation flow must leave it at zero so an advisor
        /// resume's worktree is preserved.
        mark_escalated_calls: usize,
        /// TASK-136: when `true`, `shelve_on_failure` succeeds and returns a
        /// `FailureReason` the way a real store-backed driver does, so a
        /// batch-mode inconclusive can route through the EPIC-28 shelve→advance
        /// path. `false` (default) keeps the trait default `Ok(None)`.
        shelve_succeeds: bool,
    }

    impl MockPhaseDriver {
        /// The all-defaults base — every phase green, no punt / escalate /
        /// advisor. The named constructors tweak it with struct-update syntax,
        /// so a new field is added in exactly one place.
        fn base() -> Self {
            Self {
                calls: Vec::new(),
                fail_at: None,
                verdict: Verdict::Approved,
                reconcile: PhaseReconcile::GenuineFailure,
                punt: None,
                shipped_spec_id: None,
                reviewer_escalates: None,
                advisor: None,
                advisor_calls: 0,
                resume: None,
                inconclusive: None,
                held: None,
                mark_escalated_calls: 0,
                shelve_succeeds: false,
            }
        }

        fn all_ok() -> Self {
            Self::base()
        }

        /// TASK-136: `run_implementer` returns
        /// [`ImplementerOutcome::Inconclusive`] AND `shelve_on_failure`
        /// succeeds — the batch-mode shelve→advance path. In a single-spec
        /// drain the same driver still pauses (the `batch` flag, not the
        /// driver, decides).
        fn inconclusive_and_shelves(reason: &str) -> Self {
            Self {
                inconclusive: Some(reason.to_string()),
                shelve_succeeds: true,
                ..Self::base()
            }
        }

        fn failing_at(phase: Phase) -> Self {
            Self {
                fail_at: Some(phase),
                ..Self::base()
            }
        }

        fn with_verdict(verdict: Verdict) -> Self {
            Self {
                verdict,
                ..Self::base()
            }
        }

        /// STORY-276: make `run_implementer` punt — phase 1 returns
        /// [`ImplementerOutcome::Punted`] with `reason`.
        fn punting_at_implementer(reason: &str) -> Self {
            Self {
                punt: Some(reason.to_string()),
                ..Self::base()
            }
        }

        /// BUG-257: make `run_implementer` return
        /// [`ImplementerOutcome::Inconclusive`] — the phase-1 PR lookup hit
        /// a transient GH-API blip and cannot confirm a PR either way.
        fn inconclusive_at_implementer(reason: &str) -> Self {
            Self {
                inconclusive: Some(reason.to_string()),
                ..Self::base()
            }
        }

        /// BUG-250: make `run_implementer` return [`ImplementerOutcome::Held`]
        /// — the implementer deliberately held the PR for a manual gate.
        fn holding_at_implementer(reason: &str) -> Self {
            Self {
                held: Some(reason.to_string()),
                ..Self::base()
            }
        }

        /// STORY-306: make `run_reviewer` escalate the merge decision —
        /// phase 3 returns [`ReviewerOutcome::EscalatedToHuman`] with `reason`.
        fn reviewer_escalates_merge(reason: &str) -> Self {
            Self {
                reviewer_escalates: Some(reason.to_string()),
                ..Self::base()
            }
        }

        /// BUG-245: make this driver's [`PhaseDriver::shipped_spec_id`] return
        /// `Some(id)` — phase 1's PR credits this spec instead of the
        /// dispatched id, so the orchestrator must report the mismatch.
        fn shipping_as(mut self, shipped: &str) -> Self {
            self.shipped_spec_id = Some(shipped.to_string());
            self
        }

        /// BUG-241: make this driver's [`PhaseDriver::reconcile_failure`]
        /// report `reconcile` — the ground-truth verdict the orchestrator
        /// consults before declaring a phase failed.
        fn reconciles_as(mut self, reconcile: PhaseReconcile) -> Self {
            self.reconcile = reconcile;
            self
        }

        /// STORY-306: make `run_advisor` resolve the punted fork with `answer`.
        fn advisor_resolves(mut self, answer: &str) -> Self {
            self.advisor = Some(AdvisorOutcome::Resolved {
                answer: answer.to_string(),
                reasoning: "mock advisor reasoning".to_string(),
            });
            self
        }

        /// STORY-306: make `run_advisor` escalate the punted fork to a human.
        fn advisor_escalates(mut self, reason: &str) -> Self {
            self.advisor = Some(AdvisorOutcome::Escalated {
                reason: reason.to_string(),
                category: "strategy".to_string(),
            });
            self
        }

        /// STORY-306: make `resume_implementer` open a PR — the resumed
        /// implementer shipped on the advisor's answer.
        fn resume_opens_pr(mut self) -> Self {
            self.resume = Some(ImplementerOutcome::PrOpened);
            self
        }

        /// STORY-306: make `resume_implementer` punt again — the re-punt that
        /// must be terminal (one advisor round per spec).
        fn resume_repunts(mut self, reason: &str) -> Self {
            self.resume = Some(ImplementerOutcome::Punted {
                reason: reason.to_string(),
            });
            self
        }

        fn record(&mut self, phase: Phase) -> Result<(), PhaseFailure> {
            self.calls.push(phase);
            if self.fail_at == Some(phase) {
                Err(PhaseFailure::new(format!("mock failure at {phase:?}")))
            } else {
                Ok(())
            }
        }
    }

    impl PhaseDriver for MockPhaseDriver {
        fn run_implementer(&mut self) -> Result<ImplementerOutcome, PhaseFailure> {
            self.record(Phase::Implementer)?;
            if let Some(reason) = &self.inconclusive {
                return Ok(ImplementerOutcome::Inconclusive {
                    reason: reason.clone(),
                    retry_hint: None,
                });
            }
            if let Some(reason) = &self.held {
                return Ok(ImplementerOutcome::Held {
                    reason: Some(reason.clone()),
                    branch: "held-branch".to_string(),
                });
            }
            match &self.punt {
                Some(reason) => Ok(ImplementerOutcome::Punted {
                    reason: reason.clone(),
                }),
                None => Ok(ImplementerOutcome::PrOpened),
            }
        }
        fn finish_ci(&mut self) -> Result<(), PhaseFailure> {
            self.record(Phase::Ci)
        }
        fn run_reviewer(&mut self) -> Result<ReviewerOutcome, PhaseFailure> {
            self.record(Phase::Reviewer)?;
            match &self.reviewer_escalates {
                Some(reason) => Ok(ReviewerOutcome::EscalatedToHuman {
                    reason: reason.clone(),
                }),
                None => Ok(ReviewerOutcome::Verdict(self.verdict)),
            }
        }
        fn merge(&mut self) -> Result<(), PhaseFailure> {
            self.record(Phase::Merge)
        }
        fn pull(&mut self) -> Result<(), PhaseFailure> {
            self.record(Phase::Pull)
        }
        fn build(&mut self) -> Result<(), PhaseFailure> {
            self.record(Phase::Build)
        }
        fn hint_context(&self) -> HintContext {
            HintContext {
                spec: "TASK-247".to_string(),
                branch: Some("task-247".to_string()),
                pr_number: Some(46),
                implementer_session: Some("019e2f423e7c".to_string()),
                ci_run_id: Some("9988776655".to_string()),
                forge: crate::forge::ForgeKind::GitHub,
            }
        }
        fn reconcile_failure(&mut self, _phase: Phase, _failure: &PhaseFailure) -> PhaseReconcile {
            self.reconcile.clone()
        }
        fn shipped_spec_id(&mut self) -> Option<String> {
            self.shipped_spec_id.clone()
        }
        fn run_advisor(&mut self) -> Result<AdvisorOutcome, PhaseFailure> {
            self.advisor_calls += 1;
            self.advisor.clone().ok_or_else(|| {
                PhaseFailure::of(FailureKind::Internal, "mock: no advisor outcome configured")
            })
        }
        fn resume_implementer(
            &mut self,
            _answer: &str,
        ) -> Result<ImplementerOutcome, PhaseFailure> {
            self.resume.clone().ok_or_else(|| {
                PhaseFailure::of(FailureKind::Internal, "mock: no resume outcome configured")
            })
        }
        fn mark_implementer_lease_escalated(&mut self) {
            self.mark_escalated_calls += 1;
        }
        fn shelve_on_failure(
            &mut self,
            _spec: &str,
            phase: Phase,
            failure: &PhaseFailure,
            recovery_hint: &str,
        ) -> anyhow::Result<Option<aida_core::FailureReason>> {
            if !self.shelve_succeeds {
                return Ok(None);
            }
            Ok(Some(aida_core::FailureReason {
                phase: phase.slug().to_string(),
                phase_index: phase.index() as u8,
                kind: failure.kind.slug().to_string(),
                detail: failure.reason.clone(),
                recovery_hint: Some(recovery_hint.to_string()),
                shelved_by: None,
                shelved_at: chrono::Utc::now(),
            }))
        }
    }

    // --- Core orchestration: the mock-Claude integration test -------------

    #[test]
    fn lifecycle_skip_from_tags_parses_tags_case_insensitively() {
        let skip = LifecycleSkip::from_tags([
            "Lifecycle:No-CI-Wait",
            "lifecycle:no-review",
            "LIFECYCLE:NO-BUILD",
            "lifecycle:unknown",
        ]);
        assert!(skip.no_ci_wait);
        assert!(skip.no_review);
        assert!(skip.no_build);
    }

    /// TASK-525: active_tokens lists exactly the set skips (telemetry payload),
    /// empty when none, all three for `trivial`.
    #[test]
    fn lifecycle_skip_active_tokens() {
        assert!(LifecycleSkip::none().active_tokens().is_empty());
        assert_eq!(
            LifecycleSkip::from_tags(["lifecycle:no-review"]).active_tokens(),
            vec!["no-review".to_string()]
        );
        assert_eq!(
            LifecycleSkip::from_tags(["lifecycle:trivial"]).active_tokens(),
            vec![
                "no-ci-wait".to_string(),
                "no-review".to_string(),
                "no-build".to_string()
            ]
        );
    }

    #[test]
    fn lifecycle_skip_trivial_implies_all_three() {
        let skip = LifecycleSkip::from_tags(["lifecycle:trivial"]);
        assert_eq!(
            skip,
            LifecycleSkip {
                no_ci_wait: true,
                no_review: true,
                no_build: true,
            }
        );
    }

    /// TASK-524: the typo guard flags `lifecycle:*` tags that aren't recognized
    /// short-circuits (they silently no-op), while passing the real ones and
    /// non-lifecycle tags. Stays in sync with `from_tags` via the shared
    /// RECOGNIZED_LIFECYCLE_TAGS list.
    #[test]
    fn unrecognized_lifecycle_tag_typo_guard() {
        use super::is_unrecognized_lifecycle_tag;
        // Recognized → not flagged (case-insensitive, like from_tags).
        for t in [
            "lifecycle:no-ci-wait",
            "lifecycle:no-review",
            "lifecycle:no-build",
            "lifecycle:trivial",
            "Lifecycle:Trivial",
        ] {
            assert!(!is_unrecognized_lifecycle_tag(t), "recognized: {t}");
        }
        // Typos in the lifecycle namespace → flagged.
        for t in [
            "lifecycle:no-ci",     // missing -wait
            "lifecycle:trivai",    // misspelled
            "lifecycle:no-builds", // plural
            "lifecycle:skip-ci",   // wrong name
        ] {
            assert!(is_unrecognized_lifecycle_tag(t), "typo: {t}");
        }
        // Non-lifecycle tags are never flagged.
        for t in ["papercut", "batch:x", "from-review:PR-1", "aida:queue:work"] {
            assert!(!is_unrecognized_lifecycle_tag(t), "non-lifecycle: {t}");
        }
        // Every recognized tag must agree with from_tags (no drift).
        for t in super::RECOGNIZED_LIFECYCLE_TAGS {
            assert!(
                !LifecycleSkip::from_tags([*t]).is_empty(),
                "{t} should parse"
            );
        }
    }

    #[test]
    fn lifecycle_skip_unknown_lifecycle_tag_is_ignored() {
        assert_eq!(
            LifecycleSkip::from_tags(["lifecycle:no-ci", "papercut"]),
            LifecycleSkip::none()
        );
    }

    #[test]
    fn lifecycle_skip_banner_summary_lists_skipped_phases() {
        let skip = LifecycleSkip {
            no_ci_wait: true,
            no_review: true,
            no_build: false,
        };
        assert_eq!(
            skip.banner_summary().as_deref(),
            Some("skipping CI wait + reviewer")
        );
    }

    #[test]
    fn orchestrate_full_pipeline_runs_all_six_phases() {
        let mut driver = MockPhaseDriver::all_ok();
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 0);
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ]
        );
    }

    #[test]
    fn orchestrate_no_review_skips_phase_3_and_runs_merge_pull_build() {
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate_with_lifecycle_skip(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip {
                no_review: true,
                ..LifecycleSkip::none()
            },
            false,
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Merge,
                Phase::Pull,
                Phase::Build
            ]
        );
    }

    #[test]
    fn orchestrate_no_build_skips_phase_6_only() {
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate_with_lifecycle_skip(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip {
                no_build: true,
                ..LifecycleSkip::none()
            },
            false,
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull
            ]
        );
    }

    #[test]
    fn orchestrate_trivial_skips_ci_reviewer_and_build_keeps_merge_and_pull() {
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate_with_lifecycle_skip(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip::from_tags(["lifecycle:trivial"]),
            false,
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer, Phase::Merge, Phase::Pull,],
            "trivial skips the ci-wait, reviewer, and build phases; merge + pull \
             are substrate-integrity phases and must not be skipped"
        );
    }

    #[test]
    fn orchestrate_no_ci_wait_skips_ci_keeps_rest() {
        // Review finding: lifecycle:no-ci-wait must skip the blocking CI phase
        // (CI still runs remotely) while every other phase proceeds.
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate_with_lifecycle_skip(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip {
                no_ci_wait: true,
                ..LifecycleSkip::none()
            },
            false,
        );
        assert_eq!(result.exit_code, 0);
        assert!(
            !driver.calls.contains(&Phase::Ci),
            "no-ci-wait must skip the CI phase: {:?}",
            driver.calls
        );
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ],
            "only the ci-wait phase is skipped"
        );
    }

    #[test]
    fn orchestrate_no_review_unreachable_reviewer_escalation_does_not_fire() {
        let mut driver = MockPhaseDriver::reviewer_escalates_merge("would have escalated");
        let result = orchestrate_with_lifecycle_skip(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip {
                no_review: true,
                ..LifecycleSkip::none()
            },
            false,
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.escalation.is_none());
        assert!(!driver.calls.contains(&Phase::Reviewer));
    }

    // --- STORY-306: the headless advisor tier -----------------------------

    /// A punt the advisor is confident about: it resolves the fork, the
    /// implementer resumes and opens a PR, and the full pipeline runs. The
    /// run is a clean success — neither an escalation nor a punt.
    #[test]
    fn orchestrate_punt_advisor_resolves_then_resume_runs_full_pipeline() {
        let mut driver = MockPhaseDriver::punting_at_implementer("auth flow fork")
            .advisor_resolves("use OAuth — the recorded convention")
            .resume_opens_pr();
        let result = orchestrate(
            &mut driver,
            "STORY-306",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert!(
            result.escalation.is_none(),
            "a resolved punt is not an escalation"
        );
        assert!(
            result.punt_reason.is_none(),
            "a resolved-and-resumed punt is not a punt"
        );
        // All six phases ran — the resumed implementer's PR flows through CI.
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ]
        );
        assert_eq!(driver.advisor_calls, 1);
    }

    #[test]
    fn orchestrate_text_question_punt_routes_to_advisor_not_phase1_failure() {
        let mut driver = MockPhaseDriver::punting_at_implementer(
            "headless implementer exited without opening a PR after asking: Confirm and I'll proceed?",
        )
        .advisor_resolves("proceed with option A")
        .resume_opens_pr();
        let result = orchestrate(
            &mut driver,
            "BUG-374",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.failure.is_none(), "text-question punt is not NoPr");
        assert_eq!(driver.advisor_calls, 1);
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ]
        );
    }

    /// A punt the advisor cannot safely judge, under `--escalate-blocks`: the
    /// advisor escalates, the run stops clean after phase 1 — phases 2-6 never
    /// run — exits `0`, and the result carries the escalation.
    ///
    /// TASK-358: also asserts the orchestrator stamps the implementer's lease
    /// (`mark_implementer_lease_escalated`) exactly once on this path — the
    /// marker is what `aida edit --status` out of `NeedsAttention` and
    /// `aida session prune --escalations` later use to clean up the
    /// otherwise-lingering worktree. trace:TASK-358 | ai:claude
    #[test]
    fn orchestrate_punt_advisor_escalates_blocks_skips_phases_2_to_6() {
        let mut driver = MockPhaseDriver::punting_at_implementer("project-strategy fork")
            .advisor_escalates("a strategy call with no recorded principle");
        let result = orchestrate(
            &mut driver,
            "STORY-306",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0, "an escalation is a clean exit");
        assert_eq!(result.failed_phase, None);
        let escalation = result.escalation.expect("escalation recorded");
        assert_eq!(escalation.kind, EscalationKind::DesignFork);
        // The pipeline stopped at phase 1 — only the implementer ran.
        assert_eq!(driver.calls, vec![Phase::Implementer]);
        assert_eq!(driver.advisor_calls, 1);
        // TASK-358: the lease was stamped escalated_to_human exactly once.
        assert_eq!(
            driver.mark_escalated_calls, 1,
            "--escalate-blocks must mark the lease so the lingering worktree gets cleaned"
        );
    }

    /// A punt the advisor escalates, under `--escalate-defaults`: instead of
    /// stopping, the implementer is resumed with the defensible-default
    /// instruction, opens a PR, and the full pipeline runs.
    ///
    /// TASK-358: also asserts the orchestrator does NOT stamp the lease as
    /// escalated_to_human on this path — the resume needs the worktree alive,
    /// and a stray marker would have a later triage out of NeedsAttention
    /// (the implementer's own `aida edit --status in-progress` on resume)
    /// nuke the worktree under its feet. trace:TASK-358 | ai:claude
    #[test]
    fn orchestrate_punt_advisor_escalates_defaults_resumes_with_default() {
        let mut driver = MockPhaseDriver::punting_at_implementer("flag-naming fork")
            .advisor_escalates("no recorded flag convention")
            .resume_opens_pr();
        let result = orchestrate(
            &mut driver,
            "STORY-306",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Defaults,
        );
        assert_eq!(result.exit_code, 0);
        // `--escalate-defaults` resumes rather than stopping → a full ship.
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ]
        );
        assert_eq!(driver.advisor_calls, 1);
        // TASK-358: the lease must stay un-stamped so the resume's worktree
        // is preserved — the resume is what consumes it.
        assert_eq!(
            driver.mark_escalated_calls, 0,
            "--escalate-defaults must NOT mark the lease — the resume relies on its worktree"
        );
    }

    /// A re-punt after a resume is terminal — one advisor round per spec per
    /// drain. The resumed implementer punts again; the run ends via
    /// `finish_punted` (exit `0`, `punt_reason` set) and the advisor is
    /// spawned exactly once.
    #[test]
    fn orchestrate_advisor_resume_repunt_is_terminal() {
        let mut driver = MockPhaseDriver::punting_at_implementer("first fork")
            .advisor_resolves("answer A")
            .resume_repunts("a fresh fork the answer surfaced");
        let result = orchestrate(
            &mut driver,
            "STORY-306",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0, "a re-punt is a clean exit");
        assert_eq!(
            result.punt_reason.as_deref(),
            Some("a fresh fork the answer surfaced"),
        );
        // Phases 2-6 never ran; the advisor was spawned exactly once.
        assert_eq!(driver.calls, vec![Phase::Implementer]);
        assert_eq!(driver.advisor_calls, 1, "no second advisor round");
    }

    /// `EscalateMode::from_flags` — `--escalate-defaults` set ⇒ `Defaults`;
    /// absent ⇒ the conservative `Blocks` default.
    #[test]
    fn escalate_mode_from_flags_defaults_to_blocks() {
        assert_eq!(EscalateMode::from_flags(false), EscalateMode::Blocks);
        assert_eq!(EscalateMode::from_flags(true), EscalateMode::Defaults);
    }

    // --- BUG-257: Inconclusive phase-1 outcome ---------------------------
    //
    // A transient GH-API network error during the post-implementer PR lookup
    // must NOT be reported as a phase-1 failure (that gave the operator a
    // wrong recovery hint and crashed batch drains on a network blip). It is
    // a first-class non-failure outcome: the run exits 0, no `failed_phase`,
    // `inconclusive_reason` is set, and no later phase runs.

    /// Acceptance #4 — "Test with a mocked API failure: orchestrator reports
    /// `Inconclusive`, drain pauses (not fails)." trace:BUG-257 | ai:claude
    #[test]
    fn orchestrate_inconclusive_at_phase1_is_not_a_failure() {
        let mut driver = MockPhaseDriver::inconclusive_at_implementer(
            "GH API unreachable (error connecting to api.github.com); \
             branch `bug-257` is on origin so a PR may exist",
        );
        let result = orchestrate(
            &mut driver,
            "BUG-257",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(
            result.exit_code, 0,
            "an inconclusive run is a clean exit, not a failure"
        );
        assert!(
            result.failed_phase.is_none(),
            "no `failed_phase` — the drain paused, nothing crashed",
        );
        assert!(result.failure.is_none(), "no `PhaseFailure` payload");
        assert!(
            result.inconclusive_reason.is_some(),
            "`inconclusive_reason` distinguishes this from a clean ship",
        );
        assert!(result.punt_reason.is_none(), "not a punt");
        assert!(result.escalation.is_none(), "not an escalation");
        assert!(result.shipped_spec_id.is_none(), "nothing shipped");
        // Phases 2-6 never ran — the drain paused at phase 1.
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    // --- TASK-136: inconclusive → shelve-and-advance in a batch drain -----

    /// TASK-136 acceptance: a phase-1 Inconclusive in BATCH mode shelves the
    /// spec (stamps `shelved_reason`, exit = phase-1 index) so `drain_batch`'s
    /// EPIC-28 shelve→advance path carries it — the whole batch does not pause
    /// at the head. trace:TASK-136 | ai:claude
    #[test]
    fn inconclusive_after_retries_shelves_and_advances_in_batch() {
        let mut driver =
            MockPhaseDriver::inconclusive_and_shelves("GH unreachable after the retry backoff");
        let result = orchestrate_with_lifecycle_skip(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip::none(),
            true, // batch
        );
        let fr = result
            .shelved_reason
            .as_ref()
            .expect("a batch-mode inconclusive must shelve, not pause");
        assert_eq!(
            fr.kind, "pr-verification-inconclusive",
            "the shelve records the inconclusive-verify kind for triage"
        );
        assert_eq!(result.failed_phase, Some(Phase::Implementer));
        assert_eq!(
            result.exit_code,
            Phase::Implementer.index(),
            "a shelve keeps the failed-phase exit code so drain_batch routes it",
        );
        assert!(
            result.inconclusive_reason.is_none(),
            "in batch mode it is shelved, NOT left as an inconclusive pause",
        );
        // Phases 2-6 never ran.
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    /// TASK-136: the single-spec path is UNCHANGED — the same driver pauses
    /// (exit 0, `inconclusive_reason` set, nothing shelved). The `batch` flag,
    /// not the driver, picks the behaviour. trace:TASK-136 | ai:claude
    #[test]
    fn single_spec_inconclusive_still_pauses() {
        let mut driver = MockPhaseDriver::inconclusive_and_shelves("GH unreachable");
        let result = orchestrate_with_lifecycle_skip(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip::none(),
            false, // single-spec
        );
        assert_eq!(
            result.exit_code, 0,
            "single-spec inconclusive is a clean pause"
        );
        assert!(
            result.inconclusive_reason.is_some(),
            "single-spec keeps the historical Inconclusive pause",
        );
        assert!(
            result.shelved_reason.is_none(),
            "single-spec never shelves an inconclusive",
        );
    }

    // --- STORY-492: resume re-entry skips already-complete phases ----------

    /// STORY-492: `start_phase = Implementer` (the normal entry) runs the full
    /// pipeline — resume is a strict superset of the historical behaviour.
    #[test]
    fn resume_at_implementer_runs_the_full_pipeline() {
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate_with_resume(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip::none(),
            false,
            Phase::Implementer,
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build
            ]
        );
    }

    /// STORY-492: re-entering at Merge skips implementer / CI / reviewer — their
    /// effects already exist (the resume handler seeded branch + PR) — and runs
    /// merge → pull → build. The implementer is NEVER re-run, so no PR is
    /// re-opened. trace:STORY-492 | ai:claude
    #[test]
    fn resume_at_merge_skips_implementer_ci_reviewer() {
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate_with_resume(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip::none(),
            false,
            Phase::Merge,
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            driver.calls,
            vec![Phase::Merge, Phase::Pull, Phase::Build],
            "resume at merge re-runs only merge/pull/build",
        );
    }

    /// STORY-492: re-entering at Pull (crashed after the merge landed but before
    /// the auto-bump) runs only pull → build — never re-merges. trace:STORY-492
    #[test]
    fn resume_at_pull_runs_only_pull_and_build() {
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate_with_resume(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip::none(),
            false,
            Phase::Pull,
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(driver.calls, vec![Phase::Pull, Phase::Build]);
    }

    // --- BUG-250: deliberate PR-hold phase-1 outcome ---------------------
    //
    // A deliberate push-branch, hold-PR finish must NOT be reported as a
    // phase-1 failure (that gave a wrong recovery hint — "run /aida-pr" —
    // which for a deliberate hold would ship un-gated code). It is a clean
    // non-failure stop: exit 0, no `failed_phase`, `held_reason` set, no later
    // phase runs.

    /// BUG-250 acceptance #1/#2/#4 — a held PR reports `Held`, not a phase-1
    /// failure; the drain halts cleanly at phase 1. trace:BUG-250 | ai:claude
    #[test]
    fn orchestrate_held_at_phase1_is_not_a_failure() {
        let mut driver =
            MockPhaseDriver::holding_at_implementer("SPIKE-7 smoke must pass before merge");
        let result = orchestrate(
            &mut driver,
            "STORY-306",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(
            result.exit_code, 0,
            "a deliberate hold is a clean exit, not a failure"
        );
        assert!(
            result.failed_phase.is_none(),
            "no `failed_phase` — the PR was deliberately held, nothing failed",
        );
        assert!(result.failure.is_none(), "no `PhaseFailure` payload");
        assert!(
            result.held_reason.is_some(),
            "`held_reason` distinguishes a deliberate hold from a clean ship",
        );
        assert!(result.punt_reason.is_none(), "not a punt");
        assert!(result.inconclusive_reason.is_none(), "not inconclusive");
        assert!(result.escalation.is_none(), "not an escalation");
        assert!(result.shipped_spec_id.is_none(), "nothing shipped");
        // Phases 2-6 never ran — the drain halted at phase 1.
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    /// `BatchDrainOutcome::Held` — a batch member that deliberately holds its
    /// PR pauses the drain at that spec (does not falsely count it shipped),
    /// exits 0. trace:BUG-250 | ai:claude
    #[test]
    fn drain_batch_held_pauses_without_shipping() {
        struct OneHeldDriver {
            queue: Vec<String>,
            ran: Vec<String>,
        }
        impl BatchDriver for OneHeldDriver {
            fn next_head(&mut self) -> Option<String> {
                self.queue.first().cloned()
            }
            fn run_spec(&mut self, spec: &str) -> OrchestrationResult {
                self.ran.push(spec.to_string());
                OrchestrationResult {
                    exit_code: 0,
                    failed_phase: None,
                    failure: None,
                    phase_durations: Vec::new(),
                    total_ms: 0,
                    punt_reason: None,
                    shipped_spec_id: None,
                    escalation: None,
                    inconclusive_reason: None,
                    shelved_reason: None,
                    held_reason: Some("PR held on `story-306` — SPIKE-7 smoke".to_string()),
                }
            }
        }
        let mut driver = OneHeldDriver {
            queue: vec!["STORY-306".to_string(), "STORY-307".to_string()],
            ran: Vec::new(),
        };
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.outcome, BatchDrainOutcome::Held);
        assert_eq!(result.exit_code, 0, "a deliberate hold is not a failure");
        assert!(result.shipped.is_empty(), "the held spec did not ship");
        assert_eq!(result.stopped_at.as_deref(), Some("STORY-306"));
        // The drain paused at the held head — the next member never ran.
        assert_eq!(driver.ran, vec!["STORY-306".to_string()]);
    }

    /// `BatchDrainOutcome::Inconclusive` — a batch drain that hits a phase-1
    /// inconclusive run stops at that spec without claiming ship / punt /
    /// fail, leaves the head un-advanced, and exits 0 so the next drain
    /// retries once the API is reachable. trace:BUG-257 | ai:claude
    #[test]
    fn drain_batch_inconclusive_pauses_without_advancing() {
        struct OneInconclusiveDriver {
            queue: Vec<String>,
            ran: Vec<String>,
        }
        impl BatchDriver for OneInconclusiveDriver {
            fn next_head(&mut self) -> Option<String> {
                self.queue.first().cloned()
            }
            fn run_spec(&mut self, spec: &str) -> OrchestrationResult {
                self.ran.push(spec.to_string());
                OrchestrationResult {
                    exit_code: 0,
                    failed_phase: None,
                    failure: None,
                    phase_durations: Vec::new(),
                    total_ms: 0,
                    punt_reason: None,
                    shipped_spec_id: None,
                    escalation: None,
                    inconclusive_reason: Some("GH API unreachable — cannot confirm PR".to_string()),
                    shelved_reason: None,
                    held_reason: None,
                }
            }
        }
        let mut driver = OneInconclusiveDriver {
            queue: vec!["BUG-257".to_string(), "TASK-260".to_string()],
            ran: Vec::new(),
        };
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, 0, "inconclusive is a clean exit");
        assert_eq!(result.outcome, BatchDrainOutcome::Inconclusive);
        assert_eq!(result.stopped_at.as_deref(), Some("BUG-257"));
        assert!(result.shipped.is_empty(), "the spec did not ship");
        assert!(result.punted.is_empty(), "the spec did not punt");
        assert!(result.escalated.is_empty(), "the spec did not escalate");
        // The head was not advanced — only the inconclusive spec was driven.
        // The queued TASK-260 stays for the next drain.
        assert_eq!(driver.ran, vec!["BUG-257"]);
    }

    /// BUG-266: when the orchestrator-level Inconclusive carries a leg-
    /// specific retry hint (the Anthropic-API path), the hint flows through
    /// the variant and replaces BUG-257's default GH-flavored hint. This is
    /// the only spot that exercises the new `retry_hint` field at the
    /// orchestrator boundary — the classifier itself is unit-tested in
    /// `main.rs::bug_266_anthropic_api_outage_classifier_tests`.
    /// trace:BUG-266 | ai:claude
    #[test]
    fn inconclusive_with_anthropic_hint_overrides_default() {
        let mut driver = MockPhaseDriver::base();
        driver.inconclusive = Some(
            "Anthropic API outage during the headless implementer: API Error: 529 Overloaded"
                .to_string(),
        );
        // The mock returns `retry_hint: None`; we explicitly run a variant
        // that simulates the production wiring by re-driving through
        // `finish_inconclusive` with `Some(hint)` and asserting the round
        // trip on `OrchestrationResult`. Going via `orchestrate` keeps the
        // assertion at the orchestrator boundary instead of staring at the
        // private epilogue function.
        let result = orchestrate(
            &mut driver,
            "BUG-266",
            AutoCompleteVariant::Full,
            true, // json — keeps the test output clean
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.failed_phase.is_none());
        assert!(
            result
                .inconclusive_reason
                .as_deref()
                .map(|r| r.contains("Anthropic API outage"))
                .unwrap_or(false),
            "BUG-266 reason must surface; got: {:?}",
            result.inconclusive_reason,
        );
        // Only phase 1 ran — the Inconclusive path halts cleanly without
        // touching CI / reviewer / merge.
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    /// A re-punt on the resumed implementer is terminal; symmetrically, an
    /// inconclusive on the resumed implementer is also terminal — the drain
    /// pauses, no second advisor round. trace:BUG-257 | ai:claude
    #[test]
    fn orchestrate_resume_inconclusive_is_terminal_pause() {
        let mut driver =
            MockPhaseDriver::punting_at_implementer("first fork").advisor_resolves("answer A");
        driver.resume = Some(ImplementerOutcome::Inconclusive {
            reason: "GH API unreachable after the implementer resume".to_string(),
            retry_hint: None,
        });
        let result = orchestrate(
            &mut driver,
            "BUG-257",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.failed_phase.is_none());
        assert!(result.inconclusive_reason.is_some());
        assert!(result.punt_reason.is_none());
        // Phases 2-6 never ran; the advisor was spawned exactly once.
        assert_eq!(driver.calls, vec![Phase::Implementer]);
        assert_eq!(driver.advisor_calls, 1, "no second advisor round");
    }

    // --- Variant gating ---------------------------------------------------

    #[test]
    fn orchestrate_through_ci_stops_after_phase_2() {
        // Even with a driver that would fail at phase 3, through-ci never
        // reaches it.
        let mut driver = MockPhaseDriver::failing_at(Phase::Reviewer);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::ThroughCi,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 0);
        assert_eq!(driver.calls, vec![Phase::Implementer, Phase::Ci]);
    }

    #[test]
    fn orchestrate_through_merge_stops_after_phase_4() {
        let mut driver = MockPhaseDriver::all_ok();
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::ThroughMerge,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 0);
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer, Phase::Ci, Phase::Reviewer, Phase::Merge,]
        );
    }

    #[test]
    fn orchestrate_skip_build_stops_after_phase_5() {
        let mut driver = MockPhaseDriver::all_ok();
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::SkipBuild,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 0);
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
            ]
        );
    }

    // --- Failure injection: each phase → its exit code --------------------

    #[test]
    fn failure_injection_implementer_exits_1() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Implementer);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 1);
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    #[test]
    fn failure_injection_reviewer_exits_3() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Reviewer);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 3);
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer, Phase::Ci, Phase::Reviewer]
        );
    }

    #[test]
    fn failure_injection_merge_exits_4() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Merge);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 4);
    }

    #[test]
    fn failure_injection_pull_exits_5() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Pull);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 5);
    }

    #[test]
    fn failure_injection_build_exits_6() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Build);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 6);
    }

    // --- CI-failure injection: red CI stops at phase 2 --------------------

    #[test]
    fn ci_red_stops_at_phase_2() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Ci);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 2);
        // The reviewer phase must NOT have run.
        assert_eq!(driver.calls, vec![Phase::Implementer, Phase::Ci]);
    }

    // --- Reviewer verdict gating ------------------------------------------

    #[test]
    fn reviewer_rejected_stops_at_phase_3() {
        let mut driver = MockPhaseDriver::with_verdict(Verdict::Rejected);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 3);
        // Reviewer ran; merge did not.
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer, Phase::Ci, Phase::Reviewer]
        );
    }

    #[test]
    fn reviewer_request_changes_stops_at_phase_3() {
        let mut driver = MockPhaseDriver::with_verdict(Verdict::RequestChanges);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 3);
    }

    #[test]
    fn reviewer_approved_proceeds_to_merge() {
        let mut driver = MockPhaseDriver::with_verdict(Verdict::Approved);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 0);
    }

    // --- STORY-306: reviewer escalates the merge decision to a human ------

    /// The reviewer escalates the merge decision rather than auto-deciding
    /// it. `orchestrate` stops cleanly after phase 3 — no merge / pull /
    /// build — exits `0`, and the result carries the escalation so the run
    /// is mistaken for neither a clean ship nor a failure.
    #[test]
    fn orchestrate_reviewer_escalated_to_human_skips_merge() {
        let mut driver = MockPhaseDriver::reviewer_escalates_merge(
            "uncertain whether the zen run was corroborated — a human should merge",
        );
        let result = orchestrate(
            &mut driver,
            "BUG-241",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0, "an escalation is a clean exit");
        assert_eq!(result.failed_phase, None);
        assert!(result.failure.is_none());
        let escalation = result.escalation.expect("escalation recorded");
        assert_eq!(escalation.kind, EscalationKind::MergeDecision);
        assert!(
            escalation.reason.contains("zen run"),
            "{}",
            escalation.reason
        );
        // The pipeline stopped at phase 3 — merge / pull / build never ran.
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer, Phase::Ci, Phase::Reviewer]
        );
    }

    /// Regression guard — an escalation is a *distinct* outcome from a
    /// non-Approved verdict. A plain `RequestChanges` verdict still routes
    /// through the phase-3 failure path (exit `3`), unchanged.
    #[test]
    fn orchestrate_reviewer_request_changes_still_fails() {
        let mut driver = MockPhaseDriver::with_verdict(Verdict::RequestChanges);
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 3);
        assert_eq!(result.failed_phase, Some(Phase::Reviewer));
        assert!(result.escalation.is_none());
    }

    // --- BUG-241: reconcile against reality before declaring a failure ----

    /// Instance B — phase 1 produces no PR because the spec was already
    /// resolved by supersession. The reconcile step finds the spec Completed
    /// and treats the run as a success: exit `0`, no `failed_phase`. A batch
    /// drain advances on exactly this result shape (`drain_batch` keys off
    /// `exit_code` / `failed_phase`), so the false "shipped 0" crash is gone.
    #[test]
    fn orchestrate_reconciles_phase1_no_work_spec() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Implementer).reconciles_as(
            PhaseReconcile::ShippedOutOfBand {
                reason: "BUG-230 is Completed — the spec needed no further work".to_string(),
            },
        );
        let result = orchestrate(
            &mut driver,
            "BUG-230",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(
            result.exit_code, 0,
            "a reconciled phase-1 failure is a success"
        );
        assert!(result.failed_phase.is_none());
        assert!(result.failure.is_none());
        // The pipeline still stopped at phase 1 — reconcile redeems the
        // outcome, it does not resume the remaining phases.
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    /// Instance A — phase 3 ends with no verdict file because the reviewer
    /// escalated and a human merged the PR out-of-band. The reconcile step
    /// finds the PR merged and treats the run as a success.
    #[test]
    fn orchestrate_reconciles_phase3_out_of_band_merge() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Reviewer).reconciles_as(
            PhaseReconcile::ShippedOutOfBand {
                reason: "PR-94 is merged and BUG-233 is Completed".to_string(),
            },
        );
        let result = orchestrate(
            &mut driver,
            "BUG-233",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.failed_phase.is_none());
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer, Phase::Ci, Phase::Reviewer]
        );
    }

    /// A non-Approved verdict is also routed through reconcile — if reality
    /// shows the PR merged out-of-band, even a Rejected verdict is redeemed.
    #[test]
    fn orchestrate_reconciles_rejected_verdict_when_pr_merged() {
        let mut driver = MockPhaseDriver::with_verdict(Verdict::Rejected).reconciles_as(
            PhaseReconcile::ShippedOutOfBand {
                reason: "PR-94 is already merged".to_string(),
            },
        );
        let result = orchestrate(
            &mut driver,
            "BUG-233",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.failed_phase.is_none());
    }

    /// Regression guard — the reconcile step must NOT mask a real failure.
    /// With the default `GenuineFailure` verdict (reality confirms nothing
    /// shipped) a phase-1 failure still exits `1` and names the failed phase.
    #[test]
    fn orchestrate_genuine_phase1_failure_still_fails() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Implementer);
        let result = orchestrate(
            &mut driver,
            "BUG-241",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 1);
        assert_eq!(result.failed_phase, Some(Phase::Implementer));
        assert!(result.failure.is_some());
    }

    /// TASK-133: the orchestrator-parent phase-1 status-bump compensation
    /// decision. The bug it guards: `prepare_auto_complete_phase1_status`
    /// flips Approved → InProgress before spawning the implementer; a phase-1
    /// failure with no lease ever recorded then strands the spec in
    /// NeedsAttention with no work behind it.
    #[test]
    fn compensate_phase1_bump_when_bumped_no_lease_phase1_failed() {
        // The stranded case: parent bumped, child never leased, phase 1 failed.
        assert!(should_compensate_phase1_bump(
            true,
            false,
            Some(Phase::Implementer)
        ));
    }

    #[test]
    fn no_compensation_when_lease_acquired() {
        // A lease was recorded → real work (commits / worktree) exists to
        // triage; leave the spec shelved rather than silently resetting it.
        assert!(!should_compensate_phase1_bump(
            true,
            true,
            Some(Phase::Implementer)
        ));
    }

    #[test]
    fn no_compensation_when_parent_did_not_bump() {
        // The parent left the status untouched (e.g. already InProgress /
        // Planned) → nothing to restore.
        assert!(!should_compensate_phase1_bump(
            false,
            false,
            Some(Phase::Implementer)
        ));
    }

    #[test]
    fn no_compensation_when_a_later_phase_failed() {
        // The implementer shipped a PR and a later phase failed → the bump
        // was legitimate, the work is real; do not reset.
        assert!(!should_compensate_phase1_bump(true, false, Some(Phase::Ci)));
        assert!(!should_compensate_phase1_bump(
            true,
            true,
            Some(Phase::Reviewer)
        ));
    }

    #[test]
    fn no_compensation_on_success() {
        // No failed phase at all (clean ship / punt / inconclusive) → the
        // status is legitimately advanced or deliberately held; never reset.
        assert!(!should_compensate_phase1_bump(true, false, None));
    }

    /// Regression guard at phase 3 — a genuine no-verdict failure (reality
    /// confirms nothing merged) still stops the batch at phase 3.
    #[test]
    fn orchestrate_genuine_phase3_failure_still_fails() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Reviewer);
        let result = orchestrate(
            &mut driver,
            "BUG-241",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 3);
        assert_eq!(result.failed_phase, Some(Phase::Reviewer));
    }

    /// The default `PhaseDriver::reconcile_failure` is `GenuineFailure` — a
    /// driver that does not override it (cannot check reality) leaves every
    /// failure standing. This pins the conservative default.
    #[test]
    fn reconcile_failure_default_is_genuine_failure() {
        let mut driver = MockPhaseDriver::all_ok();
        // `all_ok()` leaves `reconcile` at its `GenuineFailure` default.
        assert_eq!(
            driver.reconcile_failure(Phase::Implementer, &PhaseFailure::new("x")),
            PhaseReconcile::GenuineFailure
        );
    }

    // --- Recovery hints: per phase, per failure kind ----------------------

    fn ctx() -> HintContext {
        HintContext {
            spec: "TASK-247".to_string(),
            branch: Some("task-247".to_string()),
            pr_number: Some(46),
            implementer_session: Some("019e2f423e7c".to_string()),
            ci_run_id: Some("9988776655".to_string()),
            forge: crate::forge::ForgeKind::GitHub,
        }
    }

    // The phase-default (`Failed`) hints — the pre-BUG-218 behaviour, now
    // gated behind `FailureKind::Failed` so a spawn ENOENT no longer borrows
    // them.

    #[test]
    fn recovery_hint_implementer_failed_names_resume() {
        let hint = recovery_hint(Phase::Implementer, FailureKind::Failed, &ctx());
        assert!(hint.contains("aida queue work TASK-247 --resume 019e2f423e7c"));
    }

    /// BUG-236: by phase 3 the spec is Done (the implementer opened the PR
    /// in phase 1), so the recovery hint must use `aida queue rework`, the
    /// verb that re-opens a Done spec — never `aida queue work`, which
    /// operates on queued items only and would bounce.
    #[test]
    fn recovery_hint_reviewer_failed_names_rework_verb() {
        let hint = recovery_hint(Phase::Reviewer, FailureKind::Failed, &ctx());
        assert!(
            hint.contains("aida queue rework TASK-247 --work"),
            "hint: {hint}"
        );
        assert!(
            !hint.contains("aida queue work TASK-247"),
            "phase-3 hint must not route a Done spec through `queue work`: {hint}"
        );
    }

    #[test]
    fn recovery_hint_merge_failed_names_pr_view() {
        let hint = recovery_hint(Phase::Merge, FailureKind::Failed, &ctx());
        assert!(hint.contains("gh pr view 46"));
    }

    /// STORY-508/TASK-651: recovery hints are forge-aware — a GitLab project's
    /// drain failure names glab/MR commands, never gh.
    #[test]
    fn recovery_hints_are_forge_aware_for_gitlab() {
        let mut c = ctx();
        c.forge = crate::forge::ForgeKind::GitLab;

        let merge = recovery_hint(Phase::Merge, FailureKind::Failed, &c);
        assert!(merge.contains("glab mr view 46"), "{merge}");
        assert!(!merge.contains("gh pr"), "{merge}");

        let merge_missing = recovery_hint(Phase::Merge, FailureKind::MissingTool, &c);
        assert!(merge_missing.contains("glab"), "{merge_missing}");
        assert!(merge_missing.contains("GitLab CLI"), "{merge_missing}");
        assert!(!merge_missing.contains("gh"), "{merge_missing}");

        let ci_red = recovery_hint(Phase::Ci, FailureKind::CiRed, &c);
        assert!(ci_red.contains("glab ci view"), "{ci_red}");
        assert!(!ci_red.contains("gh run"), "{ci_red}");

        // Spawn-on-merge workaround also routes through glab.
        let spawn = recovery_hint(Phase::Merge, FailureKind::Spawn, &c);
        assert!(spawn.contains("glab mr merge 46"), "{spawn}");
        assert!(spawn.contains("--remove-source-branch"), "{spawn}");
    }

    /// BUG-236 regression guard: by phase 3 the spec is Done (the
    /// implementer opened the PR in phase 1), and `aida queue work`
    /// operates on queued items only — so no phase-3+ recovery hint, for
    /// any failure kind, may route the spec through `aida queue work
    /// <SPEC>`. The `aida queue work PR-N` form is fine (a PR review is
    /// status-independent); only the bare-spec form bounces. This test
    /// fails against the pre-fix `(Phase::Reviewer, _)` and `Internal`
    /// hints, which both did exactly that.
    #[test]
    fn recovery_hints_never_route_done_spec_through_queue_work() {
        let kinds = [
            FailureKind::Spawn,
            FailureKind::MissingTool,
            FailureKind::Internal,
            FailureKind::NoPr,
            FailureKind::CiRed,
            FailureKind::CiTimeout,
            FailureKind::NoVerdict,
            FailureKind::Failed,
        ];
        for phase in [Phase::Reviewer, Phase::Merge, Phase::Pull, Phase::Build] {
            for kind in kinds {
                let hint = recovery_hint(phase, kind, &ctx());
                // `ctx()` uses spec TASK-247 — the bare-spec `queue work`
                // form is the bounce; `queue work PR-46` is allowed.
                assert!(
                    !hint.contains("aida queue work TASK-247"),
                    "{phase:?}/{kind:?} routes the Done spec through `aida queue work`: {hint}"
                );
            }
        }
    }

    #[test]
    fn recovery_hint_pull_failed_names_rebase_dry_run() {
        let hint = recovery_hint(Phase::Pull, FailureKind::Failed, &ctx());
        assert!(hint.contains("aida rebase --dry-run --json"));
    }

    #[test]
    fn recovery_hint_build_failed_names_cargo_check() {
        let hint = recovery_hint(Phase::Build, FailureKind::Failed, &ctx());
        assert!(hint.contains("cargo check --workspace"));
    }

    #[test]
    fn recovery_hint_ci_failed_names_session_end() {
        // Phase-2 default (`Failed`) is "session would not end" — distinct
        // from a red CI run, which is its own `CiRed` kind below.
        let hint = recovery_hint(Phase::Ci, FailureKind::Failed, &ctx());
        assert!(hint.contains("would not end cleanly"));
        assert!(hint.contains("aida session end 019e2f423e7c --skip-ci"));
    }

    // CI-specific kinds: a red run vs a timeout vs a spawn ENOENT are three
    // different problems and must not collapse into "CI is red" (BUG-218).

    #[test]
    fn recovery_hint_ci_red_names_run_view_and_steal() {
        let hint = recovery_hint(Phase::Ci, FailureKind::CiRed, &ctx());
        assert!(hint.contains("CI failed on run 9988776655"));
        assert!(hint.contains("gh run view 9988776655"));
        assert!(hint.contains("--steal"));
    }

    #[test]
    fn recovery_hint_ci_red_without_run_id_falls_back_to_run_list() {
        let mut c = ctx();
        c.ci_run_id = None;
        let hint = recovery_hint(Phase::Ci, FailureKind::CiRed, &c);
        assert!(hint.contains("gh run list --branch task-247"));
    }

    #[test]
    fn recovery_hint_ci_timeout_says_not_red() {
        let hint = recovery_hint(Phase::Ci, FailureKind::CiTimeout, &ctx());
        assert!(hint.contains("terminal state"));
        assert!(hint.contains("CI is not red"));
        // A timeout is not a red run — must not route to `gh run view`.
        assert!(!hint.contains("CI failed on run"));
    }

    /// BUG-218 regression: a phase-2 subprocess spawn ENOENT must NOT be
    /// reported as a red CI run — the failure was local subprocess plumbing.
    #[test]
    fn recovery_hint_ci_spawn_does_not_blame_ci() {
        let hint = recovery_hint(Phase::Ci, FailureKind::Spawn, &ctx());
        assert!(hint.contains("Subprocess spawn failed"));
        assert!(hint.contains("not CI"));
        // The exact mis-routing BUG-218 reported:
        assert!(!hint.contains("CI is red"));
        assert!(!hint.contains("CI failed"));
        assert!(!hint.contains("gh run"));
        // It names the phase-2 manual fallback.
        assert!(hint.contains("aida session end 019e2f423e7c --wait-ci"));
    }

    #[test]
    fn recovery_hint_spawn_names_each_phases_manual_fallback() {
        // Every phase's spawn hint says "spawn failed" + names that phase's
        // own by-hand command — never a CI command.
        for (phase, fallback) in [
            (Phase::Implementer, "aida queue work TASK-247"),
            (Phase::Ci, "aida session end 019e2f423e7c --wait-ci"),
            (Phase::Reviewer, "aida queue work PR-46"),
            (Phase::Merge, "gh pr merge 46 --squash --delete-branch"),
            (Phase::Pull, "aida pull"),
            (Phase::Build, "cargo build --release"),
        ] {
            let hint = recovery_hint(phase, FailureKind::Spawn, &ctx());
            assert!(
                hint.contains("Subprocess spawn failed"),
                "{phase:?} spawn hint should name a spawn failure: {hint}"
            );
            assert!(
                hint.contains(fallback),
                "{phase:?} spawn hint should name `{fallback}`: {hint}"
            );
        }
    }

    #[test]
    fn recovery_hint_internal_routes_to_file_a_bug() {
        // An internal-invariant violation is an AIDA bug — the hint says so
        // for every phase rather than blaming the user's work.
        for phase in [
            Phase::Implementer,
            Phase::Ci,
            Phase::Reviewer,
            Phase::Merge,
            Phase::Pull,
            Phase::Build,
        ] {
            let hint = recovery_hint(phase, FailureKind::Internal, &ctx());
            assert!(
                hint.contains("orchestrator bug") && hint.contains("file it"),
                "{phase:?} internal hint should route to filing a bug: {hint}"
            );
        }
    }

    #[test]
    fn recovery_hint_implementer_no_pr_says_open_a_pr() {
        let hint = recovery_hint(Phase::Implementer, FailureKind::NoPr, &ctx());
        assert!(hint.contains("without opening a PR"));
        assert!(hint.contains("/aida-pr"));
        // TASK-401: the orchestrator's phase-1 detector fires when a
        // headless implementer commits + exits without pushing or opening
        // a PR (the failure mode `/aida-pickup` Step 5c enforces against).
        // The exit hint is the operator's only thread back into the
        // session — it must name the resume verb with the session ID, so
        // it's a one-command re-entry. A bare "run /aida-pr" without the
        // resume context leaves the operator hunting for the lease.
        // trace:TASK-401 | ai:claude
        assert!(
            hint.contains("aida queue work TASK-247 --resume 019e2f423e7c"),
            "no-PR hint must give the operator a one-command resume back into \
             the phase-1 session, not just a bare `/aida-pr` reference: {hint}"
        );
    }

    #[test]
    fn recovery_hint_missing_tool_says_install_gh() {
        for phase in [Phase::Implementer, Phase::Merge] {
            let hint = recovery_hint(phase, FailureKind::MissingTool, &ctx());
            assert!(hint.contains("`gh` is not on PATH"), "{phase:?}: {hint}");
            assert!(hint.contains("cli.github.com"), "{phase:?}: {hint}");
        }
    }

    #[test]
    fn recovery_hint_reviewer_no_verdict_says_rerun_review() {
        let hint = recovery_hint(Phase::Reviewer, FailureKind::NoVerdict, &ctx());
        assert!(hint.contains("without a usable verdict"));
        assert!(hint.contains("aida queue work PR-46 --steal"));
    }

    /// BUG-218 acceptance: each phase produces at least 3 distinguishable
    /// recovery hints. `Spawn`, `Internal`, and the phase default (`Failed`)
    /// apply to every phase and must each yield a different message.
    #[test]
    fn recovery_hint_three_distinguishable_patterns_per_phase() {
        for phase in [
            Phase::Implementer,
            Phase::Ci,
            Phase::Reviewer,
            Phase::Merge,
            Phase::Pull,
            Phase::Build,
        ] {
            let spawn = recovery_hint(phase, FailureKind::Spawn, &ctx());
            let internal = recovery_hint(phase, FailureKind::Internal, &ctx());
            let failed = recovery_hint(phase, FailureKind::Failed, &ctx());
            assert_ne!(spawn, internal, "{phase:?}: spawn vs internal collapsed");
            assert_ne!(spawn, failed, "{phase:?}: spawn vs failed collapsed");
            assert_ne!(internal, failed, "{phase:?}: internal vs failed collapsed");
        }
    }

    // --- Pure helpers -----------------------------------------------------

    #[test]
    fn variant_parse_accepts_all_forms() {
        assert_eq!(
            AutoCompleteVariant::parse(""),
            Ok(AutoCompleteVariant::Full)
        );
        assert_eq!(
            AutoCompleteVariant::parse("full"),
            Ok(AutoCompleteVariant::Full)
        );
        assert_eq!(
            AutoCompleteVariant::parse("through-ci"),
            Ok(AutoCompleteVariant::ThroughCi)
        );
        assert_eq!(
            AutoCompleteVariant::parse("THROUGH-MERGE"),
            Ok(AutoCompleteVariant::ThroughMerge)
        );
        assert_eq!(
            AutoCompleteVariant::parse("skip-build"),
            Ok(AutoCompleteVariant::SkipBuild)
        );
        assert!(AutoCompleteVariant::parse("bogus").is_err());
    }

    #[test]
    fn variant_last_phase() {
        assert_eq!(AutoCompleteVariant::ThroughCi.last_phase(), 2);
        assert_eq!(AutoCompleteVariant::ThroughMerge.last_phase(), 4);
        assert_eq!(AutoCompleteVariant::SkipBuild.last_phase(), 5);
        assert_eq!(AutoCompleteVariant::Full.last_phase(), 6);
    }

    #[test]
    fn verdict_parse_is_tolerant() {
        assert_eq!(Verdict::parse("Approved"), Some(Verdict::Approved));
        assert_eq!(Verdict::parse("  approved "), Some(Verdict::Approved));
        assert_eq!(
            Verdict::parse("RequestChanges"),
            Some(Verdict::RequestChanges)
        );
        assert_eq!(
            Verdict::parse("request-changes"),
            Some(Verdict::RequestChanges)
        );
        assert_eq!(Verdict::parse("Rejected"), Some(Verdict::Rejected));
        assert_eq!(Verdict::parse("maybe"), None);
    }

    #[test]
    fn phase_event_json_shape() {
        let line = phase_event("implementer", "started", "TASK-247", 1234, None, &[]);
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["phase"], "implementer");
        assert_eq!(v["status"], "started");
        assert_eq!(v["spec"], "TASK-247");
        assert_eq!(v["elapsed_ms"], 1234);
        assert!(v.get("exit_code").is_none());
    }

    #[test]
    fn phase_event_json_includes_exit_code_and_extra() {
        let line = phase_event(
            "ci",
            "failed",
            "TASK-247",
            5000,
            Some(2),
            &[("reason", "CI red"), ("hint", "gh run view 9")],
        );
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["exit_code"], 2);
        assert_eq!(v["reason"], "CI red");
        assert_eq!(v["hint"], "gh run view 9");
    }

    #[test]
    fn fmt_duration_renders_minutes_and_seconds() {
        assert_eq!(fmt_duration(42_000), "42s");
        assert_eq!(fmt_duration(318_000), "5m 18s");
        assert_eq!(fmt_duration(60_000), "1m 0s");
    }

    #[test]
    fn phase_exit_code_equals_index() {
        assert_eq!(Phase::Implementer.index(), 1);
        assert_eq!(Phase::Ci.index(), 2);
        assert_eq!(Phase::Reviewer.index(), 3);
        assert_eq!(Phase::Merge.index(), 4);
        assert_eq!(Phase::Pull.index(), 5);
        assert_eq!(Phase::Build.index(), 6);
    }

    #[test]
    fn phase_from_index_round_trips() {
        for phase in [
            Phase::Implementer,
            Phase::Ci,
            Phase::Reviewer,
            Phase::Merge,
            Phase::Pull,
            Phase::Build,
        ] {
            assert_eq!(Phase::from_index(phase.index()), Some(phase));
        }
        assert_eq!(Phase::from_index(0), None);
        assert_eq!(Phase::from_index(7), None);
    }

    #[test]
    fn variant_slug_matches_parse_spelling() {
        for v in [
            AutoCompleteVariant::Full,
            AutoCompleteVariant::ThroughCi,
            AutoCompleteVariant::ThroughMerge,
            AutoCompleteVariant::SkipBuild,
        ] {
            assert_eq!(AutoCompleteVariant::parse(v.slug()), Ok(v));
        }
    }

    // --- NoHumanMode (STORY-263, TASK-306) --------------------------------

    #[test]
    fn no_human_mode_parse_accepts_all_forms() {
        // TASK-306: bare `--no-human` (clap default-missing) and an empty
        // string resolve to reviewer-only — the honest default of what the
        // flag does today (the headless implementer is not shipped).
        assert_eq!(NoHumanMode::parse(""), Ok(NoHumanMode::ReviewerOnly));
        assert_eq!(
            NoHumanMode::parse("reviewer-only"),
            Ok(NoHumanMode::ReviewerOnly)
        );
        assert_eq!(
            NoHumanMode::parse("reviewer_only"),
            Ok(NoHumanMode::ReviewerOnly)
        );
        assert_eq!(
            NoHumanMode::parse("  reviewer "),
            Ok(NoHumanMode::ReviewerOnly)
        );
        // `both` still parses — the kickoff gate, not the grammar, is what
        // rejects it until the headless implementer ships.
        assert_eq!(NoHumanMode::parse("both"), Ok(NoHumanMode::Both));
        assert_eq!(NoHumanMode::parse("BOTH"), Ok(NoHumanMode::Both));
        assert!(NoHumanMode::parse("bogus").is_err());
    }

    #[test]
    fn no_human_mode_only_both_wants_headless_implementer() {
        assert!(NoHumanMode::Both.wants_headless_implementer());
        assert!(!NoHumanMode::ReviewerOnly.wants_headless_implementer());
    }

    #[test]
    fn no_human_mode_slug_round_trips_through_parse() {
        // TASK-306: the slug is the propagated `AIDA_NO_HUMAN_MODE` value and
        // must parse back to the same mode.
        for mode in [NoHumanMode::ReviewerOnly, NoHumanMode::Both] {
            assert_eq!(NoHumanMode::parse(mode.slug()), Ok(mode));
        }
        assert_eq!(NoHumanMode::ReviewerOnly.slug(), "reviewer-only");
        assert_eq!(NoHumanMode::Both.slug(), "both");
    }

    // --- OrchestrationResult telemetry fields (TASK-266) ------------------

    #[test]
    fn result_success_records_every_phase_duration_in_order() {
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.failed_phase.is_none());
        assert!(result.failure.is_none());
        let phases: Vec<Phase> = result.phase_durations.iter().map(|(p, _)| *p).collect();
        assert_eq!(
            phases,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ]
        );
    }

    #[test]
    fn result_variant_caps_phase_durations_at_last_phase() {
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::ThroughCi,
            false,
            EscalateMode::Blocks,
        );
        let phases: Vec<Phase> = result.phase_durations.iter().map(|(p, _)| *p).collect();
        assert_eq!(phases, vec![Phase::Implementer, Phase::Ci]);
    }

    #[test]
    fn result_failure_carries_failed_phase_and_reason() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Reviewer);
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 3);
        assert_eq!(result.failed_phase, Some(Phase::Reviewer));
        let failure = result.failure.expect("failure detail should be set");
        assert!(failure.reason.contains("Reviewer"));
        // Durations stop at the failed phase — they include it, not beyond.
        let phases: Vec<Phase> = result.phase_durations.iter().map(|(p, _)| *p).collect();
        assert_eq!(phases, vec![Phase::Implementer, Phase::Ci, Phase::Reviewer]);
    }

    #[test]
    fn result_rejected_verdict_records_reviewer_as_failed_phase() {
        let mut driver = MockPhaseDriver::with_verdict(Verdict::Rejected);
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Reviewer));
        assert!(result
            .failure
            .expect("failure set")
            .reason
            .contains("not Approved"));
    }

    // --- Batch drain (TASK-285) -------------------------------------------

    /// A success / failure [`OrchestrationResult`] without the timing detail,
    /// for exercising [`drain_batch`].
    fn ok_result() -> OrchestrationResult {
        OrchestrationResult {
            exit_code: 0,
            failed_phase: None,
            failure: None,
            phase_durations: Vec::new(),
            total_ms: 0,
            punt_reason: None,
            shipped_spec_id: None,
            escalation: None,
            inconclusive_reason: None,
            shelved_reason: None,
            held_reason: None,
        }
    }

    fn fail_result(phase: Phase) -> OrchestrationResult {
        OrchestrationResult {
            exit_code: phase.index(),
            failed_phase: Some(phase),
            failure: Some(PhaseFailure::new(format!("mock failure at {phase:?}"))),
            phase_durations: Vec::new(),
            total_ms: 0,
            punt_reason: None,
            shipped_spec_id: None,
            escalation: None,
            inconclusive_reason: None,
            shelved_reason: None,
            held_reason: None,
        }
    }

    /// STORY-276: a punt [`OrchestrationResult`] — exit `0`, no `failed_phase`,
    /// `punt_reason` set. The shape `finish_punted` returns.
    fn punt_result() -> OrchestrationResult {
        OrchestrationResult {
            exit_code: 0,
            failed_phase: None,
            failure: None,
            phase_durations: Vec::new(),
            total_ms: 0,
            punt_reason: Some("mock punt: design-fork".to_string()),
            shipped_spec_id: None,
            escalation: None,
            inconclusive_reason: None,
            shelved_reason: None,
            held_reason: None,
        }
    }

    /// BUG-245: a success [`OrchestrationResult`] where phase 1 credited a
    /// different spec than the dispatched id — the shape `finish_success`
    /// returns on a dispatched≠shipped mismatch. trace:BUG-245 | ai:claude
    fn mismatch_result(shipped: &str) -> OrchestrationResult {
        OrchestrationResult {
            exit_code: 0,
            failed_phase: None,
            failure: None,
            phase_durations: Vec::new(),
            total_ms: 0,
            punt_reason: None,
            shipped_spec_id: Some(shipped.to_string()),
            escalation: None,
            inconclusive_reason: None,
            shelved_reason: None,
            held_reason: None,
        }
    }

    /// EPIC-28: a *shelved* failure [`OrchestrationResult`] — the failure
    /// fields stay set (exit code = phase index, `failed_phase` populated),
    /// but `shelved_reason` is `Some` so `drain_batch` recognises it as a
    /// recoverable failure and continues to the next member.
    /// trace:EPIC-28 | ai:claude
    fn shelve_result(phase: Phase) -> OrchestrationResult {
        let fr = aida_core::FailureReason {
            phase: phase.slug().to_string(),
            phase_index: phase.index() as u8,
            kind: "failed".to_string(),
            detail: format!("mock shelved failure at {phase:?}"),
            recovery_hint: None,
            shelved_by: None,
            shelved_at: chrono::Utc::now(),
        };
        OrchestrationResult {
            exit_code: phase.index(),
            failed_phase: Some(phase),
            failure: Some(PhaseFailure::new(format!("mock failure at {phase:?}"))),
            phase_durations: Vec::new(),
            total_ms: 0,
            punt_reason: None,
            shipped_spec_id: None,
            escalation: None,
            inconclusive_reason: None,
            shelved_reason: Some(fr),
            held_reason: None,
        }
    }

    /// STORY-306: an escalated [`OrchestrationResult`] — exit `0`, no
    /// `failed_phase`, `escalation` set. The shape `finish_escalated` returns.
    fn escalated_result() -> OrchestrationResult {
        OrchestrationResult {
            exit_code: 0,
            failed_phase: None,
            failure: None,
            phase_durations: Vec::new(),
            total_ms: 0,
            punt_reason: None,
            shipped_spec_id: None,
            escalation: Some(EscalationSummary {
                kind: EscalationKind::MergeDecision,
                reason: "mock escalation: merge decision".to_string(),
            }),
            inconclusive_reason: None,
            shelved_reason: None,
            held_reason: None,
        }
    }

    /// Mock batch driver: a FIFO of batch-head spec-ids. `run_spec` consumes
    /// the head on success (mirroring a real completed spec leaving the
    /// queue); a `fail_at` spec returns a failure result *without* consuming
    /// the head; a `stall` spec succeeds but is left at the head (the
    /// non-advancing-queue case); a `punt` spec returns a punt result and —
    /// like a real `NeedsAttention` punt — leaves the queue.
    struct MockBatchDriver {
        heads: Vec<String>,
        fail_at: Option<(String, Phase)>,
        stall_spec: Option<String>,
        punt_spec: Option<String>,
        /// BUG-245: when `run_spec` is called for `dispatched`, return a
        /// success result crediting `shipped` instead. The dispatched head
        /// stays in `heads` (matching reality — the implementer dequeued the
        /// shipped spec, not the dispatched one). trace:BUG-245 | ai:claude
        mismatch: Option<(String, String)>,
        escalate_spec: Option<String>,
        /// EPIC-28: specs that should return a shelvable failure (the
        /// orchestrator wrote a `FailureReason` to the store). A drain that
        /// hits one in this set parks it and continues to the next head
        /// (mirroring the punt path) rather than stopping. The mock
        /// consumes the head on shelving, mirroring the
        /// `resolve_batch_members` filter that would skip the freshly-
        /// shelved spec on the next iteration. trace:EPIC-28 | ai:claude
        shelve_at: Vec<(String, Phase)>,
        /// EPIC-28: specs the *test* wants treated as blocked-by a
        /// just-shelved spec. The mock drops them from `heads` *after* the
        /// named blocker is shelved, and records each as a skipped row.
        /// trace:EPIC-28 | ai:claude
        skip_dependent_of: Vec<(String, String, String)>, // (dependent, blocker, reason)
        skipped: Vec<(String, String)>,
        runs: Vec<String>,
    }

    impl MockBatchDriver {
        fn new(heads: &[&str]) -> Self {
            Self {
                heads: heads.iter().map(|s| s.to_string()).collect(),
                fail_at: None,
                stall_spec: None,
                punt_spec: None,
                mismatch: None,
                escalate_spec: None,
                shelve_at: Vec::new(),
                skip_dependent_of: Vec::new(),
                skipped: Vec::new(),
                runs: Vec::new(),
            }
        }

        fn failing(mut self, spec: &str, phase: Phase) -> Self {
            self.fail_at = Some((spec.to_string(), phase));
            self
        }

        fn stalling(mut self, spec: &str) -> Self {
            self.stall_spec = Some(spec.to_string());
            self
        }

        fn punting(mut self, spec: &str) -> Self {
            self.punt_spec = Some(spec.to_string());
            self
        }

        /// BUG-245: when `run_spec` runs `dispatched`, return a success that
        /// credits `shipped`. The dispatched head is *not* consumed — that
        /// mirrors a real run where the implementer worked a different spec
        /// and the dispatched one is still queued. trace:BUG-245 | ai:claude
        fn mismatching(mut self, dispatched: &str, shipped: &str) -> Self {
            self.mismatch = Some((dispatched.to_string(), shipped.to_string()));
            self
        }

        /// STORY-306: mark `spec` so its run returns an escalated result.
        fn escalating(mut self, spec: &str) -> Self {
            self.escalate_spec = Some(spec.to_string());
            self
        }

        /// EPIC-28: mark `spec` so its run returns a shelvable failure.
        /// The mock consumes the head (the real `resolve_batch_members`
        /// would filter the freshly-NeedsAttention spec out next call).
        fn shelving(mut self, spec: &str, phase: Phase) -> Self {
            self.shelve_at.push((spec.to_string(), phase));
            self
        }

        /// EPIC-28: when `blocker` is shelved, also drop `dependent` from
        /// the head queue and record the skip with `reason`. Simulates
        /// `resolve_batch_members`'s pickability filter: a member with
        /// `BlockedBy → <shelved>` becomes un-pickable on the next round.
        fn dependent(mut self, dependent: &str, blocker: &str, reason: &str) -> Self {
            self.skip_dependent_of.push((
                dependent.to_string(),
                blocker.to_string(),
                reason.to_string(),
            ));
            self
        }
    }

    impl BatchDriver for MockBatchDriver {
        fn next_head(&mut self) -> Option<String> {
            self.heads.first().cloned()
        }

        fn run_spec(&mut self, spec: &str) -> OrchestrationResult {
            self.runs.push(spec.to_string());
            if let Some((fail_spec, phase)) = &self.fail_at {
                if fail_spec == spec {
                    // Failure leaves the spec queued — head does not advance.
                    return fail_result(*phase);
                }
            }
            // EPIC-28: a *shelvable* failure parks the spec in NeedsAttention
            // (the orchestrator wrote the FailureReason), and the next head
            // pickup would skip it. Mirror that here by consuming the head.
            // Dependents of this spec drop off too. trace:EPIC-28 | ai:claude
            for (shelve_spec, phase) in self.shelve_at.clone() {
                if shelve_spec == spec {
                    self.heads.retain(|h| h != spec);
                    let to_skip: Vec<(String, String, String)> = self
                        .skip_dependent_of
                        .iter()
                        .filter(|(_, blocker, _)| blocker == spec)
                        .cloned()
                        .collect();
                    for (dep, _blocker, reason) in to_skip {
                        if self.heads.iter().any(|h| h == &dep) {
                            self.heads.retain(|h| h != &dep);
                            self.skipped.push((dep, reason));
                        }
                    }
                    return shelve_result(phase);
                }
            }
            if self.stall_spec.as_deref() == Some(spec) {
                // "Success" but the head is intentionally not consumed.
                return ok_result();
            }
            if self.punt_spec.as_deref() == Some(spec) {
                // A punt parks the spec in NeedsAttention — it leaves the
                // queue, exactly like a shipped spec, so the drain advances.
                self.heads.retain(|h| h != spec);
                return punt_result();
            }
            if let Some((dispatched, shipped)) = &self.mismatch {
                if dispatched == spec {
                    // BUG-245: the dispatched head stays in `heads` (the
                    // implementer dequeued `shipped`, not the dispatched
                    // spec). drain_batch is expected to stop without
                    // re-running the dispatched head. trace:BUG-245
                    return mismatch_result(shipped);
                }
            }
            if self.escalate_spec.as_deref() == Some(spec) {
                // An escalation leaves the queue and advances the drain, like
                // a punt — but lands in `escalated`, not `shipped`.
                self.heads.retain(|h| h != spec);
                return escalated_result();
            }
            // Normal success — the completed spec leaves the queue.
            self.heads.retain(|h| h != spec);
            ok_result()
        }
    }

    /// Acceptance: a 3-item batch with every phase green ships all three via
    /// the auto-complete chain.
    #[test]
    fn drain_batch_three_green_ships_all_three() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2", "TASK-3"]);
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.shipped, vec!["TASK-1", "TASK-2", "TASK-3"]);
        assert_eq!(result.stopped_at, None);
        assert_eq!(driver.runs, vec!["TASK-1", "TASK-2", "TASK-3"]);
    }

    /// Acceptance: a 3-item batch where phase 1 fails on item 2 — item 1
    /// shipped, item 2 stopped the drain, item 3 never ran (queue intact).
    #[test]
    fn drain_batch_phase1_failure_on_item2_leaves_item3_untouched() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2", "TASK-3"])
            .failing("TASK-2", Phase::Implementer);
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, 1, "phase 1 → exit code 1");
        assert_eq!(
            result.outcome,
            BatchDrainOutcome::Failed(Phase::Implementer)
        );
        assert_eq!(result.shipped, vec!["TASK-1"]);
        assert_eq!(result.stopped_at, Some("TASK-2".to_string()));
        // TASK-3 was never run — the queue is intact for a retry.
        assert_eq!(driver.runs, vec!["TASK-1", "TASK-2"]);
    }

    /// A mid-batch phase-3 failure stops the drain with that phase's exit code.
    #[test]
    fn drain_batch_failure_carries_failed_phase_exit_code() {
        let mut driver =
            MockBatchDriver::new(&["TASK-1", "TASK-2"]).failing("TASK-1", Phase::Reviewer);
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, 3);
        assert_eq!(result.outcome, BatchDrainOutcome::Failed(Phase::Reviewer));
        assert!(result.shipped.is_empty());
    }

    /// `--max N` stops the drain after N items even when the batch has more.
    #[test]
    fn drain_batch_max_caps_the_drain() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2", "TASK-3", "TASK-4"]);
        let result = drain_batch(&mut driver, Some(2), None);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.outcome, BatchDrainOutcome::MaxReached);
        assert_eq!(result.shipped, vec!["TASK-1", "TASK-2"]);
        assert_eq!(driver.runs, vec!["TASK-1", "TASK-2"]);
    }

    /// `--max` equal to the batch size reports `Drained`, not `MaxReached` —
    /// the batch genuinely emptied.
    #[test]
    fn drain_batch_max_equal_to_size_reports_drained() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2"]);
        let result = drain_batch(&mut driver, Some(2), None);
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.shipped, vec!["TASK-1", "TASK-2"]);
    }

    /// An empty batch drains immediately with nothing shipped.
    #[test]
    fn drain_batch_empty_batch_is_a_clean_drain() {
        let mut driver = MockBatchDriver::new(&[]);
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert!(result.shipped.is_empty());
    }

    /// TASK-310: a multi-batch drain exhausts each batch before advancing to
    /// the next batch in the user-declared order.
    #[test]
    fn drain_batch_chain_drains_two_batches_in_order() {
        let names = vec!["alpha".to_string(), "beta".to_string()];
        let result = drain_batch_chain(&names, None, None, |name| match name {
            "alpha" => Box::new(MockBatchDriver::new(&["TASK-A1", "TASK-A2"])),
            "beta" => Box::new(MockBatchDriver::new(&["TASK-B1"])),
            other => panic!("unexpected batch {other}"),
        });

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.shipped, vec!["TASK-A1", "TASK-A2", "TASK-B1"]);
        assert_eq!(
            result
                .steps
                .iter()
                .map(|s| s.batch_name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
    }

    /// TASK-310: an empty intermediate batch is silently skipped; later
    /// batches still run.
    #[test]
    fn drain_batch_chain_skips_empty_middle_batch() {
        let names = vec!["alpha".to_string(), "empty".to_string(), "beta".to_string()];
        let result = drain_batch_chain(&names, None, None, |name| match name {
            "alpha" => Box::new(MockBatchDriver::new(&["TASK-A1"])),
            "empty" => Box::new(MockBatchDriver::new(&[])),
            "beta" => Box::new(MockBatchDriver::new(&["TASK-B1"])),
            other => panic!("unexpected batch {other}"),
        });

        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.shipped, vec!["TASK-A1", "TASK-B1"]);
        assert_eq!(result.steps.len(), 3);
        assert!(result.steps[1].result.shipped.is_empty());
    }

    /// TASK-310: a failure in one batch stops the whole chain and does not
    /// advance into later batches.
    #[test]
    fn drain_batch_chain_failure_stops_before_next_batch() {
        let names = vec!["alpha".to_string(), "beta".to_string()];
        let result = drain_batch_chain(&names, None, None, |name| match name {
            "alpha" => Box::new(
                MockBatchDriver::new(&["TASK-A1", "TASK-A2"]).failing("TASK-A2", Phase::Reviewer),
            ),
            "beta" => Box::new(MockBatchDriver::new(&["TASK-B1"])),
            other => panic!("unexpected batch {other}"),
        });

        assert_eq!(result.exit_code, 3);
        assert_eq!(result.outcome, BatchDrainOutcome::Failed(Phase::Reviewer));
        assert_eq!(result.shipped, vec!["TASK-A1"]);
        assert_eq!(result.stopped_batch.as_deref(), Some("alpha"));
        assert_eq!(result.stopped_at.as_deref(), Some("TASK-A2"));
        assert_eq!(result.steps.len(), 1, "beta should never start");
    }

    /// TASK-310: `--max` caps total acted-on specs across all batches, not
    /// per batch.
    #[test]
    fn drain_batch_chain_max_caps_across_batches() {
        let names = vec!["alpha".to_string(), "beta".to_string()];
        let result = drain_batch_chain(&names, Some(3), None, |name| match name {
            "alpha" => Box::new(MockBatchDriver::new(&["TASK-A1", "TASK-A2"])),
            "beta" => Box::new(MockBatchDriver::new(&["TASK-B1", "TASK-B2"])),
            other => panic!("unexpected batch {other}"),
        });

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.outcome, BatchDrainOutcome::MaxReached);
        assert_eq!(result.shipped, vec!["TASK-A1", "TASK-A2", "TASK-B1"]);
        assert_eq!(result.stopped_batch.as_deref(), Some("beta"));
    }

    /// STORY-276: a punted member advances the drain — it leaves the queue
    /// (`NeedsAttention`), so the batch keeps going — but it lands in `punted`,
    /// not `shipped`, so the summary does not claim a parked spec shipped.
    #[test]
    fn drain_batch_punted_member_advances_the_drain() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2", "TASK-3"]).punting("TASK-2");
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, 0, "a punt does not fail the drain");
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.shipped, vec!["TASK-1", "TASK-3"]);
        assert_eq!(result.punted, vec!["TASK-2"]);
        // Every member ran — the punt did not stall or short-circuit the drain.
        assert_eq!(driver.runs, vec!["TASK-1", "TASK-2", "TASK-3"]);
    }

    /// STORY-306: an escalated member advances the drain like a punt — it
    /// leaves the queue, the batch keeps going — but it lands in `escalated`,
    /// not `shipped`, so the summary does not claim it shipped.
    #[test]
    fn drain_batch_escalated_member_advances_the_drain() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2", "TASK-3"]).escalating("TASK-2");
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, 0, "an escalation does not fail the drain");
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.shipped, vec!["TASK-1", "TASK-3"]);
        assert_eq!(result.escalated, vec!["TASK-2"]);
        assert!(result.punted.is_empty());
        assert_eq!(driver.runs, vec!["TASK-1", "TASK-2", "TASK-3"]);
    }

    // --- EPIC-28: shelve on failure + dependency-aware skip --------------

    /// EPIC-28: a shelvable phase failure parks the spec in NeedsAttention
    /// and the drain continues — independents after the failure still ship.
    /// trace:EPIC-28 | ai:claude
    #[test]
    fn drain_batch_shelves_failure_and_continues_to_next_member() {
        let mut driver =
            MockBatchDriver::new(&["TASK-1", "TASK-2", "TASK-3"]).shelving("TASK-2", Phase::Ci);
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(
            result.exit_code, 2,
            "a drain with shelved members exits 2, not the phase code"
        );
        assert_eq!(result.outcome, BatchDrainOutcome::DrainedWithShelved);
        assert_eq!(result.shipped, vec!["TASK-1", "TASK-3"]);
        assert_eq!(result.shelved, vec!["TASK-2"]);
        // TASK-3 must have actually run — the whole point of EPIC-28.
        assert_eq!(driver.runs, vec!["TASK-1", "TASK-2", "TASK-3"]);
    }

    /// EPIC-28: when a shelving event also makes a downstream member
    /// un-pickable (BlockedBy → shelved), the dependent is skipped — never
    /// run — and shows up in `skipped`, while independents after it still
    /// ship. trace:EPIC-28 | ai:claude
    #[test]
    fn drain_batch_skips_dependent_when_blocker_shelved() {
        let mut driver = MockBatchDriver::new(&["TASK-A", "TASK-B", "TASK-D", "TASK-E"])
            .shelving("TASK-B", Phase::Ci)
            .dependent("TASK-D", "TASK-B", "blocked-by TASK-B (Needs Attention)");
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, 2);
        assert_eq!(result.outcome, BatchDrainOutcome::DrainedWithShelved);
        assert_eq!(result.shipped, vec!["TASK-A", "TASK-E"]);
        assert_eq!(result.shelved, vec!["TASK-B"]);
        // TASK-D must NOT appear in `runs` — it never reached `run_spec`.
        assert!(
            !driver.runs.iter().any(|s| s == "TASK-D"),
            "dependent TASK-D should have been skipped, not run: {:?}",
            driver.runs
        );
        // And it should show up in the mock's skipped list (a real CLI surface
        // would surface the same via `result.skipped`, but that's populated
        // by `resolve_batch_members`, not `drain_batch` itself).
        assert_eq!(
            driver.skipped,
            vec![(
                "TASK-D".to_string(),
                "blocked-by TASK-B (Needs Attention)".to_string()
            )]
        );
    }

    /// EPIC-28: the `max_failures` safety cap stops the drain when too
    /// many specs shelve in a row — the environment is probably broken.
    /// `max_failures = 2` means the third shelvable failure flips back
    /// to the historical `Failed(phase)` stop. trace:EPIC-28 | ai:claude
    #[test]
    fn drain_batch_caps_at_max_failures_and_stops() {
        let mut driver = MockBatchDriver::new(&["A", "B", "C", "D", "E"])
            .shelving("A", Phase::Ci)
            .shelving("B", Phase::Ci)
            .shelving("C", Phase::Ci);
        let result = drain_batch(&mut driver, None, Some(2));
        // First two shelve and the drain continues; the third trips the cap.
        assert_eq!(result.outcome, BatchDrainOutcome::Failed(Phase::Ci));
        assert_eq!(result.exit_code, Phase::Ci.index());
        assert_eq!(result.shelved, vec!["A", "B"]);
        assert_eq!(result.stopped_at, Some("C".to_string()));
        // D and E were never attempted — the cap stops the drain.
        assert!(!driver.runs.iter().any(|s| s == "D"));
        assert!(!driver.runs.iter().any(|s| s == "E"));
    }

    /// EPIC-28: regression — a clean drain (nothing shelved, nothing
    /// skipped) keeps the historical `Drained` outcome + exit 0, even
    /// though `DrainedWithShelved` is now reachable. trace:EPIC-28
    #[test]
    fn drain_batch_clean_drain_without_shelved_still_returns_drained_exit_0() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2"]);
        let result = drain_batch(&mut driver, None, Some(5));
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert!(result.shelved.is_empty());
        assert!(result.skipped.is_empty());
    }

    /// EPIC-28: punts and shelvings can both happen in one drain, and the
    /// summary keeps them sorted — neither leaks into `shipped`.
    /// trace:EPIC-28 | ai:claude
    #[test]
    fn drain_batch_punt_and_shelve_in_same_drain_both_counted_separately() {
        let mut driver = MockBatchDriver::new(&["P", "S", "Q", "R"])
            .punting("P")
            .shelving("S", Phase::Ci);
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.outcome, BatchDrainOutcome::DrainedWithShelved);
        assert_eq!(result.exit_code, 2);
        assert_eq!(result.punted, vec!["P"]);
        assert_eq!(result.shelved, vec!["S"]);
        assert_eq!(result.shipped, vec!["Q", "R"]);
    }

    /// EPIC-28: an un-shelvable failure (the mock's default `failing` path —
    /// `shelved_reason: None`) still stops the drain at the historical
    /// `Failed(phase)` outcome. Confirms the routing rule: only shelvable
    /// failures continue. trace:EPIC-28 | ai:claude
    #[test]
    fn drain_batch_unshelvable_failure_still_stops_the_drain() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2", "TASK-3"])
            .failing("TASK-2", Phase::Implementer);
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(
            result.outcome,
            BatchDrainOutcome::Failed(Phase::Implementer)
        );
        assert_eq!(result.exit_code, 1);
        assert!(result.shelved.is_empty());
        assert_eq!(result.shipped, vec!["TASK-1"]);
    }

    /// A "successful" run that leaves the head in place is caught by the
    /// non-advancing-queue guard rather than looping forever.
    #[test]
    fn drain_batch_stall_guard_stops_a_non_advancing_queue() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2"]).stalling("TASK-2");
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, 1);
        assert_eq!(result.outcome, BatchDrainOutcome::Stalled);
        assert_eq!(result.shipped, vec!["TASK-1"]);
        assert_eq!(result.stopped_at, Some("TASK-2".to_string()));
        // TASK-2 ran once, was re-yielded as the head, and the guard fired
        // before a second run.
        assert_eq!(driver.runs, vec!["TASK-1", "TASK-2"]);
    }

    // --- BUG-245: dispatched≠shipped attribution ---------------------------

    /// Acceptance: dispatch phase-1 for STORY-A; phase 1 produces a PR
    /// crediting STORY-B (the implementer pragmatically worked a release
    /// blocker). The orchestrator credits B in the success epilogue and
    /// carries the mismatch into `OrchestrationResult::shipped_spec_id` so
    /// the batch drain records the truth. trace:BUG-245 | ai:claude
    #[test]
    fn orchestrate_credits_shipped_when_pr_mismatches_dispatched() {
        let mut driver = MockPhaseDriver::all_ok().shipping_as("BUG-244");
        let result = orchestrate(
            &mut driver,
            "STORY-276",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.failed_phase, None);
        assert_eq!(result.shipped_spec_id, Some("BUG-244".to_string()));
        // Phases 2-6 still ran on the PR (CI, reviewer, merge, pull, build) —
        // the mismatch only re-attributes the credit, it doesn't truncate
        // the pipeline.
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ]
        );
    }

    /// When phase 1's PR credits the dispatched spec (the common path), no
    /// mismatch fires and the result carries no `shipped_spec_id`. A driver
    /// that explicitly reports the dispatched id behaves the same as one
    /// that returns `None` — both mean "the dispatched spec is what
    /// shipped". trace:BUG-245 | ai:claude
    #[test]
    fn orchestrate_no_mismatch_when_pr_credits_dispatched() {
        // The default driver returns `None` for shipped_spec_id — no
        // mismatch.
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate(
            &mut driver,
            "STORY-276",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.shipped_spec_id, None);

        // A driver that names the dispatched id is also not a mismatch.
        let mut driver = MockPhaseDriver::all_ok().shipping_as("STORY-276");
        let result = orchestrate(
            &mut driver,
            "STORY-276",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.shipped_spec_id, None);
    }

    /// Acceptance test from BUG-245's description: a batch drain whose head
    /// is dispatched but ships a different spec. The drain credits the
    /// actual shipped id in `shipped`, leaves the dispatched id queued
    /// (still in `heads`), and stops with `Mismatched` rather than looping
    /// on the un-advanced head. trace:BUG-245 | ai:claude
    #[test]
    fn drain_batch_credits_actual_shipped_on_mismatch() {
        let mut driver =
            MockBatchDriver::new(&["STORY-276", "TASK-99"]).mismatching("STORY-276", "BUG-244");
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, 0, "a mismatch is not a failure");
        assert_eq!(
            result.outcome,
            BatchDrainOutcome::Mismatched {
                dispatched: "STORY-276".to_string(),
                shipped: "BUG-244".to_string(),
            }
        );
        // The truth — BUG-244 — is what `shipped` records, not STORY-276.
        assert_eq!(result.shipped, vec!["BUG-244"]);
        // The dispatched spec is reported as where the drain stopped.
        assert_eq!(result.stopped_at, Some("STORY-276".to_string()));
        // STORY-276 is still queued — the drain did not run TASK-99 either,
        // it stopped at the mismatched head for operator inspection.
        assert_eq!(driver.runs, vec!["STORY-276"]);
        assert!(driver.heads.contains(&"STORY-276".to_string()));
    }

    /// Self-consistency invariant from BUG-245's acceptance: the drain
    /// summary must never claim a spec shipped *and* report it stayed at
    /// the head in the same run. The mismatch outcome credits the actual
    /// shipped id (not the dispatched one) in `shipped`, so a reader cannot
    /// see "✓ X shipped" alongside "X stayed at head" for the same X.
    /// trace:BUG-245 | ai:claude
    #[test]
    fn drain_batch_mismatch_summary_is_self_consistent() {
        let mut driver = MockBatchDriver::new(&["STORY-276"]).mismatching("STORY-276", "BUG-244");
        let result = drain_batch(&mut driver, None, None);
        // The dispatched head appears in `stopped_at` (queue did not
        // advance) — it must NOT also appear in `shipped`.
        let dispatched = result.stopped_at.as_deref().unwrap();
        assert!(
            !result.shipped.iter().any(|s| s == dispatched),
            "dispatched spec must not be credited as shipped"
        );
        // And `shipped` must carry the spec the PR actually shipped.
        assert!(
            result.shipped.iter().any(|s| s == "BUG-244"),
            "actual shipped spec must be credited"
        );
    }
}
