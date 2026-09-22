//! Tests for STORY-1424: Graded Review Engine.
//
// trace:STORY-1424 | ai:antigravity

use crate::evaluator::{EvaluatorError, MockEvaluator};
use crate::graded_review::{
    execute_graded_review, generate_graded_reviewer_prompt, parse_acceptance_criteria,
    CriterionKind, CriterionStatus,
};
use crate::review_verdict::VerdictKind;
use std::path::Path;

#[test]
fn test_parse_acceptance_criteria_mixed() {
    let desc = r#"
## Summary
Some overview of the feature.

## Acceptance
- [ ] `true`: simple exit 0 check
- Ensure error messages are clear and follow CLI conventions
- [ ] Run: `false`: negative check
- Documentation is accurate and complete
"#;

    let parsed = parse_acceptance_criteria(desc);
    assert_eq!(parsed.len(), 4);

    match &parsed[0] {
        CriterionKind::Executable { command, .. } => assert_eq!(command, "true"),
        _ => panic!("expected executable criterion"),
    }
    match &parsed[1] {
        CriterionKind::Prose { text } => assert!(text.contains("Ensure error messages")),
        _ => panic!("expected prose criterion"),
    }
    match &parsed[2] {
        CriterionKind::Executable { command, .. } => assert_eq!(command, "false"),
        _ => panic!("expected executable criterion"),
    }
    match &parsed[3] {
        CriterionKind::Prose { text } => assert!(text.contains("Documentation is accurate")),
        _ => panic!("expected prose criterion"),
    }
}

#[test]
fn test_graded_review_pure_executable_pass() {
    let desc = "## Acceptance\n- [ ] `true`\n- [ ] `echo hello`\n";
    let cwd = Path::new(".");
    let verdict = execute_graded_review(
        "TASK-100",
        "Deterministic Task",
        desc,
        "",
        "abc1234",
        cwd,
        None,
    )
    .unwrap();

    assert_eq!(verdict.verdict_kind, format!("{:?}", VerdictKind::Approved));
    assert_eq!(verdict.overall_verdict, "approved");
    assert_eq!(verdict.machine_verified_count, 2);
    assert_eq!(verdict.machine_passed_count, 2);
    assert_eq!(verdict.residual_prose_count, 0);
    assert!(!verdict.escalated_to_seat);
}

#[test]
fn test_graded_review_executable_failure_veto() {
    let desc = "## Acceptance\n- [ ] `true`\n- [ ] `false`\n- Prose criterion\n";
    let cwd = Path::new(".");
    let verdict =
        execute_graded_review("TASK-101", "Failing Task", desc, "", "abc1234", cwd, None).unwrap();

    assert_eq!(verdict.verdict_kind, format!("{:?}", VerdictKind::Rejected));
    assert_eq!(verdict.machine_verified_count, 2);
    assert_eq!(verdict.machine_passed_count, 1);
    assert!(!verdict.escalated_to_seat);
}

#[test]
fn test_graded_review_mixed_jev_fast_pass() {
    let desc = "## Acceptance\n- [ ] `true`\n- High quality error handling and clear docs\n";
    let cwd = Path::new(".");
    let mock = MockEvaluator::new().with_noul(0.98);

    let verdict = execute_graded_review(
        "TASK-102",
        "Passing Mixed Task",
        desc,
        "+ fn test() {}",
        "abc1234",
        cwd,
        Some(&mock),
    )
    .unwrap();

    assert_eq!(verdict.verdict_kind, format!("{:?}", VerdictKind::Approved));
    assert_eq!(verdict.overall_verdict, "approved");
    assert_eq!(verdict.machine_passed_count, 1);
    assert_eq!(verdict.results[1].status, CriterionStatus::Passed);
    assert_eq!(verdict.results[1].probability, Some(0.98));
    assert!(verdict.results[1].heuristic);
    assert!(!verdict.escalated_to_seat);
}

#[test]
fn test_graded_review_mixed_jev_fast_fail() {
    let desc = "## Acceptance\n- [ ] `true`\n- Proper error handling\n";
    let cwd = Path::new(".");
    let mock = MockEvaluator::new().with_noul(0.12);

    let verdict = execute_graded_review(
        "TASK-103",
        "Failing Mixed Task",
        desc,
        "- fn test() {}",
        "abc1234",
        cwd,
        Some(&mock),
    )
    .unwrap();

    assert_eq!(
        verdict.verdict_kind,
        format!("{:?}", VerdictKind::RequestChanges)
    );
    assert_eq!(verdict.overall_verdict, "request-changes");
    assert_eq!(verdict.machine_passed_count, 1);
    assert_eq!(verdict.results[1].status, CriterionStatus::Failed);
    assert_eq!(verdict.results[1].probability, Some(0.12));
    assert!(!verdict.escalated_to_seat);
}

#[test]
fn test_graded_review_mixed_escalation_zone() {
    let desc = "## Acceptance\n- [ ] `true`\n- Tasteful ergonomics\n";
    let cwd = Path::new(".");
    // p = 0.72 is in escalation zone (0.20 < p < 0.95)
    let mock = MockEvaluator::new().with_noul(0.72);

    let verdict = execute_graded_review(
        "TASK-104",
        "Ambiguous Task",
        desc,
        "+ fn test() {}",
        "abc1234",
        cwd,
        Some(&mock),
    )
    .unwrap();

    assert_eq!(verdict.overall_verdict, "escalated");
    assert!(verdict.escalated_to_seat);
    assert_eq!(verdict.results[1].status, CriterionStatus::Escalated);

    let prompt = generate_graded_reviewer_prompt("TASK-104", Some(123), &verdict);
    assert!(prompt.contains("/aida-review --pr 123"));
    assert!(prompt.contains("[GRADED REVIEW CONTEXT — STORY-1424]"));
    assert!(prompt.contains("already SETTLED"));
    assert!(prompt.contains("[PASSED (exit 0)] `true`"));
    assert!(prompt.contains("Tasteful ergonomics"));
}

#[test]
fn test_graded_review_fail_closed_on_evaluator_error() {
    let desc = "## Acceptance\n- [ ] `true`\n- Residual prose\n";
    let cwd = Path::new(".");
    let mock = MockEvaluator::new()
        .with_noul_error(EvaluatorError::Network("Connection refused".to_string()));

    let verdict = execute_graded_review(
        "TASK-105",
        "Error Task",
        desc,
        "+ fn test() {}",
        "abc1234",
        cwd,
        Some(&mock),
    )
    .unwrap();

    // PRIN-5: Fail closed -> must escalate to reviewer seat, NEVER auto-approve
    assert!(verdict.escalated_to_seat);
    assert_ne!(verdict.overall_verdict, "approved");
}

#[test]
fn test_graded_review_pure_prose_spec() {
    let desc = "## Acceptance\n- Clear documentation\n- Tasteful API\n";
    let cwd = Path::new(".");

    // Without evaluator, pure prose spec escalates to Phase 3 conversational reviewer
    let verdict =
        execute_graded_review("STORY-200", "Prose Story", desc, "", "abc1234", cwd, None).unwrap();

    assert_eq!(verdict.machine_verified_count, 0);
    assert_eq!(verdict.prose_count, 2);
    assert!(verdict.escalated_to_seat);

    let prompt = generate_graded_reviewer_prompt("STORY-200", None, &verdict);
    assert_eq!(prompt, "/aida-review --spec STORY-200");
}
