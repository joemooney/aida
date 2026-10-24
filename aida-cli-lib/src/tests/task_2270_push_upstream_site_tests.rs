//! Site tests for the `push -u` sites wired to the config.lock pre-clear and
//! upstream repair: each must leave the branch tracking the pushed branch
//! under the sandbox placeholder, even one forked from `origin/main` with
//! tracking (the failed write would otherwise leave that stale upstream).
//! Either half of the wiring alone passes here (the repair heals the lock
//! too); removing both fails. `git_ops::config_lock`'s tests cover the
//! repair on its own.
// trace:TASK-2306 trace:TASK-2305 | ai:claude

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn git_ok(dir: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A clone-like repo with a bare `origin`, on branch `feat` tracking
/// `origin/main`, with the read-only placeholder planted.
fn placeholder_repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    git_ok(root.path(), &["init", "-q", "--bare", "remote.git"]);
    let work = root.path().join("work");
    std::fs::create_dir(&work).unwrap();
    git_ok(&work, &["init", "-q", "-b", "main"]);
    git_ok(&work, &["config", "user.email", "t@example.invalid"]);
    git_ok(&work, &["config", "user.name", "t"]);
    git_ok(&work, &["commit", "-q", "--allow-empty", "-m", "init"]);
    git_ok(&work, &["remote", "add", "origin", "../remote.git"]);
    git_ok(&work, &["push", "-q", "origin", "main"]);
    git_ok(
        &work,
        &["checkout", "-q", "--track", "-b", "feat", "origin/main"],
    );
    let lock = work.join(".git/config.lock");
    std::fs::write(&lock, b"").unwrap();
    std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o444)).unwrap();
    (root, work, lock)
}

fn assert_tracks_feat(work: &Path, lock: &Path) {
    assert_eq!(
        git_ok(work, &["rev-parse", "--abbrev-ref", "feat@{upstream}"]),
        "origin/feat"
    );
    assert!(!lock.exists());
}

#[test]
fn gitlab_git_push_u_sets_the_upstream_under_the_placeholder() {
    let (_root, work, lock) = placeholder_repo();
    super::git_push(
        &work,
        "origin",
        "../remote.git",
        &["-q", "-u", "origin", "feat"],
    )
    .unwrap()
    .unwrap();
    assert_tracks_feat(&work, &lock);
}

#[test]
fn init_push_set_upstream_sets_the_upstream_under_the_placeholder() {
    let (_root, work, lock) = placeholder_repo();
    assert!(crate::init_cmd::push_set_upstream(&work, "feat"));
    assert_tracks_feat(&work, &lock);
}

#[test]
fn queue_recover_push_sets_the_upstream_under_the_placeholder() {
    let (_root, work, lock) = placeholder_repo();
    assert!(crate::queue_cmd::recover_push_branch(&work, "feat").unwrap());
    assert_tracks_feat(&work, &lock);
}
