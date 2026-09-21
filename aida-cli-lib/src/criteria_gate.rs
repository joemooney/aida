//! Wires the existing `aida criteria` tracer (STORY-1178) into the
//! done-gate and the headless reviewer prompt, so an untraced acceptance
//! criterion is named before review instead of discovered by it.
//!
//! The description on the spec driving this module proposed extending
//! TASK-1277's protocol-item done-gate rather than inventing new
//! machinery — but TASK-1277 is not implemented yet (still Approved,
//! unworked). This module is deliberately shaped as one small, named
//! function per concern (read the config knob, evaluate the gate, render
//! the reviewer-prompt block, render the force-override ledger comment)
//! and called from a single choke point in each caller, so TASK-1277's
//! done-gate can fold this in next to its own protocol-item checks later
//! instead of the two gates growing independently. The absorb points:
//!   - `evaluate()` + `read_enforce_mode()` are the whole policy; a future
//!     TASK-1277 gate calls `evaluate()` alongside its own protocol-item
//!     check and merges the two `Warn`/`Refuse` line lists into one report.
//!   - `queue_cmd.rs`'s `QueueCommand::Done` handler calls this at the
//!     exact point TASK-1277's protocol-item check would also live (next
//!     to the existing review-verdict gate).
//!
//! No second tracer: every check here is built on
//! [`crate::criteria::build_criteria_report`], the same tracer `aida
//! criteria <SPEC>` prints.
// trace:TASK-1290 | ai:claude

use crate::criteria::{CriteriaReport, CriterionState};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnforceMode {
    /// Untraced criteria produce a warning; the caller proceeds. Default.
    Warn,
    /// Untraced criteria block the caller (`--force` overrides, ledgered).
    Refuse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CriteriaGate {
    /// Every acceptance criterion is traced (or the spec has none) — the
    /// caller must produce NO output for this gate. Quiet when clean.
    Proceed,
    /// Untraced criteria exist under `enforce = warn` — print `lines` and
    /// proceed.
    Warn(Vec<String>),
    /// Untraced criteria exist under `enforce = refuse` — block unless the
    /// caller applies `--force` (which must ledger the override).
    Refuse(Vec<String>),
}

/// One line per untraced criterion: id + the criterion text VERBATIM, so a
/// warning or refusal names exactly what the spec said rather than
/// paraphrasing it.
// trace:TASK-1290 | ai:claude
pub(crate) fn untraced_lines(report: &CriteriaReport) -> Vec<String> {
    report
        .criteria
        .iter()
        .filter(|row| row.state == CriterionState::Untraced)
        .map(|row| format!("{} {}", row.criterion.id, row.criterion.text))
        .collect()
}

/// Pure policy: given an already-built criteria report (reuse, never
/// re-derive) and the resolved enforce mode, decide whether the caller may
/// proceed silently, must warn, or must refuse.
// trace:TASK-1290 | ai:claude
pub(crate) fn evaluate(report: &CriteriaReport, enforce: EnforceMode) -> CriteriaGate {
    if report.untested.is_empty() {
        return CriteriaGate::Proceed;
    }
    let lines = untraced_lines(report);
    match enforce {
        EnforceMode::Warn => CriteriaGate::Warn(lines),
        EnforceMode::Refuse => CriteriaGate::Refuse(lines),
    }
}

/// `[protocol] enforce` from `.aida/config.toml` — `"refuse"` (case
/// insensitive) opts into blocking; anything else (absent key, missing
/// file, unparsable TOML, unknown value) resolves to the safe default,
/// `Warn`. This is the same section name TASK-1277's protocol-item
/// done-gate is proposed to own; this reads only the one key it
/// needs so the two gates can share the section without either owning
/// the other's schema.
// trace:TASK-1290 | ai:claude
pub(crate) fn read_enforce_mode(project_root: &Path) -> EnforceMode {
    let value = match crate::read_project_config_value(project_root) {
        Some(v) => v,
        None => return EnforceMode::Warn,
    };
    match value
        .get("protocol")
        .and_then(|t| t.get("enforce"))
        .and_then(|v| v.as_str())
    {
        Some(s) if s.eq_ignore_ascii_case("refuse") => EnforceMode::Refuse,
        _ => EnforceMode::Warn,
    }
}

/// The `--force` ledger comment written onto the spec when an operator
/// overrides a `Refuse`. Recorded so the override is auditable from `aida
/// show <spec>` history, matching the review-verdict gate's ledger
/// convention.
// trace:TASK-1290 | ai:claude
pub(crate) fn force_override_ledger_comment(display_id: &str, lines: &[String]) -> String {
    let mut out = format!(
        "--force override at `aida queue done`: proceeding for {display_id} with untraced \
         acceptance criteria (no test traces them):\n"
    );
    for line in lines {
        out.push_str("- ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The block appended to the round-1 headless reviewer prompt so round 1
/// CITES the untraced list instead of discovering it during review.
/// `reports` is one `(display_id, CriteriaReport)` pair per spec the PR
/// covers. Returns `None` when every criterion across every spec is
/// traced — quiet when clean applies here too, the prompt gains no new
/// section for a clean PR.
// trace:TASK-1290 | ai:claude
pub(crate) fn reviewer_prompt_block(reports: &[(String, CriteriaReport)]) -> Option<String> {
    let mut untraced: Vec<String> = Vec::new();
    let mut post_deployment: Vec<String> = Vec::new();
    for (display_id, report) in reports {
        for line in untraced_lines(report) {
            untraced.push(format!("- {display_id}: {line}"));
        }
        for row in &report.criteria {
            if row.state == CriterionState::PostDeployment {
                post_deployment.push(format!(
                    "- {display_id}: {} {}",
                    row.criterion.id, row.criterion.text
                ));
            }
        }
    }
    if untraced.is_empty() && post_deployment.is_empty() {
        return None;
    }
    let mut block = String::new();
    if !untraced.is_empty() {
        block.push_str(&format!(
            "\n\nUntraced acceptance criteria (from `aida criteria`, reuse — do not re-derive): \
         the following criteria have no test tracing them. Cite this list against the diff \
         instead of rediscovering it; a criterion named here and untested in the diff is a \
         defect to report, not a surprise to find.\n{}\n",
            untraced.join("\n")
        ));
    }
    if !post_deployment.is_empty() {
        block.push_str(&format!(
            "\n\nPost-deployment acceptance criteria (advisory; do not block done): the \
             following criteria cannot be verified before shipping. Recommend moving each \
             outcome to a follow-up measurement spec blocked by the shipping spec, naming \
             its measurement window and falsifying threshold. Do not treat these as \
             untraced defects.\n{}\n",
            post_deployment.join("\n")
        ));
    }
    Some(block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::criteria::build_criteria_report;

    fn build_gate_report(root: &Path, spec: &str) -> CriteriaReport {
        let description = "## Acceptance\n\n- AC1: the widget renders\n- AC2: the widget saves\n";
        build_criteria_report(root, spec, description).unwrap()
    }

    #[test]
    fn evaluate_is_quiet_when_all_traced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("gate_test.rs"),
            "// trace:GATE-1.AC1 | ai:claude\n#[test]\nfn a() {\n    assert!(true);\n}\n\n#[test]\nfn b() {\n    // trace:GATE-1.AC2 | ai:claude\n    assert!(true);\n}\n",
        )
        .unwrap();
        let report = build_gate_report(dir.path(), "GATE-1");
        assert!(report.untested.is_empty(), "fixture should trace both ACs");
        assert_eq!(evaluate(&report, EnforceMode::Warn), CriteriaGate::Proceed);
        assert_eq!(
            evaluate(&report, EnforceMode::Refuse),
            CriteriaGate::Proceed
        );
    }

    #[test]
    fn evaluate_warns_with_verbatim_criterion_text() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("gate_test2.rs"),
            "// trace:GATE-2.AC1 | ai:claude\n#[test]\nfn a() {\n    assert!(true);\n}\n",
        )
        .unwrap();
        let report = build_gate_report(dir.path(), "GATE-2");
        assert_eq!(report.untested, vec!["GATE-2.AC2".to_string()]);
        match evaluate(&report, EnforceMode::Warn) {
            CriteriaGate::Warn(lines) => {
                assert_eq!(lines.len(), 1);
                assert!(lines[0].contains("GATE-2.AC2"));
                assert!(lines[0].contains("the widget saves"));
            }
            other => panic!("expected Warn, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_refuses_under_refuse_mode() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("gate_test3.rs"), "// no traces here\n").unwrap();
        let report = build_gate_report(dir.path(), "GATE-3");
        match evaluate(&report, EnforceMode::Refuse) {
            CriteriaGate::Refuse(lines) => {
                assert_eq!(lines.len(), 2);
                assert!(lines.iter().any(|l| l.contains("GATE-3.AC1")));
                assert!(lines.iter().any(|l| l.contains("GATE-3.AC2")));
            }
            other => panic!("expected Refuse, got {other:?}"),
        }
    }

    #[test]
    fn read_enforce_mode_defaults_to_warn_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_enforce_mode(dir.path()), EnforceMode::Warn);
    }

    #[test]
    fn read_enforce_mode_reads_refuse_from_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(
            dir.path().join(".aida/config.toml"),
            "[protocol]\nenforce = \"refuse\"\n",
        )
        .unwrap();
        assert_eq!(read_enforce_mode(dir.path()), EnforceMode::Refuse);
    }

    #[test]
    fn read_enforce_mode_unknown_value_falls_back_to_warn() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(
            dir.path().join(".aida/config.toml"),
            "[protocol]\nenforce = \"block-everything\"\n",
        )
        .unwrap();
        assert_eq!(read_enforce_mode(dir.path()), EnforceMode::Warn);
    }

    #[test]
    fn force_override_ledger_comment_names_the_verbatim_lines() {
        let lines = vec!["GATE-4.AC1 the widget renders".to_string()];
        let comment = force_override_ledger_comment("GATE-4", &lines);
        assert!(comment.contains("--force"));
        assert!(comment.contains("GATE-4.AC1 the widget renders"));
    }

    #[test]
    fn reviewer_prompt_block_is_none_when_all_traced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("gate_test4.rs"),
            "// trace:GATE-5.AC1 | ai:claude\n#[test]\nfn a() {\n    assert!(true);\n}\n\n#[test]\nfn b() {\n    // trace:GATE-5.AC2 | ai:claude\n    assert!(true);\n}\n",
        )
        .unwrap();
        let report = build_gate_report(dir.path(), "GATE-5");
        assert_eq!(
            reviewer_prompt_block(&[("GATE-5".to_string(), report)]),
            None
        );
    }

    #[test]
    fn reviewer_prompt_block_lists_untraced_criteria_per_spec() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("gate_test5.rs"), "// no traces here\n").unwrap();
        let report = build_gate_report(dir.path(), "GATE-6");
        let block = reviewer_prompt_block(&[("GATE-6".to_string(), report)])
            .expect("untraced criteria should produce a block");
        assert!(block.contains("GATE-6: GATE-6.AC1 the widget renders"));
        assert!(block.contains("GATE-6: GATE-6.AC2 the widget saves"));
        assert!(block.contains("Untraced acceptance criteria"));
    }

    #[test]
    fn mixed_states_only_untraced_criteria_block_done() {
        // trace:TASK-1293.ac1c7c47 | ai:codex
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("mixed_gate.rs"),
            "// trace:GATE-7.AC1 | ai:codex\n#[test]\nfn traced() {\n    assert!(true);\n}\n",
        )
        .unwrap();
        let description = "## Acceptance\n\n- AC1: the fixture renders\n- AC2: the export saves\n- AC3: Measured on the next 20 specs: median latency falls\n";
        let report = build_criteria_report(dir.path(), "GATE-7", description).unwrap();
        assert_eq!(report.criteria[0].state, CriterionState::Traced);
        assert_eq!(report.criteria[1].state, CriterionState::Untraced);
        assert_eq!(report.criteria[2].state, CriterionState::PostDeployment);

        assert_eq!(
            evaluate(&report, EnforceMode::Refuse),
            CriteriaGate::Refuse(vec!["GATE-7.AC2 the export saves".to_string()])
        );
        assert_eq!(
            evaluate(&report, EnforceMode::Warn),
            CriteriaGate::Warn(vec!["GATE-7.AC2 the export saves".to_string()])
        );
    }

    #[test]
    fn reviewer_prompt_separates_untraced_from_post_deployment_guidance() {
        // trace:TASK-1293.ac595b58 | ai:codex
        let dir = tempfile::tempdir().unwrap();
        let description = "## Acceptance\n\n- AC1: the export saves\n- AC2: Measured on the next 20 specs: median latency falls\n";
        let report = build_criteria_report(dir.path(), "GATE-8", description).unwrap();
        let block = reviewer_prompt_block(&[("GATE-8".to_string(), report)]).unwrap();

        assert!(block.contains("GATE-8: GATE-8.AC1 the export saves"));
        assert!(!block.contains("GATE-8: GATE-8.AC2 the export saves"));
        assert!(block.contains("Post-deployment acceptance criteria (advisory; do not block done)"));
        assert!(block.contains("GATE-8: GATE-8.AC2 Measured on the next 20 specs"));
        assert!(block.contains("follow-up measurement spec blocked by the shipping spec"));
        assert!(block.contains("measurement window and falsifying threshold"));
    }
}
