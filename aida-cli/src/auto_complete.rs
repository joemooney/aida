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
            Self::Failed => "failed",
        }
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
}

/// The outcome of phase 1 — the implementer session. The implementer either
/// opens a PR (the normal path), or, under a headless `--no-human=both`
/// drain, hits a design-fork it cannot safely resolve and invokes
/// `/aida-punt`, which parks the spec in `NeedsAttention`. A punt is neither a
/// failure (nothing broke) nor a ship (no PR), so it is a first-class third
/// outcome of phase 1: the orchestrator stops the pipeline cleanly and a
/// batch drain advances to the next member. trace:STORY-276 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ImplementerOutcome {
    /// The implementer opened a PR — the pipeline continues to CI (phase 2).
    PrOpened,
    /// The implementer punted on a design-fork. `reason` is the one-line punt
    /// summary (category + detail) surfaced in the run epilogue. The punt is
    /// already durably recorded by `aida punt` (status flip + ledger).
    Punted { reason: String },
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
    fn run_reviewer(&mut self) -> Result<Verdict, PhaseFailure>;
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
                Phase::Merge => format!("gh pr merge {pr} --squash --delete-branch"),
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
            "`gh` is not on PATH — auto-complete needs the GitHub CLI to track the PR. \
             Install it (https://cli.github.com), then re-run."
                .to_string()
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
            let view = ctx
                .ci_run_id
                .as_deref()
                .map(|r| format!("gh run view {r}"))
                .unwrap_or_else(|| format!("gh run list --branch {branch}"));
            let run = ctx.ci_run_id.as_deref().unwrap_or("<ID>");
            format!(
                "CI failed on run {run} — view it: `{view}`. Push fixups to the same \
                 branch: `aida queue work {spec} --branch {branch} --steal`"
            )
        }
        (Phase::Ci, FailureKind::CiTimeout) => format!(
            "CI never reached a terminal state in the wait window — it may be queued or \
             a runner is slow, CI is not red. Check progress with \
             `gh run list --branch {branch}`, then re-run auto-complete once CI settles."
        ),
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
            "`gh` is not on PATH — auto-complete needs the GitHub CLI to merge the PR. \
             Install it (https://cli.github.com), then merge the PR manually."
                .to_string()
        }
        (Phase::Merge, _) => format!("Investigate the merge failure: `gh pr view {pr}`"),

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
    }
}

/// Print the failure epilogue (reason + recovery hint) and build the
/// failure [`OrchestrationResult`] (exit code = the phase's 1-based index).
fn finish_failure(
    driver: &dyn PhaseDriver,
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
    match driver.reconcile_failure(phase, failure) {
        PhaseReconcile::ShippedOutOfBand { reason } => {
            finish_reconciled(spec, json, start, durations, phase, &reason)
        }
        PhaseReconcile::GenuineFailure => {
            finish_failure(driver, phase, spec, json, start, failure, durations)
        }
    }
}

/// Drive the phases in order, stopping at the variant's last phase or at the
/// first failure. Returns an [`OrchestrationResult`] — the process exit code
/// (`0` on success, else the 1-based index of the phase that failed) plus
/// the per-phase timing the telemetry layer (TASK-266) records.
/// trace:STORY-246 | ai:claude
/// trace:TASK-266 | ai:claude
pub(crate) fn orchestrate(
    driver: &mut dyn PhaseDriver,
    spec: &str,
    variant: AutoCompleteVariant,
    json: bool,
) -> OrchestrationResult {
    let start = Instant::now();
    // Per-phase wall time, captured as each phase runs so a failure carries
    // the timing of the phases that did complete. trace:TASK-266 | ai:claude
    let mut durations: Vec<(Phase, u128)> = Vec::new();
    if !json {
        eprintln!();
        eprintln!(
            "{} {} {}",
            "🚀".bold(),
            format!("auto-complete: {spec}").bold(),
            format!("({})", variant.describe()).dimmed()
        );
    }

    // Phase 1 — implementer session. Three outcomes: a PR was opened (run on),
    // a genuine failure (reconcile then report), or a punt — a headless
    // implementer hit a design-fork and parked the spec; the pipeline stops
    // here cleanly, not as a failure. trace:STORY-276 | ai:claude
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
            return finish_punted(spec, json, &start, durations, &reason);
        }
        Ok(ImplementerOutcome::PrOpened) => {}
    }
    durations.push((Phase::Implementer, phase_start.elapsed().as_millis()));
    emit_done(Phase::Implementer, spec, json, start.elapsed().as_millis());

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
    let credited = driver
        .shipped_spec_id()
        .filter(|id| id != spec)
        .unwrap_or_else(|| spec.to_string());
    if credited != spec {
        emit_shipped_mismatch(spec, &credited, json, start.elapsed().as_millis());
    }

    // Phase 2 — end + wait for CI.
    emit_start(Phase::Ci, spec, json, start.elapsed().as_millis());
    let phase_start = Instant::now();
    if let Err(f) = driver.finish_ci() {
        durations.push((Phase::Ci, phase_start.elapsed().as_millis()));
        return resolve_phase_failure(driver, Phase::Ci, spec, json, &start, &f, durations);
    }
    durations.push((Phase::Ci, phase_start.elapsed().as_millis()));
    emit_done(Phase::Ci, spec, json, start.elapsed().as_millis());
    if variant.last_phase() <= 2 {
        return finish_success(spec, &credited, json, &start, durations);
    }

    // Phase 3 — reviewer session.
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
        Ok(verdict) if verdict != Verdict::Approved => {
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
        Ok(_) => {}
    }
    durations.push((Phase::Reviewer, phase_start.elapsed().as_millis()));
    emit_done(Phase::Reviewer, spec, json, start.elapsed().as_millis());

    // Phase 4 — merge.
    emit_start(Phase::Merge, spec, json, start.elapsed().as_millis());
    let phase_start = Instant::now();
    if let Err(f) = driver.merge() {
        durations.push((Phase::Merge, phase_start.elapsed().as_millis()));
        return resolve_phase_failure(driver, Phase::Merge, spec, json, &start, &f, durations);
    }
    durations.push((Phase::Merge, phase_start.elapsed().as_millis()));
    emit_done(Phase::Merge, spec, json, start.elapsed().as_millis());
    if variant.last_phase() <= 4 {
        return finish_success(spec, &credited, json, &start, durations);
    }

    // Phase 5 — pull + auto-bump.
    emit_start(Phase::Pull, spec, json, start.elapsed().as_millis());
    let phase_start = Instant::now();
    if let Err(f) = driver.pull() {
        durations.push((Phase::Pull, phase_start.elapsed().as_millis()));
        return resolve_phase_failure(driver, Phase::Pull, spec, json, &start, &f, durations);
    }
    durations.push((Phase::Pull, phase_start.elapsed().as_millis()));
    emit_done(Phase::Pull, spec, json, start.elapsed().as_millis());
    if variant.last_phase() <= 5 {
        return finish_success(spec, &credited, json, &start, durations);
    }

    // Phase 6 — build verify.
    emit_start(Phase::Build, spec, json, start.elapsed().as_millis());
    let phase_start = Instant::now();
    if let Err(f) = driver.build() {
        durations.push((Phase::Build, phase_start.elapsed().as_millis()));
        return resolve_phase_failure(driver, Phase::Build, spec, json, &start, &f, durations);
    }
    durations.push((Phase::Build, phase_start.elapsed().as_millis()));
    emit_done(Phase::Build, spec, json, start.elapsed().as_millis());

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
    /// The spec the drain stopped on — set for `Failed` / `Stalled`, `None`
    /// for a clean `Drained` / `MaxReached`.
    pub(crate) stopped_at: Option<String>,
    /// Why the drain stopped.
    pub(crate) outcome: BatchDrainOutcome,
    /// Process exit code: `0` on a clean stop, else the failed-phase index
    /// (per STORY-246's exit codes) or `1` for a stall.
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
/// `--max` cap is hit, or a phase fails. A phase failure stops the drain at
/// that spec — the remaining members stay queued, intact for a retry. Pure
/// sequencing; the I/O lives in the [`BatchDriver`]. trace:TASK-285 | ai:claude
pub(crate) fn drain_batch(driver: &mut dyn BatchDriver, max: Option<usize>) -> BatchDrainResult {
    let mut shipped: Vec<String> = Vec::new();
    let mut punted: Vec<String> = Vec::new();
    loop {
        // Resolve the head first: a `--max` of exactly the batch size should
        // report `Drained` (the batch genuinely emptied), not `MaxReached`.
        let Some(head) = driver.next_head() else {
            return BatchDrainResult {
                shipped,
                punted,
                stopped_at: None,
                outcome: BatchDrainOutcome::Drained,
                exit_code: 0,
            };
        };
        // `--max` bounds how many members the drain *acts on* — shipped or
        // punted, both consume a slot (each ran a full phase-1 attempt).
        if let Some(limit) = max {
            if shipped.len() + punted.len() >= limit {
                return BatchDrainResult {
                    shipped,
                    punted,
                    stopped_at: None,
                    outcome: BatchDrainOutcome::MaxReached,
                    exit_code: 0,
                };
            }
        }
        // A spec we already acted on resurfacing as the head means the queue
        // did not advance — its run reported success but it never left the
        // queue, so it was not really shipped. Drop it from `shipped` and
        // stop rather than loop forever on it. (A punted spec leaves the
        // queue via `NeedsAttention`, so it cannot resurface this way.)
        if shipped.iter().any(|s| s == &head) {
            shipped.retain(|s| s != &head);
            return BatchDrainResult {
                shipped,
                punted,
                stopped_at: Some(head),
                outcome: BatchDrainOutcome::Stalled,
                exit_code: 1,
            };
        }
        let result = driver.run_spec(&head);
        if result.exit_code != 0 {
            let phase = result.failed_phase.unwrap_or(Phase::Implementer);
            return BatchDrainResult {
                shipped,
                punted,
                stopped_at: Some(head),
                outcome: BatchDrainOutcome::Failed(phase),
                exit_code: result.exit_code,
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
                stopped_at: Some(head.clone()),
                outcome: BatchDrainOutcome::Mismatched {
                    dispatched: head,
                    shipped: actual,
                },
                exit_code: 0,
            };
        }
        // STORY-276: a punt is a clean exit (`exit_code` 0) — the drain
        // advances — but the spec parked, it did not ship. Sort it accordingly.
        if result.punt_reason.is_some() {
            punted.push(head);
        } else {
            shipped.push(head);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }

    impl MockPhaseDriver {
        fn all_ok() -> Self {
            Self {
                calls: Vec::new(),
                fail_at: None,
                verdict: Verdict::Approved,
                reconcile: PhaseReconcile::GenuineFailure,
                punt: None,
                shipped_spec_id: None,
            }
        }

        fn failing_at(phase: Phase) -> Self {
            Self {
                calls: Vec::new(),
                fail_at: Some(phase),
                verdict: Verdict::Approved,
                reconcile: PhaseReconcile::GenuineFailure,
                punt: None,
                shipped_spec_id: None,
            }
        }

        fn with_verdict(verdict: Verdict) -> Self {
            Self {
                calls: Vec::new(),
                fail_at: None,
                verdict,
                reconcile: PhaseReconcile::GenuineFailure,
                punt: None,
                shipped_spec_id: None,
            }
        }

        /// STORY-276: make `run_implementer` punt — phase 1 returns
        /// [`ImplementerOutcome::Punted`] with `reason`.
        fn punting_at_implementer(reason: &str) -> Self {
            Self {
                calls: Vec::new(),
                fail_at: None,
                verdict: Verdict::Approved,
                reconcile: PhaseReconcile::GenuineFailure,
                punt: Some(reason.to_string()),
                shipped_spec_id: None,
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
        fn run_reviewer(&mut self) -> Result<Verdict, PhaseFailure> {
            self.record(Phase::Reviewer)?;
            Ok(self.verdict)
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
            }
        }
        fn reconcile_failure(&mut self, _phase: Phase, _failure: &PhaseFailure) -> PhaseReconcile {
            self.reconcile.clone()
        }
        fn shipped_spec_id(&mut self) -> Option<String> {
            self.shipped_spec_id.clone()
        }
    }

    // --- Core orchestration: the mock-Claude integration test -------------

    #[test]
    fn orchestrate_full_pipeline_runs_all_six_phases() {
        let mut driver = MockPhaseDriver::all_ok();
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false).exit_code;
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

    /// STORY-276 acceptance: a headless implementer that hits a design-fork
    /// punts rather than guessing. `orchestrate` stops cleanly after phase 1 —
    /// no CI / review / merge runs — exits `0`, and the result carries the
    /// punt reason so the run is not mistaken for a clean ship.
    #[test]
    fn orchestrate_punt_at_implementer_stops_clean() {
        let mut driver = MockPhaseDriver::punting_at_implementer(
            "two valid auth flows — the spec does not say which",
        );
        let result = orchestrate(&mut driver, "STORY-276", AutoCompleteVariant::Full, false);
        assert_eq!(result.exit_code, 0, "a punt is a clean exit, not a failure");
        assert_eq!(result.failed_phase, None);
        assert!(result.failure.is_none());
        assert_eq!(
            result.punt_reason.as_deref(),
            Some("two valid auth flows — the spec does not say which"),
            "the punt reason is what distinguishes a punt from a clean ship",
        );
        // The pipeline stopped dead at phase 1 — no CI, review, or merge ran.
        assert_eq!(driver.calls, vec![Phase::Implementer]);
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
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false).exit_code;
        assert_eq!(code, 1);
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    #[test]
    fn failure_injection_reviewer_exits_3() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Reviewer);
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false).exit_code;
        assert_eq!(code, 3);
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer, Phase::Ci, Phase::Reviewer]
        );
    }

    #[test]
    fn failure_injection_merge_exits_4() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Merge);
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false).exit_code;
        assert_eq!(code, 4);
    }

    #[test]
    fn failure_injection_pull_exits_5() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Pull);
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false).exit_code;
        assert_eq!(code, 5);
    }

    #[test]
    fn failure_injection_build_exits_6() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Build);
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false).exit_code;
        assert_eq!(code, 6);
    }

    // --- CI-failure injection: red CI stops at phase 2 --------------------

    #[test]
    fn ci_red_stops_at_phase_2() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Ci);
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false).exit_code;
        assert_eq!(code, 2);
        // The reviewer phase must NOT have run.
        assert_eq!(driver.calls, vec![Phase::Implementer, Phase::Ci]);
    }

    // --- Reviewer verdict gating ------------------------------------------

    #[test]
    fn reviewer_rejected_stops_at_phase_3() {
        let mut driver = MockPhaseDriver::with_verdict(Verdict::Rejected);
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false).exit_code;
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
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false).exit_code;
        assert_eq!(code, 3);
    }

    #[test]
    fn reviewer_approved_proceeds_to_merge() {
        let mut driver = MockPhaseDriver::with_verdict(Verdict::Approved);
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false).exit_code;
        assert_eq!(code, 0);
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
        let result = orchestrate(&mut driver, "BUG-230", AutoCompleteVariant::Full, false);
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
        let result = orchestrate(&mut driver, "BUG-233", AutoCompleteVariant::Full, false);
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
        let result = orchestrate(&mut driver, "BUG-233", AutoCompleteVariant::Full, false);
        assert_eq!(result.exit_code, 0);
        assert!(result.failed_phase.is_none());
    }

    /// Regression guard — the reconcile step must NOT mask a real failure.
    /// With the default `GenuineFailure` verdict (reality confirms nothing
    /// shipped) a phase-1 failure still exits `1` and names the failed phase.
    #[test]
    fn orchestrate_genuine_phase1_failure_still_fails() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Implementer);
        let result = orchestrate(&mut driver, "BUG-241", AutoCompleteVariant::Full, false);
        assert_eq!(result.exit_code, 1);
        assert_eq!(result.failed_phase, Some(Phase::Implementer));
        assert!(result.failure.is_some());
    }

    /// Regression guard at phase 3 — a genuine no-verdict failure (reality
    /// confirms nothing merged) still stops the batch at phase 3.
    #[test]
    fn orchestrate_genuine_phase3_failure_still_fails() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Reviewer);
        let result = orchestrate(&mut driver, "BUG-241", AutoCompleteVariant::Full, false);
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
        let result = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false);
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
        );
        let phases: Vec<Phase> = result.phase_durations.iter().map(|(p, _)| *p).collect();
        assert_eq!(phases, vec![Phase::Implementer, Phase::Ci]);
    }

    #[test]
    fn result_failure_carries_failed_phase_and_reason() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Reviewer);
        let result = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false);
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
        let result = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false);
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
        let result = drain_batch(&mut driver, None);
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
        let result = drain_batch(&mut driver, None);
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
        let result = drain_batch(&mut driver, None);
        assert_eq!(result.exit_code, 3);
        assert_eq!(result.outcome, BatchDrainOutcome::Failed(Phase::Reviewer));
        assert!(result.shipped.is_empty());
    }

    /// `--max N` stops the drain after N items even when the batch has more.
    #[test]
    fn drain_batch_max_caps_the_drain() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2", "TASK-3", "TASK-4"]);
        let result = drain_batch(&mut driver, Some(2));
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
        let result = drain_batch(&mut driver, Some(2));
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.shipped, vec!["TASK-1", "TASK-2"]);
    }

    /// An empty batch drains immediately with nothing shipped.
    #[test]
    fn drain_batch_empty_batch_is_a_clean_drain() {
        let mut driver = MockBatchDriver::new(&[]);
        let result = drain_batch(&mut driver, None);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert!(result.shipped.is_empty());
    }

    /// STORY-276: a punted member advances the drain — it leaves the queue
    /// (`NeedsAttention`), so the batch keeps going — but it lands in `punted`,
    /// not `shipped`, so the summary does not claim a parked spec shipped.
    #[test]
    fn drain_batch_punted_member_advances_the_drain() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2", "TASK-3"]).punting("TASK-2");
        let result = drain_batch(&mut driver, None);
        assert_eq!(result.exit_code, 0, "a punt does not fail the drain");
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.shipped, vec!["TASK-1", "TASK-3"]);
        assert_eq!(result.punted, vec!["TASK-2"]);
        // Every member ran — the punt did not stall or short-circuit the drain.
        assert_eq!(driver.runs, vec!["TASK-1", "TASK-2", "TASK-3"]);
    }

    /// A "successful" run that leaves the head in place is caught by the
    /// non-advancing-queue guard rather than looping forever.
    #[test]
    fn drain_batch_stall_guard_stops_a_non_advancing_queue() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2"]).stalling("TASK-2");
        let result = drain_batch(&mut driver, None);
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
        let result = orchestrate(&mut driver, "STORY-276", AutoCompleteVariant::Full, false);
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
        let result = orchestrate(&mut driver, "STORY-276", AutoCompleteVariant::Full, false);
        assert_eq!(result.shipped_spec_id, None);

        // A driver that names the dispatched id is also not a mismatch.
        let mut driver = MockPhaseDriver::all_ok().shipping_as("STORY-276");
        let result = orchestrate(&mut driver, "STORY-276", AutoCompleteVariant::Full, false);
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
        let result = drain_batch(&mut driver, None);
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
        let result = drain_batch(&mut driver, None);
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
