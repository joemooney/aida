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

    /// Stable machine slug for `--json` events.
    fn slug(self) -> &'static str {
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
/// user; the "what to do next" hint is derived separately via
/// [`recovery_hint`] so it can be unit-tested independent of the driver.
/// trace:STORY-246 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct PhaseFailure {
    pub(crate) reason: String,
}

impl PhaseFailure {
    pub(crate) fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
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
/// concrete next command, parameterised by whatever the driver discovered.
/// trace:STORY-246 | ai:claude
pub(crate) fn recovery_hint(phase: Phase, ctx: &HintContext) -> String {
    let spec = if ctx.spec.is_empty() {
        "<SPEC>"
    } else {
        ctx.spec.as_str()
    };
    let branch = ctx.branch.as_deref().unwrap_or("<branch>");
    match phase {
        Phase::Implementer => {
            let resume = ctx
                .implementer_session
                .as_deref()
                .map(|s| format!(" {s}"))
                .unwrap_or_default();
            format!("Continue the implementer session: `aida queue work {spec} --resume{resume}`")
        }
        Phase::Ci => {
            let investigate = ctx
                .ci_run_id
                .as_deref()
                .map(|r| format!("`gh run view {r}`"))
                .unwrap_or_else(|| format!("`gh run list --branch {branch}`"));
            format!(
                "CI is red — investigate with {investigate}, then push fixups: \
                 `aida queue work {spec} --branch {branch} --steal`"
            )
        }
        Phase::Reviewer => format!(
            "Address the review feedback: `aida queue work {spec} --branch {branch} --steal`"
        ),
        Phase::Merge => {
            let pr = ctx
                .pr_number
                .map(|n| n.to_string())
                .unwrap_or_else(|| "<N>".to_string());
            format!("Investigate the merge failure: `gh pr view {pr}`")
        }
        Phase::Pull => "Classify the divergence: `aida rebase --dry-run --json`".to_string(),
        Phase::Build => "Get fast feedback on the breakage: `cargo check --workspace`".to_string(),
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

/// Print the success epilogue and return exit code 0.
fn finish_success(spec: &str, json: bool, start: &Instant) -> i32 {
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
    0
}

/// Print the failure epilogue (reason + recovery hint) and return the
/// phase's exit code.
fn finish_failure(
    driver: &dyn PhaseDriver,
    phase: Phase,
    spec: &str,
    json: bool,
    start: &Instant,
    failure: &PhaseFailure,
) -> i32 {
    let code = phase.index();
    let elapsed = start.elapsed().as_millis();
    let hint = recovery_hint(phase, &driver.hint_context());
    if json {
        println!(
            "{}",
            phase_event(
                phase.slug(),
                "failed",
                spec,
                elapsed,
                Some(code),
                &[("reason", failure.reason.as_str()), ("hint", hint.as_str())],
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
    code
}

/// Drive the phases in order, stopping at the variant's last phase or at the
/// first failure. Returns the process exit code: `0` on success, otherwise
/// the 1-based index of the phase that failed.
/// trace:STORY-246 | ai:claude
pub(crate) fn orchestrate(
    driver: &mut dyn PhaseDriver,
    spec: &str,
    variant: AutoCompleteVariant,
    json: bool,
) -> i32 {
    let start = Instant::now();
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
    if let Err(f) = driver.run_implementer() {
        return finish_failure(driver, Phase::Implementer, spec, json, &start, &f);
    }
    emit_done(Phase::Implementer, spec, json, start.elapsed().as_millis());

    // Phase 2 — end + wait for CI.
    emit_start(Phase::Ci, spec, json, start.elapsed().as_millis());
    if let Err(f) = driver.finish_ci() {
        return finish_failure(driver, Phase::Ci, spec, json, &start, &f);
    }
    emit_done(Phase::Ci, spec, json, start.elapsed().as_millis());
    if variant.last_phase() <= 2 {
        return finish_success(spec, json, &start);
    }

    // Phase 3 — reviewer session.
    emit_start(Phase::Reviewer, spec, json, start.elapsed().as_millis());
    match driver.run_reviewer() {
        Err(f) => return finish_failure(driver, Phase::Reviewer, spec, json, &start, &f),
        Ok(verdict) if verdict != Verdict::Approved => {
            let f = PhaseFailure::new(format!(
                "reviewer verdict is {} — not Approved",
                verdict.label()
            ));
            return finish_failure(driver, Phase::Reviewer, spec, json, &start, &f);
        }
        Ok(_) => {}
    }
    emit_done(Phase::Reviewer, spec, json, start.elapsed().as_millis());

    // Phase 4 — merge.
    emit_start(Phase::Merge, spec, json, start.elapsed().as_millis());
    if let Err(f) = driver.merge() {
        return finish_failure(driver, Phase::Merge, spec, json, &start, &f);
    }
    emit_done(Phase::Merge, spec, json, start.elapsed().as_millis());
    if variant.last_phase() <= 4 {
        return finish_success(spec, json, &start);
    }

    // Phase 5 — pull + auto-bump.
    emit_start(Phase::Pull, spec, json, start.elapsed().as_millis());
    if let Err(f) = driver.pull() {
        return finish_failure(driver, Phase::Pull, spec, json, &start, &f);
    }
    emit_done(Phase::Pull, spec, json, start.elapsed().as_millis());
    if variant.last_phase() <= 5 {
        return finish_success(spec, json, &start);
    }

    // Phase 6 — build verify.
    emit_start(Phase::Build, spec, json, start.elapsed().as_millis());
    if let Err(f) = driver.build() {
        return finish_failure(driver, Phase::Build, spec, json, &start, &f);
    }
    emit_done(Phase::Build, spec, json, start.elapsed().as_millis());

    finish_success(spec, json, &start)
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
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false);
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
        );
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
        );
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
        );
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
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false);
        assert_eq!(code, 1);
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    #[test]
    fn failure_injection_reviewer_exits_3() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Reviewer);
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false);
        assert_eq!(code, 3);
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer, Phase::Ci, Phase::Reviewer]
        );
    }

    #[test]
    fn failure_injection_merge_exits_4() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Merge);
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false);
        assert_eq!(code, 4);
    }

    #[test]
    fn failure_injection_pull_exits_5() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Pull);
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false);
        assert_eq!(code, 5);
    }

    #[test]
    fn failure_injection_build_exits_6() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Build);
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false);
        assert_eq!(code, 6);
    }

    // --- CI-failure injection: red CI stops at phase 2 --------------------

    #[test]
    fn ci_red_stops_at_phase_2() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Ci);
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false);
        assert_eq!(code, 2);
        // The reviewer phase must NOT have run.
        assert_eq!(driver.calls, vec![Phase::Implementer, Phase::Ci]);
    }

    // --- Reviewer verdict gating ------------------------------------------

    #[test]
    fn reviewer_rejected_stops_at_phase_3() {
        let mut driver = MockPhaseDriver::with_verdict(Verdict::Rejected);
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false);
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
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false);
        assert_eq!(code, 3);
    }

    #[test]
    fn reviewer_approved_proceeds_to_merge() {
        let mut driver = MockPhaseDriver::with_verdict(Verdict::Approved);
        let code = orchestrate(&mut driver, "TASK-247", AutoCompleteVariant::Full, false);
        assert_eq!(code, 0);
    }

    // --- Recovery hints: one per exit code --------------------------------

    fn ctx() -> HintContext {
        HintContext {
            spec: "TASK-247".to_string(),
            branch: Some("task-247".to_string()),
            pr_number: Some(46),
            implementer_session: Some("019e2f423e7c".to_string()),
            ci_run_id: Some("9988776655".to_string()),
        }
    }

    #[test]
    fn recovery_hint_implementer_names_resume() {
        let hint = recovery_hint(Phase::Implementer, &ctx());
        assert!(hint.contains("aida queue work TASK-247 --resume 019e2f423e7c"));
    }

    #[test]
    fn recovery_hint_ci_names_run_view_and_steal() {
        let hint = recovery_hint(Phase::Ci, &ctx());
        assert!(hint.contains("gh run view 9988776655"));
        assert!(hint.contains("--steal"));
    }

    #[test]
    fn recovery_hint_ci_without_run_id_falls_back_to_run_list() {
        let mut c = ctx();
        c.ci_run_id = None;
        let hint = recovery_hint(Phase::Ci, &c);
        assert!(hint.contains("gh run list --branch task-247"));
    }

    #[test]
    fn recovery_hint_reviewer_names_steal() {
        let hint = recovery_hint(Phase::Reviewer, &ctx());
        assert!(hint.contains("aida queue work TASK-247 --branch task-247 --steal"));
    }

    #[test]
    fn recovery_hint_merge_names_pr_view() {
        let hint = recovery_hint(Phase::Merge, &ctx());
        assert!(hint.contains("gh pr view 46"));
    }

    #[test]
    fn recovery_hint_pull_names_rebase_dry_run() {
        let hint = recovery_hint(Phase::Pull, &ctx());
        assert!(hint.contains("aida rebase --dry-run --json"));
    }

    #[test]
    fn recovery_hint_build_names_cargo_check() {
        let hint = recovery_hint(Phase::Build, &ctx());
        assert!(hint.contains("cargo check --workspace"));
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
}
