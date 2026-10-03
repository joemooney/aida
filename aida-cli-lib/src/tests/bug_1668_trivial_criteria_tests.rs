//! Tests for BUG-1668: a spec whose only executable criteria are trivial
//! (`true`, `echo …`, `test …`, …) must not auto-approve at Rung 2 or via the
//! evaluator fast-pass. Trivial checks still run and are recorded; a failing
//! one still vetoes.
//
// trace:BUG-1668 | ai:claude

use crate::evaluator::MockEvaluator;
use crate::graded_review::{
    execute_graded_review, generate_graded_reviewer_prompt, is_trivial_command,
    AcceptanceCommandPolicy, CriterionStatus, TRIVIAL_COMMAND_WORDS, TRIVIAL_ONLY_SUMMARY,
};
use std::path::Path;

fn run(desc: &str, evaluator: Option<&MockEvaluator>) -> crate::graded_review::GradedReviewVerdict {
    execute_graded_review(
        "TASK-1668",
        "Trivial checks",
        desc,
        "+ fn changed() {}",
        "abc1234",
        Path::new("."),
        evaluator.map(|m| m as &dyn crate::evaluator::EvaluatorEngine),
        &AcceptanceCommandPolicy::permissive(),
    )
    .unwrap()
}

/// The list is defined once and classifies on the first word only.
#[test]
fn bug_1668_trivial_is_decided_by_first_word() {
    for word in TRIVIAL_COMMAND_WORDS {
        assert!(is_trivial_command(word), "{word} should be trivial");
        assert!(
            is_trivial_command(&format!("{word} --anything at all")),
            "{word} with arguments should be trivial"
        );
    }
    assert!(is_trivial_command("[ -f Cargo.toml ]"));
    assert!(is_trivial_command("exit 0"));
    assert!(is_trivial_command("  echo   ok"));
    assert!(is_trivial_command(""));
    assert!(!is_trivial_command("cargo test"));
    assert!(!is_trivial_command("git --version"));
    assert!(!is_trivial_command("tests/smoke.sh"));
    assert!(!is_trivial_command("./run"));
    assert!(!is_trivial_command("truex"));
}

/// A spec whose only criterion is the backticked command true escalates.
#[cfg(unix)]
#[test]
fn bug_1668_only_true_escalates() {
    let verdict = run("## Acceptance\n- [ ] `true`\n", None);
    assert_eq!(verdict.machine_verified_count, 1);
    assert_eq!(verdict.machine_passed_count, 1);
    assert_eq!(verdict.results[0].status, CriterionStatus::Passed);
    assert_ne!(verdict.overall_verdict, "approved");
    assert_eq!(verdict.overall_verdict, "escalated");
    assert!(verdict.escalated_to_seat);
    assert!(verdict.summary.contains(TRIVIAL_ONLY_SUMMARY));
}

/// Several trivial commands are no better than one.
#[cfg(unix)]
#[test]
fn bug_1668_several_trivial_commands_still_escalate() {
    let verdict = run(
        "## Acceptance\n- [ ] `true`\n- [ ] `echo hello`\n- [ ] `test -n x`\n- [ ] `[ -d . ]`\n",
        None,
    );
    assert_eq!(verdict.machine_passed_count, 4);
    assert_eq!(verdict.overall_verdict, "escalated");
    assert!(verdict.escalated_to_seat);
}

/// true plus a non-trivial passing command auto-approves as today.
#[cfg(unix)]
#[test]
fn bug_1668_true_plus_nontrivial_command_approves() {
    let verdict = run("## Acceptance\n- [ ] `true`\n- [ ] `git --version`\n", None);
    assert_eq!(verdict.machine_passed_count, 2);
    assert_eq!(verdict.overall_verdict, "approved");
    assert!(!verdict.escalated_to_seat);
    assert!(!verdict.summary.contains(TRIVIAL_ONLY_SUMMARY));
}

/// Only "echo ok" and an evaluator that would say p=1.0: no approval.
#[cfg(unix)]
#[test]
fn bug_1668_only_echo_with_evaluator_p1_does_not_approve() {
    let mock = MockEvaluator::new().with_noul_confidence(1.0, 1.0);
    let verdict = run("## Acceptance\n- [ ] `echo ok`\n", Some(&mock));
    assert_ne!(verdict.overall_verdict, "approved");
    assert!(verdict.escalated_to_seat);
    // Nothing was handed to the evaluator: there is no prose to evaluate.
    assert_eq!(mock.noul_queue.lock().unwrap().len(), 1);
}

/// A failing trivial command still vetoes, unchanged.
#[cfg(unix)]
#[test]
fn bug_1668_false_still_rejects() {
    let verdict = run("## Acceptance\n- [ ] `true`\n- [ ] `false`\n", None);
    assert_eq!(verdict.overall_verdict, "rejected");
    assert!(!verdict.escalated_to_seat);
    assert!(!verdict.summary.contains(TRIVIAL_ONLY_SUMMARY));
}

/// Trivial commands next to prose keep the existing prose path: the
/// evaluator decides, and its fast-pass still applies (the settled prose is
/// the evidence, not the trivial command).
#[cfg(unix)]
#[test]
fn bug_1668_trivial_plus_prose_still_uses_evaluator() {
    let mock = MockEvaluator::new().with_noul(0.72);
    let verdict = run(
        "## Acceptance\n- [ ] `true`\n- Error messages stay clear\n",
        Some(&mock),
    );
    assert_eq!(verdict.overall_verdict, "escalated");
    assert_eq!(verdict.results[1].status, CriterionStatus::Escalated);
    assert!(!verdict.summary.contains(TRIVIAL_ONLY_SUMMARY));
}

/// The seat prompt flags that every passed check is trivial.
#[cfg(unix)]
#[test]
fn bug_1668_prompt_notes_trivial_only_passes() {
    let verdict = run("## Acceptance\n- [ ] `true`\n", None);
    let prompt = generate_graded_reviewer_prompt("TASK-1668", Some(9), &verdict);
    assert!(prompt.contains("[PASSED (exit 0)] `true`"));
    assert!(prompt.contains("trivial and verifies nothing"));

    let verdict = run("## Acceptance\n- [ ] `git --version`\n- Prose left\n", None);
    let prompt = generate_graded_reviewer_prompt("TASK-1668", Some(9), &verdict);
    assert!(prompt.contains("[PASSED (exit 0)] `git --version`"));
    assert!(!prompt.contains("trivial and verifies nothing"));
}
