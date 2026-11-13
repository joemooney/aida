// trace:TASK-3424 | ai:codex
use super::*;

#[test]
fn every_menu_ends_with_skip() {
    for class in [
        UnblockClass::NeedsApproval,
        UnblockClass::ApprovedUnqueued,
        UnblockClass::UnderSpecified,
        UnblockClass::DecisionPending,
        UnblockClass::HeldForReview,
        UnblockClass::BlockedBy,
        UnblockClass::BuildSupervised,
        UnblockClass::Deferred,
    ] {
        let menu = sweep_menu(class);
        assert_eq!(
            *menu.last().unwrap(),
            SweepChoice::Skip,
            "menu for {class:?} must end with Skip"
        );
    }
}

#[test]
fn needs_approval_offers_approve_reject_then_common_tail() {
    assert_eq!(
        sweep_menu(UnblockClass::NeedsApproval),
        vec![
            SweepChoice::ApproveQueue,
            SweepChoice::Reject,
            SweepChoice::ShowDetails,
            SweepChoice::AskAdvisor,
            SweepChoice::Skip
        ]
    );
}

#[test]
fn actionable_menus_carry_details_and_ask_advisor() {
    for class in [
        UnblockClass::NeedsApproval,
        UnblockClass::ApprovedUnqueued,
        UnblockClass::UnderSpecified,
        UnblockClass::DecisionPending,
        UnblockClass::HeldForReview,
        UnblockClass::BlockedBy,
    ] {
        let menu = sweep_menu(class);
        assert!(menu.contains(&SweepChoice::ShowDetails), "{class:?}");
        assert!(menu.contains(&SweepChoice::AskAdvisor), "{class:?}");
    }
    // Info rows have no tail — just Skip.
    assert_eq!(sweep_menu(UnblockClass::Deferred), vec![SweepChoice::Skip]);
}

#[test]
fn only_draft_and_approved_unqueued_are_advisor_groomable() {
    assert!(sweep_is_advisor_groomable(UnblockClass::NeedsApproval));
    assert!(sweep_is_advisor_groomable(UnblockClass::ApprovedUnqueued));
    assert!(!sweep_is_advisor_groomable(UnblockClass::DecisionPending));
    assert!(!sweep_is_advisor_groomable(UnblockClass::UnderSpecified));
    assert!(!sweep_is_advisor_groomable(UnblockClass::BlockedBy));
}

#[test]
fn decision_pending_offers_guided_before_note() {
    let menu = sweep_menu(UnblockClass::DecisionPending);
    assert_eq!(menu[0], SweepChoice::LaunchGuided);
    assert_eq!(menu[1], SweepChoice::AddNote);
}

#[test]
fn cheapest_first_puts_approval_before_design() {
    assert!(
        sweep_cost_rank(UnblockClass::NeedsApproval)
            < sweep_cost_rank(UnblockClass::DecisionPending),
        "one-keystroke approvals must walk before design forks"
    );
    assert!(
        sweep_cost_rank(UnblockClass::DecisionPending) < sweep_cost_rank(UnblockClass::Deferred),
        "leave-parked rows sink below actionable ones"
    );
}

#[test]
fn walk_order_is_cheapest_first_and_stable() {
    let mk = |id: &str, class| UnblockLine {
        id: id.to_string(),
        title: String::new(),
        class,
        reason: String::new(),
    };
    let ordered = sweep_walk_order(vec![
        mk("A", UnblockClass::DecisionPending),
        mk("B", UnblockClass::NeedsApproval),
        mk("C", UnblockClass::NeedsApproval),
        mk("D", UnblockClass::BlockedBy),
    ]);
    let ids: Vec<&str> = ordered.iter().map(|l| l.id.as_str()).collect();
    // approvals first (B,C stable), then blocked-by, then the design fork.
    assert_eq!(ids, vec!["B", "C", "D", "A"]);
}

#[test]
fn parked_classes_are_not_actionable() {
    assert!(!sweep_is_actionable(UnblockClass::Deferred));
    assert!(!sweep_is_actionable(UnblockClass::BuildSupervised));
    assert!(sweep_is_actionable(UnblockClass::NeedsApproval));
    assert!(sweep_is_actionable(UnblockClass::BlockedBy));
}
