//! TASK-1543 — Reap polish:
//! 1. Where reap/teardown prunes a missing worktree, use 'git worktree remove --force -- <lease path>'
//!    (scoped to that entry) instead of a repo-wide 'git worktree prune', so another session's
//!    temporarily unavailable worktree keeps its registration.
//! 2. worktree_is_locked parses 'git worktree list --porcelain -z' so paths with newlines are detected.
//! 3. Fixture tests for both.
//
// trace:TASK-1543 | ai:antigravity

use super::*;
use std::path::{Path, PathBuf};

fn git(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit_file(root: &Path, file: &str, content: &str, message: &str) {
    std::fs::write(root.join(file), content).unwrap();
    git(root, &["add", file]);
    git(root, &["commit", "-q", "-m", message]);
}

fn fixture_lease(worktree: &Path, branch: &str, scope: &str) -> SessionLease {
    serde_json::from_value(serde_json::json!({
        "id": "t1543fixture",
        "scope": scope,
        "slug": scope.to_ascii_lowercase(),
        "owner": "test",
        "worktree_path": worktree,
        "branch": branch,
        "started_at": "2026-01-01T00:00:00Z",
        "hostname": "fixture",
    }))
    .expect("fixture lease parses")
}

fn create_fixture_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    commit_file(&root, "README.md", "base\n", "init");
    git(&root, &["branch", "-M", "main"]);
    (tmp, root)
}

/// Acceptance 1: Where reap prunes a missing worktree, use
/// `git worktree remove --force -- <lease path>` scoped to that entry instead of
/// a repo-wide `git worktree prune`, so another session's temporarily
/// unavailable worktree keeps its registration.
// trace:TASK-1543 | ai:antigravity
#[test]
fn task_1543_reap_missing_worktree_scopes_removal_preserving_sibling_missing_worktree() {
    let (tmp, root) = create_fixture_repo();

    let wt_a = tmp.path().join("wt-spec-a");
    let wt_b = tmp.path().join("wt-spec-b");

    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "spec-a",
            wt_a.to_str().unwrap(),
        ],
    );
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "spec-b",
            wt_b.to_str().unwrap(),
        ],
    );

    let wt_a = wt_a.canonicalize().unwrap();
    let wt_b = wt_b.canonicalize().unwrap();

    commit_file(&wt_a, "a.txt", "content-a\n", "feat(a): task a (TASK-1543)");
    commit_file(&wt_b, "b.txt", "content-b\n", "feat(b): task b (TASK-9999)");

    // Land spec-a on main via batched integration commit so reap can classify it landed.
    git(&root, &["checkout", "-q", "main"]);
    std::fs::write(root.join("a.txt"), "content-a\n").unwrap();
    git(&root, &["add", "a.txt"]);
    git(
        &root,
        &[
            "commit",
            "-q",
            "-m",
            "[AI:antigravity] chore(integrate): batch 1 (TASK-1543)",
            "-m",
            "Commit for spec a (TASK-1543)",
        ],
    );

    // Both worktree directories are removed by hand (e.g. unmounted / wiped).
    std::fs::remove_dir_all(&wt_a).unwrap();
    std::fs::remove_dir_all(&wt_b).unwrap();

    let initial_listing = git(&root, &["worktree", "list", "--porcelain"]);
    assert!(initial_listing.contains(wt_a.to_str().unwrap()));
    assert!(initial_listing.contains(wt_b.to_str().unwrap()));
    assert!(initial_listing.contains("prunable"));

    let (_facts, tip) = gather_merge_facts_pinned(
        &root,
        Some("main"),
        "spec-a",
        "TASK-1543",
        false,
        true,
        |_| false,
    );
    let lease = fixture_lease(&wt_a, "spec-a", "TASK-1543");
    let outcome = reap_one(&root, &lease, tip.as_deref());
    assert!(
        outcome.starts_with("reaped"),
        "expected reaped, got: {outcome}"
    );
    assert!(
        outcome.contains("deleted"),
        "expected branch deleted, got: {outcome}"
    );
    assert!(resolve_local_branch_tip(&root, "spec-a").is_none());

    // After reap of wt_a:
    // 1. wt_a registration is GONE.
    // 2. wt_b registration is PRESERVED (still in git worktree listing as prunable).
    let listing = git(&root, &["worktree", "list", "--porcelain"]);
    assert!(
        !listing.contains(wt_a.to_str().unwrap()),
        "wt_a should have been removed from worktree list:\n{listing}"
    );
    assert!(
        listing.contains(wt_b.to_str().unwrap()),
        "wt_b should have been preserved in worktree list:\n{listing}"
    );
    assert!(
        listing.contains("prunable"),
        "wt_b should still be marked prunable in worktree list:\n{listing}"
    );
}

/// Acceptance 1: teardown_worktree_path does not run repo-wide prune, preserving
/// another session's temporarily unavailable worktree.
// trace:TASK-1543 | ai:antigravity
#[test]
fn task_1543_teardown_does_not_prune_sibling_missing_worktree() {
    let (tmp, root) = create_fixture_repo();

    let wt_1 = tmp.path().join("wt-1");
    let wt_2 = tmp.path().join("wt-2");

    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "branch-1",
            wt_1.to_str().unwrap(),
        ],
    );
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "branch-2",
            wt_2.to_str().unwrap(),
        ],
    );

    let wt_1 = wt_1.canonicalize().unwrap();
    let wt_2 = wt_2.canonicalize().unwrap();

    // Directory of wt_2 is removed (temporarily unavailable).
    std::fs::remove_dir_all(&wt_2).unwrap();

    // Now teardown wt_1 (which still exists on disk).
    let ok = aida_core::worktree_pool_destroy::teardown_worktree_path(&root, &wt_1, &[]).unwrap();
    assert!(ok);
    assert!(!wt_1.exists());

    // wt_2 must NOT have been pruned by teardown of wt_1!
    let listing = git(&root, &["worktree", "list", "--porcelain"]);
    assert!(
        !listing.contains(wt_1.to_str().unwrap()),
        "wt_1 must be removed:\n{listing}"
    );
    assert!(
        listing.contains(wt_2.to_str().unwrap()),
        "wt_2 must be preserved:\n{listing}"
    );
    assert!(
        listing.contains("prunable"),
        "wt_2 must still be listed as prunable:\n{listing}"
    );
}

/// Acceptance 2: worktree_is_locked parses 'git worktree list --porcelain -z'
/// so paths with newlines are detected properly when locked or unlocked.
// trace:TASK-1543 | ai:antigravity
#[test]
fn task_1543_worktree_is_locked_detects_newline_in_path() {
    let (tmp, root) = create_fixture_repo();

    // Worktree path containing newlines.
    let wt_nl = tmp.path().join("wt\nwith\nnewlines");
    let wt_other = tmp.path().join("wt_other");

    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "b_nl",
            wt_nl.to_str().unwrap(),
        ],
    );
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "b_other",
            wt_other.to_str().unwrap(),
        ],
    );

    let wt_nl = wt_nl.canonicalize().unwrap();
    let wt_other = wt_other.canonicalize().unwrap();

    // Initially neither is locked.
    assert!(
        !crate::worktree_is_locked(&root, &wt_nl),
        "unlocked newline worktree should not report locked"
    );
    assert!(
        !crate::worktree_is_locked(&root, &wt_other),
        "unlocked regular worktree should not report locked"
    );

    // Lock wt_nl with a reason.
    git(
        &root,
        &[
            "worktree",
            "lock",
            "--reason",
            "testing newline locks",
            wt_nl.to_str().unwrap(),
        ],
    );
    assert!(
        crate::worktree_is_locked(&root, &wt_nl),
        "locked newline worktree should report locked"
    );
    assert!(
        !crate::worktree_is_locked(&root, &wt_other),
        "unlocked sibling should not report locked"
    );

    // Unlock wt_nl.
    git(&root, &["worktree", "unlock", wt_nl.to_str().unwrap()]);
    assert!(
        !crate::worktree_is_locked(&root, &wt_nl),
        "unlocked newline worktree should not report locked"
    );

    // Lock wt_other without reason (bare locked).
    git(&root, &["worktree", "lock", wt_other.to_str().unwrap()]);
    assert!(
        !crate::worktree_is_locked(&root, &wt_nl),
        "newline worktree should remain unlocked"
    );
    assert!(
        crate::worktree_is_locked(&root, &wt_other),
        "regular worktree should report locked"
    );

    // Lock wt_nl bare as well.
    git(&root, &["worktree", "lock", wt_nl.to_str().unwrap()]);
    assert!(
        crate::worktree_is_locked(&root, &wt_nl),
        "bare locked newline worktree should report locked"
    );
    assert!(
        crate::worktree_is_locked(&root, &wt_other),
        "regular worktree should still report locked"
    );

    // Non-existent repo returns false.
    let bogus = tmp.path().join("does_not_exist");
    assert!(!crate::worktree_is_locked(&bogus, &wt_nl));
}
