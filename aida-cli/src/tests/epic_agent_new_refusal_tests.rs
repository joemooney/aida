//! BUG-653: `aida agent new --spec <epic>` must give an epic-appropriate
//! message (you don't implement an epic; pick a child / open a focused
//! worktree) instead of routing into the Draft "transition to Approved"
//! gate, whose advice an epic can't follow (its status is a read-only
//! rollup). Non-epic types fall through untouched.
use super::epic_agent_new_refusal;
use aida_core::RequirementType;

#[test]
fn epic_gets_child_picking_message() {
    let msg = epic_agent_new_refusal(&RequirementType::Epic, "EPIC-0428")
        .expect("an epic must produce a refusal message");
    assert!(msg.contains("read-only rollup of its children"));
    assert!(msg.contains("you don't implement an epic"));
    // Routes the operator to the real next steps, not the dead-end approve.
    assert!(msg.contains("aida list --parent EPIC-0428 --status approved"));
    assert!(msg.contains("aida worktree enter EPIC-0428"));
    // Must NOT resurrect the dead-end advice (`aida edit <epic> --status
    // approved`, which an epic rollup rejects).
    assert!(!msg.contains("aida edit"));
}

#[test]
fn non_epic_types_fall_through() {
    for ty in [
        RequirementType::Bug,
        RequirementType::Task,
        RequirementType::Story,
        RequirementType::Functional,
        RequirementType::Spike,
    ] {
        assert!(
            epic_agent_new_refusal(&ty, "BUG-1").is_none(),
            "non-epic type {ty:?} must fall through to the existing readiness gate"
        );
    }
}
