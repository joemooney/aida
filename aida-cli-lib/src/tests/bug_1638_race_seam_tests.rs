//! BUG-1638: BUG-1637's follow-ups.
//!
//! 1. The self-reading status writers (`ensure_spec_done_after_pr`,
//!    `bump_spec_in_progress_at_lease_take`,
//!    `restore_phase1_status_on_lease_failure`) are called directly, with a
//!    concurrent change injected between their read and their write through
//!    `inject_status_write_race`. A write made from the stale read (not
//!    through the atomic re-read) would overwrite that change, so each test
//!    fails if the write stops being atomic.
//! 2. The phase-1 restore returns its outcome, and the orchestrator's line
//!    says "status restored" only when it was.
//! 3. Zen reports a spec deleted mid-flight as gone.
//! 4. The findings promote writes keep a concurrent edit.
//!
//! Every test uses a temporary git-canonical store. The fixture helpers are
//! shared with BUG-1647's `bug_1647_findings_atomic_tests`.
//! trace:BUG-1638 | ai:claude
// trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use aida_core::conflict;
use aida_core::db::DatabaseBackend;
use aida_core::{Requirement, RequirementStatus};

use crate::Phase1RestoreOutcome;

pub(crate) const SPEC: &str = "TASK-9638";

/// A project at `<tmp>` whose `.aida/config.toml` points at a git-canonical
/// store `<tmp>/.aida-store` holding one spec at `status`. Returns (tempdir,
/// project root, store root, the spec as read now).
pub(crate) fn project_with(
    status: RequirementStatus,
) -> (tempfile::TempDir, PathBuf, PathBuf, Requirement) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida").join("config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();
    let store_root = root.join(".aida-store");
    std::fs::create_dir_all(&store_root).unwrap();
    aida_core::git_ops::init(&store_root).unwrap();
    aida_core::git_ops::configure_user(&store_root, "fixture", "fixture@localhost").unwrap();
    let backend = open_backend(&store_root);
    let mut r = Requirement::new("Spec".into(), "original description".into());
    r.spec_id = Some(SPEC.into());
    r.tags.insert("from-review:bug-1638".into());
    let mut added = backend.add_requirement(r).unwrap();
    if added.status != status {
        added.status = status;
        backend.update_requirement(&added).unwrap();
    }
    let read = on_disk(&store_root).expect("the spec was just added");
    (dir, root, store_root, read)
}

pub(crate) fn open_backend(store_root: &Path) -> aida_core::CachedGitBackend {
    let cache = aida_core::CachedGitBackend::default_cache_path(store_root);
    crate::test_env::home_fixture_cached_backend(store_root, &cache).unwrap()
}

pub(crate) fn on_disk(store_root: &Path) -> Option<Requirement> {
    open_backend(store_root)
        .get_requirement_by_spec_id(SPEC)
        .unwrap()
}

/// Another writer changes the stored spec with `edit`.
pub(crate) fn concurrently_edit(store_root: &Path, edit: impl FnOnce(&mut Requirement)) {
    let backend = open_backend(store_root);
    let mut r = on_disk(store_root).expect("the spec exists");
    edit(&mut r);
    backend.update_requirement(&r).unwrap();
}

/// A person moves the stored spec to `status`.
pub(crate) fn concurrently_set(store_root: &Path, status: RequirementStatus) {
    concurrently_edit(store_root, |r| {
        conflict::set_status_recorded(r, status, "joe");
    });
}

/// Another writer rewrites the description (no status change).
fn concurrently_redescribe(store_root: &Path) {
    concurrently_edit(store_root, |r| {
        r.description = "edited concurrently".into();
    });
}

/// Arm the race seam with `edit`, run against the store root the writer
/// resolved. The returned flag records that the seam fired, so a test can't
/// pass because the writer never reached its write.
pub(crate) fn arm_race(edit: impl FnOnce(&Path) + 'static) -> Rc<Cell<bool>> {
    let fired = Rc::new(Cell::new(false));
    let flag = fired.clone();
    crate::inject_status_write_race(move |store_root| {
        flag.set(true);
        edit(store_root);
    });
    fired
}

pub(crate) fn last_status_author(r: &Requirement) -> &str {
    r.history
        .iter()
        .rev()
        .find(|h| h.changes.iter().any(|c| c.field_name == "status"))
        .map(|h| h.author.as_str())
        .expect("a status history entry")
}

/// Make `root` a git repository whose `branch` tip has `subject` (empty
/// commits, so the nested store is never added). No `origin`: the PR
/// attribution gate falls back to the branch's own commits.
// trace:BUG-2696 | ai:claude
pub(crate) fn branch_with_tip(root: &Path, branch: &str, subject: &str) {
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "fixture@localhost"]);
    git(&["config", "user.name", "fixture"]);
    git(&["commit", "-q", "--allow-empty", "-m", "chore: base"]);
    git(&["checkout", "-q", "-b", branch]);
    git(&["commit", "-q", "--allow-empty", "-m", subject]);
}

// ---------------------------------------------------------------------------
// 1. The race seam, through the writers themselves.
// ---------------------------------------------------------------------------

// trace:BUG-1638 | ai:claude
#[test]
fn bug_1638_lease_take_bump_refuses_a_spec_rejected_between_read_and_write() {
    let (_dir, root, store_root, _) = project_with(RequirementStatus::Approved);
    let fired = arm_race(|s| concurrently_set(s, RequirementStatus::Rejected));

    assert!(!crate::bump_spec_in_progress_at_lease_take(&root, SPEC));
    assert!(fired.get(), "the race seam fired");
    assert_eq!(
        on_disk(&store_root).unwrap().status,
        RequirementStatus::Rejected
    );
}

// trace:BUG-1638 | ai:claude
#[test]
fn bug_1638_lease_take_bump_keeps_an_edit_made_between_read_and_write() {
    let (_dir, root, store_root, _) = project_with(RequirementStatus::Approved);
    let fired = arm_race(concurrently_redescribe);

    assert!(crate::bump_spec_in_progress_at_lease_take(&root, SPEC));
    assert!(fired.get(), "the race seam fired");
    let after = on_disk(&store_root).unwrap();
    assert_eq!(after.status, RequirementStatus::InProgress);
    assert_eq!(after.description, "edited concurrently");
    assert_eq!(last_status_author(&after), conflict::LEASE_TAKE_AUTHOR);
}

// trace:BUG-1638 | ai:claude
#[test]
fn bug_1638_pr_open_done_refuses_a_spec_completed_between_read_and_write() {
    let (_dir, root, store_root, _) = project_with(RequirementStatus::InProgress);
    // BUG-2696 F9: the write needs a readable tip that completes the spec.
    branch_with_tip(&root, "task-9638-work", &format!("fix: work ({SPEC})"));
    let fired = arm_race(|s| concurrently_set(s, RequirementStatus::Completed));

    crate::ensure_spec_done_after_pr(&root, &root, "task-9638-work", SPEC, 42, true);
    assert!(fired.get(), "the race seam fired");
    assert_eq!(
        on_disk(&store_root).unwrap().status,
        RequirementStatus::Completed
    );
}

// trace:BUG-1638 | ai:claude
#[test]
fn bug_1638_pr_open_done_keeps_an_edit_made_between_read_and_write() {
    let (_dir, root, store_root, _) = project_with(RequirementStatus::InProgress);
    branch_with_tip(&root, "task-9638-work", &format!("fix: work ({SPEC})"));
    let fired = arm_race(concurrently_redescribe);

    crate::ensure_spec_done_after_pr(&root, &root, "task-9638-work", SPEC, 42, true);
    assert!(fired.get(), "the race seam fired");
    let after = on_disk(&store_root).unwrap();
    assert_eq!(after.status, RequirementStatus::Done);
    assert_eq!(after.description, "edited concurrently");
    assert_eq!(last_status_author(&after), conflict::PR_OPEN_AUTHOR);
}

// trace:BUG-1638 | ai:claude
#[test]
fn bug_1638_phase1_restore_refuses_a_spec_completed_between_read_and_write() {
    let (_dir, root, store_root, _) = project_with(RequirementStatus::NeedsAttention);
    let fired = arm_race(|s| concurrently_set(s, RequirementStatus::Completed));

    let outcome =
        crate::restore_phase1_status_on_lease_failure(&root, SPEC, &RequirementStatus::Approved)
            .unwrap();
    assert!(fired.get(), "the race seam fired");
    match &outcome {
        Phase1RestoreOutcome::Refused(reason) => {
            assert!(reason.contains("Completed"), "{reason}")
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(
        on_disk(&store_root).unwrap().status,
        RequirementStatus::Completed
    );
}

// trace:BUG-1638 | ai:claude
#[test]
fn bug_1638_phase1_restore_keeps_an_edit_made_between_read_and_write() {
    let (_dir, root, store_root, _) = project_with(RequirementStatus::NeedsAttention);
    let fired = arm_race(concurrently_redescribe);

    let outcome =
        crate::restore_phase1_status_on_lease_failure(&root, SPEC, &RequirementStatus::Approved)
            .unwrap();
    assert!(fired.get(), "the race seam fired");
    assert_eq!(outcome, Phase1RestoreOutcome::Restored);
    let after = on_disk(&store_root).unwrap();
    assert_eq!(after.status, RequirementStatus::Approved);
    assert_eq!(after.description, "edited concurrently");
    assert_eq!(
        last_status_author(&after),
        conflict::ORCHESTRATOR_PHASE1_AUTHOR
    );
}

// ---------------------------------------------------------------------------
// 2. The restore outcome reaches the orchestrator's line.
// ---------------------------------------------------------------------------

// trace:BUG-1638 | ai:claude
#[test]
fn bug_1638_phase1_restore_reports_a_spec_deleted_between_read_and_write() {
    let (_dir, root, store_root, read) = project_with(RequirementStatus::NeedsAttention);
    let id = read.id;
    let fired = arm_race(move |s| open_backend(s).delete_requirement(&id).unwrap());

    let outcome =
        crate::restore_phase1_status_on_lease_failure(&root, SPEC, &RequirementStatus::Approved)
            .unwrap();
    assert!(fired.get(), "the race seam fired");
    assert_eq!(
        outcome,
        Phase1RestoreOutcome::Refused(format!("{SPEC} no longer exists"))
    );
    assert!(on_disk(&store_root).is_none());
}

// trace:BUG-1638 | ai:claude
#[test]
fn bug_1638_orchestrator_says_restored_only_when_it_was() {
    let prior = RequirementStatus::Approved;
    let restored =
        crate::phase1_restore_outcome_message(&Phase1RestoreOutcome::Restored, SPEC, &prior);
    assert!(
        restored.contains("status restored to Approved"),
        "{restored}"
    );

    let refused = crate::phase1_restore_outcome_message(
        &Phase1RestoreOutcome::Refused("it is now Completed, a final status".into()),
        SPEC,
        &prior,
    );
    assert!(!refused.contains("status restored"), "{refused}");
    assert!(refused.contains("was not restored"), "{refused}");
    assert!(refused.contains("it is now Completed"), "{refused}");
}

/// The refused case end to end: the store's spec is terminal when the restore
/// runs, and the outcome (and so the orchestrator's line) is a refusal.
// trace:BUG-1638 | ai:claude
#[test]
fn bug_1638_phase1_restore_of_a_terminal_spec_is_reported_as_refused() {
    let (_dir, root, store_root, _) = project_with(RequirementStatus::Rejected);
    let prior = RequirementStatus::Approved;
    let outcome = crate::restore_phase1_status_on_lease_failure(&root, SPEC, &prior).unwrap();
    assert!(matches!(outcome, Phase1RestoreOutcome::Refused(_)));
    let line = crate::phase1_restore_outcome_message(&outcome, SPEC, &prior);
    assert!(!line.contains("status restored"), "{line}");
    assert_eq!(
        on_disk(&store_root).unwrap().status,
        RequirementStatus::Rejected
    );
}

// ---------------------------------------------------------------------------
// 3. Zen: a spec deleted mid-flight.
// ---------------------------------------------------------------------------

// trace:BUG-1638 | ai:claude
#[test]
fn bug_1638_zen_reports_a_spec_deleted_mid_flight_as_gone() {
    let (_dir, _root, store_root, stale) = project_with(RequirementStatus::Draft);
    open_backend(&store_root)
        .delete_requirement(&stale.id)
        .unwrap();

    let err = crate::zen_auto_approve(&store_root, &stale, true)
        .expect_err("approving a deleted spec must fail");
    let msg = err.to_string();
    assert!(msg.contains("no longer exists"), "{msg}");
    assert!(!msg.contains("not Draft"), "{msg}");
    assert!(on_disk(&store_root).is_none());
}

// ---------------------------------------------------------------------------
// 4. Findings promote: atomic writes keep a concurrent edit.
// ---------------------------------------------------------------------------

/// Another writer rewrites the description and adds a comment.
pub(crate) fn concurrently_comment_and_redescribe(store_root: &Path) {
    concurrently_edit(store_root, |r| {
        r.description = "edited concurrently".into();
        r.add_comment(aida_core::Comment::new(
            "someone else".into(),
            "concurrent comment".into(),
        ));
    });
}

pub(crate) fn comment_texts(r: &Requirement) -> Vec<&str> {
    r.comments.iter().map(|c| c.content.as_str()).collect()
}

// trace:BUG-1638 | ai:claude
#[test]
fn bug_1638_findings_promote_auto_complete_keeps_a_concurrent_edit() {
    let (_dir, root, store_root, stale) = project_with(RequirementStatus::Draft);
    concurrently_comment_and_redescribe(&store_root);

    let backend = open_backend(&store_root);
    crate::findings_promote_auto_complete_write(
        &backend,
        &stale,
        &root,
        "abc1234",
        "fix: the thing",
        Some("already shipped"),
        chrono::Utc::now(),
    )
    .unwrap();

    let after = on_disk(&store_root).unwrap();
    assert_eq!(after.status, RequirementStatus::Completed);
    assert_eq!(after.description, "edited concurrently");
    let texts = comment_texts(&after);
    assert!(texts.contains(&"concurrent comment"), "{texts:?}");
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("Auto-completed on promote")),
        "{texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("Promoted by") && t.ends_with("already shipped")),
        "{texts:?}"
    );
    assert_eq!(last_status_author(&after), crate::get_default_author());
}

// trace:BUG-1638 | ai:claude
#[test]
fn bug_1638_findings_promote_approve_keeps_a_concurrent_edit() {
    let (_dir, _root, store_root, stale) = project_with(RequirementStatus::Draft);
    concurrently_comment_and_redescribe(&store_root);

    let backend = open_backend(&store_root);
    crate::findings_promote_approve_write(&backend, &stale, Some("real"), chrono::Utc::now())
        .unwrap();

    let after = on_disk(&store_root).unwrap();
    assert_eq!(after.status, RequirementStatus::Approved);
    assert_eq!(after.description, "edited concurrently");
    let texts = comment_texts(&after);
    assert!(texts.contains(&"concurrent comment"), "{texts:?}");
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("Promoted by") && t.ends_with("real")),
        "{texts:?}"
    );
    assert_eq!(last_status_author(&after), crate::get_default_author());
}

// trace:BUG-1638 | ai:claude
#[test]
fn bug_1638_findings_promote_of_a_deleted_finding_fails_without_writing() {
    let (_dir, root, store_root, stale) = project_with(RequirementStatus::Draft);
    let backend = open_backend(&store_root);
    backend.delete_requirement(&stale.id).unwrap();

    let err = crate::findings_promote_approve_write(&backend, &stale, None, chrono::Utc::now())
        .expect_err("promoting a deleted finding must fail");
    assert!(err.to_string().contains("no longer exists"), "{err}");
    let err = crate::findings_promote_auto_complete_write(
        &backend,
        &stale,
        &root,
        "abc1234",
        "fix",
        None,
        chrono::Utc::now(),
    )
    .expect_err("auto-completing a deleted finding must fail");
    assert!(err.to_string().contains("no longer exists"), "{err}");
    assert!(on_disk(&store_root).is_none());
}

// ---------------------------------------------------------------------------
// BUG-2696: the PR-open Done write fails closed.
// ---------------------------------------------------------------------------

/// F9: attribution passes (no default branch to compare against), but the
/// separate tip read fails because `root` is not a repository. That failure
/// must retain the spec, not authorize Done; so must a readable Refs tip.
// trace:BUG-2696 | ai:claude
#[test]
fn bug_2696_pr_open_done_fails_closed_on_unreadable_or_partial_tip() {
    let (_dir, root, store_root, _) = project_with(RequirementStatus::InProgress);
    let fired = arm_race(|_| {});
    crate::ensure_spec_done_after_pr(&root, &root, "task-9638-work", SPEC, 42, true);
    assert!(!fired.get(), "no write may be attempted on an unread tip");
    assert_eq!(
        on_disk(&store_root).unwrap().status,
        RequirementStatus::InProgress
    );

    // A readable tip that names the spec only as a reference is partial.
    // (The still-armed no-op hook may fire; it changes nothing.)
    branch_with_tip(
        &root,
        "task-9638-work",
        &format!("fix: slice (Refs {SPEC})"),
    );
    crate::ensure_spec_done_after_pr(&root, &root, "task-9638-work", SPEC, 42, true);
    assert_eq!(
        on_disk(&store_root).unwrap().status,
        RequirementStatus::InProgress
    );
}

/// F4: an advisor adds an outstanding child slice between the unlocked
/// completion check and the locked write. The locked copy carries the new
/// edge, so the writer refuses Done.
// trace:BUG-2696 | ai:claude
#[test]
fn bug_2696_pr_open_done_refuses_a_slice_added_between_read_and_write() {
    let (_dir, root, store_root, _) = project_with(RequirementStatus::InProgress);
    branch_with_tip(&root, "task-9638-work", &format!("fix: work ({SPEC})"));
    let backend = open_backend(&store_root);
    let mut child = Requirement::new("Later slice".into(), String::new());
    child.spec_id = Some("TASK-9639".into());
    let child = backend.add_requirement(child).unwrap();
    let child_id = child.id;
    let fired = arm_race(move |s| {
        concurrently_edit(s, |r| {
            r.relationships.push(aida_core::Relationship {
                rel_type: aida_core::RelationshipType::Parent,
                target_id: child_id,
                created_at: None,
                created_by: None,
            });
        })
    });
    crate::ensure_spec_done_after_pr(&root, &root, "task-9638-work", SPEC, 42, true);
    assert!(fired.get(), "the race seam fired");
    let after = on_disk(&store_root).unwrap();
    assert_eq!(after.status, RequirementStatus::InProgress);
    assert!(after.relationships.iter().any(|r| r.target_id == child_id));
}

/// F4 round 3: the parent's edges never change, but an already-linked child
/// that was Completed is reopened (or an archived open child is unarchived)
/// between the unlocked completion check and the locked write. The writer
/// re-reads every child under the store lock and refuses Done.
// trace:BUG-2696 | ai:claude
#[test]
fn bug_2696_pr_open_done_refuses_a_child_reopened_between_read_and_write() {
    for unarchive in [false, true] {
        let (_dir, root, store_root, _) = project_with(RequirementStatus::InProgress);
        branch_with_tip(&root, "task-9638-work", &format!("fix: work ({SPEC})"));
        let backend = open_backend(&store_root);
        let mut child = Requirement::new("Earlier slice".into(), String::new());
        child.spec_id = Some("TASK-9639".into());
        let mut child = backend.add_requirement(child).unwrap();
        if unarchive {
            child.archived = true;
        } else {
            child.status = RequirementStatus::Completed;
        }
        backend.update_requirement(&child).unwrap();
        let child_id = child.id;
        concurrently_edit(&store_root, |r| {
            r.relationships.push(aida_core::Relationship {
                rel_type: aida_core::RelationshipType::Parent,
                target_id: child_id,
                created_at: None,
                created_by: None,
            });
        });
        let fired = arm_race(move |s| {
            let backend = open_backend(s);
            let mut c = backend
                .get_requirement_by_spec_id("TASK-9639")
                .unwrap()
                .unwrap();
            if unarchive {
                c.archived = false;
            } else {
                c.status = RequirementStatus::Approved;
            }
            backend.update_requirement(&c).unwrap();
        });
        crate::ensure_spec_done_after_pr(&root, &root, "task-9638-work", SPEC, 42, true);
        assert!(fired.get(), "the race seam fired (unarchive={unarchive})");
        assert_eq!(
            on_disk(&store_root).unwrap().status,
            RequirementStatus::InProgress,
            "unarchive={unarchive}"
        );
    }
}
