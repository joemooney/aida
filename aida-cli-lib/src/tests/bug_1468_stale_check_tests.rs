// trace:BUG-1468 | ai:claude
//! Regression fixture for BUG-1468: a PR green at T, a guard-strengthening
//! commit on main at T+1, the PR still reporting green at T+2. These tests
//! cover the pure classifier and the git-IO wrapper that make that staleness
//! DETERMINABLE (workflow file or guarded-test file changed on main since
//! divergence) as distinct from the common, harmless "base moved" case.

use super::*;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn guard_defining_files_flags_workflow_changes() {
    let changed = vec![".github/workflows/ci.yml".to_string()];
    assert_eq!(guard_defining_files(&changed), changed);
}

#[test]
fn guard_defining_files_flags_test_files() {
    let changed = vec![
        "aida-cli-lib/src/tests/branch_behind_main_tests.rs".to_string(),
        "aida-core/src/tests/discipline_pack_test.rs".to_string(),
    ];
    assert_eq!(guard_defining_files(&changed), changed);
}

#[test]
fn guard_defining_files_ignores_ordinary_source() {
    let changed = vec![
        "aida-cli-lib/src/lib.rs".to_string(),
        "README.md".to_string(),
    ];
    assert!(guard_defining_files(&changed).is_empty());
}

/// Init a repo with `main` and a feature branch, and let the caller add
/// commits to `main` after the branch was cut (simulating a PR's own CI
/// having already run against the tip it branched from).
fn fixture() -> (TempDir, &'static str) {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path();
    let run = |args: &[&str]| {
        let r = Command::new("git")
            .arg("-C")
            .arg(p)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(
            r.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&r.stderr)
        );
    };
    run(&["init", "--initial-branch=main", "--quiet"]);
    std::fs::write(p.join("README.md"), "root\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-m", "root", "--quiet"]);
    run(&["checkout", "-q", "-b", "pr-branch"]);
    (tmp, "pr-branch")
}

/// Base moved with no guard-defining change → warning present but
/// `guard_files` empty (the harmless case).
// trace:BUG-1468 | ai:claude
#[test]
fn base_moved_without_guard_change_is_not_flagged() {
    let (tmp, branch) = fixture();
    let p = tmp.path();
    let run = |args: &[&str]| {
        assert!(Command::new("git")
            .arg("-C")
            .arg(p)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap()
            .status
            .success());
    };
    run(&["checkout", "-q", "main"]);
    std::fs::write(p.join("notes.md"), "unrelated\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-m", "unrelated main commit", "--quiet"]);
    run(&["checkout", "-q", branch]);

    let warning = pr_stale_check_warning(p, branch, "main").expect("branch is behind main");
    assert_eq!(warning.behind_commits, 1);
    assert!(warning.guard_files.is_empty());
}

/// THE regression shape: PR green at T (branch cut), a guard-strengthening
/// commit lands on main at T+1 (a workflow file changes), and at T+2 the
/// branch is still behind with that guard-defining file in the diff —
/// exactly what PR #1979 showed as green for 15 hours after BUG-1426 landed.
// trace:BUG-1468 | ai:claude
#[test]
fn guard_strengthening_commit_on_main_is_flagged_stale() {
    let (tmp, branch) = fixture();
    let p = tmp.path();
    let run = |args: &[&str]| {
        assert!(Command::new("git")
            .arg("-C")
            .arg(p)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap()
            .status
            .success());
    };
    run(&["checkout", "-q", "main"]);
    std::fs::create_dir_all(p.join(".github/workflows")).unwrap();
    std::fs::write(p.join(".github/workflows/ci.yml"), "name: CI\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-m", "strengthen required guard", "--quiet"]);
    run(&["checkout", "-q", branch]);

    let warning = pr_stale_check_warning(p, branch, "main").expect("branch is behind main");
    assert_eq!(warning.behind_commits, 1);
    assert_eq!(
        warning.guard_files,
        vec![".github/workflows/ci.yml".to_string()]
    );
}

/// Branch not behind base at all → no warning (nothing stale to report).
#[test]
fn up_to_date_branch_returns_none() {
    let (tmp, branch) = fixture();
    assert!(pr_stale_check_warning(tmp.path(), branch, "main").is_none());
}
