//! BUG-1632: automated status writers record their transition in the status
//! history under an automated author, so the BUG-1625 terminal-status merge
//! guard keeps a terminal status the other clone reached meanwhile. Each test
//! runs the writer's own status code on one side of a three-way merge whose
//! other side is terminal, in both merge orientations.
//! trace:BUG-1632 | ai:claude
use aida_core::conflict::merge_spec_three_way;
use aida_core::{Requirement, RequirementStatus};

/// A spec at `status`, last modified an hour ago, so every writer below makes
/// the NEWER write and plain last-writer-wins would pick it.
fn spec_at(status: RequirementStatus) -> Requirement {
    let mut r = Requirement::new("Spec".to_string(), String::new());
    r.spec_id = Some("BUG-9999".to_string());
    r.status = status;
    r.modified_at = chrono::Utc::now() - chrono::Duration::hours(1);
    r
}

/// The other clone moved `base` to the terminal `status` (by a person, with no
/// automated evidence), before the writer ran on this clone.
fn terminal_side(base: &Requirement, status: RequirementStatus) -> Requirement {
    let mut t = base.clone();
    let from = t.status.clone();
    t.status = status;
    aida_core::conflict::record_status_transition(&mut t, "joe", &from);
    t.modified_at = base.modified_at + chrono::Duration::minutes(1);
    t
}

/// Merge in both orientations and return both results' statuses.
fn merged_statuses(base: &Requirement, a: &Requirement, b: &Requirement) -> [RequirementStatus; 2] {
    [
        merge_spec_three_way(base, a, b).status,
        merge_spec_three_way(base, b, a).status,
    ]
}

fn last_status_author(r: &Requirement) -> &str {
    r.history
        .iter()
        .rev()
        .find(|h| h.changes.iter().any(|c| c.field_name == "status"))
        .map(|h| h.author.as_str())
        .expect("a status history entry")
}

// trace:BUG-1632 | ai:claude
#[test]
fn orchestrator_shelve_never_regresses_a_terminal_status() {
    let base = spec_at(RequirementStatus::InProgress);
    let completed = terminal_side(&base, RequirementStatus::Completed);
    let mut shelved = base.clone();
    assert!(crate::shelve_status_flip(&mut shelved));
    assert_eq!(shelved.status, RequirementStatus::NeedsAttention);
    assert_eq!(last_status_author(&shelved), "aida-orchestrator");
    assert!(shelved.modified_at > completed.modified_at);
    assert_eq!(
        merged_statuses(&base, &shelved, &completed),
        [RequirementStatus::Completed, RequirementStatus::Completed]
    );

    // A spec already terminal on this clone is never moved by the shelve.
    let mut done = spec_at(RequirementStatus::Completed);
    assert!(!crate::shelve_status_flip(&mut done));
    assert_eq!(done.status, RequirementStatus::Completed);
    assert!(done.history.is_empty());
}

// trace:BUG-1632 | ai:claude
#[test]
fn queue_rework_loop_guard_never_regresses_a_terminal_status() {
    // The rework already returned the spec to Approved and that write reached
    // the other clone, which then rejected it. This clone's loop guard then
    // parks it again.
    let base = spec_at(RequirementStatus::Approved);
    let rejected = terminal_side(&base, RequirementStatus::Rejected);
    let mut parked = base.clone();
    crate::queue_cmd::loop_guard_park(&mut parked);
    assert_eq!(parked.status, RequirementStatus::NeedsAttention);
    assert_eq!(last_status_author(&parked), "aida-loop-guard");
    assert_eq!(
        merged_statuses(&base, &parked, &rejected),
        [RequirementStatus::Rejected, RequirementStatus::Rejected]
    );
}

// trace:BUG-1632 | ai:claude
#[test]
fn mcp_punt_never_regresses_a_terminal_status() {
    let base = spec_at(RequirementStatus::InProgress);
    let completed = terminal_side(&base, RequirementStatus::Completed);
    let mut punted = base.clone();
    assert!(crate::mcp::mcp_punt_status_flip(&mut punted));
    assert_eq!(punted.status, RequirementStatus::NeedsAttention);
    assert_eq!(last_status_author(&punted), "aida-mcp-punt");
    assert_eq!(
        merged_statuses(&base, &punted, &completed),
        [RequirementStatus::Completed, RequirementStatus::Completed]
    );

    // A spec already terminal on this clone is never moved by the punt.
    let mut superseded = spec_at(RequirementStatus::Superseded);
    assert!(!crate::mcp::mcp_punt_status_flip(&mut superseded));
    assert_eq!(superseded.status, RequirementStatus::Superseded);
}

fn requeue_ctx(author: &str, automated: bool) -> crate::requeue::ReturnCtx {
    crate::requeue::ReturnCtx {
        via: "test".to_string(),
        via_slug: "queue-rework",
        author: author.to_string(),
        clear_escalation: false,
        reason: None,
        automated,
    }
}

// trace:BUG-1632 | ai:claude
#[test]
fn requeue_automated_return_never_regresses_a_terminal_status() {
    let base = spec_at(RequirementStatus::NeedsAttention);
    let rejected = terminal_side(&base, RequirementStatus::Rejected);
    let mut returned = base.clone();
    let outcome = crate::requeue::return_to_flight(
        &mut returned,
        &RequirementStatus::NeedsAttention,
        &RequirementStatus::Approved,
        &requeue_ctx("supervisor", true),
    );
    assert!(outcome.applied());
    assert_eq!(last_status_author(&returned), "aida-supervisor");
    assert_eq!(
        merged_statuses(&base, &returned, &rejected),
        [RequirementStatus::Rejected, RequirementStatus::Rejected]
    );
}

/// A person's requeue records the person, so the guard stays off and the
/// ordinary merge (the newer write) stands exactly as before BUG-1632.
// trace:BUG-1632 | ai:claude
#[test]
fn requeue_human_return_still_wins_the_merge() {
    let base = spec_at(RequirementStatus::NeedsAttention);
    let rejected = terminal_side(&base, RequirementStatus::Rejected);
    let mut returned = base.clone();
    let outcome = crate::requeue::return_to_flight(
        &mut returned,
        &RequirementStatus::NeedsAttention,
        &RequirementStatus::Approved,
        &requeue_ctx("joe", false),
    );
    assert!(outcome.applied());
    assert_eq!(last_status_author(&returned), "joe");
    assert_eq!(
        merged_statuses(&base, &returned, &rejected),
        [RequirementStatus::Approved, RequirementStatus::Approved]
    );
}

/// A person reopening a terminal spec on this clone wins over the other
/// clone's untouched terminal status, even after an automated writer (here
/// the shelve) moved it again on the same side: the person-authored entry
/// keeps the guard off.
// trace:BUG-1632 | ai:claude
#[test]
fn orchestrator_shelve_after_a_human_reopen_still_merges() {
    let base = spec_at(RequirementStatus::Completed);
    let mut remote = base.clone();
    remote.title = "Spec (renamed)".to_string();
    remote.modified_at = base.modified_at + chrono::Duration::minutes(1);

    let mut reopened = base.clone();
    reopened.status = RequirementStatus::InProgress;
    aida_core::conflict::record_status_transition(
        &mut reopened,
        "joe",
        &RequirementStatus::Completed,
    );
    assert!(crate::shelve_status_flip(&mut reopened));
    for merged in [
        merge_spec_three_way(&base, &reopened, &remote),
        merge_spec_three_way(&base, &remote, &reopened),
    ] {
        assert_eq!(merged.status, RequirementStatus::NeedsAttention);
        assert_eq!(merged.title, "Spec (renamed)");
    }
}
