//! C2 fixtures for the destructive-entry guard (C SKETCH v1 sections 5 and 6).
//! trace:STORY-1843 | ai:claude

use super::guard::{
    remove_worktree_retained, retire_checkout, retire_clone_checkout, settle, RetentionRefusal,
    Retirement,
};
use super::tests::{
    activate, assert_open, assert_retiring, files_under, mailbox, message, record, repo, seed_mail,
    source, SEEDED,
};
use super::*;
use crate::mailbox::write_local_message;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
}

/// A repository with one commit and a linked worktree that has its own
/// `.aida/`.
fn repo_with_worktree() -> (TempDir, PathBuf) {
    let dir = repo();
    git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "init"]);
    let wt = dir.path().join("wt");
    git(
        dir.path(),
        &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()],
    );
    std::fs::create_dir_all(wt.join(".aida")).unwrap();
    (dir, wt)
}

fn refusal_of(err: &anyhow::Error) -> &RetentionRefusal {
    err.downcast_ref::<RetentionRefusal>()
        .unwrap_or_else(|| panic!("expected a retention refusal, got {err:#}"))
}

fn retained_generations(anchor: &Path, key: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(anchor.join(RETAINED_DIR).join(key))
        .map(|read| read.map(|e| e.unwrap().path()).collect())
        .unwrap_or_default();
    out.sort();
    out
}

#[test]
fn removing_an_empty_worktree_retires_then_forgets_its_source() {
    let (dir, wt) = repo_with_worktree();
    let before = source(&wt);
    remove_worktree_retained(dir.path(), &wt, true, "test").unwrap();
    assert!(!wt.exists());
    assert!(
        !record_path(&before.anchor, &before.key).exists(),
        "a finalized source is forgotten"
    );
}

#[test]
fn a_mail_bearing_worktree_is_kept_and_the_refusal_names_activation() {
    let (dir, wt) = repo_with_worktree();
    seed_mail(&wt);
    let err = remove_worktree_retained(dir.path(), &wt, true, "test").unwrap_err();
    let refusal = refusal_of(&err);
    assert!(refusal.needs_activation, "{refusal}");
    assert!(refusal.reason.contains(ACTIVATION_COMMAND), "{refusal}");
    assert!(wt.exists(), "the tree is kept");
    assert_eq!(files_under(&mailbox(&wt)), SEEDED);
    assert_open(&wt);
}

#[test]
fn an_activated_worktree_preserves_its_mail_then_goes() {
    let (dir, wt) = repo_with_worktree();
    activate(&wt);
    seed_mail(&wt);
    let before = source(&wt);
    remove_worktree_retained(dir.path(), &wt, true, "test").unwrap();
    assert!(!wt.exists());
    let kept = retained_generations(&before.anchor, &before.key);
    assert_eq!(kept.len(), 1);
    assert_eq!(files_under(&kept[0].join("data")), SEEDED);
}

#[test]
fn a_failed_destructive_step_reopens_the_source_with_its_mail() {
    let (_dir, wt) = repo_with_worktree();
    activate(&wt);
    seed_mail(&wt);
    let generation = record(&wt).generation;
    let retirement = retire_checkout(&wt, "test").unwrap().expect("retired");
    assert!(files_under(&mailbox(&wt)).is_empty(), "the bytes moved out");
    assert_retiring(&wt);
    let err = settle::<()>(
        Err(anyhow::anyhow!("the removal failed")),
        Some(retirement),
        Retirement::complete,
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("the removal failed"), "{err:#}");
    assert_eq!(files_under(&mailbox(&wt)), SEEDED, "the bytes are back");
    assert_open(&wt);
    assert!(
        record(&wt).generation > generation,
        "reopened under a new generation"
    );
}

#[test]
fn a_reuse_hands_the_next_occupant_an_empty_mailbox() {
    let (_dir, wt) = repo_with_worktree();
    activate(&wt);
    seed_mail(&wt);
    let old = source(&wt);
    let retirement = retire_checkout(&wt, "test").unwrap().expect("retired");
    retirement.complete_reuse().unwrap();
    assert!(!mailbox(&wt).exists(), "the old mailbox directory is gone");
    assert!(!record_path(&old.anchor, &old.key).exists());
    // The next occupant's first write mints a new source.
    write_local_message(&wt, &message("next")).unwrap();
    assert_eq!(files_under(&mailbox(&wt)), ["next.json"]);
    let new = record(&wt);
    assert_ne!(new.key, old.key);
    assert_eq!(new.state, SourceState::Open);
    let kept = retained_generations(&old.anchor, &old.key);
    assert_eq!(files_under(&kept[0].join("data")), SEEDED);
}

#[test]
fn a_write_after_the_commit_fails_the_reuse_and_reopens() {
    let (_dir, wt) = repo_with_worktree();
    activate(&wt);
    seed_mail(&wt);
    let retirement = retire_checkout(&wt, "test").unwrap().expect("retired");
    // A writer that took no permit.
    std::fs::write(mailbox(&wt).join("late.json"), "{}").unwrap();
    let err = retirement.complete_reuse().unwrap_err();
    assert!(err.to_string().contains("not empty"), "{err}");
    let mut expected: Vec<String> = SEEDED.iter().map(|s| s.to_string()).collect();
    expected.push("late.json".into());
    expected.sort();
    assert_eq!(files_under(&mailbox(&wt)), expected);
    assert_open(&wt);
}

#[test]
fn a_live_retirement_holds_the_source_and_an_abandoned_one_reopens() {
    let (_dir, wt) = repo_with_worktree();
    let retirement = retire_checkout(&wt, "test").unwrap().expect("retired");
    let again = retire_checkout(&wt, "test").unwrap_err();
    assert!(again.reason.contains("another process"), "{again}");
    assert_retiring(&wt);
    // The driver goes away without completing or reopening.
    drop(retirement);
    assert_open(&wt);
}

#[test]
fn a_checkout_whose_aida_is_a_symlink_owns_no_source() {
    let (dir, wt) = repo_with_worktree();
    std::fs::remove_dir(wt.join(".aida")).unwrap();
    seed_mail(dir.path());
    std::os::unix::fs::symlink(dir.path().join(".aida"), wt.join(".aida")).unwrap();
    assert!(retire_checkout(&wt, "test").unwrap().is_none());
    assert_eq!(
        files_under(&mailbox(dir.path())),
        SEEDED,
        "main's mail untouched"
    );
    assert_open(dir.path());
}

#[test]
fn a_checkout_without_aida_state_has_nothing_to_retire() {
    let (_dir, wt) = repo_with_worktree();
    std::fs::remove_dir(wt.join(".aida")).unwrap();
    assert!(retire_checkout(&wt, "test").unwrap().is_none());
    assert!(
        !wt.join(".aida").exists(),
        "nothing was created in the tree"
    );
}

#[test]
fn an_unanchored_checkout_goes_only_without_mail() {
    let dir = tempfile::tempdir().unwrap();
    let mb = mailbox(dir.path());
    std::fs::create_dir_all(mb.join(".read")).unwrap();
    assert!(retire_checkout(dir.path(), "test").unwrap().is_none());
    std::fs::write(mb.join("m.json"), "{}").unwrap();
    let refusal = retire_checkout(dir.path(), "test").unwrap_err();
    assert!(
        refusal.reason.contains("could not be retained"),
        "{refusal}"
    );
    assert!(!refusal.needs_activation);
}

#[test]
fn a_clone_retires_under_its_main_anchor_and_refuses_without_a_binding() {
    let main = repo();
    activate(main.path());
    let anchor = source(main.path()).anchor;
    let clones = tempfile::tempdir().unwrap();
    let clone = clones.path().join("aida-seat-x");
    std::fs::create_dir_all(mailbox(&clone)).unwrap();
    std::fs::write(mailbox(&clone).join("m.json"), "{}").unwrap();

    // No verified binding: a mail-bearing clone is kept.
    let refusal = retire_clone_checkout(None, &clone, "test").unwrap_err();
    assert!(
        refusal.reason.contains("verified host binding"),
        "{refusal}"
    );

    let retirement = retire_clone_checkout(Some(main.path()), &clone, "test")
        .unwrap()
        .expect("retired");
    assert!(
        retirement.retained_dir().starts_with(&anchor),
        "main's anchor"
    );
    assert_eq!(
        files_under(&retirement.retained_dir().join("data")),
        ["m.json"]
    );
    std::fs::remove_dir_all(&clone).unwrap();
    retirement.complete().unwrap();

    // An empty clone with no binding has nothing to retain.
    let empty = clones.path().join("aida-seat-y");
    std::fs::create_dir_all(mailbox(&empty)).unwrap();
    assert!(retire_clone_checkout(None, &empty, "test")
        .unwrap()
        .is_none());
}

// ── Pool staged state ───────────────────────────────────────────────────────

fn pool_repo() -> TempDir {
    let dir = repo();
    git(dir.path(), &["checkout", "-q", "-b", "main"]);
    std::fs::write(dir.path().join("README.md"), "seed").unwrap();
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-qm", "seed"]);
    dir
}

fn pool_opts() -> crate::worktree_pool::AcquireOptions {
    crate::worktree_pool::AcquireOptions {
        max_trees: Some(2),
        ..Default::default()
    }
}

#[test]
fn a_pool_return_with_unretirable_mail_keeps_the_tree_leased() {
    use crate::worktree_pool::{acquire, read_state, return_to_pool};
    let repo = pool_repo();
    let root = repo.path();
    let path = acquire(root, &pool_opts()).unwrap();
    write_local_message(&path, &message("old")).unwrap();
    let err = return_to_pool(root, &path).unwrap_err();
    assert!(refusal_of(&err).needs_activation, "{err:#}");
    let entry = read_state(root).unwrap().entries[0].clone();
    assert!(entry.owner_pid.is_some(), "still owned");
    assert!(entry.retiring.is_none(), "the staged mark is cleared");
    assert_eq!(files_under(&mailbox(&path)), ["old.json"]);
}

#[test]
fn a_pool_reuse_never_shows_the_previous_occupants_mail() {
    use crate::worktree_pool::{acquire, return_to_pool};
    // Reuse needs the holder scan; a sandboxed pid namespace cannot verify it.
    let _trust = crate::test_env::EnvVarGuard::set("AIDA_TRUST_PID_NAMESPACE", "1");
    let repo = pool_repo();
    let root = repo.path();
    let path = acquire(root, &pool_opts()).unwrap();
    activate(&path);
    write_local_message(&path, &message("old")).unwrap();
    let old = source(&path);
    return_to_pool(root, &path).unwrap();
    assert!(!mailbox(&path).exists(), "the return retired the old mail");
    let again = acquire(root, &pool_opts()).unwrap();
    assert_eq!(again, path, "the warm tree is reused");
    assert!(!mailbox(&again).exists());
    let kept = retained_generations(&old.anchor, &old.key);
    assert_eq!(files_under(&kept[0].join("data")), ["old.json"]);
}

#[test]
fn an_acquire_passes_over_a_tree_whose_mail_cannot_be_retired() {
    use crate::worktree_pool::{acquire, read_state, with_state_lock};
    let _trust = crate::test_env::EnvVarGuard::set("AIDA_TRUST_PID_NAMESPACE", "1");
    let repo = pool_repo();
    let root = repo.path();
    let first = acquire(root, &pool_opts()).unwrap();
    write_local_message(&first, &message("stuck")).unwrap();
    // Release ownership without a return, as a dead session would.
    with_state_lock(root, |pool| {
        pool.entries[0].owner_pid = None;
        pool.entries[0].owner_pid_start = None;
        Ok(())
    })
    .unwrap();
    let (second, seen) = observe_lock_classes(|| acquire(root, &pool_opts()).unwrap());
    assert!(
        seen.contains(&"lifecycle"),
        "the tree was offered and refused: {seen:?}"
    );
    assert_ne!(second, first, "the mail-bearing tree is not reset");
    assert_eq!(files_under(&mailbox(&first)), ["stuck.json"]);
    let pool = read_state(root).unwrap();
    assert!(pool.entries.iter().all(|e| e.retiring.is_none()));
}

#[test]
fn pool_retirement_never_takes_the_pool_lock_under_lifecycle() {
    use crate::worktree_pool::{acquire, return_to_pool};
    let repo = pool_repo();
    let root = repo.path();
    let path = acquire(root, &pool_opts()).unwrap();
    std::fs::create_dir_all(path.join(".aida")).unwrap();
    // `enter_lock_class` debug-asserts the order; the observer records it.
    let (result, seen) = observe_lock_classes(|| return_to_pool(root, &path));
    result.unwrap();
    assert!(seen.contains(&"lifecycle"), "{seen:?}");
    assert!(seen.contains(&"pool"), "{seen:?}");
}
