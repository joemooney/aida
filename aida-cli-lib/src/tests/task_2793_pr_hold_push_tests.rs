//! TASK-2793: `aida pr hold` must push the held branch and verify origin's
//! tip before it records the hold or claims the branch was pushed.
// trace:TASK-2793 | ai:claude

use super::push_and_verify_held_branch;
use crate::test_git::commit_fixture;

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
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

// trace:TASK-2852 | ai:codex
#[test]
fn hold_pushes_from_a_nested_directory_in_a_sibling_worktree() {
    let (tmp, work, origin) = repo_with_bare_origin();
    let sibling = tmp.path().join("sibling");
    git(
        &work,
        &[
            "worktree",
            "add",
            "-b",
            "sibling-branch",
            sibling.to_str().unwrap(),
        ],
    );
    let nested = sibling.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let local = git(&sibling, &["rev-parse", "HEAD"]);

    assert_eq!(
        push_and_verify_held_branch(&nested, "origin", "sibling-branch").unwrap(),
        local
    );
    assert_eq!(
        git(&origin, &["rev-parse", "refs/heads/sibling-branch"]),
        local
    );
}

// trace:TASK-2852 | ai:codex
#[test]
fn hold_preserves_a_seat_clone_refusal_for_an_existing_branch() {
    let (_tmp, work, origin) = repo_with_bare_origin();
    // A marker is sufficient for host_git_command to refuse. No containment
    // capability is supplied: git must not run in this seat-owned repository.
    std::fs::create_dir(work.join(".aida")).unwrap();
    std::fs::write(work.join(".aida/seat-clone.toml"), "").unwrap();
    assert!(!git(&work, &["rev-parse", "refs/heads/held-branch"]).is_empty());

    let err = push_and_verify_held_branch(&work, "origin", "held-branch").unwrap_err();
    let message = err.to_string();
    assert!(message.contains("refusing to run git"), "{message}");
    assert!(message.contains("configuration and hooks"), "{message}");
    assert!(message.contains("PR is not held"), "{message}");
    assert!(!message.contains("does not exist"), "{message}");
    assert!(git(&origin, &["for-each-ref", "refs/heads/held-branch"]).is_empty());
}

// trace:TASK-2852 | ai:codex
#[cfg(target_os = "linux")]
#[test]
fn hold_preserves_git_lookup_errors_for_an_invalid_git_file() {
    // Ignore TMPDIR, which may be inside a repository or a seat clone. An
    // invalid gitfile stops Git discovery even if /tmp has a repository above it.
    let tmp = tempfile::tempdir_in("/tmp").unwrap();
    std::fs::write(tmp.path().join(".git"), "invalid gitfile\n").unwrap();
    let err = push_and_verify_held_branch(tmp.path(), "origin", "held-branch").unwrap_err();
    let message = err.to_string();
    assert!(message.contains("invalid gitfile format"), "{message}");
    assert!(message.contains("PR is not held"), "{message}");
    assert!(!message.contains("does not exist"), "{message}");
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

#[test]
fn hold_fails_when_origin_rejects_a_non_fast_forward_push() {
    let (_tmp, work, origin) = repo_with_bare_origin();
    git(&work, &["push", "-q", "origin", "held-branch"]);
    let published = git(&work, &["rev-parse", "HEAD"]);
    // Rewrite the local tip so it no longer descends from origin's.
    git(
        &work,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-q",
            "--amend",
            "--allow-empty",
            "-m",
            "diverged",
        ],
    );
    let local = git(&work, &["rev-parse", "HEAD"]);
    assert_ne!(local, published);

    let err = push_and_verify_held_branch(&work, "origin", "held-branch")
        .expect_err("a rejected non-fast-forward push must not report a held branch");

    let msg = err.to_string();
    assert!(msg.contains("could not push"), "{msg}");
    assert!(msg.contains("not held"), "{msg}");
    assert_eq!(
        git(&origin, &["rev-parse", "refs/heads/held-branch"]),
        published,
        "origin keeps its tip"
    );
}
