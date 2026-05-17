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

    fn describe(self) -> &'static str {
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
}

/// The six phases, abstracted so the orchestrator's sequencing can be tested
/// against a mock. The real implementation spawns Claude sessions, polls CI,
/// and shells out to `gh` / `cargo`.
/// trace:STORY-246 | ai:claude
pub(crate) trait PhaseDriver {
    /// Phase 1 — run the implementer Claude session and verify it opened a PR.
    fn run_implementer(&mut self) -> Result<(), PhaseFailure>;
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
            return format!(
                "This is an orchestrator bug, not something you did — please file it \
                 (`aida add --type bug --title 'auto-complete phase {} internal error'`). \
                 Meanwhile, drive the remaining phases by hand from `aida queue work {spec}`.",
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
        (Phase::Reviewer, _) => format!(
            "Address the review feedback, then push fixups: \
             `aida queue work {spec} --branch {branch} --steal`"
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

/// Print the success epilogue and build the success [`OrchestrationResult`].
fn finish_success(
    spec: &str,
    json: bool,
    start: &Instant,
    durations: Vec<(Phase, u128)>,
) -> OrchestrationResult {
    let elapsed = start.elapsed().as_millis();
    if json {
        println!(
            "{}",
            phase_event("auto-complete", "success", spec, elapsed, Some(0), &[])
        );
    } else {
        eprintln!();
        eprintln!(
            "{} {} shipped ({})",
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

    // Phase 1 — implementer session.
    emit_start(Phase::Implementer, spec, json, start.elapsed().as_millis());
    let phase_start = Instant::now();
    if let Err(f) = driver.run_implementer() {
        durations.push((Phase::Implementer, phase_start.elapsed().as_millis()));
        return finish_failure(
            driver,
            Phase::Implementer,
            spec,
            json,
            &start,
            &f,
            durations,
        );
    }
    durations.push((Phase::Implementer, phase_start.elapsed().as_millis()));
    emit_done(Phase::Implementer, spec, json, start.elapsed().as_millis());

    // Phase 2 — end + wait for CI.
    emit_start(Phase::Ci, spec, json, start.elapsed().as_millis());
    let phase_start = Instant::now();
    if let Err(f) = driver.finish_ci() {
        durations.push((Phase::Ci, phase_start.elapsed().as_millis()));
        return finish_failure(driver, Phase::Ci, spec, json, &start, &f, durations);
    }
    durations.push((Phase::Ci, phase_start.elapsed().as_millis()));
    emit_done(Phase::Ci, spec, json, start.elapsed().as_millis());
    if variant.last_phase() <= 2 {
        return finish_success(spec, json, &start, durations);
    }

    // Phase 3 — reviewer session.
    emit_start(Phase::Reviewer, spec, json, start.elapsed().as_millis());
    let phase_start = Instant::now();
    match driver.run_reviewer() {
        Err(f) => {
            durations.push((Phase::Reviewer, phase_start.elapsed().as_millis()));
            return finish_failure(driver, Phase::Reviewer, spec, json, &start, &f, durations);
        }
        Ok(verdict) if verdict != Verdict::Approved => {
            durations.push((Phase::Reviewer, phase_start.elapsed().as_millis()));
            let f = PhaseFailure::new(format!(
                "reviewer verdict is {} — not Approved",
                verdict.label()
            ));
            return finish_failure(driver, Phase::Reviewer, spec, json, &start, &f, durations);
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
        return finish_failure(driver, Phase::Merge, spec, json, &start, &f, durations);
    }
    durations.push((Phase::Merge, phase_start.elapsed().as_millis()));
    emit_done(Phase::Merge, spec, json, start.elapsed().as_millis());
    if variant.last_phase() <= 4 {
        return finish_success(spec, json, &start, durations);
    }

    // Phase 5 — pull + auto-bump.
    emit_start(Phase::Pull, spec, json, start.elapsed().as_millis());
    let phase_start = Instant::now();
    if let Err(f) = driver.pull() {
        durations.push((Phase::Pull, phase_start.elapsed().as_millis()));
        return finish_failure(driver, Phase::Pull, spec, json, &start, &f, durations);
    }
    durations.push((Phase::Pull, phase_start.elapsed().as_millis()));
    emit_done(Phase::Pull, spec, json, start.elapsed().as_millis());
    if variant.last_phase() <= 5 {
        return finish_success(spec, json, &start, durations);
    }

    // Phase 6 — build verify.
    emit_start(Phase::Build, spec, json, start.elapsed().as_millis());
    let phase_start = Instant::now();
    if let Err(f) = driver.build() {
        durations.push((Phase::Build, phase_start.elapsed().as_millis()));
        return finish_failure(driver, Phase::Build, spec, json, &start, &f, durations);
    }
    durations.push((Phase::Build, phase_start.elapsed().as_millis()));
    emit_done(Phase::Build, spec, json, start.elapsed().as_millis());

    finish_success(spec, json, &start, durations)
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
    }

    impl MockPhaseDriver {
        fn all_ok() -> Self {
            Self {
                calls: Vec::new(),
                fail_at: None,
                verdict: Verdict::Approved,
            }
        }

        fn failing_at(phase: Phase) -> Self {
            Self {
                calls: Vec::new(),
                fail_at: Some(phase),
                verdict: Verdict::Approved,
            }
        }

        fn with_verdict(verdict: Verdict) -> Self {
            Self {
                calls: Vec::new(),
                fail_at: None,
                verdict,
            }
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
        fn run_implementer(&mut self) -> Result<(), PhaseFailure> {
            self.record(Phase::Implementer)
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

    #[test]
    fn recovery_hint_reviewer_failed_names_steal() {
        let hint = recovery_hint(Phase::Reviewer, FailureKind::Failed, &ctx());
        assert!(hint.contains("aida queue work TASK-247 --branch task-247 --steal"));
    }

    #[test]
    fn recovery_hint_merge_failed_names_pr_view() {
        let hint = recovery_hint(Phase::Merge, FailureKind::Failed, &ctx());
        assert!(hint.contains("gh pr view 46"));
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
}
