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

// trace:STORY-1889 | ai:claude
use crate::gate::branch_policy::is_branch_policy_merge_refusal;
use std::path::Path;
use std::time::Instant;

use colored::Colorize;

/// Render a registry glyph honoring the active profile. Default Unicode profile
/// reproduces the historical literals byte-for-byte. trace:TASK-840 | ai:claude
fn glyph(g: crate::glyphs::Glyph) -> &'static str {
    crate::glyphs::get(g, crate::find_project_root().ok().as_deref())
}

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

/// The fasttrack express-tier routing bucket. A spec carrying this tag rides
/// the express lane: fast because reliably *routed*, not because it is *less
/// gated*. Express keeps the FULL pipeline — CI + reviewer + build all run —
/// so any `lifecycle:*` short-circuit on the same spec is deliberately
/// overridden.
// trace:TASK-907 | ai:claude  (fasttrack express tier, STORY-692 / TASK-906)
pub(crate) const EXPRESS_TIER_TAG: &str = "batch:express";

/// Per-spec lifecycle short-circuit switches resolved from `lifecycle:*`
/// tags. These skip only non-integrity phases: CI wait, reviewer, and local
/// build. Merge and pull/auto-bump are deliberately not skippable.
///
/// The express tier (`batch:express`) is the inverse: it carries no
/// short-circuit at all and its trust contract is that it NEVER silently
/// downgrades its gate. When `express` is set, all three skips are forced off
/// regardless of any `lifecycle:*` tag also present on the spec.
/// trace:STORY-442 | ai:codex
// trace:TASK-907 | ai:claude
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LifecycleSkip {
    pub(crate) no_preflight: bool,
    pub(crate) no_ci_wait: bool,
    pub(crate) no_review: bool,
    pub(crate) no_build: bool,
    // TASK-1249: `lifecycle:no-harvest` skips the advisory harvest step
    // between the review gates and merge (never a phase, never blocking).
    pub(crate) no_harvest: bool,
    // TASK-907: this spec is on the fasttrack express tier (`batch:express`).
    // Express runs the full gate; the skip fields above are forced off when
    // this is true. Tracked so the orchestrator banner can announce the tier.
    pub(crate) express: bool,
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
                "lifecycle:no-preflight" => skip.no_preflight = true,
                // trace:TASK-1249 | ai:claude
                "lifecycle:no-harvest" => skip.no_harvest = true,
                "lifecycle:trivial" => {
                    skip.no_ci_wait = true;
                    skip.no_review = true;
                    skip.no_build = true;
                }
                // TASK-907: express tier — recognized here so the single
                // lifecycle-resolution point the drain already uses honors the
                // express routing. Match is case-insensitive like the rest.
                t if t == EXPRESS_TIER_TAG => skip.express = true,
                _ => {}
            }
        }
        // TASK-907: the express trust contract — full CI + reviewer + build, no
        // review-skip. Express overrides any `lifecycle:*` short-circuit that
        // would otherwise downgrade the gate, so an express spec can never
        // silently ship under a reduced gate.
        //
        // This must clear EVERY integrity-phase skip field, not a hand-picked
        // subset: `no_preflight` was omitted here (BUG-1507) while the
        // plain-express test case asserted the whole-set invariant
        // (`is_empty()`) instead — so a `batch:express` spec carrying
        // `lifecycle:no-preflight` shipped with preflight silently skipped
        // even though the banner said "full gate". `no_harvest` is
        // deliberately NOT cleared: harvest is advisory, not an integrity
        // phase, so express does not force it back on.
        // trace:BUG-1507 | ai:claude (PRIN-5: absent is not good evidence)
        if skip.express {
            skip.no_preflight = false;
            skip.no_ci_wait = false;
            skip.no_review = false;
            skip.no_build = false;
        }
        skip
    }

    pub(crate) fn is_empty(self) -> bool {
        !self.no_preflight
            && !self.no_ci_wait
            && !self.no_review
            && !self.no_build
            && !self.no_harvest
    }

    /// TASK-525: the active short-circuit tokens (`no-ci-wait`, `no-review`,
    /// `no-build`) for telemetry — recorded on the auto-complete JSONL event so
    /// retro analysis can see how often fast-track tags fire and on what.
    /// Empty when no skip is active. trace:TASK-525 | ai:claude
    pub(crate) fn active_tokens(self) -> Vec<String> {
        let mut v = Vec::new();
        if self.no_preflight {
            v.push("no-preflight".to_string());
        }
        if self.no_ci_wait {
            v.push("no-ci-wait".to_string());
        }
        if self.no_review {
            v.push("no-review".to_string());
        }
        if self.no_build {
            v.push("no-build".to_string());
        }
        if self.no_harvest {
            v.push("no-harvest".to_string());
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
    "lifecycle:no-preflight",
    "lifecycle:no-harvest",
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
        // TASK-907: the express tier announces itself even though it skips
        // nothing — its contract is "fast because reliably routed, full gate".
        //
        // The early return used to be unconditional, so it printed "full
        // gate" even on a spec where `express` was true but a skip field had
        // leaked past the from_tags() force-off (BUG-1507's THE BUG). That is
        // exactly the failure this banner exists to prevent: it is the only
        // operator-facing surface reporting which phases actually ran, and a
        // hardcoded string cannot be wrong in a way a compiler or test would
        // catch. Gate the canned string on `is_empty()` — the same universal
        // check the trust contract itself is defined by — so any residual
        // skip is still named instead of silently contradicted.
        // trace:BUG-1507 | ai:claude (PRIN-5: absent is not good evidence)
        if self.express && self.is_empty() {
            return Some("express tier — full gate (CI + reviewer + build)".to_string());
        }
        if self.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        if self.no_preflight {
            parts.push("implementer preflight");
        }
        if self.no_ci_wait {
            parts.push("CI wait");
        }
        if self.no_review {
            parts.push("reviewer");
        }
        if self.no_build {
            parts.push("build");
        }
        // trace:BUG-1222 | ai:codex
        if self.no_harvest {
            parts.push("harvest");
        }
        let summary = format!("skipping {}", parts.join(" + "));
        if self.express {
            // Should be unreachable given the from_tags() force-off, but the
            // banner must never claim "full gate" while parts is non-empty.
            // trace:BUG-1507 | ai:claude (PRIN-5: absent is not good evidence)
            Some(format!(
                "express tier — {summary} (trust-contract violation)"
            ))
        } else {
            Some(summary)
        }
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
    /// Phase 2: the CI verdict could not be established after bounded probe
    /// retries — either the forge could not be read at all (transient
    /// infrastructure trouble) or a supervised hold made the coarse red
    /// ambiguous and the per-check rows stayed unavailable. Never permission
    /// to bypass the gate.
    // trace:BUG-1250 trace:BUG-1265 | ai:codex
    CiUnavailable,
    /// Phase 3: the reviewer session wrote no verdict file, or an unreadable
    /// one — the review never produced a usable decision.
    NoVerdict,
    /// Phase 3: the reviewer produced a RequestChanges verdict. This is a
    /// typed shelve cause, not a crash.
    // trace:STORY-974 | ai:codex
    VerdictRequestChanges,
    /// External evidence only; never an implementer retry or shelf.
    // trace:BUG-2500 | ai:codex
    VerdictWaiting,
    /// Phase 3: the reviewer produced a Rejected verdict. This is a typed
    /// shelve cause, not a crash.
    // trace:STORY-974 | ai:codex
    VerdictReject,
    /// BUG-1186: the reviewer seat moved the PR head it was asked to judge —
    /// an independence breach, not a transient. Shelved for triage, never
    /// retried (a retry would review reviewer-authored commits).
    // trace:BUG-1186 | ai:claude
    ReviewerWrote,
    /// BUG-1213: a rework implementer exited successfully without moving the
    /// already-open PR head. The previous round's commit cannot satisfy the
    /// current review delta; this is a terminal attention item, not retryable.
    // trace:BUG-1213 | ai:codex
    ReworkNoOp,
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
    /// BUG-908: a retry could not reclaim its own predecessor lease. This is a
    /// typed self-collision, not a generic tool exit; telemetry records the
    /// lease/worktree that blocked retry and auto-draft suppresses duplicates.
    // trace:BUG-908 | ai:codex
    LeaseConflict,
    /// BUG-1930: the phase's headless stream recorded permission denials and
    /// the phase produced no commit or PR. The agent stopped because its
    /// writes were refused (sandbox / permission mode), an environmental
    /// cause: never transient-retried, and not a reason to draft a BUG.
    // trace:BUG-1930 | ai:claude
    PermissionDenied,
    /// BUG-826: the headless vendor process exited non-zero after creating a
    /// zero-byte JSONL log. That is a launch-layer transient, not evidence that
    /// the implementer attempted the work and failed.
    ///
    /// BUG-1716 widens it to the never-started sibling: a vendor that rejects
    /// its own launch (an argv/usage error) exits non-zero WITHOUT creating
    /// the log at all, corroborated by no commits on the session's lease
    /// branch. Either shape means no agent ever worked, so this kind spends
    /// no STORY-975 transient retry (`cause_slug` "environmental" is not in
    /// `is_transient_retry_cause`), the empty lease is released, and the
    /// TASK-133 compensation returns the spec to its pre-bump status instead
    /// of leaving it parked `NeedsAttention`.
    // trace:BUG-826 trace:BUG-1716 | ai:claude
    LaunchNoOutput,
    /// BUG-1063: a headless phase ended its turn while waiting for a future
    /// notification / monitor / watcher callback. Under headless vendor exec
    /// there is no next turn, so this is a typed waiting-contract violation
    /// rather than a generic tool exit.
    // trace:BUG-1063 | ai:codex
    HeadlessWait,
    /// BUG-1218: the stale-base auto-rebase refused to force-push because the
    /// live remote contains commits the rebased branch does not incorporate.
    /// This requires operator reconciliation and must never spend a transient
    /// retry (which would deterministically hit the same guard again).
    // trace:BUG-1218 | ai:codex
    StaleBaseRefused,
    /// BUG-1268: the reviewer pre-flight attempted its one bounded stale-base
    /// recovery, but the rebase conflicted. `aida pr rebase` aborts and removes
    /// its disposable worktree before this is returned, so the PR branch is
    /// unchanged and a human can follow the interactive recovery recipe.
    // trace:BUG-1268 | ai:codex
    StaleBaseConflict,
    /// BUG-1316: the substrate correctly refused a supervised PR because its
    /// human/advisor merge-hold is still open. Retrying cannot change this
    /// stable gate, so preserve it as a typed, non-transient shelve cause.
    // trace:BUG-1316 | ai:codex
    MergeHold,
    /// TASK-1448: the merge phase refused because the PR's newest recorded
    /// APPROVAL does not cover its current head — the head moved past the
    /// approved commit, the approval names no commit, or the head could not
    /// be read (PRIN-5: fail closed). A retry cannot change it; only a fresh
    /// review of the current head can, so this shelves and never retries.
    // trace:TASK-1448 | ai:claude
    StaleApproval,
    /// TASK-1459: the merge phase refused because a review-in-progress
    /// marker (STORY-1405) is live on this PR's head — another seat (or an
    /// explicit `aida review claim`) is actively reviewing it. Distinct from
    /// [`Self::LeaseConflict`] (a merge-LEASE contention, i.e. two mergers
    /// racing the same PR): this is a REVIEW still running, so the right
    /// hint names the marker's own clearing verbs, not `aida merge-lock`.
    /// Shelvable (a human should see it), never auto-retried — a retry
    /// would just hit the same live marker again until it expires or the
    /// review records a verdict.
    // trace:TASK-1459 | ai:claude
    ReviewInProgress,
    /// BUG-1527: the implementer's worktree ended on a branch other than the
    /// one this phase was dispatched for, AND that branch's commits credit a
    /// DIFFERENT spec — the drain accepted a branch swap and nearly marked
    /// the dispatched spec Done on someone else's PR. Never retried (a retry
    /// would just re-launch against the same swapped-away worktree); shelved
    /// so the swap is triaged rather than silently written over.
    // trace:BUG-1527 | ai:claude
    ShippedMismatch,
    /// Phase 1 never launched an implementer session at all — a
    /// preflight refusal (e.g. a diverged PR branch), a lease/claim refusal,
    /// or an agent-binary launch failure, all caught before any child
    /// process could have committed anything. This is distinct from
    /// `Failed`-like kinds where a session ran and then failed: a
    /// launch refusal means the branch head is provably unchanged, so it
    /// must never be treated as "phase 1 failed but an already-open PR from
    /// a PREVIOUS round proves the work happened" — that conflation is what
    /// manufactured a phantom CI/review round against a head no implementer
    /// this round ever touched. Shelvable (so the refusal is visible where
    /// findings are read), but never routed through
    /// `recover_phase1_failure_with_open_pr`.
    // trace:BUG-1524 | ai:claude
    LaunchRefused,
    /// STORY-1690: the merge phase refused because the PR's head is behind
    /// its base (or that could not be computed), so its green was never
    /// tested with main's newer commits. Not a broken PR: it lands through
    /// `aida pr train`, which tests it on the current main. Shelved, never
    /// retried (a retry meets the same moved main).
    // trace:STORY-1690 | ai:claude
    BehindBase,
    /// The spawned work ran and reported failure — the phase-specific default.
    /// The hint points at the phase's normal "address it and retry" path.
    Failed,
}

impl FailureKind {
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
                | Self::CiUnavailable
                | Self::NoVerdict
                | Self::VerdictRequestChanges
                | Self::VerdictReject
                // trace:BUG-1186 | ai:claude
                | Self::ReviewerWrote
                | Self::ReworkNoOp
                | Self::PrVerificationInconclusive
                | Self::Watchdog
                | Self::CacheLocked
                | Self::LeaseConflict
                | Self::PermissionDenied
                // trace:BUG-826 | ai:codex
                | Self::LaunchNoOutput
                // trace:BUG-1063 | ai:codex
                | Self::HeadlessWait
                | Self::StaleBaseRefused
                | Self::StaleBaseConflict
                | Self::MergeHold
                // trace:TASK-1448 | ai:claude
                | Self::StaleApproval
                // trace:BUG-1524 | ai:claude
                | Self::LaunchRefused
                | Self::Failed
                // trace:BUG-1527 | ai:claude
                | Self::ShippedMismatch
                // trace:TASK-1459 | ai:claude
                | Self::ReviewInProgress
                // trace:STORY-1690 | ai:claude
                | Self::BehindBase
        )
    }

    /// STORY-974: closed public cause vocabulary for persisted shelvings and
    /// operator-facing telemetry. Legacy `slug()` values stay available for
    /// old dedupe tags/tests, but new FailureReason/SpecShelved rows use this.
    pub(crate) fn cause_slug(self) -> &'static str {
        match self {
            Self::VerdictWaiting => "verdict:waiting",
            Self::VerdictRequestChanges => "verdict:request-changes",
            Self::VerdictReject => "verdict:reject",
            Self::ReviewerWrote => "reviewer-wrote",
            Self::ReworkNoOp => "rework-no-op",
            Self::CiRed => "ci-red",
            Self::CiUnavailable => "ci-unavailable",
            Self::NoVerdict => "no-verdict",
            Self::NoPr => "no-pr",
            Self::Watchdog | Self::CiTimeout => "watchdog",
            Self::CacheLocked => "cache-locked",
            Self::LeaseConflict => "lease-conflict",
            Self::PermissionDenied => "permission-denied",
            Self::Internal => "internal",
            Self::Spawn
            | Self::MissingTool
            | Self::PrVerificationInconclusive
            | Self::LaunchNoOutput => "environmental",
            Self::HeadlessWait => "headless-wait",
            Self::StaleBaseRefused => "stale-base-refused",
            Self::StaleBaseConflict => "stale-base-conflict",
            Self::MergeHold => "merge-hold",
            // trace:TASK-1448 | ai:claude
            Self::StaleApproval => "stale-approval",
            // trace:BUG-1524 | ai:claude
            Self::LaunchRefused => "launch-refused",
            Self::Failed => "tool-exit",
            // trace:BUG-1527 | ai:claude
            Self::ShippedMismatch => "shipped-mismatch",
            // trace:TASK-1459 | ai:claude
            Self::ReviewInProgress => "review-in-progress",
            // trace:STORY-1690 | ai:claude
            Self::BehindBase => "behind-base",
        }
    }

    /// BUG-1930: the inverse of [`Self::cause_slug`], used where a failure
    /// crosses a process boundary as its slug (the pipelined child sidecar).
    /// Shared slugs map to one representative kind with the same slug, so
    /// `from_cause_slug(k.cause_slug()).cause_slug() == k.cause_slug()` for
    /// every kind. An unknown slug is `None`.
    // trace:BUG-1930 | ai:claude
    pub(crate) fn from_cause_slug(slug: &str) -> Option<Self> {
        Some(match slug {
            "verdict:waiting" => Self::VerdictWaiting,
            "verdict:request-changes" => Self::VerdictRequestChanges,
            "verdict:reject" => Self::VerdictReject,
            "reviewer-wrote" => Self::ReviewerWrote,
            "rework-no-op" => Self::ReworkNoOp,
            "ci-red" => Self::CiRed,
            "ci-unavailable" => Self::CiUnavailable,
            "no-verdict" => Self::NoVerdict,
            "no-pr" => Self::NoPr,
            "watchdog" => Self::Watchdog,
            "cache-locked" => Self::CacheLocked,
            "lease-conflict" => Self::LeaseConflict,
            "permission-denied" => Self::PermissionDenied,
            "internal" => Self::Internal,
            "environmental" => Self::LaunchNoOutput,
            "headless-wait" => Self::HeadlessWait,
            "stale-base-refused" => Self::StaleBaseRefused,
            "stale-base-conflict" => Self::StaleBaseConflict,
            "merge-hold" => Self::MergeHold,
            "stale-approval" => Self::StaleApproval,
            "launch-refused" => Self::LaunchRefused,
            "tool-exit" => Self::Failed,
            "shipped-mismatch" => Self::ShippedMismatch,
            "review-in-progress" => Self::ReviewInProgress,
            "behind-base" => Self::BehindBase,
            _ => return None,
        })
    }
}

/// A reviewer's verdict, read from `.aida/review-verdicts/PR-N.json`.
/// trace:STORY-246 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    Waiting,
    Approved,
    RequestChanges,
    Rejected,
}

impl Verdict {
    /// Parse the `verdict` field of a verdict file. Routes through the one
    /// canonical parser ([`crate::review_verdict::VerdictKind::parse`]) so
    /// the orchestrator and every other verdict reader agree on every
    /// spelling. An unrecognised or qualified word (`APPROVED pending …`) is
    /// `None` — never an approval.
    // trace:BUG-1505 | ai:claude
    pub(crate) fn parse(s: &str) -> Option<Self> {
        use crate::gate::review_verdict::VerdictKind;
        match VerdictKind::parse(s) {
            VerdictKind::Approved => Some(Self::Approved),
            VerdictKind::RequestChanges => Some(Self::RequestChanges),
            VerdictKind::Rejected => Some(Self::Rejected),
            VerdictKind::Waiting => Some(Self::Waiting),
            VerdictKind::Unknown => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Waiting => "Waiting",
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
///
/// `hint_override`, when set, REPLACES that generic per-`kind` hint. It
/// exists so a failure whose specific hint depends on more than `kind` alone
/// — today, only the watchdog's fired [`WatchdogTrip`] — can carry the hint
/// [`watchdog_trip_report`] resolved in the SAME match arm as `reason`,
/// rather than have a caller re-derive it from `kind` alone in a second,
/// independent match that can drift out of agreement with `reason`
/// (BUG-1299). Additive and `None` for every other failure kind.
/// trace:STORY-246 | ai:claude
// trace:BUG-1299 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PhaseFailure {
    pub(crate) reason: String,
    pub(crate) kind: FailureKind,
    pub(crate) hint_override: Option<String>,
}

impl PhaseFailure {
    /// A failure of the phase's default kind ([`FailureKind::Failed`]) — the
    /// spawned work ran and reported failure. The common case, so it keeps
    /// the terse constructor name.
    pub(crate) fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            kind: FailureKind::Failed,
            hint_override: None,
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
            hint_override: None,
        }
    }

    /// Attach a hint that was resolved together with `reason` (from the same
    /// decision) so it must be used verbatim instead of the generic
    /// per-`kind` [`recovery_hint`].
    // trace:BUG-1299 | ai:claude
    pub(crate) fn with_hint_override(mut self, hint: impl Into<String>) -> Self {
        self.hint_override = Some(hint.into());
        self
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
    // external-prose-classifier: auto_complete::is_database_locked_message
    aida_core::external_tool_output::contains_any_case_insensitive(
        reason,
        aida_core::external_tool_output::SQLITE_LOCKED,
    )
}

/// BUG-657: does a failure `message` describe an ENVIRONMENTAL fault — the disk
/// is full or the machine ran out of memory — rather than a spec or
/// orchestrator bug? The auto-draft text the orchestrator files on a failure
/// already tells the triager to *reject* environment issues, so the drive
/// suppresses the auto-file entirely for these (one less phantom Draft to open
/// and reject). Pure, case-insensitive substring match on the spellings the
/// OS / toolchain emit, chosen to avoid false positives (no bare `oom` — it
/// hides in `groom`/`room`).
// trace:BUG-657 | ai:claude
pub(crate) fn is_environmental_failure(message: &str) -> bool {
    // external-prose-classifier: auto_complete::is_environmental_failure
    aida_core::external_tool_output::contains_any_case_insensitive(
        message,
        aida_core::external_tool_output::OS_RESOURCE_EXHAUSTION,
    )
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
    /// Successful native seat with no changes or PR. trace:TASK-3608 | ai:codex
    NoChange,
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
    /// BUG-709: the implementer ran the FULL ship itself (create + CI + merge,
    /// e.g. a codex implementer that drove `aida pr ship` end-to-end), so by
    /// the time the orchestrator's phase-1 verify runs there is no OPEN PR to
    /// shepherd — the branch's PR is already MERGED and the spec already
    /// auto-bumped. The work shipped: the drive completes cleanly (skipping
    /// CI/review/merge/pull) instead of spinning the open-PR verify to its
    /// retry ceiling and false-negativing a landed drive as "inconclusive,
    /// retry". `pr_number` is the merged PR surfaced in the epilogue.
    // trace:BUG-709 | ai:claude
    AlreadyMerged { pr_number: u32 },
}

/// BUG-420: which watchdog tripped on a degenerate headless phase. The
/// no-progress signal (no commit + no file-change for N minutes) is the
/// precise catch for the echo/sleep filler-spin; the wall-clock ceiling is a
/// hard backstop. trace:BUG-420 | ai:claude
#[allow(dead_code)] // decision core; wired into the phase spawn-wait by slice 2
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WatchdogTrip {
    NoProgress,
    Ceiling,
    /// STORY-998: the session is alive and emitting bytes, but the bytes are a
    /// low-information repeated template.
    Spinning {
        template: String,
        count: usize,
    },
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
    watchdog_verdict_with_ci_wait(since_progress, total, no_progress_limit, ceiling, false)
}

/// BUG-749: an implementer blocked inside `aida pr ship` while CI is pending is
/// live progress even when the worktree and headless log are quiet. CI-wait
/// suppresses only the no-progress trip; the wall-clock ceiling remains the
/// hard stop for a truly overlong phase.
// trace:BUG-749 | ai:codex
pub(crate) fn watchdog_verdict_with_ci_wait(
    since_progress: std::time::Duration,
    total: std::time::Duration,
    no_progress_limit: std::time::Duration,
    ceiling: std::time::Duration,
    ci_waiting: bool,
) -> Option<WatchdogTrip> {
    if !ci_waiting && !no_progress_limit.is_zero() && since_progress >= no_progress_limit {
        return Some(WatchdogTrip::NoProgress);
    }
    if !ceiling.is_zero() && total >= ceiling {
        return Some(WatchdogTrip::Ceiling);
    }
    None
}

/// BUG-1299: turn a resolved [`WatchdogTrip`] into the failure DETAIL (what
/// happened, prefixed with a machine-parseable `trip-name:` tag naming the
/// trip and the limit crossed) and the RECOVERY HINT (what to do about it) —
/// together, in one function.
///
/// This is the single place that reads `trip`. `NoProgress` and `Ceiling`
/// have opposite remedies (BUG-1299's whole point): a no-progress trip means
/// the session stalled — go find out why; a ceiling trip means the session
/// simply ran out of clock — it may have been working the entire time, so
/// the fix is a bigger budget or a smaller spec, never "inspect for a spin".
/// Because one `match` arm produces both strings, a ceiling detail can never
/// ship next to a no-progress hint (or vice versa) — there is no second,
/// independently-maintained switch on the same enum for the two to drift
/// apart in. PRIN-4 rule 1 ("a diagnostic must be unable to drift away from
/// the decision it explains") applied at the site that motivated it.
// trace:BUG-1299 | ai:claude
pub(crate) fn watchdog_trip_report(
    trip: &WatchdogTrip,
    no_progress_limit: std::time::Duration,
    ceiling_limit: std::time::Duration,
    pr_ship_wait_seen: bool,
    output_only: bool,
) -> (String, String) {
    match trip {
        WatchdogTrip::NoProgress => {
            let detail = if output_only {
                format!(
                    "no-progress: no session output for {}s",
                    no_progress_limit.as_secs()
                )
            } else {
                format!(
                    "no-progress: no commit, file-change, or session output for {}s",
                    no_progress_limit.as_secs()
                )
            };
            let hint = "The phase watchdog stopped a headless session that made no progress \
                 (no commit, file-change, or session output) within its window — likely a \
                 degenerate spin. Inspect the worktree and pick the spec back up by hand: \
                 the session was STUCK, so find out why before re-running it."
                .to_string();
            (detail, hint)
        }
        WatchdogTrip::Ceiling => {
            let detail = if pr_ship_wait_seen {
                format!(
                    "ceiling: {}s wall-clock exceeded while `aida pr ship` was waiting on CI",
                    ceiling_limit.as_secs()
                )
            } else {
                format!("ceiling: {}s wall-clock exceeded", ceiling_limit.as_secs())
            };
            let hint = "The phase watchdog stopped a headless session because it exceeded its \
                 wall-clock ceiling, NOT because it stalled — the session may have been \
                 working the entire time. Do not assume a spin: check the worktree's commit \
                 history for progress, then raise the phase budget for this class of spec (or \
                 split the spec into a smaller slice) before re-running it."
                .to_string();
            (detail, hint)
        }
        WatchdogTrip::Spinning { template, count } => {
            let detail =
                format!("spinning: low-information session output repeated `{template}` x{count}");
            let hint = format!(
                "The phase watchdog stopped a headless session whose output was a repeated \
                 low-information template (`{template}`, seen {count} times) rather than real \
                 progress — a spin, not a stall on silence and not a wall-clock overrun. \
                 Inspect the worktree and pick the spec back up by hand."
            );
            (detail, hint)
        }
    }
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

/// TASK-975: what phase 2 does with a red-CI failure — attempt one bounded
/// in-drain fix cycle, or let the failure fall through to the EPIC-28 shelve.
// trace:TASK-975 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RedCiAction {
    /// Spawn one headless fix session (`attempt` is 1-based), push its fix,
    /// and re-poll CI.
    AttemptFix { attempt: usize },
    /// The budget is exhausted, the feature is off (budget 0, the default),
    /// or the failure is not a red terminal CI run — proceed to shelve.
    Shelve,
}

/// TASK-975: pure retry-or-shelve decision for the CI auto-fix loop. Only a
/// genuinely *red* terminal CI run ([`FailureKind::CiRed`]) is fixable by
/// editing code — a CI timeout, a spawn error, or an internal fault is not
/// "failing checks", so those always shelve. `budget` comes from
/// `[drain] ci_auto_fix` (default 0 = off); `attempts_made` is how many fix
/// cycles already ran this phase, so the loop is bounded by construction.
/// Pure so the decision is pinned by unit tests.
// trace:TASK-975 | ai:claude
pub(crate) fn red_ci_action(budget: usize, attempts_made: usize, kind: FailureKind) -> RedCiAction {
    if kind != FailureKind::CiRed || attempts_made >= budget {
        return RedCiAction::Shelve;
    }
    RedCiAction::AttemptFix {
        attempt: attempts_made + 1,
    }
}

/// TASK-975: does a phase-4 merge failure read as a merge *conflict* (the
/// base moved under the PR so the squash cannot apply), as opposed to any
/// other merge failure (auth, network, branch protection)? Matched against
/// the `gh pr merge` error text carried in the failure reason. Conservative:
/// only a conflict is rebase-recoverable, so anything unrecognized is not.
// trace:TASK-975 | ai:claude
// trace:BUG-1447 | ai:codex
pub(crate) fn is_merge_conflict_failure(reason: &str) -> bool {
    // external-prose-classifier: auto_complete::is_merge_conflict_failure
    if is_branch_policy_merge_refusal(reason) {
        return false;
    }
    aida_core::external_tool_output::contains_any_case_insensitive(
        reason,
        aida_core::external_tool_output::GH_MERGE_CONFLICT,
    )
}

/// TASK-975: pure gate for the in-drain merge-conflict rebase — attempt it
/// only when the feature is opted in (`budget > 0`, the same `[drain]
/// ci_auto_fix` knob that arms the CI-fix loop), the failure reads as a
/// conflict, and no rebase was already attempted this phase (one rebase +
/// one merge retry, then shelve — bounded by construction).
// trace:TASK-975 | ai:claude
pub(crate) fn should_attempt_conflict_rebase(budget: usize, attempted: bool, reason: &str) -> bool {
    budget > 0 && !attempted && is_merge_conflict_failure(reason)
}

/// TASK-1230: the recovery-path worktree-teardown decision.
///
/// Verdict for tearing down a FAILED phase-1 attempt's worktree in the open-PR
/// recovery path. This deliberately does NOT reuse the merged-based gc/reap gate
/// (`classify_agent_worktree`): that gate needs a positive *merged* signal
/// (ancestor-of-main or a merged PR), but this recovery fires precisely while
/// the PR is still OPEN by design — so the merged gate would return Keep every
/// time and the teardown would be a silent no-op. The safety question here is
/// "is any work at risk of LOSS?", not "has it merged?". The failed attempt's
/// commits are safely captured on the pushed PR branch, so the tree is
/// disposable once nothing local is at risk: keep it on a dirty tree
/// (uncommitted work), a locked worktree (operator-protected), or when it holds
/// local commits not yet on the pushed PR branch (unpushed work); otherwise
/// remove.
// trace:TASK-1230 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecoveryTeardown {
    /// Nothing local is at risk — the worktree may be removed.
    Remove,
    /// Work is at risk (reason names the strongest objection) — keep it.
    Keep(String),
}

/// Pure decision for [`RecoveryTeardown`] — no git/forge/process probes, so the
/// whole matrix is unit-testable without a repo. The order names the strongest
/// objection first: uncommitted work (costliest to lose), then operator lock,
/// then unpushed local commits.
// trace:TASK-1230 | ai:claude
pub(crate) fn recovery_worktree_teardown_decision(
    dirty: bool,
    locked: bool,
    local_commits_not_on_pr_branch: u32,
) -> RecoveryTeardown {
    if dirty {
        return RecoveryTeardown::Keep(
            "uncommitted changes present — never auto-removed".to_string(),
        );
    }
    if locked {
        return RecoveryTeardown::Keep("worktree is locked — operator-protected".to_string());
    }
    if local_commits_not_on_pr_branch > 0 {
        return RecoveryTeardown::Keep(format!(
            "{local_commits_not_on_pr_branch} local commit(s) not on the pushed PR branch — keep"
        ));
    }
    RecoveryTeardown::Remove
}

/// STORY-975: closed transient cause set eligible for a bounded whole-phase
/// retry before a spec is parked `NeedsAttention`.
// trace:STORY-975 | ai:codex
pub(crate) fn is_transient_retry_cause(cause: &str) -> bool {
    matches!(
        cause,
        "watchdog" | "no-verdict" | "no-pr" | "tool-exit" | "cache-locked" | "headless-wait"
    )
}

/// TASK-1350: the shell tools — Claude Code's `Bash` and its Windows
/// counterpart `PowerShell` — which can write just like the file-editing
/// tools. Together with [`crate::contained_boundary::WRITE_TOOLS`] (derived
/// from it so the two lists cannot drift apart) they form the write-capable
/// built-in set: only a denial of one of these (or of a tool treated as
/// write-capable by [`is_write_capable_tool`]) reclassifies a stop as
/// [`FailureKind::PermissionDenied`], which feeds the environment-blocked
/// batch halt. A denial of a read/network tool (Read, Grep, WebFetch,
/// WebSearch, ...) on a stop that failed for spec reasons must not halt the
/// batch or blame the environment.
// trace:TASK-1350 | ai:claude
pub(crate) const SHELL_TOOLS: [&str; 2] = ["Bash", "PowerShell"];

/// TASK-1350: can a denial of `tool` mean the agent's writes were refused?
///
/// The built-in write set (`contained_boundary::WRITE_TOOLS` plus
/// [`SHELL_TOOLS`]) is an ALLOWLIST: a new mutating built-in tool Claude Code
/// adds defaults to read-only here until it is added to one of those lists.
///
/// Deliberate handling of tools outside that set:
/// - MCP tools (`mcp__<server>__<tool>`) are unknown to AIDA — any of them
///   may write (a database, a ticket, a file) — so they are treated
///   conservatively as write-capable.
/// - A denial with no tool name (`"an unknown tool"`, as
///   [`crate::questions::headless_log_permission_denied_tools`] records it,
///   or an empty/blank `tool_name`) is likewise treated as write-capable:
///   nothing proves it was a read.
/// - Every other named built-in tool (Read, Glob, Grep, WebFetch, WebSearch,
///   Task, ...) is read-only for this purpose, per the advisor ruling that
///   only the explicit write set reclassifies the stop.
// trace:TASK-1350 | ai:claude
pub(crate) fn is_write_capable_tool(tool: &str) -> bool {
    crate::contained_boundary::WRITE_TOOLS.contains(&tool)
        || SHELL_TOOLS.contains(&tool)
        || tool.starts_with("mcp__")
        || tool.trim().is_empty()
        || tool == "an unknown tool"
}

/// TASK-1350: should these denied tools reclassify a failed stop as
/// [`FailureKind::PermissionDenied`]? True only when at least one denied
/// tool is write-capable; an empty or read-only list keeps the original kind.
// trace:TASK-1350 | ai:claude
pub(crate) fn denials_reclassify_as_permission_denied(tools: &[String]) -> bool {
    tools.iter().any(|t| is_write_capable_tool(t))
}

/// BUG-1930: the failure for a phase that stopped on permission denials
/// (`tools`, as [`crate::questions::headless_log_permission_denied_tools`]
/// lists them) without producing a commit or PR.
// trace:BUG-1930 | ai:claude
pub(crate) fn permission_denied_failure(tools: &[String]) -> PhaseFailure {
    PhaseFailure::of(
        FailureKind::PermissionDenied,
        format!(
            "the agent stopped after permission denials ({}) and produced no commit or PR — \
             its writes were refused, so this is an environment problem, not the spec",
            tools.join(", ")
        ),
    )
}

/// STORY-975: clamp `[drain] retry_transient` to the supported retry budget.
/// The value is retries, not total attempts: default `1` means attempt `2/2`.
// trace:STORY-975 | ai:codex
pub(crate) fn clamp_transient_retry_budget(n: usize) -> usize {
    n.min(3)
}

/// STORY-975: should this phase failure spend another transient retry?
// trace:STORY-975 | ai:codex
pub(crate) fn should_retry_transient_failure(
    kind: FailureKind,
    retries_used: usize,
    retry_budget: usize,
) -> bool {
    retries_used < retry_budget && is_transient_retry_cause(kind.cause_slug())
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
    /// External evidence; fresh review resumes without implementer retries.
    // trace:BUG-2500 | ai:codex
    Waiting,
    /// The reviewer escalated a merge decision (phase 3).
    MergeDecision,
    /// The headless advisor escalated a punted design-fork (phase 1 tier).
    DesignFork,
    /// The spec's `execution_mode` holds the merge for a human/advisor — the
    /// substrate marker refused the phase-4 auto-merge (only `drain` licenses
    /// an unattended merge; no mode at all is supervised, fail safe).
    // trace:BUG-727 | ai:claude
    SupervisedMerge,
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

/// TASK-1054: canonical drive PROCESS exit codes — what a script wrapping
/// `aida queue work … --auto-complete` (single-spec or batch) reads to branch
/// on the outcome. Before TASK-1054 a hard failure and a shelve both surfaced
/// as `2` (a single-spec failure exited the failed-phase index — `2` for a CI
/// failure — colliding with EPIC-28's `2 = shelved` sentinel), so a wrapper
/// could not tell "the drive parked a spec and moved on" from "the drive hit a
/// wall". The codes below split them:
///
/// | code | meaning                                                            |
/// |------|--------------------------------------------------------------------|
/// | 0    | clean — shipped / punted / escalated / held / inconclusive / no-op |
/// | 1    | nothing to do — empty batch, or a non-advancing-queue stall        |
/// | 2    | shelved / parked-and-advanced (EPIC-28) — recoverable, re-drivable  |
/// | 3    | hard unrecoverable failure — un-shelved phase fail, build/env/internal |
/// | 7    | a `--max-tokens` / `--max-iterations` / `--max-runtime` cap stop    |
///
/// `2` (shelved) is the load-bearing EPIC-28 contract — scripts + `/goal`
/// conditions depend on it — so it is preserved exactly; `3` is the new
/// distinct hard-failure code.
// trace:TASK-1054 | ai:claude
pub(crate) const DRIVE_EXIT_CLEAN: i32 = 0;
/// TASK-1054: a spec was parked `NeedsAttention` and the drive treated it as
/// recoverable (a single-spec shelved phase failure, or a batch that fully
/// drained with ≥1 member shelved/skipped).
// trace:TASK-1054 | ai:claude
pub(crate) const DRIVE_EXIT_SHELVED: i32 = 2;
/// TASK-1054: an unrecoverable hard failure — an un-shelvable phase failure
/// (`Spawn` / `MissingTool` / `Internal`), a batch that hard-stopped on a
/// failure (un-shelvable, or over the `--max-failures` budget), or a build /
/// environment break. Distinct from `2` so a wrapper branches correctly.
// trace:TASK-1054 | ai:claude
pub(crate) const DRIVE_EXIT_HARD_FAIL: i32 = 3;

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
    /// false `shipped` line. `None` when dispatched == shipped (the common path)
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
    /// Host fast-gate repair is queued: do not retry or claim a ship.
    // trace:STORY-2640 | ai:codex
    pub(crate) fast_gate_pending: bool,
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
    /// A successful native seat produced no publication and has no open PR.
    /// Stops a drain without shipping credit or shelving. trace:TASK-3608 | ai:codex
    pub(crate) no_change: bool,
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
            fast_gate_pending: false,
            inconclusive_reason: None,
            shelved_reason: None,
            held_reason: None,
            no_change: false,
        }
    }

    /// A clean success result — exit `0`, no failed phase, every non-failure
    /// field cleared. Used by the single-branch driver's per-member and cluster
    /// runs (and the unit-test mock) to signal "this stage landed cleanly".
    // trace:TASK-1003 | ai:claude
    pub(crate) fn ok() -> Self {
        Self {
            exit_code: 0,
            failed_phase: None,
            failure: None,
            phase_durations: Vec::new(),
            total_ms: 0,
            punt_reason: None,
            shipped_spec_id: None,
            escalation: None,
            fast_gate_pending: false,
            inconclusive_reason: None,
            shelved_reason: None,
            held_reason: None,
            no_change: false,
        }
    }

    /// TASK-1054: the canonical PROCESS exit code for a single-spec drive.
    ///
    /// The `exit_code` field stays the 1-based failed-phase index (consumed by
    /// the JSON phase events, the telemetry log, and `drain_batch`'s internal
    /// non-zero check), but the *process* a wrapping script observes must
    /// collapse to the stable [`DRIVE_EXIT_CLEAN`] / [`DRIVE_EXIT_SHELVED`] /
    /// [`DRIVE_EXIT_HARD_FAIL`] table so it can branch on the outcome:
    ///
    /// - no `failed_phase` → `0` (shipped / punted / escalated / held /
    ///   inconclusive / a BUG-657 terminal-status no-op);
    /// - a failure that *shelved* the spec (`shelved_reason` set — a
    ///   recoverable phase failure parked `NeedsAttention`) → `2`, the
    ///   preserved EPIC-28 sentinel;
    /// - a failure that did NOT shelve (`Spawn` / `MissingTool` / `Internal`,
    ///   or a shelve that failed) → `3`, the new hard-failure code.
    // trace:TASK-1054 | ai:claude
    pub(crate) fn process_exit_code(&self) -> i32 {
        if self.failed_phase.is_none() {
            DRIVE_EXIT_CLEAN
        } else if self.shelved_reason.is_some() {
            DRIVE_EXIT_SHELVED
        } else {
            DRIVE_EXIT_HARD_FAIL
        }
    }
}

/// The six phases, abstracted so the orchestrator's sequencing can be tested
/// against a mock. The real implementation spawns Claude sessions, polls CI,
/// and shells out to `gh` / `cargo`.
/// trace:STORY-246 | ai:claude
pub(crate) trait PhaseDriver {
    /// BUG-1213: snapshot any existing rework PR head before phase 1. Drivers
    /// without forge/store access remain no-ops.
    fn begin_rework_guard(&mut self) {}
    /// After a successful implementer outcome, fail when a rework round left
    /// the snapshotted PR head unchanged. A punt never reaches this hook.
    fn rework_no_op_failure(&mut self) -> Option<PhaseFailure> {
        None
    }
    /// Phase 1 — run the implementer Claude session. Returns
    /// [`ImplementerOutcome::PrOpened`] once a PR is verified, or
    /// [`ImplementerOutcome::Punted`] when a headless implementer hit a
    /// design-fork and punted the spec to `NeedsAttention` (STORY-276).
    fn run_implementer(&mut self) -> Result<ImplementerOutcome, PhaseFailure>;

    /// A host fast-gate repair must finish before any PR recovery or review.
    // trace:STORY-2640 | ai:codex
    fn fast_gate_blocks_review(&mut self) -> Result<bool, String> {
        Ok(false)
    }

    /// BUG-1244: freeze the PR proved by phase 1 for this orchestration run.
    /// Later phases may probe forge state, but may not silently switch to a PR
    /// discovered from an inherited/sibling branch.
    // trace:BUG-1244 | ai:codex
    fn capture_phase_done_pr(&mut self) {}
    fn phase_done_pr_number(&self) -> Option<u32> {
        self.review_pr_number()
    }
    fn requires_phase_done_pr(&self) -> bool {
        false
    }

    /// BUG-1244: phase 1 must be operating in the target spec's own worktree.
    /// Real drivers validate the lease's live branch; mocks default to valid.
    /// BUG-1628: `Err` carries a resolver failure (the shared pickup resolver
    /// could not derive a workspace), reported before any lease is taken.
    // trace:BUG-1244 | ai:codex
    // trace:BUG-1628 | ai:claude
    fn implementer_workspace(&self) -> Result<Option<(String, String)>, String> {
        Ok(None)
    }
    /// BUG-1628: every requirement-ID prefix the project recognises (built-in,
    /// custom type, allowed, and in-use prefixes). The phase-1 ownership check
    /// uses it so a project-defined prefix such as `SPEC` counts as a real
    /// requirement ID. The target spec's own prefix is always accepted.
    // trace:BUG-1628 | ai:claude
    fn known_spec_prefixes(&self) -> Vec<String> {
        Vec::new()
    }
    /// BUG-1629: phase 1 hands the workspace it resolved and checked back to
    /// the driver, which launches exactly that workspace. The driver never
    /// re-resolves between the check and the launch, so a resolver failure
    /// or a changed answer in that window cannot launch an unpinned
    /// implementer (BUG-1244).
    // trace:BUG-1629 | ai:claude
    fn pin_implementer_workspace(&mut self, _worktree: &str, _branch: &str) {}
    /// Phase 2 — wait for CI to go terminal, then end the implementer session
    /// (which auto-queues the `Review PR-N` item for the reviewer).
    fn finish_ci(&mut self) -> Result<(), PhaseFailure>;
    /// Confirm the reviewer queue before reporting the through-CI handoff,
    /// including a resume or lifecycle tag that bypassed session teardown.
    // trace:BUG-1807 | ai:codex
    fn confirm_review_handoff(&mut self) -> Result<(), PhaseFailure> {
        Ok(())
    }

    /// Prove phase 2's terminal result belongs to the head phase 3 would
    /// review. This boundary hook runs before PhaseEntered(reviewer).
    // trace:BUG-1460 | ai:codex
    fn verify_ci_for_review(&mut self) -> Result<(), PhaseFailure> {
        Ok(())
    }
    /// Phase 3 — run the reviewer Claude session and read its verdict file.
    /// Returns [`ReviewerOutcome::Verdict`] on a normal review, or
    /// [`ReviewerOutcome::EscalatedToHuman`] when the reviewer wrote
    /// `merge: escalated-to-human` rather than auto-deciding the merge
    /// (STORY-306).
    fn run_reviewer(&mut self) -> Result<ReviewerOutcome, PhaseFailure>;
    /// Optional selector-scoped agent gates that run after the built-in
    /// reviewer and before merge. Default no-op keeps test drivers and
    /// alternate drivers unchanged.
    // trace:STORY-1155 | ai:codex
    fn run_agent_gates(&mut self) -> Result<(), PhaseFailure> {
        Ok(())
    }
    // TASK-1249 / ADR-45: the ADVISORY harvest step, run after the agent
    /// gates and before merge. Never a phase, never a failure: it proposes
    /// (ledger comment + candidates file + a brief for the advisor) and the
    /// drain continues whatever happens. Default: no-op.
    // trace:TASK-1249 | ai:claude
    fn run_harvest_gate(&mut self) {}
    /// Phase 4 — merge the PR.
    fn merge(&mut self) -> Result<(), PhaseFailure>;
    /// The supervised-merge hold — `Some(reason)` when the spec's
    /// `execution_mode` refuses an unattended phase-4 merge (only `drain`
    /// licenses one; no mode at all is supervised, fail safe). The
    /// orchestrator consults this right before phase 4 and, on a hold, stops
    /// cleanly via [`finish_escalated`] — exit `0`, no merge, the PR left
    /// mergeable for the human/advisor. Default `None` (no hold) keeps
    /// drivers without store access unchanged.
    // trace:BUG-727 | ai:claude
    fn merge_supervision_hold(&mut self) -> Option<String> {
        None
    }
    /// Persist the supervised-merge hold before the orchestrator exits cleanly.
    /// Default no-op keeps pure test drivers and non-store drivers unchanged.
    // trace:BUG-1168 | ai:codex
    fn record_merge_supervision_hold(&mut self, _reason: &str) {}
    /// Phase 5 — `aida pull` (auto-bumps Done → Completed).
    fn pull(&mut self) -> Result<(), PhaseFailure>;
    /// Phase 6 — `cargo build --release`.
    fn build(&mut self) -> Result<(), PhaseFailure>;
    /// Snapshot of what the driver has discovered, for the recovery hint.
    fn hint_context(&self) -> HintContext;
    /// BUG-879: PR number the reviewer/merge phases are allowed to drive.
    /// `None` or zero means there is no reviewable PR, so phase 3 must fail
    /// before a reviewer session can be spawned.
    // trace:BUG-879 | ai:codex
    fn review_pr_number(&self) -> Option<u32> {
        self.hint_context().pr_number.filter(|n| *n > 0)
    }
    /// BUG-895: phase 3 gets one last chance to recover a missing review PR
    /// after phase 2 has ended the implementer session. The default is
    /// fail-closed; only a driver that can prove a pushed branch has
    /// reviewable commits should return a positive PR number.
    // trace:BUG-895 | ai:codex
    fn recover_missing_review_pr(&mut self) -> Option<u32> {
        None
    }
    /// BUG-1145: phase 1 can fail locally after the implementer already
    /// pushed/opened a PR, especially when the orchestrator cannot match the
    /// child lease after a non-zero tool exit. A driver that can prove an open
    /// PR exists may seed its branch/PR state here and return the PR-only
    /// re-entry phase (Reviewer/Merge), so the pipeline proceeds instead of
    /// retrying phase 1 or shelving the spec.
    // trace:BUG-1145 | ai:codex
    fn recover_phase1_failure_with_open_pr(&mut self, _failure: &PhaseFailure) -> Option<Phase> {
        None
    }
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

    /// BUG-657: the target spec's current status label *if it is already
    /// terminal* (`Completed` or `Rejected`) — `None` when the spec is in a
    /// drivable state, when the driver cannot read the store, or when this is a
    /// `--resume`/`--from-pr` re-entry (those legitimately re-drive a spec that
    /// reaches `Completed` mid-pipeline). When `Some`, the orchestrator skips
    /// phase 1 entirely and finishes as a clean NO-OP — never spawning an
    /// implementer that would exit 1 ("nothing to implement") and auto-draft a
    /// phantom failure BUG. The default `None` keeps test mocks simple; only
    /// `RealPhaseDriver` reads the real status.
    // trace:BUG-657 | ai:claude
    fn terminal_status(&mut self) -> Option<&'static str> {
        None
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

    /// BUG-770: the project root whose `.aida/events.jsonl` this run's
    /// supervision events belong to — the ONE seam that decides where an
    /// orchestration event lands. `None` means "this driver has no event
    /// stream" and every emit on the orchestration path is skipped.
    ///
    /// The default is `None`, deliberately NOT `crate::find_project_root()`.
    /// Resolving the root from the process cwd *inside* the emit site is
    /// exactly what let the in-tree unit tests — which run with the repo as
    /// cwd — append synthetic `AdvisorEscalated` lines to the developer's REAL
    /// `.aida/events.jsonl` on every `cargo test`, for months. A fail-safe
    /// default means a driver that never opts in can only lose events (benign,
    /// best-effort by contract), never fabricate them in someone's live drain
    /// history. `RealPhaseDriver` — which already carries its project root —
    /// is the only implementor that overrides it.
    // trace:BUG-770 | ai:claude
    fn events_root(&self) -> Option<std::path::PathBuf> {
        None
    }

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

    /// BUG-1291: once shelving has durably parked the spec, transfer any open
    /// PR to the reviewer queue. This is deliberately a separate post-shelve
    /// hook: a failed/declined shelf must not manufacture a handoff, while a
    /// successful shelf must not be able to forget the PR it abandons.
    fn handoff_open_pr_after_shelve(
        &mut self,
        _spec: &str,
        _phase: Phase,
        _failure: &PhaseFailure,
    ) {
    }

    /// TASK-975: the CI auto-fix budget — how many in-drain fix cycles phase 2
    /// may attempt on a red CI run before the failure proceeds to the EPIC-28
    /// shelve. The default `0` keeps the feature OFF: red CI shelves
    /// immediately, exactly the pre-TASK-975 behaviour. `RealPhaseDriver`
    /// resolves it from `[drain] ci_auto_fix` in `.aida/config.toml` (env
    /// override `AIDA_CI_AUTO_FIX`). The same knob arms the phase-4
    /// merge-conflict rebase.
    // trace:TASK-975 | ai:claude
    fn ci_fix_budget(&self) -> usize {
        0
    }

    /// TASK-975: attempt ONE in-drain CI fix cycle — spawn a headless fix
    /// session with the failing-check log + spec context in the still-leased
    /// implementer worktree, which commits and pushes a fix to the PR branch.
    /// Returns `true` when a fix was pushed (the orchestrator loops back to
    /// [`Self::finish_ci`] to re-poll CI on the new head) and `false` when no
    /// fix could be attempted or nothing was pushed (the original red-CI
    /// failure proceeds to shelve). The default is `false` so a driver
    /// without the capability behaves exactly as before.
    // trace:TASK-975 | ai:claude
    fn attempt_ci_fix(&mut self, _attempt: usize, _budget: usize, _failure: &PhaseFailure) -> bool {
        false
    }

    /// TASK-975: attempt ONE in-drain rebase of the PR branch after a
    /// merge-conflict phase-4 failure (reusing the same `aida pr rebase <N>
    /// --no-smoke` subprocess as the STORY-429 phase-3 auto-rebase). Returns
    /// `true` when the rebase landed cleanly and was pushed — the
    /// orchestrator retries the merge exactly once — and `false` when the
    /// rebase itself conflicted or failed (the original failure proceeds to
    /// shelve). The default is `false` so a driver without the capability
    /// behaves exactly as before.
    // trace:TASK-975 | ai:claude
    fn attempt_merge_conflict_rebase(&mut self, _failure: &PhaseFailure) -> bool {
        false
    }

    /// STORY-975: whole-phase transient retry budget from `[drain]
    /// retry_transient`, clamped to 3. Default one retry.
    // trace:STORY-975 | ai:codex
    fn transient_retry_budget(&self) -> usize {
        1
    }

    /// STORY-975: record that a transient phase failure is being retried.
    /// Default no-op keeps pure/mock drivers file-free unless they opt in.
    /// `detail`, when present, is the [`PhaseFailure::reason`] that triggered
    /// the retry — carried through to `EventKind::SpecRetried.detail` so a
    /// retry no longer loses it at the event boundary (BUG-1299).
    // trace:STORY-975 | ai:codex
    // trace:BUG-1299 | ai:claude
    fn record_transient_retry(
        &mut self,
        _spec: &str,
        _phase: Phase,
        _cause: &str,
        _attempt: u32,
        _max: u32,
        _detail: Option<&str>,
    ) {
    }

    /// BUG-908: last hook before a transient retry re-runs the failed phase.
    /// The real phase-1 driver uses it to release the just-dead predecessor
    /// lease and aim the next child at the same worktree; mocks keep the
    /// historical no-op behavior.
    // trace:BUG-908 | ai:codex
    fn prepare_transient_retry(
        &mut self,
        _spec: &str,
        _phase: Phase,
        _cause: &str,
    ) -> Result<(), PhaseFailure> {
        Ok(())
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
                // No branch-cleanup flag here: the drain's spec worktree still
                // holds the PR branch, so `--delete-branch` would fail its
                // local-cleanup step. Worktree cleanup owns branch deletion.
                // trace:BUG-758 | ai:claude
                Phase::Merge => ctx
                    .forge
                    .change_cmd_hint("merge", &format!("{pr} --squash"))
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
        // trace:BUG-1250 | ai:codex
        FailureKind::CiUnavailable => {
            let check = match ctx.forge {
                crate::forge::ForgeKind::GitHub => "gh api /rate_limit",
                crate::forge::ForgeKind::GitLab => "glab api /version",
                crate::forge::ForgeKind::None => "check your CI provider",
            };
            return format!(
                "CI state could not be read after bounded retries, so the gate stayed closed. \
                 Check forge connectivity with `{check}`, then re-run: \
                 `aida queue work {spec} --auto-complete`."
            );
        }
        // BUG-420 / BUG-1299: the no-progress / ceiling / spinning watchdog
        // stopped a headless phase. The production path always sets
        // `PhaseFailure::hint_override` (from [`watchdog_trip_report`], which
        // knows WHICH trip fired) before this generic hint is ever consulted
        // — see `finish_failure` / `finish_inconclusive_shelved`. This branch
        // is only the fallback for a `PhaseFailure` built directly (e.g. a
        // test) without going through that path, so it deliberately does NOT
        // guess which trip fired. trace:BUG-420 | ai:claude trace:BUG-1299 | ai:claude
        FailureKind::Watchdog => {
            return format!(
                "The phase watchdog stopped a headless session — see the failure detail \
                 above for which limit it crossed. Inspect the worktree and pick the spec \
                 back up by hand: `aida queue rework {spec} --work`."
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
        // trace:BUG-1063 | ai:codex
        FailureKind::HeadlessWait => {
            return format!(
                "The headless phase ended while waiting for a future notification, but \
                 headless runs have no next turn. Re-run it and poll waits in <=60s \
                 bounded steps that print state, then write the phase result before exit: \
                 `aida queue rework {spec} --work`."
            );
        }
        FailureKind::StaleBaseRefused => {
            return format!(
                "The stale-base auto-rebase refused to overwrite remote commits that are not \
                 incorporated by patch-id. Follow the fetch + rebase recovery printed in the \
                 failure detail, then re-run the review: `aida queue work PR-{pr}`."
            );
        }
        FailureKind::MergeHold => {
            return format!(
                "A human/advisor merge-hold is open on PR-{pr}. Read the hold reason in the \
                 failure detail, then clear it after review with `aida merge-hold clear {pr}`."
            );
        }
        // trace:TASK-1448 | ai:claude
        FailureKind::StaleApproval => {
            return format!(
                "PR-{pr}'s recorded approval does not cover its current head. Re-review the \
                 current head (`aida queue work PR-{pr} --role reviewer`), then merge it."
            );
        }
        // BUG-1524: no implementer session ever launched — the failure
        // detail above names the specific refusal (e.g. a diverged PR
        // branch, with the exact realign command). Resolve that, then
        // retry from scratch; there is no session to resume.
        FailureKind::LaunchRefused => {
            return format!(
                "The implementer never launched — see the failure detail above for the \
                 preflight/lease refusal it hit. Resolve that, then retry: \
                 `aida queue work {spec}`."
            );
        }
        _ => {}
    }

    // Phase- and kind-specific hints. Each phase has a `_` arm carrying its
    // [`FailureKind::Failed`] default (the spawned work ran and reported
    // failure) — the pre-BUG-218 behaviour for that phase.
    match (phase, kind) {
        // BUG-1930: writes were refused, so re-running changes nothing until
        // the permission boundary does. trace:BUG-1930 | ai:claude
        (_, FailureKind::PermissionDenied) => format!(
            "The agent's writes were denied — check the sandbox / permission mode \
             (`[agents] contained` in ~/.aida/agents.toml, `aida doctor`), then re-drive: \
             `aida queue work {spec}`"
        ),
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
        // trace:BUG-1527 | ai:claude
        (Phase::Implementer, FailureKind::ShippedMismatch) => format!(
            "The implementer's worktree ended on a different branch than {spec} was dispatched \
             on — read the failure detail for which spec the new branch actually credits. That \
             PR is untouched and stands on its own; re-queue {spec} for a fresh attempt: \
             `aida queue rework {spec} --work`"
        ),
        // trace:BUG-1445 | ai:codex
        (Phase::Implementer, FailureKind::ReworkNoOp) => format!(
            "The rework round produced no patch-unique change (the failure detail says whether \
             the head was unchanged or rebase-only). Inspect the quoted open findings with \
             `aida why {spec}`; do not retry the same branch contribution. Either \
             push a new fixup commit or explicitly punt the disputed finding for advisor triage."
        ),
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
            // BUG-1226: GitLab's coarse CI probe does not currently provide a
            // pipeline id. Do not expose the internal `<ID>` placeholder when
            // the forge-specific branch viewer is the best available target.
            // trace:BUG-1226 | ai:codex
            let failure = ctx
                .ci_run_id
                .as_deref()
                .map(|run| format!("CI failed on run {run}"))
                .unwrap_or_else(|| "CI failed".to_string());
            format!(
                "{failure} — view it: `{view}`. Push fixups to the same branch: \
                 `aida queue work {spec} --branch {branch}`"
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
        (Phase::Ci, FailureKind::CiUnavailable) => format!(
            "CI check details stayed unavailable while PR-{pr} was under a supervised merge-hold. \
             The coarse red may be the hold gate, so no code failure was inferred. Check the forge, \
             then re-run auto-complete once per-check rows are readable."
        ),
        (Phase::Ci, _) => format!(
            "The implementer session would not end cleanly — it may have uncommitted \
             changes. Commit or discard them in the worktree, then end it: \
             `aida session end {session} --skip-ci`"
        ),

        (Phase::Reviewer, FailureKind::NoVerdict) => format!(
            "The reviewer session ended without a usable verdict — re-run the review: \
             `aida queue work PR-{pr}`"
        ),
        // BUG-1186: the reviewer seat modified the branch it was reviewing.
        // Never retry (that would review reviewer-authored commits under the
        // drain's own approval): a human inspects, then re-runs the review.
        // trace:BUG-1186 | ai:claude
        (Phase::Reviewer, FailureKind::ReviewerWrote) => format!(
            "The reviewer pushed to `{branch}` during the review — inspect the extra commits \
             (`git log origin/main..origin/{branch}`), drop or keep them deliberately, then \
             re-run the review: `aida queue work PR-{pr}`"
        ),
        // By phase 3 the spec is Done — the implementer opened the PR in
        // phase 1, so `aida queue work` (queued items only) would reject
        // it. `aida queue rework` is the verb that re-opens a Done spec.
        // trace:BUG-236 | ai:claude
        (Phase::Reviewer, _) => format!(
            "Address the review feedback, then push fixups: \
             `aida queue rework {spec} --work`"
        ),

        // TASK-1244 / ADR-41: contention on the per-branch merge-lease.
        (Phase::Merge, FailureKind::LeaseConflict) => format!(
            "Another merger holds the merge-lease — see who with `aida merge-lock`; once it is \
             released, finish the merge: `aida pr ship {pr}`"
        ),
        // TASK-1459: a live review-in-progress marker (STORY-1405), not a
        // merge-lease conflict — no other merger is racing this PR, a
        // review is actively running on it.
        (Phase::Merge, FailureKind::ReviewInProgress) => format!(
            "PR-{pr} is under review — see who and since when with `aida review verdict {spec}` \
             or `aida awaiting`. Wait for the verdict (`aida review record … --pr {pr}` clears \
             this marker), or, if that review was abandoned, release it with \
             `aida review claim --pr {pr} --release` and retry."
        ),
        // STORY-1690: the drain-to-train handoff. trace:STORY-1690 | ai:claude
        (Phase::Merge, FailureKind::BehindBase) => format!(
            "PR-{pr} is approved and green, but its head is behind main, so the drain will \
             not merge it. Land it through the merge train, which tests it on the current \
             main first: `aida pr train {pr}`."
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
            glyph(crate::glyphs::Glyph::FlowActive).cyan().bold(),
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
            glyph(crate::glyphs::Glyph::Check).green(),
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
            glyph(crate::glyphs::Glyph::Info).cyan(),
            dispatched.bold(),
            shipped.bold(),
            shipped,
            dispatched,
        );
    }
}

/// BUG-657: print the terminal-status NO-OP epilogue and build a clean
/// *non-failure* [`OrchestrationResult`] (exit `0`, no `failed_phase`). Reached
/// when the orchestrator is dispatched against a spec that is already
/// `Completed` or `Rejected`: there is nothing to drive, so it must NOT spawn an
/// implementer (which would exit 1 — "nothing to implement" — and auto-draft a
/// phantom failure BUG; this is exactly the BUG-638 → BUG-644..649 incident).
/// `status` is the terminal status label for the message. Also called by the
/// real `run_auto_complete` entry BEFORE it queues anything, so a terminal
/// target never even reaches the implementer queue.
// trace:BUG-657 | ai:claude
pub(crate) fn finish_noop(
    spec: &str,
    status: &str,
    json: bool,
    start: &Instant,
) -> OrchestrationResult {
    let elapsed = start.elapsed().as_millis();
    if json {
        println!(
            "{}",
            phase_event(
                "auto-complete",
                "noop",
                spec,
                elapsed,
                Some(0),
                &[("reason", &format!("already {status}"))],
            )
        );
    } else {
        eprintln!();
        eprintln!(
            "{} {} is already {} — nothing to drive.",
            glyph(crate::glyphs::Glyph::Info).cyan().bold(),
            spec.bold(),
            status,
        );
    }
    OrchestrationResult {
        exit_code: 0,
        failed_phase: None,
        failure: None,
        phase_durations: Vec::new(),
        total_ms: elapsed,
        punt_reason: None,
        shipped_spec_id: None,
        escalation: None,
        fast_gate_pending: false,
        inconclusive_reason: None,
        shelved_reason: None,
        held_reason: None,
        no_change: false,
    }
}

/// Print the success epilogue and build the success [`OrchestrationResult`].
///
/// `dispatched` is the spec the orchestrator was launched for; `credited` is
/// the id reported in the `shipped` line. They differ only in the BUG-245
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
            glyph(crate::glyphs::Glyph::Check).green().bold(),
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
        fast_gate_pending: false,
        inconclusive_reason: None,
        shelved_reason: None,
        held_reason: None,
        no_change: false,
    }
}

// trace:TASK-1155 trace:ADR-11 | ai:codex
fn render_through_ci_checkpoint(
    spec: &str,
    pr_number: Option<u32>,
    forge: crate::forge::ForgeKind,
) -> String {
    let noun = forge.change_noun();
    let pr = pr_number
        .map(|n| format!("{noun}-{n}"))
        .unwrap_or_else(|| format!("{noun}-N"));
    let review_cmd = if let Some(n) = pr_number {
        format!("aida queue work {noun}-{n} --role reviewer")
    } else {
        format!("aida queue work {noun}-N --role reviewer")
    };
    // The spec's worktree still holds the PR branch here, so a suggested
    // `--delete-branch` is guaranteed to fail its local-cleanup step — and an
    // `&&` chain then silently drops the `aida pull` auto-bump leg. Suggest a
    // `;` chain with no branch delete; worktree cleanup owns branch deletion.
    // trace:BUG-758 trace:TASK-1240 | ai:claude+codex
    let merge_cmd = if let Some(n) = pr_number {
        forge
            .change_cmd_hint("merge", &format!("{n} --squash"))
            .map(|cmd| format!("{cmd}; aida pull"))
            .unwrap_or_else(|| format!("merge the {noun}; aida pull"))
    } else {
        forge
            .change_cmd_hint("merge", "<N> --squash")
            .map(|cmd| format!("{cmd}; aida pull"))
            .unwrap_or_else(|| format!("merge the {noun}; aida pull"))
    };
    format!(
        "{spec} {noun} checkpoint\n\
         {noun}: {pr}\n\
         CI: green\n\
         Review: routed to reviewer queue\n\
         Next: {review_cmd}\n\
         After review: {merge_cmd}\n\
         (';' so the pull auto-bump runs regardless; the worktree still holds \
         the branch — worktree cleanup deletes it later)"
    )
}

/// The `through-ci` variant is a ready-PR checkpoint, not a full ship. Keep
/// the full-pipeline success epilogue unchanged while giving `aida do` and
/// `--auto-complete=through-ci` the next-step report the user needs.
// trace:TASK-1155 trace:ADR-11 | ai:codex
fn finish_through_ci_success(
    dispatched: &str,
    credited: &str,
    json: bool,
    start: &Instant,
    durations: Vec<(Phase, u128)>,
    ctx: &HintContext,
) -> OrchestrationResult {
    let elapsed = start.elapsed().as_millis();
    let shipped_spec_id = (credited != dispatched).then(|| credited.to_string());
    if json {
        let mut extra: Vec<(&str, String)> = Vec::new();
        if let Some(id) = shipped_spec_id.as_deref() {
            extra.push(("shipped", id.to_string()));
        }
        extra.push(("variant", "through-ci".to_string()));
        if let Some(pr) = ctx.pr_number {
            // trace:BUG-1807 | ai:codex
            extra.push(("pr", format!("{}-{pr}", ctx.forge.change_noun())));
        }
        let borrowed: Vec<(&str, &str)> = extra.iter().map(|(k, v)| (*k, v.as_str())).collect();
        println!(
            "{}",
            phase_event(
                "auto-complete",
                "success",
                dispatched,
                elapsed,
                Some(0),
                &borrowed
            )
        );
    } else {
        eprintln!();
        eprintln!(
            "{} {} ready for review ({})",
            glyph(crate::glyphs::Glyph::Check).green().bold(),
            credited.bold(),
            fmt_duration(elapsed)
        );
        eprintln!(
            "{}",
            render_through_ci_checkpoint(credited, ctx.pr_number, ctx.forge)
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
        fast_gate_pending: false,
        inconclusive_reason: None,
        shelved_reason: None,
        held_reason: None,
        no_change: false,
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
        fast_gate_pending: false,
        inconclusive_reason: None,
        shelved_reason: None,
        held_reason: None,
        no_change: false,
    }
}

// trace:BUG-2500 | ai:codex
fn escalation_recovery_hint(kind: EscalationKind) -> &'static str {
    match kind {
        EscalationKind::Waiting => {
            "obtain the named external evidence, then record a fresh review at the current head"
        }
        _ => "left for a human — triage it with `aida findings list`",
    }
}

// trace:BUG-2500 | ai:codex
fn escalation_closing_line(kind: EscalationKind) -> &'static str {
    match kind {
        EscalationKind::Waiting => {
            "waiting — obtain the named external evidence, then record a fresh review at the current head; drain continues"
        }
        _ => "escalated — drain continues, a human decides",
    }
}

#[test]
fn bug_2500_waiting_recovery_requires_fresh_review() {
    let hint = escalation_recovery_hint(EscalationKind::Waiting);
    assert!(
        hint.contains("external evidence") && hint.contains("fresh review at the current head")
    );
    assert!(!hint.contains("findings"));
    let closing = escalation_closing_line(EscalationKind::Waiting);
    assert!(closing.contains("waiting") && closing.contains("external evidence"));
    assert!(closing.contains("fresh review at the current head"));
    assert!(!closing.contains("a human decides"));
    assert!(escalation_closing_line(EscalationKind::MergeDecision).contains("a human decides"));
    assert!(escalation_recovery_hint(EscalationKind::DesignFork).contains("findings list"));
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
    events_root: Option<&Path>,
) -> OrchestrationResult {
    let elapsed = start.elapsed().as_millis();
    // STORY-712: reaching the human tier is an actionable wake. This is the one
    // convergence point for both escalation kinds (the advisor's design-fork
    // and the reviewer's merge decision), so emit AdvisorEscalated here.
    //
    // BUG-770: the root is INJECTED by the caller ([`PhaseDriver::events_root`])
    // rather than resolved here via `crate::find_project_root()`. The old
    // resolve-from-cwd form made this function write to whatever project the
    // process happened to be standing in — so the two in-tree unit tests that
    // drive this path appended a synthetic `STORY-306` AdvisorEscalated line to
    // the developer's real `.aida/events.jsonl` on every `cargo test` run.
    // `None` (every test driver, and any driver without a project) skips the
    // emit; best-effort either way. trace:BUG-770 trace:TASK-988 | ai:claude
    if let Some(root) = events_root {
        let (_, run_uuid) = crate::drain_state::current_context(root);
        crate::events::emit(
            root,
            &crate::events::Event::new(
                Some(spec.to_string()),
                run_uuid,
                crate::events::EventKind::AdvisorEscalated {
                    reason: reason.to_string(),
                },
            ),
        );
    }
    // The escalation surfaced in a specific phase — phase 3 for a merge
    // decision, phase 1 (the punt) for a design-fork.
    let phase = match kind {
        EscalationKind::Waiting | EscalationKind::MergeDecision => Phase::Reviewer,
        EscalationKind::DesignFork => Phase::Implementer,
        // trace:BUG-727 | ai:claude
        EscalationKind::SupervisedMerge => Phase::Merge,
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
            EscalationKind::Waiting => {
                "review is WAITING; resume with a fresh review at the current head"
            }
            EscalationKind::MergeDecision => "the reviewer escalated the merge decision to a human",
            EscalationKind::DesignFork => "the advisor escalated a design-fork to a human",
            // trace:BUG-727 | ai:claude
            EscalationKind::SupervisedMerge => {
                "the spec's execution mode holds the merge for a human/advisor"
            }
        };
        eprintln!();
        eprintln!("{} {}: {}", "⏸".yellow().bold(), what, reason);
        eprintln!(
            "  {} {}",
            "→".dimmed(),
            escalation_recovery_hint(kind).cyan()
        );
        eprintln!();
        eprintln!(
            "{} {} ({}) {}",
            "⏸".yellow().bold(),
            spec.bold(),
            fmt_duration(elapsed),
            escalation_closing_line(kind)
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
        fast_gate_pending: false,
        inconclusive_reason: None,
        shelved_reason: None,
        held_reason: None,
        no_change: false,
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
    forge: crate::forge::ForgeKind,
) -> OrchestrationResult {
    let elapsed = start.elapsed().as_millis();
    // BUG-266: per-leg recovery hint. `None` preserves BUG-257's GH-API
    // wording (the only inconclusive case at the time it shipped); BUG-266's
    // Anthropic-API path passes a `aida queue work <spec> --resume <session>`
    // hint that recovers the exact session the API outage interrupted.
    let default_hint = match forge {
        crate::forge::ForgeKind::GitHub => "transient — retry once the GH API is reachable: \
                        `gh api /rate_limit` then re-run `aida queue work --auto-complete`"
            .to_string(),
        crate::forge::ForgeKind::GitLab => "transient — retry once the GitLab API is reachable: \
                        `glab api user` then re-run `aida queue work --auto-complete`"
            .to_string(),
        crate::forge::ForgeKind::None => {
            "transient — retry once the forge or network is reachable: \
                        re-run `aida queue work --auto-complete`"
                .to_string()
        }
    };
    let hint_line = retry_hint.unwrap_or(default_hint.as_str());
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
        fast_gate_pending: false,
        inconclusive_reason: Some(reason.to_string()),
        shelved_reason: None,
        held_reason: None,
        no_change: false,
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
    // BUG-1299: same override precedence as `finish_failure` — see there.
    let hint = failure
        .hint_override
        .clone()
        .unwrap_or_else(|| recovery_hint(phase, failure.kind, &driver.hint_context()));
    // Best-effort shelve — a shelve-side error leaves `shelved_reason` None, so
    // `drain_batch` falls back to treating the inconclusive as a hard pause
    // rather than silently swallowing the spec.
    let shelved_reason: Option<aida_core::FailureReason> =
        match driver.shelve_on_failure(spec, phase, &failure, &hint) {
            Ok(fr) => {
                if fr.is_some() {
                    driver.handoff_open_pr_after_shelve(spec, phase, &failure);
                }
                fr
            }
            Err(e) => {
                eprintln!(
                    "  {} could not shelve {} into Needs Attention: {} \
                     — the batch drain will treat it as a pause",
                    glyph(crate::glyphs::Glyph::Info).cyan(),
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
                &[("kind", failure.kind.cause_slug())],
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
            glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
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
        fast_gate_pending: false,
        inconclusive_reason: None,
        shelved_reason,
        held_reason: None,
        no_change: false,
    }
}

// trace:TASK-3608 | ai:codex
fn finish_no_change(
    spec: &str,
    json: bool,
    start: &Instant,
    durations: Vec<(Phase, u128)>,
) -> OrchestrationResult {
    if json {
        for phase in [Phase::Implementer.slug(), "auto-complete"] {
            println!(
                "{}",
                phase_event(
                    phase,
                    "no-change",
                    spec,
                    start.elapsed().as_millis(),
                    Some(0),
                    &[]
                )
            );
        }
    } else {
        eprintln!("{spec}: no-change — no work published");
    }
    let mut result = OrchestrationResult::ok();
    result.no_change = true;
    result.total_ms = start.elapsed().as_millis();
    result.phase_durations = durations;
    result
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
    forge: crate::forge::ForgeKind,
) -> OrchestrationResult {
    let elapsed = start.elapsed().as_millis();
    let summary = match reason {
        Some(r) => format!("PR held on `{branch}` — {r}"),
        None => format!("PR held on `{branch}`"),
    };
    // The recovery hint matches the deliberate-hold state: open the PR when the
    // gate is satisfied, then hand it to the reviewer. NOT "run /aida-pr".
    let noun = forge.change_noun();
    let create = forge
        .create_cmd()
        .unwrap_or_else(|| "open a change request".to_string());
    let hint_line = format!(
        "ready for review when you are — run your gate, then `{create}` and `aida queue work {noun}-N --role reviewer`"
    );
    if json {
        println!(
            "{}",
            phase_event(
                Phase::Implementer.slug(),
                "held",
                spec,
                elapsed,
                Some(0),
                &[("reason", summary.as_str()), ("hint", hint_line.as_str())],
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
        fast_gate_pending: false,
        inconclusive_reason: None,
        shelved_reason: None,
        held_reason: Some(summary),
        no_change: false,
    }
}

/// STORY-731: build the human-readable TAIL of a single-spec drive failure.
///
/// EPIC-28 parity. When the failure was *shelved* (`shelved` mirrors
/// `OrchestrationResult::shelved_reason.is_some()` — a recoverable phase
/// failure: CI red / RequestChanges / build fail → the spec parked in Needs
/// Attention) the tail names it parked-NOT-crashed and points at the recovery
/// commands (`aida why <spec>` for what failed, `aida findings list` for all
/// parked work), exactly the legibility the batch drain already gives its
/// shelved members. A non-shelve (a genuine hard crash — Spawn / MissingTool /
/// Internal) keeps the terminal "auto-complete failed at phase N" line so it
/// still reads as a crash. The single-spec drive is the MOST COMMON first
/// failure, so this is where the weakest recovery signal hurt most.
///
/// Returned as one `String` per line so a unit test can assert the wording
/// without an external process.
// trace:STORY-731 | ai:claude
fn single_spec_failure_tail(
    spec: &str,
    phase: Phase,
    elapsed_ms: u128,
    shelved: bool,
) -> Vec<String> {
    if shelved {
        vec![
            format!(
                "{} {} parked for triage — not a crash. Fix the blocker, then re-drive.",
                "⏸".yellow().bold(),
                spec.bold(),
            ),
            format!(
                "  {} `aida why {}` (what failed) · `aida findings list` (all parked work)",
                "→".dimmed(),
                spec,
            ),
        ]
    } else {
        vec![format!(
            "{} {} — auto-complete failed at phase {} ({})",
            glyph(crate::glyphs::Glyph::Cross).red().bold(),
            spec.bold(),
            phase.index(),
            fmt_duration(elapsed_ms),
        )]
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
    // BUG-1299: a watchdog failure carries its hint pre-resolved (by
    // `watchdog_trip_report`, in the same match arm as `reason`) rather than
    // re-derived here from `kind` alone — see `PhaseFailure::hint_override`.
    // trace:BUG-1299 | ai:claude
    let hint = failure
        .hint_override
        .clone()
        .unwrap_or_else(|| recovery_hint(phase, failure.kind, &driver.hint_context()));

    // EPIC-28: shelve the spec into `NeedsAttention` so a batch drain can
    // continue past the failure. Best-effort — a shelve-side error is
    // logged but never replaces the original phase failure (we want the
    // real diagnostic on stderr, not a meta-failure about the shelving).
    // trace:EPIC-28 | ai:claude
    let shelved_reason: Option<aida_core::FailureReason> = if failure.kind.is_shelvable() {
        match driver.shelve_on_failure(spec, phase, failure, &hint) {
            Ok(fr) => {
                if fr.is_some() {
                    driver.handoff_open_pr_after_shelve(spec, phase, failure);
                }
                fr
            }
            Err(e) => {
                eprintln!(
                    "  {} could not shelve {} into Needs Attention: {} \
                     — the batch drain will treat the failure as un-shelvable",
                    glyph(crate::glyphs::Glyph::Info).cyan(),
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
                    ("kind", failure.kind.cause_slug()),
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
            glyph(crate::glyphs::Glyph::Cross).red().bold(),
            phase.index(),
            phase.label(),
            failure.reason
        );
        eprintln!("  {} {}", "→".dimmed(), hint.cyan());
        eprintln!();
        // STORY-731: when the failure was shelved, the tail names it
        // parked-not-crashed and points at recovery commands (parity with the
        // batch drain); otherwise it stays a terminal crash line.
        for line in single_spec_failure_tail(spec, phase, elapsed, shelved_reason.is_some()) {
            eprintln!("{line}");
        }
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
        fast_gate_pending: false,
        inconclusive_reason: None,
        shelved_reason,
        held_reason: None,
        no_change: false,
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
            glyph(crate::glyphs::Glyph::Info).cyan(),
            phase.index(),
            phase.label(),
            reason,
        );
        eprintln!(
            "{} {} shipped ({}, reconciled)",
            glyph(crate::glyphs::Glyph::Check).green().bold(),
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
        fast_gate_pending: false,
        inconclusive_reason: None,
        shelved_reason: None,
        held_reason: None,
        no_change: false,
    }
}

/// BUG-1628: the phase-1 refusal when the workspace auto-complete resolved
/// does not belong to the target spec. It names the resolver mismatch — the
/// workspace disagrees with what `aida queue work <spec>` would pick up — so
/// the operator is not left reading a generic sibling-workspace refusal.
// trace:BUG-1628 | ai:claude
pub(crate) fn phase1_resolver_mismatch_reason(spec: &str, worktree: &str, branch: &str) -> String {
    format!(
        "phase 1 workspace resolver mismatch for {spec}: branch `{branch}` (worktree \
         `{worktree}`) does not name {spec}, so it is not the workspace the pickup \
         resolver (`aida queue work {spec}`) would use; refused before taking a lease"
    )
}

/// BUG-1628: the phase-1 refusal when the shared pickup resolver could not
/// derive a workspace at all.
// trace:BUG-1628 | ai:claude
pub(crate) fn phase1_resolver_failure_reason(spec: &str, err: &str) -> String {
    format!(
        "phase 1 workspace resolver failed for {spec}: {err} (auto-complete uses the \
         same branch/worktree resolver as `aida queue work {spec}`); refused before \
         taking a lease"
    )
}

// Preserve the queued repair and its status; ordinary failure shelving would
// park the very work this gate requires. No CI, reviewer, or merge may follow.
// trace:STORY-2640 | ai:codex
fn stop_for_fast_gate(
    driver: &mut dyn PhaseDriver,
    spec: &str,
    json: bool,
    start: &Instant,
    durations: &[(Phase, u128)],
) -> Option<OrchestrationResult> {
    let (reason, pending) = match driver.fast_gate_blocks_review() {
        Ok(false) => return None,
        Ok(true) => (
            "fast gate repair is pending; leaving its queued round for pickup".to_string(),
            true,
        ),
        Err(error) => (
            format!("cannot establish fast gate repair state: {error}"),
            false,
        ),
    };
    let mut result = finish_inconclusive(
        spec,
        json,
        start,
        durations.to_vec(),
        &reason,
        Some(if pending {
            "complete the queued fast gate repair before resuming this PR"
        } else {
            "retry once the forge or network is reachable"
        }),
        driver.hint_context().forge,
    );
    result.fast_gate_pending = pending;
    Some(result)
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

/// STORY-975: central retry gate for transient phase failures. The caller owns
/// re-running only the failed phase; this helper records state/event side
/// effects and returns whether to continue the phase loop.
// trace:STORY-975 | ai:codex
fn maybe_retry_transient_failure(
    driver: &mut dyn PhaseDriver,
    phase: Phase,
    spec: &str,
    failure: &PhaseFailure,
    retries_used: &mut usize,
    json: bool,
    start: &Instant,
) -> Result<bool, PhaseFailure> {
    let failure = failure.clone().reclassify_transient();
    let budget = clamp_transient_retry_budget(driver.transient_retry_budget());
    if !should_retry_transient_failure(failure.kind, *retries_used, budget) {
        return Ok(false);
    }
    *retries_used += 1;
    let attempt = (*retries_used + 1) as u32;
    let max = (budget + 1) as u32;
    let cause = failure.kind.cause_slug();
    // BUG-1299: carry the failure detail through the retry event boundary —
    // it's the same `reason` a shelve would keep as `SpecShelved.detail`. For
    // a watchdog retry this is what distinguishes "stalled, go find out why"
    // from "ran out of clock, raise the budget" instead of the bare cause
    // word `watchdog`. trace:BUG-1299 | ai:claude
    let detail = failure.reason.as_str();
    driver.record_transient_retry(spec, phase, cause, attempt, max, Some(detail));
    driver.prepare_transient_retry(spec, phase, cause)?;
    if json {
        let attempt_s = attempt.to_string();
        let max_s = max.to_string();
        println!(
            "{}",
            phase_event(
                phase.slug(),
                "retrying",
                spec,
                start.elapsed().as_millis(),
                None,
                &[
                    ("kind", cause),
                    ("attempt", attempt_s.as_str()),
                    ("max", max_s.as_str()),
                    ("detail", detail),
                ],
            )
        );
    } else {
        eprintln!(
            "  {} transient {} failure ({cause}): {detail}; retrying phase {} attempt {attempt}/{max}",
            glyph(crate::glyphs::Glyph::Info).cyan(),
            phase.slug(),
            phase.index(),
        );
    }
    Ok(true)
}

/// STORY-975: when retry budget is exhausted, the eventual NeedsAttention
/// failure should carry the attempt count in its finding detail.
// trace:STORY-975 | ai:codex
fn failure_with_retry_attempt(
    driver: &dyn PhaseDriver,
    failure: &PhaseFailure,
    retries_used: usize,
) -> PhaseFailure {
    let failure = failure.clone().reclassify_transient();
    if retries_used == 0 || !is_transient_retry_cause(failure.kind.cause_slug()) {
        return failure;
    }
    let max = clamp_transient_retry_budget(driver.transient_retry_budget()) + 1;
    // BUG-1299: `PhaseFailure::of` starts a fresh `hint_override: None` — carry
    // the original forward explicitly. Before this fix, a watchdog failure
    // that exhausted its retry budget and fell through to a shelve lost the
    // trip-specific hint right here: the eventual `SpecShelved.recovery_hint`
    // fell back to the generic per-`FailureKind::Watchdog` text (which
    // asserted NoProgress) even when `reason` — one line below — named the
    // Ceiling trip. That contradiction (detail says ceiling, hint describes
    // no-progress and calls it "likely a degenerate spin") is the exact
    // defect the TASK-1274 observation reported. trace:BUG-1299 | ai:claude
    let mut updated = PhaseFailure::of(
        failure.kind,
        format!("{} (attempt {}/{})", failure.reason, retries_used + 1, max),
    );
    updated.hint_override = failure.hint_override.clone();
    updated
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
                    glyph(crate::glyphs::Glyph::Check).green()
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
                // trace:BUG-770 | ai:claude — the driver names the event root.
                let events_root = driver.events_root();
                PuntFlow::Terminal(Box::new(finish_escalated(
                    spec,
                    json,
                    start,
                    durations.to_vec(),
                    EscalationKind::DesignFork,
                    &reason,
                    events_root.as_deref(),
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
        Ok(ImplementerOutcome::NoChange) => PuntFlow::Terminal(Box::new(finish_no_change(
            spec,
            json,
            start,
            durations.to_vec(),
        ))),
        Ok(ImplementerOutcome::PrOpened) => PuntFlow::Proceed,
        // BUG-709: the resumed implementer merged its own PR — terminal
        // success; the work shipped and the spec auto-bumped, so there is no
        // pipeline left to run. trace:BUG-709 | ai:claude
        Ok(ImplementerOutcome::AlreadyMerged { pr_number: _ }) => PuntFlow::Terminal(Box::new(
            finish_success(spec, spec, json, start, durations.to_vec()),
        )),
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
                    driver.hint_context().forge,
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
                driver.hint_context().forge,
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

/// BUG-1134: after a lease-less phase-1 failure the TASK-133 compensation
/// restores the spec to a clean un-started status. That is right when no work
/// happened — but a lease-less phase-1 failure can also be a worktree-reuse
/// collision AFTER a prior attempt already opened a PR. Restoring to un-started
/// then ORPHANS that successful PR and misleads with "no work was stranded".
///
/// So skip the restore only when an open PR DEFINITIVELY exists. An absent or
/// flaky forge (CliMissing / CliFailed / Unreachable) is inconclusive and must
/// fall back to the restore — a transient `gh` hiccup must never strand the
/// spec InProgress with no lease.
// trace:BUG-1134 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase1FailureRecovery {
    /// No definitive open PR — run the original TASK-133 restore-to-un-started.
    RestoreUnstarted,
    /// An open PR already exists — do not restore; surface it + the resume path.
    PreserveOpenPr,
}

pub(crate) fn classify_phase1_failure_recovery(
    pr_lookup: &crate::forge::ChangeLookup,
) -> Phase1FailureRecovery {
    match pr_lookup {
        crate::forge::ChangeLookup::Found(_) => Phase1FailureRecovery::PreserveOpenPr,
        // NoChange / CliMissing / CliFailed / Unreachable are all NON-definitive:
        // fall back to the safe restore rather than strand the spec on a
        // transient or absent forge.
        _ => Phase1FailureRecovery::RestoreUnstarted,
    }
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
/// slice 2's `aida queue work <SPEC> --plan-only` (the interactive planning
/// session; a headless drain uses the hidden `--plan-phase --no-human` lane)
/// and slice 1's `aida plan promote <SPEC>` (the
/// Approved → Planned transition). Modelling the steps as data (rather than
/// inlining the shell-outs) keeps the decision — *with-plan ⇒ plan then drain;
/// without ⇒ drain only* — a pure function the unit tests pin down in
/// isolation.
// trace:STORY-265 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlanPreludeStep {
    /// `aida queue work <spec> --plan-only` — the slice-2 planning session.
    /// `headless` swaps in the hidden `--plan-phase --no-human` lane when the
    /// drain runs a headless implementer (`--no-human=both`), matching how the
    /// phase-1 implementer is launched.
    // trace:STORY-265 | ai:claude
    // trace:TASK-1953 | ai:claude — prose names the headless --plan-phase lane.
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

/// The one-line "you can watch this live" hint printed under the kickoff
/// banner. The out-of-band `aida drain status` reads `.aida/drain-state.json`
/// and shows the live phase / members / command — it is good but
/// undiscoverable, so the banner advertises it. Pure + named so a test can
/// assert the banner keeps naming the command.
// trace:STORY-726 | ai:claude
pub(crate) fn watch_live_hint() -> &'static str {
    "watch live: aida drain status"
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
    let mut start_phase = start_phase;
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
        // STORY-726: advertise the out-of-band live view so an unattended drive
        // is discoverable, not just trustworthy. trace:STORY-726 | ai:claude
        eprintln!("   {}", watch_live_hint().dimmed());
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
        driver.capture_phase_done_pr();
    } else {
        // BUG-657: refuse to drive a spec that is already terminal (Completed or
        // Rejected) BEFORE spawning the implementer. The implementer would exit 1
        // ("nothing to implement") and the orchestrator would auto-draft a phantom
        // failure BUG — the BUG-638 → BUG-644..649 incident (6 identical drafts in
        // a 6-minute window). A terminal target is a clean NO-OP, not a phase-1
        // failure. Only the from-scratch (phase-1) entry checks this; a
        // `--resume`/`--from-pr` re-entry returns `None` from `terminal_status`
        // because it legitimately re-drives a spec that reached Completed mid-
        // pipeline (the merge already promoted it; BUG-241 reconcile handles that
        // case as an out-of-band success). trace:BUG-657 | ai:claude
        if let Some(status) = driver.terminal_status() {
            return finish_noop(spec, status, json, &start);
        }
        // Phase 1 — implementer session. Outcomes: a PR was opened (run on), a
        // genuine failure (reconcile then report), or a punt. STORY-306: a punt no
        // longer stops the drain outright — it routes through the headless advisor
        // tier, which resolves the fork (the implementer resumes, the pipeline
        // continues) or escalates it (the run ends per `escalate_mode`).
        // trace:STORY-276, STORY-306 | ai:claude
        emit_start(Phase::Implementer, spec, json, start.elapsed().as_millis());
        // BUG-1628: a resolver failure and a resolver mismatch are named as
        // such, before any lease exists, instead of a generic refusal.
        // trace:BUG-1628 | ai:claude
        let workspace = match driver.implementer_workspace() {
            Ok(workspace) => workspace,
            Err(err) => {
                let failure = PhaseFailure::of(
                    FailureKind::Failed,
                    phase1_resolver_failure_reason(spec, &err),
                );
                return resolve_phase_failure(
                    driver,
                    Phase::Implementer,
                    spec,
                    json,
                    &start,
                    &failure,
                    durations,
                );
            }
        };
        if let Some((worktree, branch)) = workspace {
            if !json {
                // BUG-1928: this is the REQUESTED workspace. The child's session
                // start may serve a warm-pool tree instead; the child announces
                // the tree it actually runs in (`phase 1 workspace: …`) and the
                // contained write boundary derived from it. trace:BUG-1928 | ai:claude
                eprintln!(
                    "  {} phase 1 workspace requested: {} [{}]",
                    "↳".cyan(),
                    worktree,
                    branch
                );
            }
            let known_prefixes = driver.known_spec_prefixes();
            if !crate::workflow_hints::branch_belongs_to_spec_with_prefixes(
                &branch,
                spec,
                &known_prefixes,
            ) {
                let failure = PhaseFailure::of(
                    FailureKind::Failed,
                    phase1_resolver_mismatch_reason(spec, &worktree, &branch),
                );
                return resolve_phase_failure(
                    driver,
                    Phase::Implementer,
                    spec,
                    json,
                    &start,
                    &failure,
                    durations,
                );
            }
            // BUG-1629: the checked value is the launched value.
            // trace:BUG-1629 | ai:claude
            driver.pin_implementer_workspace(&worktree, &branch);
        }
        driver.begin_rework_guard();
        // BUG-1522: the rework no-op guard was consulted on only ONE of the
        // implementer-advance arms below (PrOpened) — but a rework round runs
        // against a PR that is ALREADY open, so it never returns PrOpened.
        // The guard was wired to the one case it was not written for, and an
        // unchanged, already-refused head advanced silently on every other
        // arm. This macro is the single check point every advancing arm
        // (Err(f)-recovered, Punted->Proceed, PrOpened, AlreadyMerged,
        // Inconclusive, Held) now runs through, so the six call sites cannot
        // drift apart again. trace:BUG-1522 | ai:claude
        //
        // The optional `$hint_override` arm exists for exactly one caller:
        // the Inconclusive arm below. `driver.rework_no_op_failure()` always
        // resolves its own generic `FailureKind::ReworkNoOp` hint ("push a
        // fixup commit or punt the finding") through `recovery_hint`, which
        // is right for a genuine no-progress rework round but wrong when
        // phase 1 ALSO reported Inconclusive (BUG-257/BUG-266 — a transient
        // GH/Anthropic-API blip, not a stalled rework): that case's own
        // `retry_hint` (the `--resume <session>` hint `finish_inconclusive`
        // showed before this guard intercepted it) is what an operator needs
        // to recover, and PhaseFailure::with_hint_override is the same BUG-
        // 1299 mechanism that already carries a resolved-together hint
        // through `resolve_phase_failure` instead of a kind-only re-
        // derivation. trace:BUG-1522 | ai:claude
        macro_rules! fail_on_rework_no_op {
            () => {
                fail_on_rework_no_op!(None::<String>);
            };
            ($hint_override:expr) => {
                if let Some(f) = driver.rework_no_op_failure() {
                    let f = match $hint_override {
                        Some(hint) => f.with_hint_override(hint),
                        None => f,
                    };
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
            };
        }
        let phase_start = Instant::now();
        let mut retries_used = 0usize;
        loop {
            let outcome = driver.run_implementer();
            // Check before failure recovery: an open PR cannot redeem a queued repair.
            // trace:STORY-2640 | ai:codex
            if let Some(result) = stop_for_fast_gate(driver, spec, json, &start, &durations) {
                return result;
            }
            match outcome {
                Err(f) => {
                    // BUG-1524: a launch refusal (preflight/lease refusal,
                    // agent-binary launch failure) means no implementer ran
                    // this round, so the branch head is provably unchanged.
                    // An already-open PR from a PREVIOUS round is not
                    // evidence this round did anything — never route a
                    // launch refusal through the "phase 1 failed, but an
                    // open PR already exists" recovery, which would
                    // manufacture a CI/review round against a head this
                    // failure never touched. Fall straight through to the
                    // normal shelve path below. trace:BUG-1524 | ai:claude
                    // Review fix: a LeaseConflict is also a pre-launch
                    // refusal (the claim gate blocked the child), so it must
                    // not be "recovered" through a previous round's open PR
                    // either. trace:BUG-1524 | ai:claude
                    if !matches!(
                        f.kind,
                        FailureKind::LaunchRefused
                            | FailureKind::LeaseConflict
                            // BUG-1527 review fix: a branch swap onto another
                            // spec must not be "recovered" through a PR the
                            // spec search happens to find. trace:BUG-1527
                            | FailureKind::ShippedMismatch
                    ) {
                        if let Some(reentry_phase) = driver.recover_phase1_failure_with_open_pr(&f)
                        {
                            driver.capture_phase_done_pr();
                            durations.push((Phase::Implementer, phase_start.elapsed().as_millis()));
                            fail_on_rework_no_op!();
                            emit_done(Phase::Implementer, spec, json, start.elapsed().as_millis());
                            start_phase = reentry_phase;
                            break;
                        }
                    }
                    match maybe_retry_transient_failure(
                        driver,
                        Phase::Implementer,
                        spec,
                        &f,
                        &mut retries_used,
                        json,
                        &start,
                    ) {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(prep) => {
                            durations.push((Phase::Implementer, phase_start.elapsed().as_millis()));
                            return resolve_phase_failure(
                                driver,
                                Phase::Implementer,
                                spec,
                                json,
                                &start,
                                &prep,
                                durations,
                            );
                        }
                    }
                    let f = failure_with_retry_attempt(driver, &f, retries_used);
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
                            driver.capture_phase_done_pr();
                            fail_on_rework_no_op!();
                            emit_done(Phase::Implementer, spec, json, start.elapsed().as_millis());
                        }
                    }
                }
                Ok(ImplementerOutcome::NoChange) => {
                    durations.push((Phase::Implementer, phase_start.elapsed().as_millis()));
                    return finish_no_change(spec, json, &start, durations);
                }
                Ok(ImplementerOutcome::PrOpened) => {
                    driver.capture_phase_done_pr();
                    durations.push((Phase::Implementer, phase_start.elapsed().as_millis()));
                    fail_on_rework_no_op!();
                    emit_done(Phase::Implementer, spec, json, start.elapsed().as_millis());
                }
                // BUG-709: the implementer already merged its own PR (it ran the
                // full ship itself). There is nothing left for phases 2-5 to do —
                // the spec already auto-bumped on merge — so complete cleanly
                // instead of shepherding a merged PR through CI/review/merge again.
                // trace:BUG-709 | ai:claude
                Ok(ImplementerOutcome::AlreadyMerged { pr_number }) => {
                    driver.capture_phase_done_pr();
                    durations.push((Phase::Implementer, phase_start.elapsed().as_millis()));
                    fail_on_rework_no_op!();
                    if !json {
                        eprintln!(
                            "  {} PR-{} already merged by the implementer — work shipped; \
                         completing (skipping CI/review/merge/pull)",
                            glyph(crate::glyphs::Glyph::Check).green(),
                            pr_number,
                        );
                    }
                    emit_done(Phase::Implementer, spec, json, start.elapsed().as_millis());
                    return finish_success(spec, spec, json, &start, durations);
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
                    driver.capture_phase_done_pr();
                    durations.push((Phase::Implementer, phase_start.elapsed().as_millis()));
                    // BUG-1522: if the rework guard also shelves this round as a
                    // no-op, keep the `--resume` hint this Inconclusive arm would
                    // otherwise have shown (see the macro doc above) rather than
                    // losing it to the generic ReworkNoOp hint.
                    fail_on_rework_no_op!(retry_hint.clone());
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
                        driver.hint_context().forge,
                    );
                }
                // BUG-250: the implementer deliberately held the PR (branch pushed, PR
                // intentionally not opened, pending a manual gate). A clean non-failure
                // stop — exit `0`, no `failed_phase` — distinct from a punt (no
                // design-fork) and a failure (nothing broke). The drain halts at phase
                // 1 with the correct "open the PR when your gate passes" hint.
                // trace:BUG-250
                Ok(ImplementerOutcome::Held { reason, branch }) => {
                    driver.capture_phase_done_pr();
                    durations.push((Phase::Implementer, phase_start.elapsed().as_millis()));
                    fail_on_rework_no_op!();
                    return finish_held(
                        spec,
                        json,
                        &start,
                        durations,
                        reason.as_deref(),
                        &branch,
                        driver.hint_context().forge,
                    );
                }
            }
            break;
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

    // Also protect --from-pr/--resume runs that skipped the implementer.
    // trace:STORY-2640 | ai:codex
    if let Some(result) = stop_for_fast_gate(driver, spec, json, &start, &durations) {
        return result;
    }

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
        // TASK-975: bounded CI auto-fix loop. On a red CI run an opted-in
        // drain (`[drain] ci_auto_fix = N`) spawns up to N headless fix
        // sessions — each edits in the implementer worktree, pushes, and the
        // loop re-polls CI — before the failure falls through to the EPIC-28
        // shelve. Budget 0 (the default) is identical to the old single
        // attempt; shelve stays the terminal fallback either way.
        // trace:TASK-975 | ai:claude
        let mut ci_fix_attempts = 0usize;
        let mut retries_used = 0usize;
        loop {
            match driver.finish_ci() {
                Ok(()) => break,
                Err(f) => {
                    let budget = driver.ci_fix_budget();
                    if let RedCiAction::AttemptFix { attempt } =
                        red_ci_action(budget, ci_fix_attempts, f.kind)
                    {
                        ci_fix_attempts = attempt;
                        if driver.attempt_ci_fix(attempt, budget, &f) {
                            // A fix was pushed — re-poll CI on the new head.
                            continue;
                        }
                    }
                    match maybe_retry_transient_failure(
                        driver,
                        Phase::Ci,
                        spec,
                        &f,
                        &mut retries_used,
                        json,
                        &start,
                    ) {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(prep) => {
                            durations.push((Phase::Ci, phase_start.elapsed().as_millis()));
                            return resolve_phase_failure(
                                driver,
                                Phase::Ci,
                                spec,
                                json,
                                &start,
                                &prep,
                                durations,
                            );
                        }
                    }
                    let f = failure_with_retry_attempt(driver, &f, retries_used);
                    durations.push((Phase::Ci, phase_start.elapsed().as_millis()));
                    return resolve_phase_failure(
                        driver,
                        Phase::Ci,
                        spec,
                        json,
                        &start,
                        &f,
                        durations,
                    );
                }
            }
        }
        if variant.last_phase() > 2 {
            if let Err(f) = driver.verify_ci_for_review() {
                durations.push((Phase::Ci, phase_start.elapsed().as_millis()));
                return resolve_phase_failure(driver, Phase::Ci, spec, json, &start, &f, durations);
            }
        }
        durations.push((Phase::Ci, phase_start.elapsed().as_millis()));
        emit_done(Phase::Ci, spec, json, start.elapsed().as_millis());
    }
    if variant.last_phase() <= 2 {
        // trace:BUG-1807 | ai:codex
        if let Err(failure) = driver.confirm_review_handoff() {
            return resolve_phase_failure(
                driver,
                Phase::Ci,
                spec,
                json,
                &start,
                &failure,
                durations,
            );
        }
        let ctx = driver.hint_context();
        return finish_through_ci_success(spec, &credited, json, &start, durations, &ctx);
    }

    // Phase 3 — reviewer session. lifecycle:no-review skips only this model
    // phase; merge and pull still run so substrate state stays coherent.
    if let Some(result) = stop_for_fast_gate(driver, spec, json, &start, &durations) {
        return result;
    }
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
        let mut retries_used = 0usize;
        loop {
            // trace:STORY-975 | ai:codex
            let bound_pr = driver.phase_done_pr_number();
            let review_pr = driver.review_pr_number();
            if driver.requires_phase_done_pr() && bound_pr.is_none() {
                let f = PhaseFailure::of(
                    FailureKind::NoPr,
                    format!(
                        "no PhaseDonePr was recorded for this {spec} run — refusing to launch a reviewer"
                    ),
                );
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
            if driver.requires_phase_done_pr() && review_pr.is_some() && review_pr != bound_pr {
                let f = PhaseFailure::of(
                    FailureKind::NoPr,
                    format!(
                        "review PR {:?} does not match this run's PhaseDonePr {:?} for {spec} — refusing cross-spec review",
                        review_pr, bound_pr
                    ),
                );
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
            if review_pr.is_none() {
                if driver.recover_missing_review_pr().is_some() {
                    continue;
                }
                let f = PhaseFailure::of(
                    FailureKind::NoPr,
                    "no open PR resolved before the reviewer phase — refusing to launch reviewer for PR-0",
                );
                match maybe_retry_transient_failure(
                    driver,
                    Phase::Reviewer,
                    spec,
                    &f,
                    &mut retries_used,
                    json,
                    &start,
                ) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(prep) => {
                        durations.push((Phase::Reviewer, phase_start.elapsed().as_millis()));
                        return resolve_phase_failure(
                            driver,
                            Phase::Reviewer,
                            spec,
                            json,
                            &start,
                            &prep,
                            durations,
                        );
                    }
                }
                let f = failure_with_retry_attempt(driver, &f, retries_used);
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
            match driver.run_reviewer() {
                Err(f) => {
                    match maybe_retry_transient_failure(
                        driver,
                        Phase::Reviewer,
                        spec,
                        &f,
                        &mut retries_used,
                        json,
                        &start,
                    ) {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(prep) => {
                            durations.push((Phase::Reviewer, phase_start.elapsed().as_millis()));
                            return resolve_phase_failure(
                                driver,
                                Phase::Reviewer,
                                spec,
                                json,
                                &start,
                                &prep,
                                durations,
                            );
                        }
                    }
                    let f = failure_with_retry_attempt(driver, &f, retries_used);
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
                    // trace:BUG-770 | ai:claude — the driver names the event root.
                    let events_root = driver.events_root();
                    return finish_escalated(
                        spec,
                        json,
                        &start,
                        durations,
                        EscalationKind::MergeDecision,
                        &reason,
                        events_root.as_deref(),
                    );
                }
                // trace:BUG-2500 | ai:codex
                Ok(ReviewerOutcome::Verdict(Verdict::Waiting)) => {
                    durations.push((Phase::Reviewer, phase_start.elapsed().as_millis()));
                    return finish_escalated(spec, json, &start, durations, EscalationKind::Waiting,
                        "WAITING: external evidence required; resume with a fresh recorded review at the current head",
                        driver.events_root().as_deref());
                }
                Ok(ReviewerOutcome::Verdict(verdict)) if verdict != Verdict::Approved => {
                    durations.push((Phase::Reviewer, phase_start.elapsed().as_millis()));
                    let kind = match verdict {
                        Verdict::RequestChanges => FailureKind::VerdictRequestChanges,
                        Verdict::Rejected => FailureKind::VerdictReject,
                        Verdict::Waiting => unreachable!("waiting stops before retries"),
                        Verdict::Approved => FailureKind::Failed,
                    };
                    let f = PhaseFailure::of(
                        kind,
                        format!("reviewer verdict is {} — not Approved", verdict.label()),
                    );
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
            break;
        }
        durations.push((Phase::Reviewer, phase_start.elapsed().as_millis()));
        emit_done(Phase::Reviewer, spec, json, start.elapsed().as_millis());
    }

    if let Err(f) = driver.run_agent_gates() {
        if f.kind == FailureKind::VerdictWaiting {
            return finish_escalated(
                spec,
                json,
                &start,
                durations,
                EscalationKind::Waiting,
                &f.reason,
                driver.events_root().as_deref(),
            );
        }
        let f = failure_with_retry_attempt(driver, &f, 0);
        return resolve_phase_failure(driver, Phase::Reviewer, spec, json, &start, &f, durations);
    }
    // TASK-1249 / ADR-45: advisory harvest between the gates and merge —
    // propose-only, cannot fail the run.
    // trace:TASK-1249 | ai:claude
    driver.run_harvest_gate();

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
        // BUG-727: the substrate supervised-merge gate. Ask the driver whether
        // the spec's `execution_mode` holds the merge (anything but `drain` —
        // or no mode at all — does). A hold is an honest clean stop, not a
        // failure: the same terminal path as the reviewer's own merge
        // escalation — exit `0`, no merge, the PR left mergeable for the
        // human/advisor, a batch drain advances. trace:BUG-727 | ai:claude
        if let Some(reason) = driver.merge_supervision_hold() {
            driver.record_merge_supervision_hold(&reason);
            // trace:BUG-770 | ai:claude — the driver names the event root.
            let events_root = driver.events_root();
            return finish_escalated(
                spec,
                json,
                &start,
                durations,
                EscalationKind::SupervisedMerge,
                &reason,
                events_root.as_deref(),
            );
        }
        emit_start(Phase::Merge, spec, json, start.elapsed().as_millis());
        let phase_start = Instant::now();
        // TASK-975: in-drain merge-conflict rebase. When the merge fails
        // because the base moved under the PR (a conflict), an opted-in drain
        // (`[drain] ci_auto_fix > 0`) attempts ONE clean rebase (the same
        // `pr rebase --no-smoke` subprocess as the STORY-429 phase-3
        // auto-rebase) and retries the merge exactly once before the failure
        // falls through to shelve. trace:TASK-975 | ai:claude
        //
        // TASK-1458 (known limit): since TASK-1448 the retried merge always
        // meets the approval-covers-head gate, and a rebase always moves the
        // head past the recorded approval — so in practice this path SHELVES
        // the spec (`StaleApproval`) rather than shipping. The rebase is not
        // wasted: the shelved PR is conflict-free and needs only a re-review
        // of the rebased head. Accepting a rebased head whose patch-id is
        // unchanged, or re-requesting review in-drain, is the open redesign.
        // trace:TASK-1458 | ai:claude
        let mut conflict_rebase_attempted = false;
        let mut retries_used = 0usize;
        loop {
            match driver.merge() {
                Ok(()) => break,
                Err(f) => {
                    if should_attempt_conflict_rebase(
                        driver.ci_fix_budget(),
                        conflict_rebase_attempted,
                        &f.reason,
                    ) {
                        conflict_rebase_attempted = true;
                        if driver.attempt_merge_conflict_rebase(&f) {
                            // Rebased clean — retry the merge once.
                            continue;
                        }
                    }
                    match maybe_retry_transient_failure(
                        driver,
                        Phase::Merge,
                        spec,
                        &f,
                        &mut retries_used,
                        json,
                        &start,
                    ) {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(prep) => {
                            durations.push((Phase::Merge, phase_start.elapsed().as_millis()));
                            return resolve_phase_failure(
                                driver,
                                Phase::Merge,
                                spec,
                                json,
                                &start,
                                &prep,
                                durations,
                            );
                        }
                    }
                    let f = failure_with_retry_attempt(driver, &f, retries_used);
                    durations.push((Phase::Merge, phase_start.elapsed().as_millis()));
                    return resolve_phase_failure(
                        driver,
                        Phase::Merge,
                        spec,
                        json,
                        &start,
                        &f,
                        durations,
                    );
                }
            }
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
        let mut retries_used = 0usize;
        loop {
            match driver.pull() {
                Ok(()) => break,
                Err(f) => {
                    match maybe_retry_transient_failure(
                        driver,
                        Phase::Pull,
                        spec,
                        &f,
                        &mut retries_used,
                        json,
                        &start,
                    ) {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(prep) => {
                            durations.push((Phase::Pull, phase_start.elapsed().as_millis()));
                            return resolve_phase_failure(
                                driver,
                                Phase::Pull,
                                spec,
                                json,
                                &start,
                                &prep,
                                durations,
                            );
                        }
                    }
                    let f = failure_with_retry_attempt(driver, &f, retries_used);
                    durations.push((Phase::Pull, phase_start.elapsed().as_millis()));
                    return resolve_phase_failure(
                        driver,
                        Phase::Pull,
                        spec,
                        json,
                        &start,
                        &f,
                        durations,
                    );
                }
            }
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
        let mut retries_used = 0usize;
        loop {
            match driver.build() {
                Ok(()) => break,
                Err(f) => {
                    match maybe_retry_transient_failure(
                        driver,
                        Phase::Build,
                        spec,
                        &f,
                        &mut retries_used,
                        json,
                        &start,
                    ) {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(prep) => {
                            durations.push((Phase::Build, phase_start.elapsed().as_millis()));
                            return resolve_phase_failure(
                                driver,
                                Phase::Build,
                                spec,
                                json,
                                &start,
                                &prep,
                                durations,
                            );
                        }
                    }
                    let f = failure_with_retry_attempt(driver, &f, retries_used);
                    durations.push((Phase::Build, phase_start.elapsed().as_millis()));
                    return resolve_phase_failure(
                        driver,
                        Phase::Build,
                        spec,
                        json,
                        &start,
                        &f,
                        durations,
                    );
                }
            }
        }
        durations.push((Phase::Build, phase_start.elapsed().as_millis()));
        emit_done(Phase::Build, spec, json, start.elapsed().as_millis());
    }

    finish_success(spec, &credited, json, &start, durations)
}

// --- Batch drain (TASK-285) -------------------------------------------------
//
// `aida queue work --batch NAME --auto-complete` drives batch members through
// the same per-spec phase engine. Depth 1 keeps the historical one-full-
// lifecycle-at-a-time loop; depth >1 starts later implementer/CI children while
// the parent serializes review/merge/pull/build for ready PRs.
// trace:TASK-285 STORY-1091 | ai:claude

/// Why a [`drain_batch`] run stopped. trace:TASK-285 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BatchDrainOutcome {
    /// Successful seat with nothing published. trace:TASK-3608 | ai:codex
    NoChange,
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
    /// BUG-1930: consecutive specs failed with the same environmental cause
    /// (see [`environmental_failure_class`]), so the drain halted before
    /// dispatching another one. The rest of the queue is untouched; the
    /// operator fixes the environment and re-runs. `cause` is the class slug.
    // trace:BUG-1930 | ai:claude
    EnvironmentBlocked { cause: &'static str },
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
    /// EPIC-28: members the drain skipped, as `(display_id, reason_label)`.
    /// Three sources fill it:
    /// - members blocked by a just-shelved (or already-`NeedsAttention`)
    ///   spec, labelled with the `pickability_reason_label` the queue UI
    ///   uses. The pure [`drain_batch`] cannot fill these (the mock has no
    ///   relationship graph); the real CLI surface (`resolve_batch_members`)
    ///   records them after the fact. trace:EPIC-28 | ai:claude
    /// - in-flight pipelined members whose serial finish was skipped because
    ///   the parent's serial leg broke or the queue stalled (TASK-1615);
    /// - in-flight members whose held, inconclusive, failed or
    ///   environment-blocked result arrived after another member's halt had
    ///   latched (TASK-1617). trace:TASK-1951 trace:TASK-2106 | ai:claude
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

/// Remove `spec` from every final-disposition bucket before recording its latest
/// drain outcome. A spec can be shelved, retried later in the same drain, and
/// then ship; the exit summary must report only the final state.
// trace:BUG-852 | ai:codex
fn forget_batch_disposition(
    spec: &str,
    shipped: &mut Vec<String>,
    punted: &mut Vec<String>,
    escalated: &mut Vec<String>,
    shelved: &mut Vec<String>,
    skipped: &mut Vec<(String, String)>,
) {
    shipped.retain(|s| s != spec);
    punted.retain(|s| s != spec);
    escalated.retain(|s| s != spec);
    shelved.retain(|s| s != spec);
    skipped.retain(|(s, _)| s != spec);
}

/// Drives a sequential batch drain: yields the current batch head and runs one
/// spec's full `--auto-complete` orchestration. The pipelined scheduler layers
/// a child-process implementer/CI leg on top of this trait via
/// [`PipelinedBatchDriver`].
// trace:TASK-285 STORY-1091 | ai:claude
pub(crate) trait BatchDriver {
    /// The current batch head spec-id, or `None` when the batch is drained.
    /// Re-resolved each call — a completed spec leaves the queue, so the head
    /// advances naturally.
    fn next_head(&mut self) -> Option<String>;
    // trace:STORY-2640 | ai:codex
    fn next_head_excluding(&mut self, excluded: &[String]) -> Option<String> {
        self.next_head().filter(|id| !excluded.contains(id))
    }
    /// Run one spec's full `--auto-complete` orchestration.
    fn run_spec(&mut self, spec: &str) -> OrchestrationResult;
    /// TASK-966: cumulative reported tokens across every headless phase the
    /// drain has run so far (input + output + cache), parsed from each phase's
    /// `claude -p --output-format stream-json` log. Only consulted when a
    /// `--max-tokens` cap is active; the default `0` means a driver that does
    /// not (or cannot) account tokens — e.g. the test mock or an interactive
    /// drain with no headless logs — never trips the token cap.
    // trace:TASK-966 | ai:claude
    fn cumulative_tokens(&mut self) -> u64 {
        0
    }
    /// STORY-1130: has a cooperative `aida drain stop` been requested? The
    /// drain loop asks the driver instead of reading the process-wide
    /// `AIDA_DRAIN_STOP_FILE` env var itself, so a test driver cannot observe
    /// another (parallel) test's stop request. Required — no default — so a
    /// new production driver cannot silently ignore `aida drain stop`; real
    /// drivers delegate to [`crate::drain_cmd::stop_requested_from_env`].
    // trace:BUG-1980 | ai:claude
    fn stop_requested(&mut self) -> bool;
    /// BUG-1930: how many consecutive same-cause environmental failures halt
    /// the drain (`0` = never). Real drivers read `[orchestrator]
    /// environment_halt_after`.
    // trace:BUG-1930 | ai:claude
    fn environment_halt_after(&self) -> usize {
        DEFAULT_ENVIRONMENT_HALT_AFTER
    }
}

/// Handle for one pipelined implementer/CI child.
// trace:STORY-1091 trace:ADR-28 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PipelinedHandle(pub(crate) usize);

/// Batch driver extension for the bounded child-process scheduler.
// trace:STORY-1091 trace:ADR-28 | ai:codex
pub(crate) trait PipelinedBatchDriver: BatchDriver {
    /// Maximum number of implementer/CI children that may be in flight.
    fn pipeline_depth(&self) -> usize {
        1
    }

    /// Start one member's implementer + CI leg. Implementations may run this
    /// asynchronously.
    fn start_spec_through_ci(&mut self, spec: &str) -> PipelinedHandle;

    /// Wait for the implementer/CI leg.
    fn wait_spec_through_ci(&mut self, handle: PipelinedHandle) -> OrchestrationResult;

    /// Run the remaining serial phases for a member whose PR is ready. The
    /// parent scheduler calls this one member at a time.
    fn finish_spec_after_ci(&mut self, spec: &str) -> OrchestrationResult;
}

#[derive(Debug, Clone)]
struct InFlightMember {
    spec: String,
    handle: PipelinedHandle,
}

/// BUG-1608: has the `--max-failures` budget been spent? `max_failures = N`
/// (N ≥ 1) allows exactly N shelves; once the Nth lands the drain must stop
/// BEFORE it dispatches another spec. The check therefore runs right after a
/// shelve is recorded, not when the next failure arrives — the old
/// `shelved + 1 > cap` test only fired on the (N+1)th failure, so a cap of one
/// let a second spec launch after the first shelve. `0` keeps its documented
/// meaning ("the first failure stops the drain"): that case never reaches a
/// shelve, because `shelved + 1 > 0` already routes the first failure to the
/// hard stop.
// trace:BUG-1608 | ai:claude
///
/// STORY-1429: the count is shelve EVENTS, not distinct shelved specs. A spec
/// that is requeued and shelves again in the same drain spends the budget
/// again: a second failure after triage is more evidence that something is
/// wrong, and the budget is the drain's only automatic circuit breaker. The
/// `shelved` list still reports FINAL dispositions (BUG-852).
// trace:STORY-1429 | ai:claude
fn failure_budget_exhausted(shelve_events: usize, max_failures: Option<usize>) -> bool {
    max_failures.is_some_and(|cap| shelve_events >= cap)
}

/// BUG-1930: default for `[orchestrator] environment_halt_after`.
pub(crate) const DEFAULT_ENVIRONMENT_HALT_AFTER: usize = 2;

/// BUG-1930: the environmental class of a failed run, or `None` when the
/// failure (if any) is about the spec itself. These causes are properties of
/// the machine, so the next spec would hit them too.
// trace:BUG-1930 | ai:claude
pub(crate) fn environmental_failure_class(result: &OrchestrationResult) -> Option<&'static str> {
    if result.exit_code == 0 {
        return None;
    }
    let failure = result.failure.as_ref()?;
    if failure.kind == FailureKind::PermissionDenied {
        return Some(FailureKind::PermissionDenied.cause_slug());
    }
    is_environmental_failure(&failure.reason).then_some("resource-exhaustion")
}

/// BUG-1930: the one-line halt message for an environment-blocked drain:
/// what happened and what the operator should do about it.
// trace:BUG-1930 | ai:claude
pub(crate) fn environment_blocked_message(cause: &str) -> String {
    let fix = match cause {
        "permission-denied" => {
            "the agent's tool calls were denied; allow writes for the drain \
             (permission mode or agent sandbox settings), then re-run the drain"
        }
        "resource-exhaustion" => "free disk space or memory, then re-run the drain",
        _ => "fix the environment, then re-run the drain",
    };
    format!("environment blocked: consecutive specs failed with {cause}; {fix}")
}

/// BUG-1930: counts consecutive finished specs that failed with the same
/// environmental class. Any other result resets it.
// trace:BUG-1930 | ai:claude
#[derive(Debug, Default)]
struct EnvironmentStreak {
    class: Option<&'static str>,
    count: usize,
}

impl EnvironmentStreak {
    /// Record one finished spec. Returns the class once `halt_after`
    /// consecutive failures share it (`halt_after == 0` never halts).
    fn observe(&mut self, result: &OrchestrationResult, halt_after: usize) -> Option<&'static str> {
        let Some(class) = environmental_failure_class(result) else {
            *self = Self::default();
            return None;
        };
        if self.class == Some(class) {
            self.count += 1;
        } else {
            self.class = Some(class);
            self.count = 1;
        }
        (halt_after > 0 && self.count >= halt_after).then_some(class)
    }

    /// Should the pipelined scheduler hold off launching another spec? Yes
    /// while a streak is open and the `in_flight` members alone could bring
    /// it to `halt_after`. With nothing in flight this is always `false`
    /// (an open streak is below `halt_after`), so the scheduler never stalls.
    // trace:BUG-1930 | ai:claude
    fn defers_launch(&self, in_flight: usize, halt_after: usize) -> bool {
        halt_after > 0 && self.count > 0 && self.count + in_flight >= halt_after
    }
}

// why: the drain's disposition buckets are threaded through as separate
// `&mut` lists (the same shape `forget_batch_disposition` takes); STORY-1429
// adds the shelve-event counter alongside them.
#[allow(clippy::too_many_arguments)]
fn apply_batch_result(
    head: String,
    result: OrchestrationResult,
    shipped: &mut Vec<String>,
    punted: &mut Vec<String>,
    escalated: &mut Vec<String>,
    shelved: &mut Vec<String>,
    skipped: &mut Vec<(String, String)>,
    max_failures: Option<usize>,
    shelve_events: &mut usize,
    env_halt: Option<&'static str>,
) -> Option<BatchDrainResult> {
    if result.exit_code != 0 {
        let phase = result.failed_phase.unwrap_or(Phase::Implementer);
        // STORY-1429: budget counts shelve events. trace:STORY-1429 | ai:claude
        let over_failure_budget = max_failures
            .map(|cap| *shelve_events + 1 > cap)
            .unwrap_or(false);
        if result.shelved_reason.is_some() && !over_failure_budget {
            forget_batch_disposition(&head, shipped, punted, escalated, shelved, skipped);
            shelved.push(head.clone());
            *shelve_events += 1;
            if let Some(cause) = env_halt {
                // BUG-1930: the same environmental failure again — keep this
                // spec's disposition, then halt instead of failing the next
                // one the same way. trace:BUG-1930 | ai:claude
                return Some(BatchDrainResult {
                    shipped: shipped.clone(),
                    punted: punted.clone(),
                    escalated: escalated.clone(),
                    shelved: shelved.clone(),
                    skipped: skipped.clone(),
                    stopped_at: Some(head),
                    outcome: BatchDrainOutcome::EnvironmentBlocked { cause },
                    exit_code: DRIVE_EXIT_HARD_FAIL,
                });
            }
            // BUG-1608: the budget is spent the moment the Nth shelve lands —
            // stop here, before the scheduler launches another member.
            // trace:BUG-1608 | ai:claude
            if failure_budget_exhausted(*shelve_events, max_failures) {
                return Some(BatchDrainResult {
                    shipped: shipped.clone(),
                    punted: punted.clone(),
                    escalated: escalated.clone(),
                    shelved: shelved.clone(),
                    skipped: skipped.clone(),
                    stopped_at: Some(head),
                    outcome: BatchDrainOutcome::Failed(phase),
                    exit_code: DRIVE_EXIT_HARD_FAIL,
                });
            }
            return None;
        }
        return Some(BatchDrainResult {
            shipped: shipped.clone(),
            punted: punted.clone(),
            escalated: escalated.clone(),
            shelved: shelved.clone(),
            skipped: skipped.clone(),
            stopped_at: Some(head),
            // BUG-1930: an un-shelvable stop that completes an environmental
            // streak still reports the environment. trace:BUG-1930 | ai:claude
            outcome: env_halt.map_or(BatchDrainOutcome::Failed(phase), |cause| {
                BatchDrainOutcome::EnvironmentBlocked { cause }
            }),
            exit_code: DRIVE_EXIT_HARD_FAIL,
        });
    }
    if result.fast_gate_pending {
        skipped.push((
            head,
            result
                .inconclusive_reason
                .unwrap_or_else(|| "fast gate repair pending".into()),
        ));
        return None;
    }
    if result.inconclusive_reason.is_some() {
        return Some(BatchDrainResult {
            shipped: shipped.clone(),
            punted: punted.clone(),
            escalated: escalated.clone(),
            shelved: shelved.clone(),
            skipped: skipped.clone(),
            stopped_at: Some(head),
            outcome: BatchDrainOutcome::Inconclusive,
            exit_code: 0,
        });
    }
    if result.held_reason.is_some() || result.no_change {
        return Some(BatchDrainResult {
            shipped: shipped.clone(),
            punted: punted.clone(),
            escalated: escalated.clone(),
            shelved: shelved.clone(),
            skipped: skipped.clone(),
            stopped_at: Some(head),
            outcome: if result.no_change {
                BatchDrainOutcome::NoChange
            } else {
                BatchDrainOutcome::Held
            },
            exit_code: 0,
        });
    }
    if let Some(actual) = result.shipped_spec_id.clone() {
        forget_batch_disposition(&actual, shipped, punted, escalated, shelved, skipped);
        shipped.push(actual.clone());
        return Some(BatchDrainResult {
            shipped: shipped.clone(),
            punted: punted.clone(),
            escalated: escalated.clone(),
            shelved: shelved.clone(),
            skipped: skipped.clone(),
            stopped_at: Some(head.clone()),
            outcome: BatchDrainOutcome::Mismatched {
                dispatched: head,
                shipped: actual,
            },
            exit_code: 0,
        });
    }
    if result.punt_reason.is_some() {
        forget_batch_disposition(&head, shipped, punted, escalated, shelved, skipped);
        punted.push(head);
    } else if result.escalation.is_some() {
        forget_batch_disposition(&head, shipped, punted, escalated, shelved, skipped);
        escalated.push(head);
    } else {
        forget_batch_disposition(&head, shipped, punted, escalated, shelved, skipped);
        shipped.push(head);
    }
    None
}

/// A member whose terminal result arrives after another member's halt has
/// latched: the first halt keeps `stopped_at`, so a held, inconclusive or
/// un-shelvable result would otherwise appear in no summary list. Mismatched
/// and just-shelved results already recorded their member; `just_shelved`
/// says whether this result itself shelved it (not whether an earlier run of
/// the same spec did). Any earlier disposition of a requeued spec is
/// forgotten first, as every other recorder does (BUG-852).
// trace:TASK-1951 trace:TASK-1617 trace:TASK-2105 | ai:claude
#[allow(clippy::too_many_arguments)]
fn record_result_after_latched_halt(
    done: &BatchDrainResult,
    just_shelved: bool,
    shipped: &mut Vec<String>,
    punted: &mut Vec<String>,
    escalated: &mut Vec<String>,
    shelved: &mut Vec<String>,
    skipped: &mut Vec<(String, String)>,
) {
    if just_shelved {
        return;
    }
    let Some(head) = done.stopped_at.as_ref() else {
        return;
    };
    let reason = match &done.outcome {
        BatchDrainOutcome::NoChange => "no-change — nothing published".to_string(),
        BatchDrainOutcome::Held => "held after the drain halted — PR left open".to_string(),
        BatchDrainOutcome::Inconclusive => {
            "inconclusive after the drain halted — left for retry".to_string()
        }
        BatchDrainOutcome::Failed(phase) => {
            format!("{phase:?} failed after the drain halted — not shelved")
        }
        BatchDrainOutcome::EnvironmentBlocked { cause } => {
            format!("environment blocked ({cause}) after the drain halted")
        }
        _ => return,
    };
    forget_batch_disposition(head, shipped, punted, escalated, shelved, skipped);
    skipped.push((head.clone(), reason));
}

/// Bounded pipeline scheduler: start later implementer/CI children while the
/// parent serializes each ready member's review/merge/pull/build leg.
// trace:STORY-1091 trace:ADR-28 | ai:codex
pub(crate) fn drain_batch_pipelined_with_caps(
    driver: &mut dyn PipelinedBatchDriver,
    max: Option<usize>,
    max_failures: Option<usize>,
    caps: &crate::drain_caps::DrainCaps,
    start: std::time::Instant,
    cap_stop_out: &mut Option<crate::drain_caps::CapStop>,
) -> BatchDrainResult {
    let depth = driver.pipeline_depth().clamp(1, 3);
    if depth <= 1 {
        return drain_batch_with_caps(driver, max, max_failures, caps, start, cap_stop_out);
    }

    let mut shipped = Vec::new();
    let mut punted = Vec::new();
    let mut escalated = Vec::new();
    let mut shelved = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    // STORY-1429: monotonic shelve-event count for the failure budget.
    // trace:STORY-1429 | ai:claude
    let mut shelve_events = 0usize;
    let mut env_streak = EnvironmentStreak::default(); // trace:BUG-1930 | ai:claude
    let env_halt_after = driver.environment_halt_after();
    let mut in_flight: std::collections::VecDeque<InFlightMember> =
        std::collections::VecDeque::new();
    let mut launched = 0usize;
    let mut no_more_heads = false;

    struct LatchedHalt {
        stopped_at: Option<String>,
        outcome: BatchDrainOutcome,
        exit_code: i32,
        /// Why members still in flight skip their serial finish leg, or None
        /// when they finish normally. A stalled queue is labelled as a stall,
        /// not as a broken serial leg.
        // trace:TASK-1951 trace:TASK-1615 | ai:claude
        skip_in_flight: Option<&'static str>,
    }
    const SERIAL_LEG_BROKEN: &str = "serial leg broken — PR left open";
    const QUEUE_STALLED: &str = "drain stalled (queue re-offered a shipped spec) — PR left open";
    let mut halted: Option<LatchedHalt> = None;

    loop {
        while !no_more_heads && halted.is_none() && in_flight.len() < depth {
            if driver.stop_requested() {
                break;
            }
            // BUG-1930: an environmental streak is open and the members
            // already in flight could complete it. Let them finish first
            // rather than launch another spec into the same broken machine.
            // trace:BUG-1930 | ai:claude
            if env_streak.defers_launch(in_flight.len(), env_halt_after) {
                break;
            }
            if let Some(limit) = max {
                if launched >= limit {
                    break;
                }
            }
            if caps.is_active() {
                let counters = crate::drain_caps::DrainCounters {
                    tokens: caps
                        .max_tokens
                        .map(|_| driver.cumulative_tokens())
                        .unwrap_or(0),
                    iterations: launched as u64,
                    elapsed: start.elapsed(),
                };
                let stop = caps
                    .check_before_iteration(&counters)
                    .or_else(|| caps.check_tokens(&counters));
                if let Some(stop) = stop {
                    *cap_stop_out = Some(stop);
                    break;
                }
            }
            let Some(head) = driver
                .next_head_excluding(&skipped.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>())
            else {
                no_more_heads = true;
                break;
            };
            if in_flight.iter().any(|m| m.spec == head) {
                // trace:TASK-1603 | ai:codex
                // The child may not have bumped/leased this head yet. Wait
                // for progress, then select again; this is not exhaustion.
                break;
            }
            if shipped.iter().any(|s| s == &head) {
                shipped.retain(|s| s != &head);
                halted = Some(LatchedHalt {
                    stopped_at: Some(head),
                    outcome: BatchDrainOutcome::Stalled,
                    exit_code: 1,
                    skip_in_flight: Some(QUEUE_STALLED),
                });
                break;
            }
            let handle = driver.start_spec_through_ci(&head);
            launched += 1;
            in_flight.push_back(InFlightMember { spec: head, handle });
        }

        let Some(member) = in_flight.pop_front() else {
            if let Some(halt) = halted {
                return BatchDrainResult {
                    shipped,
                    punted,
                    escalated,
                    shelved,
                    skipped,
                    stopped_at: halt.stopped_at,
                    outcome: halt.outcome,
                    exit_code: halt.exit_code,
                };
            }
            if cap_stop_out.is_some()
                || driver.stop_requested()
                || (max.is_some()
                    && launched >= max.unwrap()
                    && !no_more_heads
                    && driver
                        .next_head_excluding(
                            &skipped.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
                        )
                        .is_some())
            {
                return BatchDrainResult {
                    shipped,
                    punted,
                    escalated,
                    shelved,
                    skipped,
                    stopped_at: None,
                    outcome: BatchDrainOutcome::MaxReached,
                    // A stop that came from SIGTERM is not a clean drain.
                    // trace:TASK-1518 | ai:claude
                    exit_code: crate::drain_signal::stop_exit_code(DRIVE_EXIT_CLEAN),
                };
            }
            let outcome = if shelved.is_empty() && skipped.is_empty() {
                BatchDrainOutcome::Drained
            } else {
                BatchDrainOutcome::DrainedWithShelved
            };
            let exit_code = if matches!(outcome, BatchDrainOutcome::DrainedWithShelved) {
                DRIVE_EXIT_SHELVED
            } else {
                DRIVE_EXIT_CLEAN
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

        let through_ci = driver.wait_spec_through_ci(member.handle);
        let should_finish_serial = through_ci.exit_code == 0
            && through_ci.inconclusive_reason.is_none()
            && through_ci.held_reason.is_none()
            && !through_ci.no_change
            && through_ci.shipped_spec_id.is_none()
            && through_ci.punt_reason.is_none()
            && through_ci.escalation.is_none();
        // TASK-1351: once a halt has latched, do not launch new specs, but
        // finish draining members already in flight through their review/merge
        // leg (unless the parent's serial leg itself broke un-shelvably, in
        // which case the member is recorded in `skipped`) so an in-flight child
        // is never left running without its serial finish.
        // trace:TASK-1351 | ai:antigravity
        let effective_max_failures = if halted.is_some() { None } else { max_failures };
        if !should_finish_serial {
            let env_halt = env_streak.observe(&through_ci, env_halt_after);
            let shelves_before = shelve_events;
            if let Some(done) = apply_batch_result(
                member.spec,
                through_ci,
                &mut shipped,
                &mut punted,
                &mut escalated,
                &mut shelved,
                &mut skipped,
                effective_max_failures,
                &mut shelve_events,
                env_halt,
            ) {
                if halted.is_some() {
                    record_result_after_latched_halt(
                        &done,
                        shelve_events > shelves_before,
                        &mut shipped,
                        &mut punted,
                        &mut escalated,
                        &mut shelved,
                        &mut skipped,
                    );
                } else {
                    halted = Some(LatchedHalt {
                        stopped_at: done.stopped_at,
                        outcome: done.outcome,
                        exit_code: done.exit_code,
                        skip_in_flight: None,
                    });
                }
            }
            continue;
        }
        if let Some(reason) = halted.as_ref().and_then(|h| h.skip_in_flight) {
            forget_batch_disposition(
                &member.spec,
                &mut shipped,
                &mut punted,
                &mut escalated,
                &mut shelved,
                &mut skipped,
            );
            skipped.push((member.spec, reason.to_string()));
            continue;
        }
        let final_result = driver.finish_spec_after_ci(&member.spec);
        let serial_unshelved_fail =
            final_result.exit_code != 0 && final_result.shelved_reason.is_none();
        let env_halt = env_streak.observe(&final_result, env_halt_after);
        let shelves_before = shelve_events;
        if let Some(done) = apply_batch_result(
            member.spec,
            final_result,
            &mut shipped,
            &mut punted,
            &mut escalated,
            &mut shelved,
            &mut skipped,
            effective_max_failures,
            &mut shelve_events,
            env_halt,
        ) {
            if let Some(existing) = halted.as_mut() {
                record_result_after_latched_halt(
                    &done,
                    shelve_events > shelves_before,
                    &mut shipped,
                    &mut punted,
                    &mut escalated,
                    &mut shelved,
                    &mut skipped,
                );
                if serial_unshelved_fail {
                    existing.skip_in_flight = Some(SERIAL_LEG_BROKEN);
                }
            } else {
                halted = Some(LatchedHalt {
                    stopped_at: done.stopped_at,
                    outcome: done.outcome,
                    exit_code: done.exit_code,
                    skip_in_flight: serial_unshelved_fail.then_some(SERIAL_LEG_BROKEN),
                });
            }
        }
    }
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
#[allow(dead_code)] // uncapped entry: production callers moved to drain_batch_with_caps; kept as the test/doc anchor
pub(crate) fn drain_batch(
    driver: &mut dyn BatchDriver,
    max: Option<usize>,
    max_failures: Option<usize>,
) -> BatchDrainResult {
    // Default entry: no budget caps. Existing callers + tests are unchanged.
    // trace:TASK-966 | ai:claude
    let mut cap_stop = None;
    drain_batch_with_caps(
        driver,
        max,
        max_failures,
        &crate::drain_caps::DrainCaps::default(),
        std::time::Instant::now(),
        &mut cap_stop,
    )
}

/// EPIC-28 [`drain_batch`] with TASK-966 hard budget caps layered on. The
/// `caps` (`--max-tokens` / `--max-iterations` / `--max-runtime`) are checked at
/// each spec boundary; when one fires, `cap_stop_out` is set to the reason and
/// the drain returns a clean [`BatchDrainOutcome::MaxReached`] (the same "a
/// configured limit stopped us early" family as `--max`). The caps compose with
/// `--max-failures`: whichever stop condition is reached first wins. `start` is
/// the drain's wall-clock origin, injected so the runtime check is deterministic
/// under test.
// trace:TASK-966 | ai:claude
pub(crate) fn drain_batch_with_caps(
    driver: &mut dyn BatchDriver,
    max: Option<usize>,
    max_failures: Option<usize>,
    caps: &crate::drain_caps::DrainCaps,
    start: std::time::Instant,
    cap_stop_out: &mut Option<crate::drain_caps::CapStop>,
) -> BatchDrainResult {
    drain_batch_with_caps_streak(
        driver,
        max,
        max_failures,
        caps,
        start,
        cap_stop_out,
        &mut EnvironmentStreak::default(),
    )
}

/// BUG-1930: [`drain_batch_with_caps`] with a caller-owned environment streak,
/// so a `--batches` chain counts consecutive environmental failures across
/// batch boundaries instead of restarting the count in every batch.
// trace:BUG-1930 | ai:claude
fn drain_batch_with_caps_streak(
    driver: &mut dyn BatchDriver,
    max: Option<usize>,
    max_failures: Option<usize>,
    caps: &crate::drain_caps::DrainCaps,
    start: std::time::Instant,
    cap_stop_out: &mut Option<crate::drain_caps::CapStop>,
    env_streak: &mut EnvironmentStreak,
) -> BatchDrainResult {
    let mut shipped: Vec<String> = Vec::new();
    let mut punted: Vec<String> = Vec::new();
    let mut escalated: Vec<String> = Vec::new();
    let mut shelved: Vec<String> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    let mut acted = 0usize;
    // STORY-1429: monotonic shelve-event count for the failure budget; the
    // `shelved` list keeps reporting final dispositions only.
    // trace:STORY-1429 | ai:claude
    let mut shelve_events = 0usize;
    let env_halt_after = driver.environment_halt_after();
    loop {
        // Resolve the head first: a `--max` of exactly the batch size should
        // report `Drained` (the batch genuinely emptied), not `MaxReached`.
        let Some(head) = driver
            .next_head_excluding(&skipped.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>())
        else {
            // EPIC-28: clean drain or drained-with-shelved. The latter
            // wins whenever any failure shelved a spec, regardless of how
            // many independents shipped after — the operator still has to
            // triage the parked set.
            let outcome = if shelved.is_empty() && skipped.is_empty() {
                BatchDrainOutcome::Drained
            } else {
                BatchDrainOutcome::DrainedWithShelved
            };
            // TASK-1054: a fully-drained batch that shelved/skipped ≥1 member is
            // the preserved EPIC-28 `2` sentinel; a genuinely-clean drain is `0`.
            let exit_code = if matches!(outcome, BatchDrainOutcome::DrainedWithShelved) {
                DRIVE_EXIT_SHELVED
            } else {
                DRIVE_EXIT_CLEAN
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
        // STORY-1130: `aida drain stop` is cooperative: let the in-flight spec
        // land, then stop before picking up another head.
        if driver.stop_requested() {
            return BatchDrainResult {
                shipped,
                punted,
                escalated,
                shelved,
                skipped,
                stopped_at: None,
                outcome: BatchDrainOutcome::MaxReached,
                // trace:TASK-1518 | ai:claude
                exit_code: crate::drain_signal::stop_exit_code(DRIVE_EXIT_CLEAN),
            };
        }
        // `--max` bounds how many members the drain *acts on* — shipped,
        // punted, escalated, or shelved each consumed a slot (a full phase
        // attempt).
        if let Some(limit) = max {
            if acted >= limit {
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
        // TASK-966: hard budget caps, checked once the queue still has a head
        // (so a genuinely-drained batch reports `Drained`, not a cap stop) but
        // BEFORE this head is run — `--max-iterations` / `--max-runtime` stop
        // *between* specs, and `--max-tokens` stops once the prior phases'
        // accumulated reported tokens crossed the cap. A cap stop is a clean
        // intentional stop: the in-flight head stays queued for the next drain.
        // trace:TASK-966 | ai:claude
        if caps.is_active() {
            let counters = crate::drain_caps::DrainCounters {
                tokens: caps
                    .max_tokens
                    .map(|_| driver.cumulative_tokens())
                    .unwrap_or(0),
                iterations: acted as u64,
                elapsed: start.elapsed(),
            };
            let stop = caps
                .check_before_iteration(&counters)
                .or_else(|| caps.check_tokens(&counters));
            if let Some(stop) = stop {
                *cap_stop_out = Some(stop);
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
        acted += 1;
        let env_halt = env_streak.observe(&result, env_halt_after);
        if result.exit_code != 0 {
            let phase = result.failed_phase.unwrap_or(Phase::Implementer);
            // EPIC-28: a shelvable failure parks the spec and the drain
            // continues — unless we have already shelved `max_failures`
            // specs (probably a broken environment) in which case fall
            // back to the historical `Failed` stop.
            // trace:EPIC-28 | ai:claude
            let over_failure_budget = max_failures
                .map(|cap| shelve_events + 1 > cap)
                .unwrap_or(false);
            if result.shelved_reason.is_some() && !over_failure_budget {
                forget_batch_disposition(
                    &head,
                    &mut shipped,
                    &mut punted,
                    &mut escalated,
                    &mut shelved,
                    &mut skipped,
                );
                shelved.push(head.clone());
                shelve_events += 1;
                if let Some(cause) = env_halt {
                    // trace:BUG-1930 | ai:claude
                    return BatchDrainResult {
                        shipped,
                        punted,
                        escalated,
                        shelved,
                        skipped,
                        stopped_at: Some(head),
                        outcome: BatchDrainOutcome::EnvironmentBlocked { cause },
                        exit_code: DRIVE_EXIT_HARD_FAIL,
                    };
                }
                // BUG-1608: stop as soon as the Nth shelve lands — never pick
                // (let alone run) another head on a spent failure budget.
                // trace:BUG-1608 | ai:claude
                if failure_budget_exhausted(shelve_events, max_failures) {
                    return BatchDrainResult {
                        shipped,
                        punted,
                        escalated,
                        shelved,
                        skipped,
                        stopped_at: Some(head),
                        outcome: BatchDrainOutcome::Failed(phase),
                        exit_code: DRIVE_EXIT_HARD_FAIL,
                    };
                }
                continue;
            }
            return BatchDrainResult {
                shipped,
                punted,
                escalated,
                shelved,
                skipped,
                stopped_at: Some(head),
                // BUG-1930: an un-shelvable stop that completes an
                // environmental streak still reports the environment.
                // trace:BUG-1930 | ai:claude
                outcome: env_halt.map_or(BatchDrainOutcome::Failed(phase), |cause| {
                    BatchDrainOutcome::EnvironmentBlocked { cause }
                }),
                // TASK-1054: a batch that hard-stopped on a failure (un-shelvable,
                // or over the `--max-failures` budget) is the canonical hard
                // failure — `3`, NOT the failed-phase index (which collided with
                // the EPIC-28 `2 = shelved` sentinel when the failed phase was CI).
                // The shelved-and-advanced case keeps exiting `2` via the
                // `DrainedWithShelved` branch above. trace:TASK-1054 | ai:claude
                exit_code: DRIVE_EXIT_HARD_FAIL,
            };
        }
        // trace:STORY-2640 | ai:codex — leave the repair row for a fresh pickup.
        if result.fast_gate_pending {
            skipped.push((
                head,
                result
                    .inconclusive_reason
                    .unwrap_or_else(|| "fast gate repair pending".into()),
            ));
            continue;
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
        if result.held_reason.is_some() || result.no_change {
            return BatchDrainResult {
                shipped,
                punted,
                escalated,
                shelved,
                skipped,
                stopped_at: Some(head),
                outcome: if result.no_change {
                    BatchDrainOutcome::NoChange
                } else {
                    BatchDrainOutcome::Held
                },
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
            forget_batch_disposition(
                &actual,
                &mut shipped,
                &mut punted,
                &mut escalated,
                &mut shelved,
                &mut skipped,
            );
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
            forget_batch_disposition(
                &head,
                &mut shipped,
                &mut punted,
                &mut escalated,
                &mut shelved,
                &mut skipped,
            );
            punted.push(head);
        } else if result.escalation.is_some() {
            forget_batch_disposition(
                &head,
                &mut shipped,
                &mut punted,
                &mut escalated,
                &mut shelved,
                &mut skipped,
            );
            escalated.push(head);
        } else {
            forget_batch_disposition(
                &head,
                &mut shipped,
                &mut punted,
                &mut escalated,
                &mut shelved,
                &mut skipped,
            );
            shipped.push(head);
        }
    }
}

// --- Single-branch coupled-sequential drain (TASK-1003 / SPIKE-70) ----------
//
// `aida queue work --batch NAME --auto-complete --single-branch` drives a set
// of TIGHTLY-COUPLED batch members on ONE shared feature branch in ONE
// worktree. Each member is implemented + CI'd and committed IN PLACE — no reset
// between members, no per-member merge-to-main — so commits ACCUMULATE on the
// shared branch; then ONE cluster PR is opened at the end linking every member
// SPEC-ID. Because each member commits with its own `(SPEC-ID)` trailer, the
// merge of that one PR lets the existing `aida pull` Done→Completed scan credit
// EVERY member, not just one.
//
// This is the genuinely-missing capability vs [`drain_batch`] (which merges each
// member to main as its own PR). For coupled work where later increments build
// on earlier commits (EPIC-54's TUI children), the failure rule INVERTS: a
// member failure HALTS the drain — prior members' commits stay on the branch,
// the failed member is parked `NeedsAttention` — rather than EPIC-28's
// shelve-and-continue, because continuing would stack later commits on broken
// code. (EPIC-28's shelve→skip-dependents→continue is correct only when members
// are INDEPENDENT — the `drain_batch` / parallel case.)
//
// Pure sequencing behind a [`SingleBranchDriver`] — the same testability shape
// as [`BatchDriver`]. The I/O (the one worktree+branch creation, the per-member
// Implementer+CI run, the cluster Reviewer/Merge/PR) lives in the real driver
// in `main.rs`. trace:TASK-1003 SPIKE-70 | ai:claude

/// Why a [`drain_batch_single_branch`] run stopped. trace:TASK-1003 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SingleBranchOutcome {
    /// No member was pickable — the batch was empty for the role, so nothing
    /// was driven and no branch/PR was created.
    Empty,
    /// Every member committed on the shared branch and ONE cluster PR opened
    /// (or, in a `through-ci` variant, every member committed and CI is green).
    Clustered,
    /// A member's Implementer/CI phase failed — the drain HALTED. Prior
    /// members' commits are intact on the shared branch; the failed member is
    /// parked `NeedsAttention`. The [`Phase`] is the phase that failed.
    Halted(Phase),
    /// A member reported success but the queue head did not advance — stopped
    /// to avoid looping forever on the same spec (mirrors [`BatchDrainOutcome::Stalled`]).
    Stalled,
    /// The cluster Reviewer/Merge phase failed AFTER every member committed.
    /// The accumulated branch is intact; only the cluster wrap-up failed.
    ClusterFailed(Phase),
    /// The per-increment checkpoint (the `--zen` pause) asked to stop before
    /// the next member started. Prior commits are intact; the remaining members
    /// were not begun. A clean stop (exit `0`).
    Paused,
}

/// Outcome of a [`drain_batch_single_branch`] run — which members landed on the
/// shared branch, which members the one cluster PR links, where it stopped, and
/// the process exit code. trace:TASK-1003 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct SingleBranchResult {
    /// Member spec-ids committed to the shared branch, in drain order. On a
    /// `Halted` / `Paused` / `Stalled` stop this holds the members that DID
    /// commit before the stop (their work is kept on the branch).
    pub(crate) committed: Vec<String>,
    /// The member spec-ids the ONE cluster PR links — equal to `committed` once
    /// the cluster finish ran, empty on an `Empty` / `Halted` / `Paused` /
    /// `Stalled` stop (no PR was opened). The cluster merge's Done→Completed
    /// auto-bump must credit every id in this list. trace:TASK-1003
    pub(crate) cluster_members: Vec<String>,
    /// The member the drain stopped on — set for `Halted` / `Stalled`, `None`
    /// for a clean `Clustered` / `Empty` / `ClusterFailed`.
    pub(crate) stopped_at: Option<String>,
    /// Why the drain stopped.
    pub(crate) outcome: SingleBranchOutcome,
    /// Process exit code: `0` on a clean cluster / empty / paused stop, else the
    /// failed-phase index (per STORY-246's exit codes) or `1` for a stall.
    pub(crate) exit_code: i32,
}

/// Drives a single-branch coupled-sequential drain: yields the next member and
/// runs it through Implementer+CI ONLY (committing in place on the shared
/// branch), then runs the cluster Reviewer/Merge ONCE at the end. The real
/// implementation re-resolves the `batch:NAME` head against the queue and drives
/// the shared worktree; the mock stands in for both so the loop is testable.
/// trace:TASK-1003 | ai:claude
pub(crate) trait SingleBranchDriver {
    /// The current batch head spec-id, or `None` when every member has been
    /// driven. Re-resolved each call — a member marked Done leaves the queue, so
    /// the head advances naturally (the same mechanism [`BatchDriver`] uses).
    fn next_head(&mut self) -> Option<String>;

    /// Run ONE member through phases 1-2 (Implementer + CI) ONLY, committing IN
    /// PLACE on the shared branch — NO per-member PR, NO merge, NO reset. On a
    /// clean run the member is marked Done and its commit stays on the branch. A
    /// non-zero `exit_code` (or any non-ship outcome — punt / escalation /
    /// inconclusive / hold / shipped-mismatch) means this member did not cleanly
    /// land, so the coupled drain HALTS rather than stacking later members on a
    /// branch missing this one's work. The driver parks a failed member
    /// `NeedsAttention`.
    fn run_member_through_ci(&mut self, spec: &str) -> OrchestrationResult;

    /// After EVERY member committed: run the Reviewer + Merge phases ONCE for
    /// the whole cluster and open ONE PR linking `members`. Returns the cluster
    /// [`OrchestrationResult`]; a non-zero exit means the cluster wrap-up failed
    /// (the accumulated branch is left intact for triage).
    fn run_cluster_finish(&mut self, members: &[String]) -> OrchestrationResult;

    /// Per-increment checkpoint, called BETWEEN members (after the first). The
    /// real driver honors the `--zen` pause (operator validates the increment
    /// before the next) / the `--no-human=both` auto-continue. Return `true` to
    /// proceed to `next`, `false` to stop the drain with prior commits intact.
    /// Default `true` (no checkpoint) so the pure-logic tests need not implement
    /// it. trace:TASK-1003 | ai:claude
    fn checkpoint_between_members(&mut self, _prev: &str, _next: &str) -> bool {
        true
    }
}

/// What the per-member checkpoint of a `--single-branch` drain should do between
/// two members.
// trace:TASK-1137 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckpointAction {
    /// Ask the operator to validate the increment that just landed on the shared
    /// branch before the next member stacks commits on top of it.
    Prompt,
    /// Continue without asking — there is no human at the keyboard to answer.
    AutoContinue,
}

/// Decide whether the per-member checkpoint pauses or auto-continues.
///
/// The single-branch drain is the one shape where a bad member poisons every
/// member after it: later members commit ON TOP of earlier ones on the SAME
/// branch, with no reset and no per-member merge. So whenever a human is
/// actually there to answer, the checkpoint pauses and lets them inspect the
/// increment before the next builds on it — that includes the plain interactive
/// drain, `--zen` (the operator is at the keyboard; this is a substantive
/// validation gate, not one of the mechanical confirmations zen waves through),
/// and `--no-human=reviewer-only` (only the reviewer phase is headless).
///
/// It auto-continues in exactly the two cases where a prompt would hang forever
/// with nobody to answer it: `--no-human=both` (fully headless — the implementer
/// runs headless too), and any run without a terminal on both stdin and stdout.
/// That non-TTY guard is the load-bearing one: it holds even if a future caller
/// forgets to thread the autonomy mode through.
// trace:TASK-1137 | ai:claude
pub(crate) fn single_branch_checkpoint_action(
    no_human: Option<NoHumanMode>,
    interactive: bool,
) -> CheckpointAction {
    if no_human == Some(NoHumanMode::Both) || !interactive {
        return CheckpointAction::AutoContinue;
    }
    CheckpointAction::Prompt
}

/// Drive a single-branch coupled-sequential drain: loop members in queue order,
/// running each through Implementer+CI and committing in place on ONE shared
/// branch, then run the cluster Reviewer/Merge ONCE → ONE PR. HALT on the first
/// member that does not cleanly land (prior commits kept; failed member parked).
/// `max` caps how many members the drain commits. Pure sequencing; the I/O lives
/// in the [`SingleBranchDriver`]. trace:TASK-1003 SPIKE-70 | ai:claude
pub(crate) fn drain_batch_single_branch(
    driver: &mut dyn SingleBranchDriver,
    max: Option<usize>,
) -> SingleBranchResult {
    let mut committed: Vec<String> = Vec::new();
    loop {
        let Some(head) = driver.next_head() else {
            // The batch drained — every member committed. Fall through to the
            // cluster finish below.
            break;
        };
        // `--max` bounds how many members the single-branch drain accumulates.
        if let Some(limit) = max {
            if committed.len() >= limit {
                break;
            }
        }
        // Stall guard: a member we already committed resurfacing as the head
        // means the queue did not advance (its run reported success but it never
        // left the queue). Stop rather than loop forever on the same spec.
        if committed.iter().any(|s| s == &head) {
            return SingleBranchResult {
                committed,
                cluster_members: Vec::new(),
                stopped_at: Some(head),
                outcome: SingleBranchOutcome::Stalled,
                exit_code: 1,
            };
        }
        // Per-increment checkpoint BEFORE running the next member (after the
        // first commit): honor the `--zen` pause / `--no-human` auto-continue.
        // A stop request parks the drain with prior commits intact — NEVER
        // discards accumulated work.
        if let Some(prev) = committed.last() {
            if !driver.checkpoint_between_members(prev, &head) {
                return SingleBranchResult {
                    committed,
                    cluster_members: Vec::new(),
                    stopped_at: Some(head),
                    outcome: SingleBranchOutcome::Paused,
                    exit_code: 0,
                };
            }
        }
        let result = driver.run_member_through_ci(&head);
        // trace:TASK-3608 | ai:codex — nothing to stack or credit as committed.
        if result.no_change {
            return SingleBranchResult {
                committed,
                cluster_members: Vec::new(),
                stopped_at: Some(head),
                outcome: SingleBranchOutcome::Paused,
                exit_code: 0,
            };
        }
        // HALT-on-failure: a phase failure, OR any non-ship outcome (punt /
        // escalation / inconclusive / hold / shipped-mismatch), means this
        // member did not cleanly land on the shared branch. In a coupled
        // single-branch drain we cannot stack later members on a branch missing
        // this one's work, so STOP — prior commits stay on the branch, the
        // failed member is parked `NeedsAttention` by the driver. This is the
        // deliberate inverse of EPIC-28's shelve-and-continue (correct only for
        // INDEPENDENT members). trace:TASK-1003 | ai:claude
        // trace:TASK-3749 | ai:codex
        let unclean = result.punt_reason.is_some()
            || result.escalation.is_some()
            || result.inconclusive_reason.is_some()
            || result.held_reason.is_some()
            || result.shipped_spec_id.is_some();
        if result.exit_code != 0 || unclean {
            let phase = result.failed_phase.unwrap_or(Phase::Implementer);
            // A clean-but-unclean stop (punt/hold/etc, exit 0) still halts the
            // coupled drain; surface a non-zero exit so scripts notice.
            let exit_code = if result.exit_code != 0 {
                result.exit_code
            } else {
                phase.index()
            };
            return SingleBranchResult {
                committed,
                cluster_members: Vec::new(),
                stopped_at: Some(head),
                outcome: SingleBranchOutcome::Halted(phase),
                exit_code,
            };
        }
        committed.push(head);
    }
    // Nothing committed → nothing to cluster (empty batch, or `--max 0`).
    if committed.is_empty() {
        return SingleBranchResult {
            committed,
            cluster_members: Vec::new(),
            stopped_at: None,
            outcome: SingleBranchOutcome::Empty,
            exit_code: 0,
        };
    }
    // Every member committed on the shared branch — run Reviewer/Merge ONCE for
    // the whole cluster → ONE PR linking every member. trace:TASK-1003
    let cluster = driver.run_cluster_finish(&committed);
    if cluster.exit_code != 0 {
        let phase = cluster.failed_phase.unwrap_or(Phase::Reviewer);
        return SingleBranchResult {
            cluster_members: committed.clone(),
            committed,
            stopped_at: None,
            outcome: SingleBranchOutcome::ClusterFailed(phase),
            exit_code: cluster.exit_code,
        };
    }
    SingleBranchResult {
        cluster_members: committed.clone(),
        committed,
        stopped_at: None,
        outcome: SingleBranchOutcome::Clustered,
        exit_code: 0,
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
#[allow(dead_code)] // uncapped entry: production callers moved to drain_batch_chain_with_caps; kept as the test/doc anchor
pub(crate) fn drain_batch_chain<'a, F>(
    batch_names: &[String],
    max: Option<usize>,
    max_failures: Option<usize>,
    make_driver: F,
) -> BatchChainDrainResult
where
    F: FnMut(&str) -> Box<dyn BatchDriver + 'a>,
{
    // Default entry: no budget caps. Existing callers + tests unchanged.
    // trace:TASK-966 | ai:claude
    let mut cap_stop = None;
    drain_batch_chain_with_caps(
        batch_names,
        max,
        max_failures,
        &crate::drain_caps::DrainCaps::default(),
        std::time::Instant::now(),
        &mut cap_stop,
        make_driver,
    )
}

/// [`drain_batch_chain`] with TASK-966 hard budget caps threaded across the
/// whole chain. `--max-runtime` and `--max-tokens` are cumulative across batches
/// (a shared `start` and the global headless-log token meter respectively);
/// `--max-iterations` is carried forward by shrinking each batch's iteration
/// budget by the specs already acted on. A cap stop in any batch sets
/// `cap_stop_out` and ends the chain with a clean [`BatchDrainOutcome::MaxReached`].
// trace:TASK-966 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn drain_batch_chain_with_caps<'a, F>(
    batch_names: &[String],
    max: Option<usize>,
    max_failures: Option<usize>,
    caps: &crate::drain_caps::DrainCaps,
    start: std::time::Instant,
    cap_stop_out: &mut Option<crate::drain_caps::CapStop>,
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
    // BUG-1930: one streak for the whole chain — a denied spec at the end of
    // one batch and another at the start of the next are still consecutive.
    // trace:BUG-1930 | ai:claude
    let mut env_streak = EnvironmentStreak::default();

    for batch_name in batch_names {
        let consumed = shipped.len() + punted.len() + escalated.len() + shelved.len();
        let remaining = max.map(|limit| limit.saturating_sub(consumed));
        // TASK-966: carry the iteration budget forward — each batch sees only
        // the chain-wide `--max-iterations` minus what prior batches consumed.
        // `--max-tokens` (global log meter) and `--max-runtime` (shared `start`)
        // are already cumulative, so they pass through unchanged.
        let batch_caps = crate::drain_caps::DrainCaps {
            max_tokens: caps.max_tokens,
            max_iterations: caps
                .max_iterations
                .map(|c| c.saturating_sub(consumed as u64)),
            max_runtime: caps.max_runtime,
        };
        let mut batch_cap_stop = None;
        let mut driver = make_driver(batch_name);
        let result = drain_batch_with_caps_streak(
            driver.as_mut(),
            remaining,
            max_failures,
            &batch_caps,
            start,
            &mut batch_cap_stop,
            &mut env_streak,
        );

        for spec in &result.shipped {
            forget_batch_disposition(
                spec,
                &mut shipped,
                &mut punted,
                &mut escalated,
                &mut shelved,
                &mut skipped,
            );
            shipped.push(spec.clone());
        }
        for spec in &result.punted {
            forget_batch_disposition(
                spec,
                &mut shipped,
                &mut punted,
                &mut escalated,
                &mut shelved,
                &mut skipped,
            );
            punted.push(spec.clone());
        }
        for spec in &result.escalated {
            forget_batch_disposition(
                spec,
                &mut shipped,
                &mut punted,
                &mut escalated,
                &mut shelved,
                &mut skipped,
            );
            escalated.push(spec.clone());
        }
        for spec in &result.shelved {
            forget_batch_disposition(
                spec,
                &mut shipped,
                &mut punted,
                &mut escalated,
                &mut shelved,
                &mut skipped,
            );
            shelved.push(spec.clone());
        }
        skipped.extend(result.skipped.iter().cloned());

        let step = BatchChainStep {
            batch_name: batch_name.clone(),
            result: result.clone(),
        };
        steps.push(step);

        // TASK-966: a budget cap fired inside this batch — end the chain with a
        // clean MaxReached and surface the reason. trace:TASK-966 | ai:claude
        if let Some(stop) = batch_cap_stop {
            *cap_stop_out = Some(stop);
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
                exit_code: 0,
            };
        }

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
            // trace:BUG-1930 | ai:claude
            outcome @ BatchDrainOutcome::EnvironmentBlocked { .. } => {
                return BatchChainDrainResult {
                    steps,
                    shipped,
                    punted,
                    escalated,
                    shelved,
                    skipped,
                    stopped_batch: Some(batch_name.clone()),
                    stopped_at: result.stopped_at,
                    outcome,
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
            outcome @ (BatchDrainOutcome::Held | BatchDrainOutcome::NoChange) => {
                return BatchChainDrainResult {
                    steps,
                    shipped,
                    punted,
                    escalated,
                    shelved,
                    skipped,
                    stopped_batch: Some(batch_name.clone()),
                    stopped_at: result.stopped_at,
                    outcome,
                    exit_code: result.exit_code,
                };
            }
        }
    }

    // Every batch drained cleanly. EPIC-28: if any individual batch reported
    // a final shelve/skip, the chain summary is `DrainedWithShelved` (exit 2)
    // so the operator still sees the parked set. A retried-then-shipped spec is
    // no longer parked and therefore returns to the clean `Drained` path.
    // trace:BUG-852 | ai:codex
    let (outcome, exit_code) = if !shelved.is_empty() || !skipped.is_empty() {
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
#[path = "tests/auto_complete_tests.rs"]
mod tests;
