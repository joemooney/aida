//! `aida compete <SPEC> --vendors <csv> [--gate "<cmd>"]` — run one spec through
//! N vendors headless, in isolated worktrees, then a deterministic objective
//! gate; report a table and leave every branch in place for the advisor to pick.
//!
//! This is the flagship niche feature productizing the SPIKE-64 finding: the
//! value of running N vendors is NOT architectural diversity (vendors converge
//! on a real codebase because the substrate dictates the shape) — it is
//! execution-QUALITY variance within the converged design. So `aida compete` is
//! framed as competition-as-QA: run a spec through N vendors + an objective gate,
//! surface the one that drifted, and let a neutral substrate-owner pick the best.
//! Slice 1 = orchestration + objective gate + REPORT-ONLY (no rubric judge, no
//! synthesis, no auto-merge — those are later slices).
//!
//! This module holds the PURE, unit-testable parts: the vendor→command mapping
//! (the adapter table), the per-vendor branch naming, the gate-result parsing,
//! and the report-table formatting. The I/O orchestration (spawning vendors,
//! creating worktrees, running the gate) lives in `main.rs::handle_compete_command`
//! so the costly real-vendor runs never execute in tests.
//!
//! trace:STORY-659 | ai:claude

/// How a vendor is driven in slice 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VendorAdapter {
    /// A vendor with a headless CLI we can spawn directly. `command` is the
    /// binary; `args_template` is the argv WITHOUT the brief (the brief is
    /// appended as the final positional arg by the caller). Keeping the brief
    /// out of the template keeps this a pure description of the adapter.
    Headless {
        command: &'static str,
        /// argv that precede the brief positional (e.g. `["-p", ...]` for claude,
        /// `["exec", "--dangerously-bypass-approvals-and-sandbox"]` for codex).
        args_template: Vec<String>,
    },
    /// A vendor with NO headless CLI (e.g. antigravity). Instead of running it we
    /// emit an `aida brief <vendor> <SPEC>` for the human to run, and report it
    /// as a "human-run arm (briefed)". This is the cross-vendor coordination
    /// path: the neutral substrate routes the work even when it can't execute it.
    HumanBriefed,
}

/// Resolve a vendor name to its adapter. Adding a vendor later is a single row.
/// Returns `None` for an unknown vendor so the caller can skip-with-a-note
/// rather than fail the whole run.
///
/// The claude argv intentionally does NOT reuse `session::claude_headless_args`:
/// that builder forces `--session-id`/`--output-format stream-json` for the
/// orchestrator's resumable, machine-parsed drains. A compete arm just needs a
/// one-shot headless run whose stdout we tee to a log, so the simpler
/// `-p --permission-mode bypassPermissions` flag set is the right surface here.
pub fn vendor_adapter(vendor: &str) -> Option<VendorAdapter> {
    match vendor {
        "claude" => Some(VendorAdapter::Headless {
            command: "claude",
            args_template: vec![
                "-p".to_string(),
                "--permission-mode".to_string(),
                "bypassPermissions".to_string(),
            ],
        }),
        "codex" => Some(VendorAdapter::Headless {
            command: "codex",
            args_template: vec![
                "exec".to_string(),
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
            ],
        }),
        "antigravity" => Some(VendorAdapter::HumanBriefed),
        _ => None,
    }
}

/// The full argv for a headless vendor run: the adapter template plus the brief
/// as the final positional argument. Returns `None` for a non-headless adapter.
pub fn headless_argv(adapter: &VendorAdapter, brief: &str) -> Option<Vec<String>> {
    match adapter {
        VendorAdapter::Headless { args_template, .. } => {
            let mut argv = args_template.clone();
            argv.push(brief.to_string());
            Some(argv)
        }
        VendorAdapter::HumanBriefed => None,
    }
}

/// Per-vendor branch name: `compete/<spec-lower>-<vendor>`. Stable + collision-
/// free across vendors for the same spec, and namespaced under `compete/` so a
/// `git branch --list 'compete/*'` finds every arm.
pub fn vendor_branch(spec_id: &str, vendor: &str) -> String {
    format!("compete/{}-{}", spec_id.to_ascii_lowercase(), vendor)
}

/// The default deterministic gate for THIS repo: build, then run the aida-cli
/// test suite with the advisor's role env unset (it leaks into role-gated tests
/// and causes false failures — see the project memory on `AIDA_SESSION_ROLE`).
pub const DEFAULT_GATE: &str = "cargo build && env -u AIDA_SESSION_ROLE cargo test -p aida-cli";

/// Outcome of one vendor arm. Pure data — assembled by the orchestrator, then
/// handed to [`render_report`] for formatting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArmResult {
    pub vendor: String,
    /// Did the vendor actually run? `Ran::*` distinguishes the honest outcomes.
    pub ran: Ran,
    /// Did the gate's build step succeed? `None` when the gate didn't run
    /// (vendor skipped / errored / human-briefed).
    pub built: Option<bool>,
    /// Did the full gate command exit 0? `None` when the gate didn't run.
    pub gate_passed: Option<bool>,
    /// Lines changed vs the base (added + removed), if measurable.
    pub diff_lines: Option<usize>,
    /// The per-vendor branch the work landed on (empty for human-briefed/skipped).
    pub branch: String,
}

/// Honest run outcomes — partial results are still useful (competition-as-QA).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ran {
    /// Vendor CLI ran and exited successfully.
    Ok,
    /// Vendor CLI ran but exited non-zero (we still gate + report its branch).
    Failed,
    /// Vendor CLI not found on PATH — skipped with a note, run continues.
    Skipped,
    /// Non-headless vendor: an `aida brief` was emitted; a human runs this arm.
    Briefed,
}

impl Ran {
    /// Short cell label for the `ran` column.
    pub fn label(self) -> &'static str {
        match self {
            Ran::Ok => "ok",
            Ran::Failed => "fail",
            Ran::Skipped => "skip",
            Ran::Briefed => "briefed",
        }
    }
}

/// Parse a gate run into (built?, gate_passed?). The gate is one shell command
/// of the shape `cargo build && <tests>`; we treat a zero overall exit as a full
/// pass, and infer the build sub-result from whether the combined stderr/stdout
/// shows a build failure. We keep this conservative and HONEST: if the overall
/// command passed, the build necessarily passed (it's the first `&&` clause);
/// if it failed, we look for the cargo build-failure marker to attribute the
/// failure to the build vs. the tests.
pub fn parse_gate_result(exit_ok: bool, combined_output: &str) -> (bool, bool) {
    if exit_ok {
        // A `cargo build && cargo test` that exits 0 means both built and tested.
        return (true, true);
    }
    // Overall failure: did the BUILD fail, or did it build and the tests fail?
    let lower = combined_output.to_ascii_lowercase();
    let build_failed = lower.contains("error: could not compile")
        || lower.contains("error[e")
        || lower.contains("build failed");
    let built = !build_failed;
    (built, false)
}

/// Count changed lines (added + removed) from a `git diff --numstat` body.
/// Each line is `<added>\t<removed>\t<path>`; binary files show `-` and are
/// skipped. Returns the summed magnitude — a coarse but honest "how much did
/// this arm change" signal for the report.
pub fn count_diff_lines(numstat: &str) -> usize {
    numstat
        .lines()
        .filter_map(|line| {
            let mut cols = line.split('\t');
            let added = cols.next()?.parse::<usize>().ok()?;
            let removed = cols.next()?.parse::<usize>().ok()?;
            Some(added + removed)
        })
        .sum()
}

/// Glyphs the report renderer needs, supplied by the caller from the central
/// registry (so this pure module never embeds raw glyph literals — they route
/// through `[ui] glyphs` / `AIDA_GLYPHS`). trace:TASK-835
#[derive(Debug, Clone)]
pub struct ReportGlyphs {
    pub check: String,
    pub cross: String,
    pub pending: String,
}

/// Format the per-cell yes/no/na value with the supplied glyphs.
fn tri(value: Option<bool>, g: &ReportGlyphs) -> String {
    match value {
        Some(true) => g.check.clone(),
        Some(false) => g.cross.clone(),
        None => g.pending.clone(),
    }
}

/// Render the report: a `vendor | ran | built | gate | diff-lines | branch`
/// table plus a one-line verdict per arm. Pure — fully unit-testable.
pub fn render_report(spec_id: &str, results: &[ArmResult], g: &ReportGlyphs) -> String {
    let mut out = String::new();
    out.push_str(&format!("Competition results for {spec_id}\n\n"));

    // Column widths sized to content (with sane minimums for the headers).
    let vendor_w = results
        .iter()
        .map(|r| r.vendor.len())
        .max()
        .unwrap_or(6)
        .max("vendor".len());
    let branch_w = results
        .iter()
        .map(|r| r.branch.len())
        .max()
        .unwrap_or(6)
        .max("branch".len());

    out.push_str(&format!(
        "{:<vw$}  {:<7}  {:<5}  {:<4}  {:>10}  {:<bw$}\n",
        "vendor",
        "ran",
        "built",
        "gate",
        "diff-lines",
        "branch",
        vw = vendor_w,
        bw = branch_w,
    ));
    for r in results {
        let diff = r
            .diff_lines
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".to_string());
        let branch = if r.branch.is_empty() { "-" } else { &r.branch };
        out.push_str(&format!(
            "{:<vw$}  {:<7}  {:<5}  {:<4}  {:>10}  {:<bw$}\n",
            r.vendor,
            r.ran.label(),
            tri(r.built, g),
            tri(r.gate_passed, g),
            diff,
            branch,
            vw = vendor_w,
            bw = branch_w,
        ));
    }

    out.push('\n');
    for line in verdict_lines(results) {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// One honest one-line verdict per arm.
pub fn verdict_lines(results: &[ArmResult]) -> Vec<String> {
    results
        .iter()
        .map(|r| match r.ran {
            Ran::Skipped => format!("{}: skipped — CLI not found on PATH", r.vendor),
            Ran::Briefed => format!(
                "{}: human-run arm — brief emitted (no headless CLI); run it, then re-gate",
                r.vendor
            ),
            Ran::Failed if r.gate_passed == Some(true) => format!(
                "{}: vendor exited non-zero but gate PASS on branch {} — inspect the log",
                r.vendor, r.branch
            ),
            Ran::Failed => format!(
                "{}: vendor run errored — gate {} on branch {} — see the log",
                r.vendor,
                gate_word(r.gate_passed),
                r.branch
            ),
            Ran::Ok => format!(
                "{}: gate {} on branch {}",
                r.vendor,
                gate_word(r.gate_passed),
                r.branch
            ),
        })
        .collect()
}

fn gate_word(passed: Option<bool>) -> &'static str {
    match passed {
        Some(true) => "PASS",
        Some(false) => "FAIL",
        None => "not run",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyphs() -> ReportGlyphs {
        ReportGlyphs {
            check: "[x]".to_string(),
            cross: "[ ]".to_string(),
            pending: "( )".to_string(),
        }
    }

    #[test]
    fn vendor_adapter_maps_known_vendors() {
        assert!(matches!(
            vendor_adapter("claude"),
            Some(VendorAdapter::Headless {
                command: "claude",
                ..
            })
        ));
        assert!(matches!(
            vendor_adapter("codex"),
            Some(VendorAdapter::Headless {
                command: "codex",
                ..
            })
        ));
        assert_eq!(
            vendor_adapter("antigravity"),
            Some(VendorAdapter::HumanBriefed)
        );
        assert_eq!(vendor_adapter("nope"), None);
    }

    #[test]
    fn headless_argv_appends_brief_as_final_positional() {
        let adapter = vendor_adapter("claude").unwrap();
        let argv = headless_argv(&adapter, "do the thing").unwrap();
        assert_eq!(argv.first().unwrap(), "-p");
        assert_eq!(argv.last().unwrap(), "do the thing");
        assert!(argv.contains(&"bypassPermissions".to_string()));
    }

    #[test]
    fn headless_argv_codex_uses_exec_and_bypass() {
        let adapter = vendor_adapter("codex").unwrap();
        let argv = headless_argv(&adapter, "BRIEF").unwrap();
        assert_eq!(argv[0], "exec");
        assert_eq!(argv[1], "--dangerously-bypass-approvals-and-sandbox");
        assert_eq!(argv.last().unwrap(), "BRIEF");
    }

    #[test]
    fn headless_argv_none_for_human_briefed() {
        assert!(headless_argv(&VendorAdapter::HumanBriefed, "x").is_none());
    }

    #[test]
    fn vendor_branch_is_namespaced_and_lowercased() {
        assert_eq!(
            vendor_branch("STORY-659", "claude"),
            "compete/story-659-claude"
        );
        assert_eq!(vendor_branch("BUG-12", "codex"), "compete/bug-12-codex");
    }

    #[test]
    fn parse_gate_result_pass() {
        let (built, gate) = parse_gate_result(true, "ignored");
        assert!(built);
        assert!(gate);
    }

    #[test]
    fn parse_gate_result_build_failure() {
        let (built, gate) = parse_gate_result(
            false,
            "error[E0599]: no method named foo\nerror: could not compile `aida-cli`",
        );
        assert!(!built);
        assert!(!gate);
    }

    #[test]
    fn parse_gate_result_test_failure_still_built() {
        let (built, gate) =
            parse_gate_result(false, "test some_test ... FAILED\ntest result: FAILED");
        assert!(built, "build clause passed; only tests failed");
        assert!(!gate);
    }

    #[test]
    fn count_diff_lines_sums_added_and_removed_skips_binary() {
        let numstat = "10\t2\tsrc/a.rs\n5\t0\tsrc/b.rs\n-\t-\timg.png\n";
        assert_eq!(count_diff_lines(numstat), 17);
    }

    #[test]
    fn count_diff_lines_empty_is_zero() {
        assert_eq!(count_diff_lines(""), 0);
    }

    #[test]
    fn render_report_has_header_row_and_one_line_per_arm() {
        let results = vec![
            ArmResult {
                vendor: "codex".to_string(),
                ran: Ran::Ok,
                built: Some(true),
                gate_passed: Some(true),
                diff_lines: Some(42),
                branch: "compete/story-659-codex".to_string(),
            },
            ArmResult {
                vendor: "claude".to_string(),
                ran: Ran::Failed,
                built: Some(true),
                gate_passed: Some(false),
                diff_lines: Some(7),
                branch: "compete/story-659-claude".to_string(),
            },
        ];
        let report = render_report("STORY-659", &results, &glyphs());
        assert!(report.contains("vendor"));
        assert!(report.contains("diff-lines"));
        assert!(report.contains("compete/story-659-codex"));
        assert!(report.contains("codex: gate PASS on branch compete/story-659-codex"));
        assert!(report.contains("claude: vendor run errored — gate FAIL"));
    }

    #[test]
    fn verdict_distinguishes_skipped_and_briefed() {
        let results = vec![
            ArmResult {
                vendor: "gemini".to_string(),
                ran: Ran::Skipped,
                built: None,
                gate_passed: None,
                diff_lines: None,
                branch: String::new(),
            },
            ArmResult {
                vendor: "antigravity".to_string(),
                ran: Ran::Briefed,
                built: None,
                gate_passed: None,
                diff_lines: None,
                branch: String::new(),
            },
        ];
        let lines = verdict_lines(&results);
        assert!(lines[0].contains("skipped — CLI not found"));
        assert!(lines[1].contains("human-run arm — brief emitted"));
    }

    #[test]
    fn default_gate_unsets_session_role() {
        assert!(DEFAULT_GATE.contains("env -u AIDA_SESSION_ROLE"));
        assert!(DEFAULT_GATE.contains("cargo build"));
    }
}
