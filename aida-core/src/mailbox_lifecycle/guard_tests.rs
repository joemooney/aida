//! C2 fixtures for the destructive-entry guard (C SKETCH v1 sections 5 and 6).
//! trace:STORY-1843 | ai:claude

use super::guard::{
    checkout_holds_mail, remove_worktree_retained, retire_checkout, retire_clone_checkout, settle,
    RetentionRefusal, Retirement,
};
use super::tests::{
    activate, assert_open, assert_retiring, files_under, mailbox, message, record, repo, seed_mail,
    source, test_source_build, SEEDED,
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

/// R5-F2: session end keeps a seat clone's session only when the clone holds
/// mail again after its retirement reopened.
#[test]
fn checkout_holds_mail_sees_only_the_checkouts_own_mail() {
    let (dir, wt) = repo_with_worktree();
    assert!(!checkout_holds_mail(&wt), "no mailbox yet");
    std::fs::create_dir_all(mailbox(&wt).join("inbox")).unwrap();
    assert!(!checkout_holds_mail(&wt), "empty directories are no mail");
    seed_mail(&wt);
    assert!(checkout_holds_mail(&wt));
    // A mailbox linked to another checkout is not this checkout's mail.
    std::fs::remove_dir_all(wt.join(".aida")).unwrap();
    seed_mail(dir.path());
    std::os::unix::fs::symlink(dir.path().join(".aida"), wt.join(".aida")).unwrap();
    assert!(!checkout_holds_mail(&wt));
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
fn an_unanchored_checkout_is_kept_even_when_its_mailbox_looks_empty() {
    // C SKETCH section 5 / F3: no anchor means no fence, so no destruction,
    // whether the mailbox is absent, empty or mail-bearing.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    let absent = retire_checkout(dir.path(), "test").unwrap_err();
    assert!(absent.reason.contains("cannot be fenced"), "{absent}");
    let mb = mailbox(dir.path());
    std::fs::create_dir_all(mb.join(".read")).unwrap();
    let empty = retire_checkout(dir.path(), "test").unwrap_err();
    assert!(empty.reason.contains("cannot be fenced"), "{empty}");
    std::fs::write(mb.join("m.json"), "{}").unwrap();
    let refusal = retire_checkout(dir.path(), "test").unwrap_err();
    assert!(refusal.reason.contains("cannot be fenced"), "{refusal}");
    assert!(!refusal.needs_activation);
    assert!(mb.join("m.json").exists(), "nothing moved");
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

    // An empty clone with no binding is kept too: it cannot be fenced.
    let empty = clones.path().join("aida-seat-y");
    std::fs::create_dir_all(mailbox(&empty)).unwrap();
    let refusal = retire_clone_checkout(None, &empty, "test").unwrap_err();
    assert!(
        refusal.reason.contains("verified host binding"),
        "{refusal}"
    );
    // A clone with no `.aida/` of its own owns no source at all.
    let bare = clones.path().join("aida-seat-z");
    std::fs::create_dir_all(&bare).unwrap();
    assert!(retire_clone_checkout(None, &bare, "test")
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

// ── Pool destroy, teardown and the lock observer (F5 c, d, g) ───────────────

/// Two idle pool trees with no live owner, as `destroy` classifies
/// `disposable`.
fn two_idle_trees(root: &Path) -> (PathBuf, PathBuf) {
    use crate::worktree_pool::{acquire, with_state_lock};
    let a = acquire(root, &pool_opts()).unwrap();
    let b = acquire(root, &pool_opts()).unwrap();
    with_state_lock(root, |pool| {
        for e in &mut pool.entries {
            e.owner_pid = None;
            e.owner_pid_start = None;
        }
        Ok(())
    })
    .unwrap();
    (a, b)
}

fn destroy_all(root: &Path) -> anyhow::Result<crate::worktree_pool_destroy::DestroyReport> {
    crate::worktree_pool_destroy::destroy(
        root,
        &crate::worktree_pool_destroy::DestroySelector::All,
        &crate::worktree_pool_destroy::DestroyOptions::default(),
        &mut |_| {},
    )
}

#[test]
fn a_pool_destroy_refusal_keeps_the_entry_and_clears_its_staged_marks() {
    use crate::worktree_pool::read_state;
    use crate::worktree_pool_destroy::DestroyAction;
    let _trust = crate::test_env::EnvVarGuard::set("AIDA_TRUST_PID_NAMESPACE", "1");
    let repo = pool_repo();
    let root = repo.path();
    let (kept, gone) = two_idle_trees(root);
    // `kept` holds mail and nothing is activated: QC1 refuses it.
    write_local_message(&kept, &message("stay")).unwrap();
    std::fs::create_dir_all(gone.join(".aida")).unwrap();

    let report = destroy_all(root).unwrap();
    let by_path = |p: &Path| {
        report
            .targets
            .iter()
            .find(|t| t.entry.path == p)
            .unwrap()
            .clone()
    };
    let refused = by_path(&kept);
    assert_eq!(refused.action, DestroyAction::Skipped);
    assert!(refused.reason.contains("kept"), "{}", refused.reason);
    assert_eq!(by_path(&gone).action, DestroyAction::Removed);

    let pool = read_state(root).unwrap();
    assert_eq!(pool.entries.len(), 1, "the removed entry is deregistered");
    let entry = &pool.entries[0];
    assert_eq!(entry.path, kept);
    assert!(entry.retiring.is_none() && !entry.destroying, "{entry:?}");
    assert!(kept.exists() && !gone.exists());
    assert_eq!(files_under(&mailbox(&kept)), ["stay.json"]);
}

#[test]
fn a_failed_pool_removal_reopens_its_mail_and_stops_later_targets() {
    use crate::worktree_pool::read_state;
    let _trust = crate::test_env::EnvVarGuard::set("AIDA_TRUST_PID_NAMESPACE", "1");
    let repo = pool_repo();
    let root = repo.path();
    let (first, second) = two_idle_trees(root);
    activate(&first);
    write_local_message(&first, &message("back")).unwrap();
    std::fs::create_dir_all(second.join(".aida")).unwrap();
    // A locked worktree refuses `git worktree remove --force`.
    git(root, &["worktree", "lock", first.to_str().unwrap()]);

    let err = destroy_all(root).unwrap_err();
    assert!(format!("{err:#}").contains("remove"), "{err:#}");
    assert!(
        first.exists() && second.exists(),
        "nothing after the failure"
    );
    assert_eq!(
        files_under(&mailbox(&first)),
        ["back.json"],
        "the failed removal reopened the retired mail"
    );
    assert_open(&first);
    let pool = read_state(root).unwrap();
    assert_eq!(pool.entries.len(), 2);
    assert!(pool
        .entries
        .iter()
        .all(|e| e.retiring.is_none() && !e.destroying));
    git(root, &["worktree", "unlock", first.to_str().unwrap()]);
}

#[test]
fn a_teardown_refusal_keeps_the_tree_and_runs_no_hook() {
    let (dir, wt) = repo_with_worktree();
    write_local_message(&wt, &message("stay")).unwrap();
    let marker = dir.path().join("hook-ran");
    let hook = format!("touch {}", marker.display());
    let err =
        crate::worktree_pool_destroy::teardown_worktree_path(dir.path(), &wt, &[hook]).unwrap_err();
    assert!(refusal_of(&err).needs_activation, "{err:#}");
    assert!(wt.exists(), "the tree is kept");
    assert!(
        !marker.exists(),
        "no pre_destroy hook ran before the refusal"
    );
    assert_eq!(files_under(&mailbox(&wt)), ["stay.json"]);
}

/// Every section 5 entry that core owns takes the Lifecycle lock and enters
/// no other lock class (Git, the pool lock, a claim) while holding it.
#[test]
fn every_core_destructive_entry_keeps_the_lifecycle_lock_a_leaf() {
    use crate::worktree_pool::{acquire, return_to_pool, with_state_lock};
    let _trust = crate::test_env::EnvVarGuard::set("AIDA_TRUST_PID_NAMESPACE", "1");
    let check = |entry: &str, seen: &[&str], pool: bool| {
        assert!(seen.contains(&"lifecycle"), "{entry}: {seen:?}");
        assert!(!seen.contains(&UNDER_LIFECYCLE), "{entry}: {seen:?}");
        if pool {
            assert!(seen.contains(&"pool"), "{entry}: {seen:?}");
        }
    };

    let (dir, wt) = repo_with_worktree();
    activate(&wt);
    seed_mail(&wt);
    let (removed, seen) =
        observe_lock_classes(|| remove_worktree_retained(dir.path(), &wt, true, "test"));
    removed.unwrap();
    check("remove_worktree_retained", &seen, false);

    let (dir, wt) = repo_with_worktree();
    activate(&wt);
    seed_mail(&wt);
    let (torn, seen) = observe_lock_classes(|| {
        crate::worktree_pool_destroy::teardown_worktree_path(dir.path(), &wt, &[])
    });
    torn.unwrap();
    check("teardown_worktree_path", &seen, false);

    let repo = pool_repo();
    let root = repo.path();
    let path = acquire(root, &pool_opts()).unwrap();
    activate(&path);
    seed_mail(&path);
    let (returned, seen) = observe_lock_classes(|| return_to_pool(root, &path));
    returned.unwrap();
    check("return_to_pool", &seen, true);

    seed_mail(&path);
    with_state_lock(root, |pool| {
        pool.entries[0].owner_pid = None;
        Ok(())
    })
    .unwrap();
    let (reused, seen) = observe_lock_classes(|| acquire(root, &pool_opts()));
    assert_eq!(reused.unwrap(), path);
    check("acquire (reuse)", &seen, true);

    with_state_lock(root, |pool| {
        pool.entries[0].owner_pid = None;
        pool.entries[0].owner_pid_start = None;
        Ok(())
    })
    .unwrap();
    seed_mail(&path);
    let (destroyed, seen) = observe_lock_classes(|| destroy_all(root));
    assert_eq!(destroyed.unwrap().removed_count(), 1);
    check("worktree_pool_destroy::destroy", &seen, true);
}

// ── Rework r3: hook writes, reservation tokens, clone writers ───────────────

/// Binding 63 #1: a pre_destroy hook's mail write is caught by the check
/// right before the removal, never trusted. The tree is kept, the source is
/// reopened with the retired mail copied back, and the late write stays.
#[test]
fn a_pre_destroy_hook_that_writes_mail_keeps_the_torn_down_tree() {
    let (dir, wt) = repo_with_worktree();
    activate(&wt);
    write_local_message(&wt, &message("old")).unwrap();
    let hook = "printf '{}' > .aida/mailbox/late.json".to_string();
    let err =
        crate::worktree_pool_destroy::teardown_worktree_path(dir.path(), &wt, &[hook]).unwrap_err();
    let refusal = refusal_of(&err);
    assert!(refusal.reason.contains("late.json"), "{err:#}");
    assert!(!refusal.needs_activation);
    assert!(wt.exists(), "the tree is kept");
    assert_eq!(files_under(&mailbox(&wt)), ["late.json", "old.json"]);
    assert_open(&wt);
}

#[test]
fn a_pre_destroy_hook_that_writes_mail_keeps_the_pool_tree() {
    use crate::worktree_pool::read_state;
    use crate::worktree_pool_destroy::{destroy, DestroyAction, DestroyOptions, DestroySelector};
    let _trust = crate::test_env::EnvVarGuard::set("AIDA_TRUST_PID_NAMESPACE", "1");
    let repo = pool_repo();
    let root = repo.path();
    let (first, second) = two_idle_trees(root);
    activate(&first);
    write_local_message(&first, &message("old")).unwrap();
    std::fs::create_dir_all(second.join(".aida")).unwrap();
    // The hook runs in every tree; it writes into `first`'s mailbox only.
    let late = mailbox(&first).join("late.json");
    let hook = format!(
        "[ \"$(pwd -P)\" = {:?} ] && printf '{{}}' > .aida/mailbox/late.json || true",
        first.canonicalize().unwrap().display().to_string()
    );
    let report = destroy(
        root,
        &DestroySelector::All,
        &DestroyOptions {
            pre_destroy_hooks: vec![hook],
            ..Default::default()
        },
        &mut |_| {},
    )
    .unwrap();
    let target = |p: &Path| report.targets.iter().find(|t| t.entry.path == p).unwrap();
    assert_eq!(target(&first).action, DestroyAction::Skipped);
    assert!(
        target(&first).reason.contains("kept"),
        "{}",
        target(&first).reason
    );
    assert_eq!(
        target(&second).action,
        DestroyAction::Removed,
        "a late write keeps only its own tree"
    );
    assert!(first.exists() && late.exists());
    assert_eq!(files_under(&mailbox(&first)), ["late.json", "old.json"]);
    assert_open(&first);
    let pool = read_state(root).unwrap();
    assert_eq!(pool.entries.len(), 1);
    assert!(pool.entries[0].retiring.is_none());
}

/// A destroy releases only the reservations it made: a concurrent pool call
/// in the same process (the server, a dispatcher thread) keeps its own.
#[test]
fn a_pool_destroy_never_clears_another_calls_reservation_in_this_process() {
    use crate::worktree_pool::{read_state, with_state_lock, PoolRetiring};
    let _trust = crate::test_env::EnvVarGuard::set("AIDA_TRUST_PID_NAMESPACE", "1");
    let repo = pool_repo();
    let root = repo.path();
    let (busy, idle) = two_idle_trees(root);
    std::fs::create_dir_all(idle.join(".aida")).unwrap();
    let other = PoolRetiring::mine();
    with_state_lock(root, |pool| {
        for e in &mut pool.entries {
            if e.path == busy {
                e.retiring = Some(other.clone());
            }
        }
        Ok(())
    })
    .unwrap();
    let report = destroy_all(root).unwrap();
    assert_eq!(report.removed_count(), 1);
    let pool = read_state(root).unwrap();
    assert_eq!(pool.entries.len(), 1);
    assert_eq!(pool.entries[0].path, busy);
    assert!(
        pool.entries[0]
            .retiring
            .as_ref()
            .is_some_and(|r| r.is(&other)),
        "the other call's reservation is kept: {:?}",
        pool.entries[0].retiring
    );
    assert!(
        !PoolRetiring::mine().is(&other),
        "every call has its own token"
    );
}

/// A seat clone laid out as the creator makes it: a real `.git` whose
/// alternates borrow main's objects, the marker naming main, and a `clone`
/// lease in main's sessions naming the clone.
fn bound_clone(main: &Path, parent: &Path) -> PathBuf {
    let clone = parent.join("aida-seat-w");
    let info = clone.join(".git/objects/info");
    std::fs::create_dir_all(&info).unwrap();
    let objects = main.join(".git/objects").canonicalize().unwrap();
    std::fs::write(info.join("alternates"), format!("{}\n", objects.display())).unwrap();
    std::fs::create_dir_all(mailbox(&clone)).unwrap();
    std::fs::write(
        clone.join(crate::seat_clone_detect::MARKER_RELATIVE),
        format!(
            "main = {:?}\n",
            main.canonicalize().unwrap().display().to_string()
        ),
    )
    .unwrap();
    let sessions = main.join(".aida/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("seat.toml"),
        format!(
            "tree_kind = \"clone\"\nworktree_path = {:?}\n",
            clone.canonicalize().unwrap().display().to_string()
        ),
    )
    .unwrap();
    clone
}

/// Writers inside a seat clone take permits under main's anchor through the
/// clone binding, so the fence the clone's teardown takes there is honoured.
#[test]
fn a_writer_inside_a_bound_clone_is_admitted_and_fenced_under_mains_anchor() {
    let main = repo();
    activate(main.path());
    let clones = tempfile::tempdir().unwrap();
    let clone = bound_clone(main.path(), clones.path());
    let anchor = source(main.path()).anchor;
    let clone_source = resolve_clone_source(main.path(), &clone).unwrap().unwrap();

    let permit = admit_local_write(&clone).unwrap().expect("admitted");
    let held = lock(&anchor, LIFECYCLE_LOCK_TIMEOUT).unwrap();
    let record = load_record(&anchor, &clone_source.key, &held)
        .unwrap()
        .unwrap();
    drop(held);
    assert_eq!(record.permits.len(), 1, "the permit is under main's anchor");
    permit.complete().unwrap();

    std::fs::write(mailbox(&clone).join("m.json"), "{}").unwrap();
    let retirement = retire_clone_checkout(Some(main.path()), &clone, "test")
        .unwrap()
        .expect("retired");
    assert!(
        matches!(
            admit_local_write(&clone),
            Err(LifecycleError::SourceRetiring { .. })
        ),
        "a writer in the retiring clone is refused"
    );
    retirement.reopen().unwrap();

    // Without the lease in main the binding does not verify: no anchor.
    std::fs::remove_file(main.path().join(".aida/sessions/seat.toml")).unwrap();
    assert!(admit_local_write(&clone).unwrap().is_none());
}

/// Only the build that recorded the activation honours it: a record from any
/// other build, or one with no build, reads as not activated.
#[test]
fn r3_f4_a_mail_bearing_retirement_names_the_other_build_that_recorded() {
    let dir = repo();
    seed_mail(dir.path());
    let anchor = source(dir.path()).anchor;
    let this = test_source_build();
    write_json(
        &anchor.join(ACTIVATION_FILE),
        &Activation {
            schema: SCHEMA,
            all_writer_hosts_admitting: true,
            recorded_at: "t".into(),
            source_build: Some("0.0.0+other".into()),
        },
    )
    .unwrap();
    let refusal = retire_checkout(dir.path(), "test").unwrap_err();
    assert!(refusal.needs_activation);
    assert!(
        refusal
            .reason
            .contains("different build (0.0.0+other; this is ")
            && refusal.reason.contains(&this)
            && refusal.reason.contains(ACTIVATION_COMMAND),
        "{}",
        refusal.reason
    );
}

#[test]
fn an_activation_recorded_by_another_build_is_not_honoured() {
    let dir = repo();
    let anchor = source(dir.path()).anchor;
    let write = |build| {
        write_json(
            &anchor.join(ACTIVATION_FILE),
            &Activation {
                schema: SCHEMA,
                all_writer_hosts_admitting: true,
                recorded_at: "t".into(),
                source_build: build,
            },
        )
        .unwrap()
    };
    let this = test_source_build();
    write(None);
    assert!(!activated(&anchor).unwrap());
    // R3-F4: a record from other source says so, naming both builds.
    write(Some("0.0.0+other".into()));
    assert_eq!(
        activation_state(&anchor).unwrap(),
        ActivationState::OtherBuild {
            recorded: Some("0.0.0+other".into()),
            this: Some(this.clone()),
        }
    );
    // The same source build (aida-server, or a re-promoted binary) honours it.
    write(Some(this));
    assert!(activated(&anchor).unwrap());
    // A pre-source-build record (an executable inode) is ignored, not corrupt.
    std::fs::write(
        anchor.join(ACTIVATION_FILE),
        r#"{"schema":1,"all_writer_hosts_admitting":true,"recorded_at":"t","build":[1,2]}"#,
    )
    .unwrap();
    assert_eq!(
        activation_state(&anchor).unwrap(),
        ActivationState::OtherBuild {
            recorded: None,
            this: source_build().map(str::to_string),
        }
    );
}

// ── BUG-011: reuse never shows the mailbox without its nonce ────────────────

/// A late entry that lands once the mailbox is in its tombstone sends the
/// directory back with the same nonce bytes, so the reopened source is the
/// one it resolves to.
// trace:BUG-b6b1278890774ef993d2ebc327862470-011 | ai:claude
#[test]
fn a_late_entry_in_the_tombstone_returns_the_mailbox_with_its_nonce() {
    let (_dir, wt) = repo_with_worktree();
    seed_mail(&wt);
    activate(&wt);
    let old = source(&wt);
    let id = mailbox(&wt).join(SOURCE_ID_FILE);
    let nonce = std::fs::read(&id).unwrap();
    let retirement = retire_checkout(&wt, "test").unwrap().expect("retired");
    super::guard::AFTER_TOMB.with(|h| {
        *h.borrow_mut() = Some(Box::new(|tomb: &Path| {
            assert!(
                tomb.join(SOURCE_ID_FILE).is_file(),
                "the nonce moved with it"
            );
            std::fs::write(tomb.join("late.json"), "{}").unwrap();
        }))
    });
    let err = retirement.complete_reuse().unwrap_err();
    assert!(err.to_string().contains("not empty"), "{err}");
    assert_eq!(
        std::fs::read(&id).unwrap(),
        nonce,
        "the nonce bytes survive"
    );
    assert!(files_under(&mailbox(&wt)).contains(&"late.json".to_string()));
    assert_eq!(source(&wt).key, old.key);
    assert_open(&wt);
}

/// A late entry whose path was taken by a new mailbox meanwhile is never
/// merged into it: the error names both paths and the entry stays put.
// trace:BUG-b6b1278890774ef993d2ebc327862470-011 | ai:claude
#[test]
fn a_late_entry_never_merges_into_a_new_mailbox() {
    let (_dir, wt) = repo_with_worktree();
    seed_mail(&wt);
    activate(&wt);
    let retirement = retire_checkout(&wt, "test").unwrap().expect("retired");
    let tomb_seen = std::rc::Rc::new(std::cell::RefCell::new(PathBuf::new()));
    let seen = tomb_seen.clone();
    let path = mailbox(&wt);
    super::guard::AFTER_TOMB.with(|h| {
        *h.borrow_mut() = Some(Box::new(move |tomb: &Path| {
            std::fs::write(tomb.join("late.json"), "{}").unwrap();
            create_mailbox_dir(&path).unwrap();
            *seen.borrow_mut() = tomb.to_path_buf();
        }))
    });
    let err = retirement.complete_reuse().unwrap_err();
    let tomb = tomb_seen.borrow().clone();
    let text = err.to_string();
    assert!(text.contains(&tomb.display().to_string()), "{text}");
    assert!(text.contains(&mailbox(&wt).display().to_string()), "{text}");
    assert!(tomb.join("late.json").is_file(), "the late entry stays");
    assert!(files_under(&mailbox(&wt)).is_empty(), "nothing merged");
}

/// A mailbox whose checkout has carried a nonce and lost it fails closed.
// trace:BUG-b6b1278890774ef993d2ebc327862470-011 | ai:claude
#[test]
fn a_lost_nonce_in_the_nonce_era_fails_closed() {
    let (_dir, wt) = repo_with_worktree();
    write_local_message(&wt, &message("m1")).unwrap();
    resolve_source(&wt).unwrap().expect("anchored");
    std::fs::remove_file(mailbox(&wt).join(SOURCE_ID_FILE)).unwrap();
    let err = resolve_source(&wt).unwrap_err();
    assert!(matches!(err, LifecycleError::Unsafe { .. }), "{err}");
}

/// Without the era marker a nonce-less mailbox is legacy.
// trace:BUG-b6b1278890774ef993d2ebc327862470-011 | ai:claude
#[test]
fn a_nonce_less_mailbox_before_the_era_is_legacy() {
    let (_dir, wt) = repo_with_worktree();
    std::fs::create_dir_all(mailbox(&wt)).unwrap();
    let anchored = resolve_source(&wt).unwrap().expect("anchored");
    assert!(!anchored.anchor.join(NONCE_ERA_DIR).exists());
}
