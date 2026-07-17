use super::*;
use std::process::Command;
use tempfile::TempDir;

/// Init a real git repo with one commit and a `.gitignore` that
/// excludes `target/` and `.aida/cache.db` — the two canonical
/// "disposable" patterns from BUG-67's acceptance.
fn init_repo_with_gitignore() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path();
    Command::new("git")
        .arg("-C")
        .arg(p)
        .args(["init", "--initial-branch=main", "--quiet"])
        .status()
        .unwrap();
    std::fs::write(p.join(".gitignore"), "target/\n.aida/cache.db\n").unwrap();
    Command::new("git")
        .arg("-C")
        .arg(p)
        .args(["add", ".gitignore"])
        .status()
        .unwrap();
    Command::new("git")
        .arg("-C")
        .arg(p)
        .args(["commit", "-m", "init", "--quiet"])
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .unwrap();
    tmp
}

// Clean worktree → empty vec. trace:BUG-67 | ai:claude
#[test]
fn clean_worktree_is_clean() {
    let tmp = init_repo_with_gitignore();
    assert!(worktree_dirty_entries(tmp.path()).is_empty());
}

/// Build artifacts under `target/` are gitignored → no entries.
/// This is the headline case from BUG-67: every reviewer session
// builds with cargo before reviewing. trace:BUG-67 | ai:claude
#[test]
fn gitignored_target_dir_is_clean() {
    let tmp = init_repo_with_gitignore();
    std::fs::create_dir_all(tmp.path().join("target/release")).unwrap();
    std::fs::write(tmp.path().join("target/release/aida"), b"binary").unwrap();
    assert!(worktree_dirty_entries(tmp.path()).is_empty());
}

// `.aida/cache.db` is gitignored → no entries. trace:BUG-67 | ai:claude
#[test]
fn gitignored_cache_db_is_clean() {
    let tmp = init_repo_with_gitignore();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(tmp.path().join(".aida/cache.db"), b"sqlite").unwrap();
    assert!(worktree_dirty_entries(tmp.path()).is_empty());
}

// A modified tracked file shows up. trace:BUG-67 | ai:claude
#[test]
fn tracked_modified_file_is_dirty() {
    let tmp = init_repo_with_gitignore();
    // Modify the tracked .gitignore so it shows as "M".
    std::fs::write(
        tmp.path().join(".gitignore"),
        "target/\n.aida/cache.db\n# touched\n",
    )
    .unwrap();
    let entries = worktree_dirty_entries(tmp.path());
    assert_eq!(entries.len(), 1, "{:?}", entries);
    assert!(entries[0].contains(".gitignore"), "{:?}", entries);
}

// Untracked-but-not-ignored file shows up. trace:BUG-67 | ai:claude
#[test]
fn untracked_unignored_file_is_dirty() {
    let tmp = init_repo_with_gitignore();
    std::fs::write(tmp.path().join("scratch.rs"), b"// notes").unwrap();
    let entries = worktree_dirty_entries(tmp.path());
    assert_eq!(entries.len(), 1, "{:?}", entries);
    assert!(entries[0].contains("scratch.rs"), "{:?}", entries);
}

/// Mix: gitignored build output is hidden, real changes show.
// trace:BUG-67 | ai:claude
#[test]
fn mixed_only_real_changes_show() {
    let tmp = init_repo_with_gitignore();
    std::fs::create_dir_all(tmp.path().join("target")).unwrap();
    std::fs::write(tmp.path().join("target/foo"), b"x").unwrap();
    std::fs::write(tmp.path().join("scratch.rs"), b"// notes").unwrap();
    let entries = worktree_dirty_entries(tmp.path());
    assert_eq!(entries.len(), 1, "{:?}", entries);
    assert!(entries[0].contains("scratch.rs"), "{:?}", entries);
    assert!(
        !entries.iter().any(|e| e.contains("target")),
        "{:?}",
        entries
    );
}

/// Non-git path → empty vec (git errors, we treat as clean and let
/// the downstream remove --force produce the real error).
// trace:BUG-67 | ai:claude
#[test]
fn non_git_path_is_clean() {
    let tmp = TempDir::new().unwrap();
    assert!(worktree_dirty_entries(tmp.path()).is_empty());
}
