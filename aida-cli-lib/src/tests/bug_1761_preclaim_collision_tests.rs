use super::{
    preclaim_collision_check, queue_work_plan_wants_pr_head_branch, PreclaimDecision,
    QueueWorkEntry, QueueWorkMode, QueueWorkPlan,
};

// trace:BUG-1761 | ai:codex
#[test]
fn open_pr_refusal_names_pr_spec_and_both_recovery_actions() {
    let decision = preclaim_collision_check(
        "BUG-1819",
        Some((
            2319,
            "Fix the pickup",
            "https://github.com/example/aida/pull/2319",
        )),
        "bug-1819",
        true,
        false,
    );
    let PreclaimDecision::Refuse(message) = decision else {
        panic!("expected refusal");
    };
    for expected in [
        "2319",
        "BUG-1819",
        "Fix the pickup",
        "https://github.com/example/aida/pull/2319",
        "OPEN",
        "--from-pr",
        "--force-claim",
    ] {
        assert!(
            message.contains(expected),
            "missing {expected:?}: {message}"
        );
    }
}

// Source-order assertion follows criteria_red_run.rs:943: the guard must be
// textually before the first claim call, which is the only mutating boundary.
#[test]
fn guard_is_before_claim_boundary() {
    let source = include_str!("../queue_cmd.rs");
    let work = source
        .split_once("pub(crate) fn handle_queue_work(")
        .unwrap()
        .1;
    let guard = work.find("// BUG-1761: the suffix allocator").unwrap();
    let claim = work.find("    session_start(").unwrap();
    assert!(guard < claim, "guard must run before claim boundary");
}

// trace:BUG-1761 | ai:codex
#[test]
fn force_claim_bypasses_open_pr_refusal() {
    assert_eq!(
        preclaim_collision_check(
            "BUG-1819",
            Some((2319, "title", "https://example.invalid/pr/2319")),
            "bug-1819",
            true,
            true,
        ),
        PreclaimDecision::Proceed
    );
}

// trace:BUG-1761 | ai:codex
#[test]
fn stale_branch_refusal_names_branch_without_suggesting_suffix() {
    let decision = preclaim_collision_check("BUG-7", None, "bug-7", true, false);
    let PreclaimDecision::Refuse(message) = decision else {
        panic!("expected refusal");
    };
    assert!(message.contains("bug-7"));
    assert!(message.contains("no open PR"));
    assert!(message.contains("--force-claim"));
    assert!(message.contains("--branch <name>"));
    assert!(message.contains("delete the stale branch"));
    assert!(message.contains("will not silently allocate `bug-7-2`"));
}

// trace:BUG-1761 | ai:codex
#[test]
fn unoccupied_branch_proceeds_and_rework_plan_bypasses_pr() {
    assert_eq!(
        preclaim_collision_check("BUG-7", None, "bug-7", false, false),
        PreclaimDecision::Proceed
    );

    let plan = QueueWorkPlan {
        mode: QueueWorkMode::Item,
        entries: vec![QueueWorkEntry {
            queue: aida_core::QueueEntry {
                user_id: "user".into(),
                requirement_id: uuid::Uuid::nil(),
                position: 0,
                added_by: "user".into(),
                note: None,
                added_at: chrono::Utc::now(),
                for_role: None,
                for_scope: None,
                for_session: None,
                added_by_machine: None,
            },
            spec_id: "BUG-7".into(),
            status_at_plan: "In Progress".into(),
        }],
        scope: "BUG-7".into(),
        review_target: None,
        anchor_display: "BUG-7".into(),
        anchor_title: "Rework".into(),
    };
    let bypass = queue_work_plan_wants_pr_head_branch(&plan);
    assert!(bypass);
    assert_eq!(
        preclaim_collision_check(
            "BUG-7",
            Some((2319, "existing PR", "https://example.invalid/pr/2319")),
            "bug-7",
            true,
            bypass,
        ),
        PreclaimDecision::Proceed
    );
}
