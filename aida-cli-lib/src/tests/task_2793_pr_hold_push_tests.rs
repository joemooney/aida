//! TASK-2793: `aida pr hold` must push the held branch and verify origin's
//! tip before it records the hold or claims the branch was pushed.
// trace:TASK-2793 | ai:claude

use super::push_and_verify_held_branch;
use crate::test_git::{commit_fixture, git_command};

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = git_command()
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn repo_with_bare_origin() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let origin = tmp.path().join("origin.git");
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(
        tmp.path(),
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    commit_fixture(&work);
    git(
        &work,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&work, &["checkout", "-q", "-b", "held-branch"]);
    (tmp, work, origin)
}

#[test]
fn hold_pushes_an_unpushed_branch_and_returns_the_verified_tip() {
    let (_tmp, work, origin) = repo_with_bare_origin();
    let local = git(&work, &["rev-parse", "HEAD"]);

    let sha = push_and_verify_held_branch(&work, "origin", "held-branch").expect("push + verify");

    assert_eq!(sha, local);
    assert_eq!(
        git(&origin, &["rev-parse", "refs/heads/held-branch"]),
        local
    );
    assert_eq!(
        git(
            &work,
            &["rev-parse", "--abbrev-ref", "held-branch@{upstream}"]
        ),
        "origin/held-branch"
    );
}

#[test]
fn hold_fails_when_the_push_cannot_reach_the_remote() {
    let (tmp, work, _origin) = repo_with_bare_origin();
    let missing = tmp.path().join("no-such-remote.git");
    git(
        &work,
        &["remote", "set-url", "origin", missing.to_str().unwrap()],
    );

    let err = push_and_verify_held_branch(&work, "origin", "held-branch")
        .expect_err("an unreachable remote must not report a held, pushed branch");

    assert!(err.to_string().contains("could not push"), "{err}");
}

#[test]
fn hold_fails_when_the_local_branch_is_missing() {
    let (_tmp, work, _origin) = repo_with_bare_origin();

    let err = push_and_verify_held_branch(&work, "origin", "never-created")
        .expect_err("a missing branch has nothing to push");

    assert!(err.to_string().contains("does not exist"), "{err}");
}

#[test]
fn hold_succeeds_when_the_push_lands_but_the_upstream_config_write_fails() {
    let (_tmp, work, origin) = repo_with_bare_origin();
    let local = git(&work, &["rev-parse", "HEAD"]);
    // A held config lock makes `push -u` exit nonzero after the ref uploaded —
    // the sandboxed-seat failure seen while shipping this fix.
    std::fs::write(work.join(".git").join("config.lock"), "").unwrap();

    let sha = push_and_verify_held_branch(&work, "origin", "held-branch")
        .expect("origin's verified tip, not the push exit code, decides the hold");

    assert_eq!(sha, local);
    assert_eq!(
        git(&origin, &["rev-parse", "refs/heads/held-branch"]),
        local
    );
}
