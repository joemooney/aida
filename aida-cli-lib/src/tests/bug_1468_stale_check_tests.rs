// trace:BUG-1468 | ai:claude
//! Regression fixtures for BUG-1468 and its follow-up (two-tier
//! false-positive fix): a PR green at T, a change on main at T+1, the PR
//! still reporting green at T+2. Only a CI-DEFINITION change (a workflow, or
//! a script it invokes) is refuse-worthy; an ordinary test-file change on
//! main — the common case in this repo — only warns.

use super::*;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn classify_flags_workflow_changes_as_definition() {
    let changed = vec![".github/workflows/ci.yml".to_string()];
    let (definition, test_only) = classify_changed_files(&changed, &|_| false);
    assert_eq!(definition, changed);
    assert!(test_only.is_empty());
}

#[test]
fn classify_flags_workflow_invoked_scripts_as_definition() {
    let changed = vec!["scripts/check-portability.sh".to_string()];
    let (definition, test_only) = classify_changed_files(&changed, &|_| true);
    assert_eq!(definition, changed);
    assert!(test_only.is_empty());
}

#[test]
fn classify_treats_non_invoked_scripts_as_ordinary_source() {
    let changed = vec!["scripts/one-off-cleanup.sh".to_string()];
    let (definition, test_only) = classify_changed_files(&changed, &|_| false);
    assert!(definition.is_empty());
    assert!(test_only.is_empty());
}

#[test]
fn classify_flags_test_files_as_warn_only() {
    let changed = vec![
        "aida-cli-lib/src/tests/branch_behind_main_tests.rs".to_string(),
        "aida-core/src/tests/discipline_pack_test.rs".to_string(),
    ];
    let (definition, test_only) = classify_changed_files(&changed, &|_| false);
    assert!(definition.is_empty());
    assert_eq!(test_only, changed);
}

#[test]
fn classify_ignores_ordinary_source() {
    let changed = vec![
        "aida-cli-lib/src/lib.rs".to_string(),
        "README.md".to_string(),
    ];
    let (definition, test_only) = classify_changed_files(&changed, &|_| false);
    assert!(definition.is_empty());
    assert!(test_only.is_empty());
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

fn run_git(p: &std::path::Path, args: &[&str]) {
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
}

/// Base moved with no definition or test change → warning present but both
/// tiers empty (the harmless "base moved" case).
// trace:BUG-1468 | ai:claude
#[test]
fn base_moved_without_definition_or_test_change_is_not_flagged() {
    let (tmp, branch) = fixture();
    let p = tmp.path();
    run_git(p, &["checkout", "-q", "main"]);
    std::fs::write(p.join("notes.md"), "unrelated\n").unwrap();
    run_git(p, &["add", "."]);
    run_git(p, &["commit", "-m", "unrelated main commit", "--quiet"]);
    run_git(p, &["checkout", "-q", branch]);

    let warning = pr_stale_check_warning(p, branch, "main").expect("branch is behind main");
    assert_eq!(warning.behind_commits, 1);
    assert!(warning.definition_files.is_empty());
    assert!(warning.test_files.is_empty());
}

/// Follow-up regression: an ordinary TEST-only change on main — the common
/// shape in this repo, where nearly every commit touches `*/tests/` or
/// `*_tests.rs` — must WARN, never land in `definition_files` (which would
/// make `--override-stale-check` routine).
// trace:BUG-1468 | ai:claude
#[test]
fn test_only_change_on_main_warns_but_does_not_flag_definition() {
    let (tmp, branch) = fixture();
    let p = tmp.path();
    run_git(p, &["checkout", "-q", "main"]);
    std::fs::create_dir_all(p.join("aida-cli-lib/src/tests")).unwrap();
    std::fs::write(
        p.join("aida-cli-lib/src/tests/some_new_tests.rs"),
        "// test\n",
    )
    .unwrap();
    run_git(p, &["add", "."]);
    run_git(p, &["commit", "-m", "add a test", "--quiet"]);
    run_git(p, &["checkout", "-q", branch]);

    let warning = pr_stale_check_warning(p, branch, "main").expect("branch is behind main");
    assert_eq!(warning.behind_commits, 1);
    assert!(warning.definition_files.is_empty());
    assert_eq!(
        warning.test_files,
        vec!["aida-cli-lib/src/tests/some_new_tests.rs".to_string()]
    );
}

/// THE regression shape: PR green at T (branch cut), a CI-DEFINITION change
/// lands on main at T+1 (a workflow file changes), and at T+2 the branch is
/// still behind with that file in the diff — exactly what PR #1979 showed as
/// green for 15 hours after BUG-1426 landed. This tier REFUSES.
// trace:BUG-1468 | ai:claude
#[test]
fn workflow_change_on_main_is_flagged_as_definition() {
    let (tmp, branch) = fixture();
    let p = tmp.path();
    run_git(p, &["checkout", "-q", "main"]);
    std::fs::create_dir_all(p.join(".github/workflows")).unwrap();
    std::fs::write(p.join(".github/workflows/ci.yml"), "name: CI\n").unwrap();
    run_git(p, &["add", "."]);
    run_git(p, &["commit", "-m", "strengthen required guard", "--quiet"]);
    run_git(p, &["checkout", "-q", branch]);

    let warning = pr_stale_check_warning(p, branch, "main").expect("branch is behind main");
    assert_eq!(warning.behind_commits, 1);
    assert_eq!(
        warning.definition_files,
        vec![".github/workflows/ci.yml".to_string()]
    );
    assert!(warning.test_files.is_empty());
}

/// A `scripts/` file that a tracked workflow YAML literally references is
/// also definition-tier, found via `git grep` over `.github/workflows` on
/// `base_ref` rather than a fixed list.
// trace:BUG-1468 | ai:claude
#[test]
fn workflow_invoked_script_change_is_flagged_as_definition() {
    let (tmp, branch) = fixture();
    let p = tmp.path();
    run_git(p, &["checkout", "-q", "main"]);
    std::fs::create_dir_all(p.join(".github/workflows")).unwrap();
    std::fs::create_dir_all(p.join("scripts")).unwrap();
    std::fs::write(
        p.join(".github/workflows/ci.yml"),
        "name: CI\njobs:\n  build:\n    steps:\n      - run: scripts/check-portability.sh\n",
    )
    .unwrap();
    std::fs::write(
        p.join("scripts/check-portability.sh"),
        "#!/bin/sh\necho ok\n",
    )
    .unwrap();
    run_git(p, &["add", "."]);
    run_git(p, &["commit", "-m", "add workflow + script", "--quiet"]);
    run_git(p, &["checkout", "-q", branch]);

    // Strengthen the script on main only (the workflow YAML itself doesn't
    // change again) so the diff exercises the script→workflow lookup path.
    run_git(p, &["checkout", "-q", "main"]);
    std::fs::write(
        p.join("scripts/check-portability.sh"),
        "#!/bin/sh\necho stricter\n",
    )
    .unwrap();
    run_git(p, &["add", "."]);
    run_git(
        p,
        &["commit", "-m", "strengthen the invoked script", "--quiet"],
    );
    run_git(p, &["checkout", "-q", branch]);

    let warning = pr_stale_check_warning(p, branch, "main").expect("branch is behind main");
    assert!(warning
        .definition_files
        .contains(&"scripts/check-portability.sh".to_string()));
}

/// Branch not behind base at all → no warning (nothing stale to report).
#[test]
fn up_to_date_branch_returns_none() {
    let (tmp, branch) = fixture();
    assert!(pr_stale_check_warning(tmp.path(), branch, "main").is_none());
}
