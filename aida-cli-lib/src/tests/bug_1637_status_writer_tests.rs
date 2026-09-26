//! BUG-1637: every status write records a status-history entry through the
//! one shared helper (`aida_core::conflict::record_status_transition`), with
//! the right author class. Automated writers use an automated author, so the
//! BUG-1625 merge guard keeps a terminal status the other clone reached
//! meanwhile; human writers record the caller, so a human change after an
//! automated one still wins. Each merge test runs the writer's own status code
//! on one side of a three-way merge, in both merge orientations. All tests are
//! in memory or use a temporary store.
//! trace:BUG-1637 | ai:claude
use aida_core::conflict::{self, merge_spec_three_way};
use aida_core::{Requirement, RequirementStatus};

/// A spec at `status`, last modified an hour ago, so every writer below makes
/// the NEWER write and plain last-writer-wins would pick it.
fn spec_at(status: RequirementStatus) -> Requirement {
    let mut r = Requirement::new("Spec".to_string(), String::new());
    r.spec_id = Some("BUG-9998".to_string());
    r.status = status;
    r.modified_at = chrono::Utc::now() - chrono::Duration::hours(1);
    r
}

/// The other clone moved `base` to the terminal `status` (by a person) before
/// the writer ran on this clone.
fn terminal_side(base: &Requirement, status: RequirementStatus) -> Requirement {
    let mut t = base.clone();
    conflict::set_status_recorded(&mut t, status, "joe");
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

/// Run `write` on a copy of `base`, check it moved the spec to `moved_to`
/// under `author`, and check the other clone's `terminal` status survives the
/// merge in both orientations.
fn assert_terminal_survives(
    base_status: RequirementStatus,
    terminal: RequirementStatus,
    moved_to: RequirementStatus,
    author: &str,
    write: impl FnOnce(&mut Requirement) -> bool,
) {
    let base = spec_at(base_status);
    let other = terminal_side(&base, terminal.clone());
    let mut written = base.clone();
    assert!(write(&mut written), "the writer applies on the base status");
    written.modified_at = chrono::Utc::now();
    assert_eq!(written.status, moved_to);
    assert_eq!(last_status_author(&written), author);
    assert!(conflict::is_automated_status_author(author));
    assert!(written.modified_at > other.modified_at);
    assert_eq!(
        merged_statuses(&base, &written, &other),
        [terminal.clone(), terminal]
    );
}

// trace:BUG-1637 | ai:claude
#[test]
fn pr_open_done_assertion_never_regresses_a_terminal_status() {
    assert_terminal_survives(
        RequirementStatus::InProgress,
        RequirementStatus::Completed,
        RequirementStatus::Done,
        conflict::PR_OPEN_AUTHOR,
        crate::pr_open_done_flip,
    );
    // It only moves In Progress, so it never leaves a terminal status here.
    let mut rejected = spec_at(RequirementStatus::Rejected);
    assert!(!crate::pr_open_done_flip(&mut rejected));
    assert_eq!(rejected.status, RequirementStatus::Rejected);
}

// trace:BUG-1637 | ai:claude
#[test]
fn lease_take_bump_never_regresses_a_terminal_status() {
    assert_terminal_survives(
        RequirementStatus::Approved,
        RequirementStatus::Rejected,
        RequirementStatus::InProgress,
        conflict::LEASE_TAKE_AUTHOR,
        |r| crate::approved_to_in_progress_bump(r, conflict::LEASE_TAKE_AUTHOR),
    );
}

// trace:BUG-1637 | ai:claude
#[test]
fn session_start_bump_never_regresses_a_terminal_status() {
    assert_terminal_survives(
        RequirementStatus::Approved,
        RequirementStatus::Superseded,
        RequirementStatus::InProgress,
        conflict::SESSION_START_AUTHOR,
        |r| crate::approved_to_in_progress_bump(r, conflict::SESSION_START_AUTHOR),
    );
    let mut completed = spec_at(RequirementStatus::Completed);
    assert!(!crate::approved_to_in_progress_bump(
        &mut completed,
        conflict::SESSION_START_AUTHOR
    ));
    assert_eq!(completed.status, RequirementStatus::Completed);
    assert!(completed.history.is_empty());
}

// trace:BUG-1637 | ai:claude
#[test]
fn zen_auto_approve_never_regresses_a_terminal_status() {
    assert_terminal_survives(
        RequirementStatus::Draft,
        RequirementStatus::Rejected,
        RequirementStatus::Approved,
        conflict::ZEN_APPROVE_AUTHOR,
        crate::zen_approve_flip,
    );
    let mut rejected = spec_at(RequirementStatus::Rejected);
    assert!(!crate::zen_approve_flip(&mut rejected));
    assert_eq!(rejected.status, RequirementStatus::Rejected);
}

/// The BUG-1632 review's case: base Approved, a person rejects it on the other
/// clone, and a newer phase-1 bump here must not regress Rejected to In
/// Progress.
// trace:BUG-1637 | ai:claude
#[test]
fn orchestrator_phase1_bump_never_regresses_a_terminal_status() {
    let now = chrono::Utc::now();
    assert_terminal_survives(
        RequirementStatus::Approved,
        RequirementStatus::Rejected,
        RequirementStatus::InProgress,
        conflict::ORCHESTRATOR_PHASE1_AUTHOR,
        |r| crate::phase1_status_bump(r, &RequirementStatus::InProgress, now),
    );
    // Inside the atomic write: a spec rejected since the status was read is
    // not bumped.
    let mut rejected = spec_at(RequirementStatus::Rejected);
    assert!(!crate::phase1_status_bump(
        &mut rejected,
        &RequirementStatus::InProgress,
        now
    ));
    assert_eq!(rejected.status, RequirementStatus::Rejected);
}

// trace:BUG-1637 | ai:claude
#[test]
fn orchestrator_phase1_restore_never_regresses_a_terminal_status() {
    assert_terminal_survives(
        RequirementStatus::NeedsAttention,
        RequirementStatus::Rejected,
        RequirementStatus::Approved,
        conflict::ORCHESTRATOR_PHASE1_AUTHOR,
        |r| crate::phase1_status_restore(r, &RequirementStatus::Approved),
    );
    let mut completed = spec_at(RequirementStatus::Completed);
    assert!(!crate::phase1_status_restore(
        &mut completed,
        &RequirementStatus::Approved
    ));
    assert_eq!(completed.status, RequirementStatus::Completed);
}

/// The phase-1 bump through the real store write: a spec rejected between the
/// status read and the compare-and-swap stays Rejected.
// trace:BUG-1637 | ai:claude
#[test]
fn orchestrator_phase1_bump_rechecks_inside_the_atomic_write() {
    let tmp = tempfile::tempdir().unwrap();
    let storage = aida_core::Storage::new(tmp.path().join("requirements.yaml"));
    let mut store = aida_core::models::RequirementsStore::new();
    let stale = spec_at(RequirementStatus::Approved);
    let mut current = stale.clone();
    current.status = RequirementStatus::Rejected;
    store.requirements.push(current);
    storage.save(&store).unwrap();

    let mut bumped = false;
    storage
        .update_spec_atomically(&stale, |r| {
            bumped =
                crate::phase1_status_bump(r, &RequirementStatus::InProgress, chrono::Utc::now());
        })
        .unwrap();
    assert!(!bumped);
    let reloaded = storage.load().unwrap();
    assert_eq!(reloaded.requirements[0].status, RequirementStatus::Rejected);
}

/// The loop guard refuses to leave a terminal status, and the check runs on
/// the copy read inside the atomic write: a spec completed after `queue
/// rework` read its status stays Completed.
// trace:BUG-1637 | ai:claude
#[test]
fn loop_guard_park_refuses_a_terminal_status_inside_the_atomic_write() {
    for terminal in [
        RequirementStatus::Completed,
        RequirementStatus::Rejected,
        RequirementStatus::Superseded,
    ] {
        let mut r = spec_at(terminal.clone());
        assert!(!crate::queue_cmd::loop_guard_park(&mut r));
        assert_eq!(r.status, terminal);
        assert!(r.history.is_empty());
    }

    let tmp = tempfile::tempdir().unwrap();
    let storage = aida_core::Storage::new(tmp.path().join("requirements.yaml"));
    let mut store = aida_core::models::RequirementsStore::new();
    let stale = spec_at(RequirementStatus::Approved);
    let mut current = stale.clone();
    current.status = RequirementStatus::Completed;
    store.requirements.push(current);
    storage.save(&store).unwrap();

    let mut parked = true;
    storage
        .update_spec_atomically(&stale, |r| parked = crate::queue_cmd::loop_guard_park(r))
        .unwrap();
    assert!(!parked);
    let reloaded = storage.load().unwrap();
    assert_eq!(
        reloaded.requirements[0].status,
        RequirementStatus::Completed
    );

    // A non-terminal spec is still parked, under the loop guard's author.
    let mut open = spec_at(RequirementStatus::Approved);
    assert!(crate::queue_cmd::loop_guard_park(&mut open));
    assert_eq!(open.status, RequirementStatus::NeedsAttention);
    assert_eq!(last_status_author(&open), conflict::LOOP_GUARD_AUTHOR);
}

/// A human status write after an automated one on the same side is recorded
/// under the caller, so the side is not read as all-automated and the human's
/// newer status wins over the other clone's terminal one.
// trace:BUG-1637 | ai:claude
#[test]
fn human_edit_after_an_automated_one_still_wins_the_merge() {
    let caller = crate::get_default_author();
    assert!(
        !conflict::is_automated_status_author(&caller),
        "the test caller must be a person: {caller}"
    );
    let base = spec_at(RequirementStatus::Approved);
    let rejected = terminal_side(&base, RequirementStatus::Rejected);

    let mut side = base.clone();
    assert!(crate::approved_to_in_progress_bump(
        &mut side,
        conflict::SESSION_START_AUTHOR
    ));
    // `aida queue done` / `aida edit --status done`: the caller-authored write.
    let from = side.status.clone();
    side.set_status_from_str("Done");
    crate::record_caller_status_transition(&mut side, &from);
    side.modified_at = chrono::Utc::now();

    assert_eq!(last_status_author(&side), caller);
    assert_eq!(
        merged_statuses(&base, &side, &rejected),
        [RequirementStatus::Done, RequirementStatus::Done]
    );
}

/// The legacy `aida edit` records its status change through the shared helper
/// under the caller, next to the other field changes.
// trace:BUG-1637 | ai:claude
#[test]
fn legacy_edit_records_status_through_the_helper() {
    let mut r = spec_at(RequirementStatus::InProgress);
    let changes = vec![
        Requirement::field_change("title", "Spec".into(), "Spec 2".into()),
        Requirement::field_change("status", "InProgress".into(), "Done".into()),
    ];
    r.title = "Spec 2".into();
    r.status = RequirementStatus::Done;
    crate::record_edit_changes(&mut r, "joe", &changes);
    assert_eq!(r.history.len(), 2);
    assert_eq!(r.history[0].changes.len(), 1);
    assert_eq!(r.history[0].changes[0].field_name, "title");
    assert_eq!(last_status_author(&r), "joe");
    let status = &r.history[1].changes[0];
    assert_eq!(
        (status.old_value.as_str(), status.new_value.as_str()),
        ("InProgress", "Done")
    );
}
