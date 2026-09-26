//! BUG-1647: `aida findings dismiss` writes atomically, and the findings
//! promote edge cases found in BUG-1638's review.
//!
//! 1. Dismiss re-reads the finding under the store lock (a concurrent edit
//!    made between its read and its write, injected through BUG-1638's race
//!    seam, is kept), and a finding deleted meanwhile fails cleanly.
//! 2. Plain promote withdraws its queue entry when the status write does not
//!    land, and says so truthfully when it cannot.
//! 3. The Approved write refuses a finding that is now in a final status.
//! 4. Auto-complete of an already-Completed finding changes nothing; Rejected
//!    or Superseded is refused.
//! 5. `transition_to_completed_atomically` emits exactly one ship record per
//!    real completion.
//! 6. The phase-1 restore line shows statuses as the CLI shows them.
//!
//! Every test uses a temporary git-canonical store.
//! trace:BUG-1647 | ai:claude
use std::path::Path;

use aida_core::db::DatabaseBackend;
use aida_core::{Requirement, RequirementStatus};

use crate::bug_1638_race_seam_tests::{
    arm_race, comment_texts, concurrently_comment_and_redescribe, concurrently_set,
    last_status_author, on_disk, open_backend, project_with, SPEC,
};

fn queued_ids(store_root: &Path) -> Vec<uuid::Uuid> {
    aida_core::Storage::new(store_root)
        .queue_list(&crate::current_user_id(None), true)
        .unwrap()
        .into_iter()
        .map(|e| e.requirement_id)
        .collect()
}

fn ship_records(root: &Path, spec: &str) -> usize {
    crate::events::read_all(root)
        .into_iter()
        .filter(|e| {
            e.spec.as_deref() == Some(spec)
                && matches!(e.kind, crate::events::EventKind::SpecCompleted { .. })
        })
        .count()
}

fn status_entries(r: &Requirement) -> usize {
    r.history
        .iter()
        .filter(|h| h.changes.iter().any(|c| c.field_name == "status"))
        .count()
}

// ---------------------------------------------------------------------------
// 1. Dismiss.
// ---------------------------------------------------------------------------

// trace:BUG-1647 | ai:claude
#[test]
fn bug_1647_dismiss_keeps_an_edit_made_between_read_and_write() {
    let (_dir, _root, store_root, _) = project_with(RequirementStatus::Draft);
    let fired = arm_race(concurrently_comment_and_redescribe);

    let backend = open_backend(&store_root);
    let outcome = crate::findings_dismiss(
        &backend,
        &store_root,
        SPEC,
        Some("not a real issue"),
        chrono::Utc::now(),
    )
    .unwrap();
    assert!(fired.get(), "the race seam fired");
    assert_eq!(outcome, crate::DismissOutcome::Dismissed);

    let after = on_disk(&store_root).unwrap();
    assert_eq!(after.status, RequirementStatus::Rejected);
    assert_eq!(after.description, "edited concurrently");
    let texts = comment_texts(&after);
    assert!(texts.contains(&"concurrent comment"), "{texts:?}");
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("Dismissed by") && t.ends_with("not a real issue")),
        "{texts:?}"
    );
    assert_eq!(last_status_author(&after), crate::get_default_author());
}

// trace:BUG-1647 | ai:claude
#[test]
fn bug_1647_dismiss_of_a_finding_deleted_between_read_and_write_fails_cleanly() {
    let (_dir, _root, store_root, read) = project_with(RequirementStatus::Draft);
    let id = read.id;
    let fired = arm_race(move |s| open_backend(s).delete_requirement(&id).unwrap());

    let backend = open_backend(&store_root);
    let err = crate::findings_dismiss(&backend, &store_root, SPEC, None, chrono::Utc::now())
        .expect_err("dismissing a deleted finding must fail");
    assert!(fired.get(), "the race seam fired");
    let msg = err.to_string();
    assert!(msg.contains("no longer exists"), "{msg}");
    assert!(msg.contains("nothing was changed"), "{msg}");
    assert!(on_disk(&store_root).is_none(), "nothing was recreated");
}

/// A finding dismissed by someone else between the read and the write is a
/// no-op: no second audit comment or history entry.
// trace:BUG-1647 | ai:claude
#[test]
fn bug_1647_dismiss_of_an_already_rejected_finding_changes_nothing() {
    let (_dir, _root, store_root, _) = project_with(RequirementStatus::Draft);
    let fired = arm_race(|s| concurrently_set(s, RequirementStatus::Rejected));
    let backend = open_backend(&store_root);

    let outcome = crate::findings_dismiss(
        &backend,
        &store_root,
        SPEC,
        Some("again"),
        chrono::Utc::now(),
    )
    .unwrap();
    assert!(fired.get(), "the race seam fired");
    assert_eq!(outcome, crate::DismissOutcome::AlreadyDismissed);
    let after = on_disk(&store_root).unwrap();
    assert_eq!(after.status, RequirementStatus::Rejected);
    assert!(
        !comment_texts(&after)
            .iter()
            .any(|t| t.starts_with("Dismissed by")),
        "no dismissal comment was added"
    );
    // Only the concurrent writer's transition is in the history.
    assert_eq!(status_entries(&after), 1);
    assert_eq!(last_status_author(&after), "joe");

    // A second dismiss of the stored Rejected finding is also a no-op.
    let before = on_disk(&store_root).unwrap();
    assert_eq!(
        crate::findings_dismiss(&backend, &store_root, SPEC, None, chrono::Utc::now()).unwrap(),
        crate::DismissOutcome::AlreadyDismissed
    );
    let again = on_disk(&store_root).unwrap();
    assert_eq!(again.comments.len(), before.comments.len());
    assert_eq!(again.history.len(), before.history.len());
    assert_eq!(again.modified_at, before.modified_at);
}

/// A finding that reached Completed or Superseded between the read and the
/// write is refused, and nothing is written.
// trace:BUG-1647 | ai:claude
#[test]
fn bug_1647_dismiss_refuses_a_completed_or_superseded_finding() {
    for terminal in [RequirementStatus::Completed, RequirementStatus::Superseded] {
        let (_dir, _root, store_root, _) = project_with(RequirementStatus::Draft);
        let status = terminal.clone();
        let fired = arm_race(move |s| concurrently_set(s, status));
        let backend = open_backend(&store_root);

        let err = crate::findings_dismiss(&backend, &store_root, SPEC, None, chrono::Utc::now())
            .expect_err("a final status is refused");
        assert!(fired.get(), "the race seam fired");
        let msg = err.to_string();
        assert!(
            msg.contains(&format!(
                "is now {terminal}, a final status, so it was not dismissed"
            )),
            "{msg}"
        );
        let after = on_disk(&store_root).unwrap();
        assert_eq!(after.status, terminal);
        assert!(
            !comment_texts(&after)
                .iter()
                .any(|t| t.starts_with("Dismissed")),
            "no dismissal comment was added"
        );
        assert_eq!(status_entries(&after), 1, "only the concurrent transition");
    }
}

// ---------------------------------------------------------------------------
// 2. Plain promote: the queue entry follows the status write.
// ---------------------------------------------------------------------------

fn promote_to_work(store_root: &Path, stale: &Requirement) -> anyhow::Result<String> {
    let backend = open_backend(store_root);
    crate::findings_promote_to_work(
        &backend,
        store_root,
        stale,
        SPEC,
        Some("implementer"),
        None,
        chrono::Utc::now(),
    )
}

// trace:BUG-1647 | ai:claude
#[test]
fn bug_1647_promote_queues_and_approves() {
    let (_dir, _root, store_root, stale) = project_with(RequirementStatus::Draft);
    let role = promote_to_work(&store_root, &stale).unwrap();
    assert_eq!(role, "implementer");
    assert_eq!(
        on_disk(&store_root).unwrap().status,
        RequirementStatus::Approved
    );
    assert_eq!(queued_ids(&store_root), vec![stale.id]);
}

// trace:BUG-1647 | ai:claude
#[test]
fn bug_1647_promote_withdraws_the_queue_entry_when_the_finding_became_final() {
    let (_dir, _root, store_root, stale) = project_with(RequirementStatus::Draft);
    let fired = arm_race(|s| concurrently_set(s, RequirementStatus::Rejected));

    let err = promote_to_work(&store_root, &stale).expect_err("a final status is refused");
    assert!(fired.get(), "the race seam fired");
    let msg = err.to_string();
    assert!(msg.contains("is now Rejected"), "{msg}");
    assert!(msg.contains("the finding is unchanged"), "{msg}");
    assert_eq!(
        on_disk(&store_root).unwrap().status,
        RequirementStatus::Rejected
    );
    assert!(
        queued_ids(&store_root).is_empty(),
        "the entry was withdrawn"
    );
}

// trace:BUG-1647 | ai:claude
#[test]
fn bug_1647_promote_withdraws_the_queue_entry_when_the_finding_was_deleted() {
    let (_dir, _root, store_root, stale) = project_with(RequirementStatus::Draft);
    let id = stale.id;
    let fired = arm_race(move |s| open_backend(s).delete_requirement(&id).unwrap());

    let err = promote_to_work(&store_root, &stale).expect_err("a deleted finding is refused");
    assert!(fired.get(), "the race seam fired");
    let msg = err.to_string();
    assert!(msg.contains("no longer exists"), "{msg}");
    assert!(msg.contains("the finding is unchanged"), "{msg}");
    assert!(
        queued_ids(&store_root).is_empty(),
        "the entry was withdrawn"
    );
}

/// `queue_add` upserts, so a failed promote puts back the entry the spec
/// already had rather than dropping it.
// trace:BUG-1647 | ai:claude
#[test]
fn bug_1647_promote_restores_an_earlier_queue_entry_when_the_write_fails() {
    let (_dir, _root, store_root, stale) = project_with(RequirementStatus::Draft);
    let storage = aida_core::Storage::new(&store_root);
    let user = crate::current_user_id(None);
    storage
        .queue_add(aida_core::QueueEntry {
            user_id: user.clone(),
            requirement_id: stale.id,
            position: 5000,
            added_by: user.clone(),
            note: Some("queued earlier by hand".into()),
            added_at: chrono::Utc::now(),
            for_role: Some("reviewer".into()),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        })
        .unwrap();
    let fired = arm_race(|s| concurrently_set(s, RequirementStatus::Superseded));

    let err = promote_to_work(&store_root, &stale).expect_err("a final status is refused");
    assert!(fired.get(), "the race seam fired");
    assert!(
        err.to_string().contains("the finding is unchanged"),
        "{err}"
    );
    let entries = storage.queue_list(&user, true).unwrap();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].for_role.as_deref(), Some("reviewer"));
    assert_eq!(entries[0].note.as_deref(), Some("queued earlier by hand"));
    assert_eq!(entries[0].position, 5000);
}

/// When the withdrawal itself fails, the error names the entry that remains
/// and never claims nothing was changed.
// trace:BUG-1647 | ai:claude
#[test]
fn bug_1647_promote_reports_a_queue_entry_it_could_not_withdraw() {
    let (_dir, _root, store_root, stale) = project_with(RequirementStatus::Draft);
    let fired = arm_race(|s| {
        concurrently_set(s, RequirementStatus::Rejected);
        // An unparseable queue file makes the withdrawal fail.
        corrupt_queue_files(&s.join("registry").join("queues"));
    });

    let err = promote_to_work(&store_root, &stale).expect_err("a final status is refused");
    assert!(fired.get(), "the race seam fired");
    let msg = err.to_string();
    assert!(msg.contains("is now Rejected"), "{msg}");
    assert!(!msg.contains("the finding is unchanged"), "{msg}");
    assert!(msg.contains("could not be withdrawn"), "{msg}");
    assert!(
        msg.contains(&format!("aida queue remove {SPEC} --for implementer")),
        "{msg}"
    );
}

fn corrupt_queue_files(dir: &Path) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            corrupt_queue_files(&path);
        } else if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
            std::fs::write(&path, "{ not: [valid yaml").unwrap();
        }
    }
}

// ---------------------------------------------------------------------------
// 3. The Approved write refuses a final status.
// ---------------------------------------------------------------------------

// trace:BUG-1647 | ai:claude
#[test]
fn bug_1647_approve_write_refuses_a_finding_now_in_a_final_status() {
    for terminal in [
        RequirementStatus::Completed,
        RequirementStatus::Rejected,
        RequirementStatus::Superseded,
    ] {
        let (_dir, _root, store_root, stale) = project_with(RequirementStatus::Draft);
        concurrently_set(&store_root, terminal.clone());
        let before = on_disk(&store_root).unwrap();

        let backend = open_backend(&store_root);
        let err = crate::findings_promote_approve_write(
            &backend,
            &stale,
            Some("real"),
            chrono::Utc::now(),
        )
        .expect_err("a final status is refused");
        assert!(
            err.to_string().contains(&format!("is now {terminal}")),
            "{err}"
        );
        let after = on_disk(&store_root).unwrap();
        assert_eq!(after.status, terminal);
        assert_eq!(after.comments.len(), before.comments.len());
        assert_eq!(after.history.len(), before.history.len());
    }
}

// ---------------------------------------------------------------------------
// 4. Auto-complete re-promote.
// ---------------------------------------------------------------------------

fn auto_complete(
    store_root: &Path,
    root: &Path,
    stale: &Requirement,
) -> anyhow::Result<crate::PromoteAutoComplete> {
    let backend = open_backend(store_root);
    crate::findings_promote_auto_complete_write(
        &backend,
        stale,
        root,
        "abc1234",
        "fix: the thing",
        Some("already shipped"),
        chrono::Utc::now(),
    )
}

// trace:BUG-1647 | ai:claude
#[test]
fn bug_1647_auto_complete_of_a_completed_finding_changes_nothing() {
    let (_dir, root, store_root, stale) = project_with(RequirementStatus::Draft);
    assert_eq!(
        auto_complete(&store_root, &root, &stale).unwrap(),
        crate::PromoteAutoComplete::Completed
    );
    let first = on_disk(&store_root).unwrap();
    assert_eq!(first.status, RequirementStatus::Completed);

    // Re-promoting from the stale Draft copy: the lock-held copy is Completed.
    assert_eq!(
        auto_complete(&store_root, &root, &stale).unwrap(),
        crate::PromoteAutoComplete::AlreadyCompleted
    );
    let second = on_disk(&store_root).unwrap();
    assert_eq!(second.comments.len(), first.comments.len());
    assert_eq!(status_entries(&second), status_entries(&first));
    assert_eq!(second.history.len(), first.history.len());
    assert_eq!(second.modified_at, first.modified_at);
    assert_eq!(ship_records(&root, SPEC), 1, "exactly one ship record");
}

// trace:BUG-1647 | ai:claude
#[test]
fn bug_1647_auto_complete_refuses_a_rejected_or_superseded_finding() {
    for terminal in [RequirementStatus::Rejected, RequirementStatus::Superseded] {
        let (_dir, root, store_root, stale) = project_with(RequirementStatus::Draft);
        concurrently_set(&store_root, terminal.clone());
        let before = on_disk(&store_root).unwrap();

        let err = auto_complete(&store_root, &root, &stale).expect_err("a final status is refused");
        let msg = err.to_string();
        assert!(msg.contains(&format!("is now {terminal}")), "{msg}");
        assert!(msg.contains("nothing was changed"), "{msg}");
        let after = on_disk(&store_root).unwrap();
        assert_eq!(after.status, terminal);
        assert_eq!(after.comments.len(), before.comments.len());
        assert_eq!(after.history.len(), before.history.len());
        assert_eq!(ship_records(&root, SPEC), 0);
    }
}

// ---------------------------------------------------------------------------
// 5. One ship record per real completion, through the atomic seam.
// ---------------------------------------------------------------------------

/// Modelled on STORY-1418's `transition_to_completed_emits_only_on_a_real_transition`,
/// through `transition_to_completed_atomically`.
// trace:BUG-1647 trace:STORY-1418 | ai:claude
#[test]
fn bug_1647_atomic_transition_emits_one_ship_record_per_real_completion() {
    let (_dir, root, store_root, stale) = project_with(RequirementStatus::Done);
    let backend = open_backend(&store_root);

    let mut prepared = 0;
    let outcome = crate::completion::transition_to_completed_atomically(
        &backend,
        &stale,
        Some(&root),
        SPEC,
        "",
        "test",
        |r, prior| {
            assert!(matches!(r.status, RequirementStatus::Completed));
            assert!(matches!(prior, RequirementStatus::Done));
            prepared += 1;
        },
    )
    .unwrap();
    assert_eq!(outcome, crate::completion::AtomicCompletion::Completed);
    assert_eq!(prepared, 1);
    assert_eq!(ship_records(&root, SPEC), 1);

    // Again, from the same stale copy: already Completed under the lock.
    let again = crate::completion::transition_to_completed_atomically(
        &backend,
        &stale,
        Some(&root),
        SPEC,
        "",
        "test",
        |_, _| panic!("prepare must not run for an already-Completed spec"),
    )
    .unwrap();
    assert_eq!(again, crate::completion::AtomicCompletion::AlreadyCompleted);
    assert_eq!(ship_records(&root, SPEC), 1, "exactly one ship record");

    // A deleted spec writes and emits nothing.
    backend.delete_requirement(&stale.id).unwrap();
    let gone = crate::completion::transition_to_completed_atomically(
        &backend,
        &stale,
        Some(&root),
        SPEC,
        "",
        "test",
        |_, _| panic!("prepare must not run for a deleted spec"),
    )
    .unwrap();
    assert_eq!(gone, crate::completion::AtomicCompletion::Gone);
    assert_eq!(ship_records(&root, SPEC), 1);
}

// ---------------------------------------------------------------------------
// 6. The phase-1 restore line's status formatting.
// ---------------------------------------------------------------------------

// trace:BUG-1647 | ai:claude
#[test]
fn bug_1647_phase1_restore_line_shows_statuses_as_the_cli_does() {
    let prior = RequirementStatus::NeedsAttention;
    for outcome in [
        crate::Phase1RestoreOutcome::Restored,
        crate::Phase1RestoreOutcome::Refused(format!(
            "it is now {}, a final status, so it was left as is",
            RequirementStatus::Completed
        )),
    ] {
        let line = crate::phase1_restore_outcome_message(&outcome, SPEC, &prior);
        assert!(line.contains("Needs Attention"), "{line}");
        assert!(!line.contains("NeedsAttention"), "{line}");
    }
}
