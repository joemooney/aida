//! BUG-1651: when `aida findings promote --to work` fails its status write,
//! the queue rollback is compare-and-swap: it undoes the queue add only while
//! the spec's entry is still the one the promote added. A concurrent
//! `aida queue remove` or same-role re-add, injected through BUG-1638's race
//! seam between the queue add and the status write, is left alone.
//!
//! Also: an earlier entry positioned at `i64::MAX` is restored exactly, and
//! the phase-1 restore line (both outcomes) is suppressed under `--json`.
//!
//! Every test uses a temporary git-canonical store.
//! trace:BUG-1651 | ai:claude
use std::path::Path;

use aida_core::{QueueEntry, RequirementStatus};

use crate::bug_1638_race_seam_tests::{
    arm_race, concurrently_set, open_backend, pin_queue_user, project_with, SPEC,
};

fn promote_to_work(store_root: &Path, stale: &aida_core::Requirement) -> anyhow::Result<String> {
    crate::findings_promote_to_work(
        &open_backend(store_root),
        store_root,
        stale,
        SPEC,
        Some("implementer"),
        None,
        chrono::Utc::now(),
    )
}

fn entry(id: uuid::Uuid, role: &str, note: &str, position: i64) -> QueueEntry {
    let user = crate::current_user_id(None);
    QueueEntry {
        user_id: user.clone(),
        requirement_id: id,
        position,
        added_by: user,
        note: Some(note.into()),
        added_at: chrono::Utc::now(),
        for_role: Some(role.into()),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    }
}

fn queue(store_root: &Path) -> Vec<QueueEntry> {
    aida_core::Storage::new(store_root)
        .queue_list(&crate::current_user_id(None), true)
        .unwrap()
}

fn add(store_root: &Path, e: QueueEntry) {
    aida_core::Storage::new(store_root).queue_add(e).unwrap();
}

/// Someone removes the spec from the queue while promote is between its
/// queue add and its status write. The rollback must not put the earlier
/// entry back over that removal.
// trace:BUG-1651 | ai:claude
#[test]
fn bug_1651_rollback_keeps_a_concurrent_queue_remove() {
    let _user = pin_queue_user();
    let (_dir, _root, store_root, stale) = project_with(RequirementStatus::Draft);
    add(
        &store_root,
        entry(stale.id, "reviewer", "queued earlier", 5000),
    );
    let id = stale.id;
    let fired = arm_race(move |s| {
        concurrently_set(s, RequirementStatus::Rejected);
        aida_core::Storage::new(s)
            .queue_remove(&crate::current_user_id(None), &id)
            .unwrap();
    });

    let err = promote_to_work(&store_root, &stale).expect_err("a final status is refused");
    assert!(fired.get(), "the race seam fired");
    let msg = err.to_string();
    assert!(msg.contains("is now Rejected"), "{msg}");
    assert!(msg.contains("removed by someone else"), "{msg}");
    assert!(msg.contains("the finding is unchanged"), "{msg}");
    assert!(!msg.contains("was withdrawn"), "{msg}");
    assert!(
        queue(&store_root).is_empty(),
        "the concurrent remove was kept: {:?}",
        queue(&store_root)
    );
}

/// Someone re-adds the spec for the same role in the window. The role-scoped
/// withdrawal must not drop their entry.
// trace:BUG-1651 | ai:claude
#[test]
fn bug_1651_rollback_keeps_a_concurrent_same_role_re_add() {
    let _user = pin_queue_user();
    let (_dir, _root, store_root, stale) = project_with(RequirementStatus::Draft);
    let id = stale.id;
    let fired = arm_race(move |s| {
        concurrently_set(s, RequirementStatus::Rejected);
        add(
            s,
            entry(id, "implementer", "re-added by a teammate", i64::MAX),
        );
    });

    let err = promote_to_work(&store_root, &stale).expect_err("a final status is refused");
    assert!(fired.get(), "the race seam fired");
    let msg = err.to_string();
    assert!(msg.contains("changed by someone else"), "{msg}");
    assert!(msg.contains("the finding is unchanged"), "{msg}");
    let entries = queue(&store_root);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].for_role.as_deref(), Some("implementer"));
    assert_eq!(entries[0].note.as_deref(), Some("re-added by a teammate"));
}

/// With an earlier entry, a concurrent re-add in the window is kept rather
/// than overwritten by the restore.
// trace:BUG-1651 | ai:claude
#[test]
fn bug_1651_rollback_does_not_restore_over_a_concurrent_re_add() {
    let _user = pin_queue_user();
    let (_dir, _root, store_root, stale) = project_with(RequirementStatus::Draft);
    add(
        &store_root,
        entry(stale.id, "reviewer", "queued earlier", 5000),
    );
    let id = stale.id;
    let fired = arm_race(move |s| {
        concurrently_set(s, RequirementStatus::Superseded);
        add(s, entry(id, "implementer", "re-added by a teammate", 7000));
    });

    let err = promote_to_work(&store_root, &stale).expect_err("a final status is refused");
    assert!(fired.get(), "the race seam fired");
    assert!(err.to_string().contains("changed by someone else"), "{err}");
    let entries = queue(&store_root);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].note.as_deref(), Some("re-added by a teammate"));
    assert_eq!(entries[0].position, 7000);
}

/// An undisturbed rollback still withdraws, and says so in the new wording.
// trace:BUG-1651 | ai:claude
#[test]
fn bug_1651_undisturbed_rollback_withdraws_with_truthful_wording() {
    let _user = pin_queue_user();
    let (_dir, _root, store_root, stale) = project_with(RequirementStatus::Draft);
    let fired = arm_race(|s| concurrently_set(s, RequirementStatus::Rejected));

    let err = promote_to_work(&store_root, &stale).expect_err("a final status is refused");
    assert!(fired.get(), "the race seam fired");
    let msg = err.to_string();
    assert!(
        msg.contains("Its implementer queue entry was withdrawn; the finding is unchanged."),
        "{msg}"
    );
    assert!(!msg.contains("nothing was changed"), "{msg}");
    assert!(queue(&store_root).is_empty());
}

/// An earlier entry stored at `i64::MAX` (a legacy unresolved sentinel) is
/// put back at exactly that position, not re-resolved to the bottom.
// trace:BUG-1651 | ai:claude
#[test]
fn bug_1651_rollback_restores_an_i64_max_position_exactly() {
    let _user = pin_queue_user();
    let (_dir, _root, store_root, stale) = project_with(RequirementStatus::Draft);
    let storage = aida_core::Storage::new(&store_root);
    let user = crate::current_user_id(None);
    add(&store_root, entry(stale.id, "reviewer", "legacy entry", 0));
    // `queue_add` resolves the sentinel; `queue_reorder` stores it as given.
    storage
        .queue_reorder(&user, &[(stale.id, i64::MAX)])
        .unwrap();
    assert_eq!(queue(&store_root)[0].position, i64::MAX, "setup");
    let fired = arm_race(|s| concurrently_set(s, RequirementStatus::Rejected));

    let err = promote_to_work(&store_root, &stale).expect_err("a final status is refused");
    assert!(fired.get(), "the race seam fired");
    assert!(err.to_string().contains("was withdrawn"), "{err}");
    let entries = queue(&store_root);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].note.as_deref(), Some("legacy entry"));
    assert_eq!(entries[0].position, i64::MAX);
}

/// The orchestrator's phase-1 restore line, restored or refused, is printed
/// only without `--json`.
// trace:BUG-1651 trace:BUG-1647 | ai:claude
#[test]
fn bug_1651_phase1_restore_line_is_suppressed_under_json() {
    let prior = RequirementStatus::Approved;
    let restored = crate::Phase1RestoreOutcome::Restored;
    let refused = crate::Phase1RestoreOutcome::Refused("it is now Rejected".into());
    for outcome in [&restored, &refused] {
        assert_eq!(
            crate::phase1_restore_report_line(outcome, SPEC, &prior, true),
            None,
            "{outcome:?} must be silent under --json"
        );
        let line = crate::phase1_restore_report_line(outcome, SPEC, &prior, false)
            .unwrap_or_else(|| panic!("{outcome:?} is printed without --json"));
        assert!(
            line.contains(&crate::phase1_restore_outcome_message(
                outcome, SPEC, &prior
            )),
            "{line}"
        );
    }
}

/// Review nit: when the earlier entry is put back but its exact `i64::MAX`
/// position cannot be re-set, the error reports a withdrawal with an inexact
/// position, never "could not be withdrawn" or an `aida queue remove` hint.
/// Only a failed withdrawal gets that hint.
// trace:BUG-1651 | ai:claude
#[test]
fn bug_1651_reorder_failure_reports_an_inexact_position_not_a_failed_withdrawal() {
    use crate::PromoteQueueWithdrawal;
    let write_err = anyhow::anyhow!("{SPEC} is now Rejected, a final status");
    let inexact = crate::promote_rollback_error(
        &write_err,
        "implementer",
        SPEC,
        &Ok(PromoteQueueWithdrawal::WithdrawnPositionInexact(
            "disk full".into(),
        )),
    );
    assert!(inexact.contains("was withdrawn"), "{inexact}");
    assert!(
        inexact.contains("position could not be restored exactly (disk full)"),
        "{inexact}"
    );
    assert!(inexact.contains("The finding is unchanged."), "{inexact}");
    assert!(!inexact.contains("could not be withdrawn"), "{inexact}");
    assert!(!inexact.contains("aida queue remove"), "{inexact}");

    let failed = crate::promote_rollback_error(
        &write_err,
        "implementer",
        SPEC,
        &Err(anyhow::anyhow!("queue file unreadable")),
    );
    assert!(failed.contains("could not be withdrawn"), "{failed}");
    assert!(
        failed.contains(&format!("aida queue remove {SPEC} --for implementer")),
        "{failed}"
    );
}
