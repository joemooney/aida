//! trace:BUG-75 BUG-76 | ai:claude
use super::*;
use std::process::Command;
use tempfile::TempDir;

fn git(p: &std::path::Path, args: &[&str]) {
    let o = Command::new("git")
        .arg("-C")
        .arg(p)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(o.status.success(), "git {:?} failed", args);
}

fn fixture_with_linked_worktree() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let main = tmp.path().join("repo");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "--initial-branch=main", "--quiet"]);
    std::fs::write(main.join("a.txt"), "x").unwrap();
    git(&main, &["add", "a.txt"]);
    git(&main, &["commit", "-m", "base", "--quiet"]);
    git(&main, &["checkout", "-b", "feature", "--quiet"]);
    std::fs::write(main.join("a.txt"), "y").unwrap();
    git(&main, &["add", "a.txt"]);
    git(&main, &["commit", "-m", "feature", "--quiet"]);
    git(&main, &["checkout", "main", "--quiet"]);

    let linked = tmp.path().join("repo-feature");
    git(
        &main,
        &[
            "worktree",
            "add",
            linked.to_str().unwrap(),
            "feature",
            "--quiet",
        ],
    );
    (tmp, main, linked)
}

/// BUG-75: from inside a linked worktree, main_worktree_root_from
/// returns the MAIN worktree path, not the linked one.
#[test]
fn main_worktree_root_resolves_from_linked() {
    let (_tmp, main, linked) = fixture_with_linked_worktree();
    let resolved = main_worktree_root_from(&linked);
    assert_eq!(
        std::fs::canonicalize(&resolved).unwrap(),
        std::fs::canonicalize(&main).unwrap(),
        "expected main worktree, got {}",
        resolved.display()
    );
}

/// BUG-75: from the main worktree, the helper returns the same path
/// (no regression).
#[test]
fn main_worktree_root_returns_main_unchanged() {
    let (_tmp, main, _linked) = fixture_with_linked_worktree();
    let resolved = main_worktree_root_from(&main);
    assert_eq!(
        std::fs::canonicalize(&resolved).unwrap(),
        std::fs::canonicalize(&main).unwrap()
    );
}

/// BUG-76: detect_default_branch_ref picks origin/main when present,
/// falls back to local main, returns None when neither exists.
#[test]
fn default_branch_ref_prefers_origin_main() {
    // Origin-less repo with a local main → falls back to local.
    let (_tmp, main, _linked) = fixture_with_linked_worktree();
    let resolved = detect_default_branch_ref(&main);
    assert_eq!(resolved.as_deref(), Some("main"));
}

/// BUG-76: current_branch_at returns the branch checked out at path.
#[test]
fn current_branch_at_returns_branch_name() {
    let (_tmp, main, linked) = fixture_with_linked_worktree();
    assert_eq!(current_branch_at(&main).as_deref(), Some("main"));
    assert_eq!(current_branch_at(&linked).as_deref(), Some("feature"));
}
