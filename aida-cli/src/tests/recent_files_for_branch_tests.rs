use super::*;
use std::process::Command;
use tempfile::TempDir;

/// Init a git repo on `branch_name` with `files` committed across N
// commits (one commit per file). trace:TASK-53 | ai:claude
fn init_repo_with_files(branch_name: &str, files: &[&str]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path();
    let run = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(p)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap();
    };
    run(&[
        "init",
        &format!("--initial-branch={}", branch_name),
        "--quiet",
    ]);
    for (i, f) in files.iter().enumerate() {
        // Create the file (with subdirs if needed) and commit it.
        let path = p.join(f);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, format!("v{}", i)).unwrap();
        run(&["add", f]);
        run(&["commit", "-m", &format!("commit {}: {}", i, f), "--quiet"]);
    }
    tmp
}

/// Branch with recent commits → returns the touched files (deduped,
// sorted by BTreeSet ordering). trace:TASK-53 | ai:claude
#[test]
fn returns_committed_files() {
    let tmp = init_repo_with_files("feat-x", &["a.rs", "b.rs", "c.rs"]);
    let files = recent_files_for_branch(tmp.path(), "feat-x", "14 days ago", 10);
    assert_eq!(files, vec!["a.rs", "b.rs", "c.rs"]);
}

/// Max cap respected — a busy branch only surfaces the first N
/// unique files so the warning stays scannable.
#[test]
fn respects_max_cap() {
    let many = ["a.rs", "b.rs", "c.rs", "d.rs", "e.rs", "f.rs"];
    let tmp = init_repo_with_files("feat-x", &many);
    let files = recent_files_for_branch(tmp.path(), "feat-x", "14 days ago", 3);
    assert_eq!(files.len(), 3);
}

/// Same file touched in multiple commits → deduped.
#[test]
fn dedupes_repeated_files() {
    let tmp = init_repo_with_files("feat-x", &["a.rs", "a.rs", "b.rs"]);
    let files = recent_files_for_branch(tmp.path(), "feat-x", "14 days ago", 10);
    assert_eq!(files, vec!["a.rs", "b.rs"]);
}

/// Unknown branch → empty vec (git errors, we treat as no signal).
#[test]
fn unknown_branch_returns_empty() {
    let tmp = init_repo_with_files("feat-x", &["a.rs"]);
    let files = recent_files_for_branch(tmp.path(), "ghost-branch", "14 days ago", 10);
    assert!(files.is_empty());
}

/// Non-git path → empty vec.
#[test]
fn non_git_path_returns_empty() {
    let tmp = TempDir::new().unwrap();
    let files = recent_files_for_branch(tmp.path(), "main", "14 days ago", 10);
    assert!(files.is_empty());
}
