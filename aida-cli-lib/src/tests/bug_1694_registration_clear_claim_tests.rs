//! BUG-1694 — reap's registration clear claims the worktree path atomically.
//!
//! TASK-1543 stopped reap from force-removing a reappeared worktree PATH, but
//! left a TOCTOU window between the final exists() check and
//! `remove_dir_all(admin_dir)`. These tests inject a recreation INSIDE that
//! window (via the after-final-check seam) and assert the registration
//! survives, plus the happy path where nothing reappears and the stale
//! registration is cleared with no sentinel left behind.
//
// trace:BUG-1694 | ai:claude

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
        "id": "b1694fixture",
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

fn canonical_for_git(path: &Path) -> PathBuf {
    let canon = path.canonicalize().expect("fixture path canonicalizes");
    let text = canon.to_string_lossy().to_string();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    canon
}

/// Build a landed-and-deleted worktree whose registration is clearable, and
/// return (tmp, root, worktree path, saved `.git` file text, pinned tip).
fn landed_then_deleted_worktree(
    branch: &str,
    scope: &str,
) -> (tempfile::TempDir, PathBuf, PathBuf, String, Option<String>) {
    let (tmp, root) = create_fixture_repo();
    let wt = tmp.path().join("wt-1694");
    git(
        &root,
        &["worktree", "add", "-q", "-b", branch, wt.to_str().unwrap()],
    );
    let wt = canonical_for_git(&wt);
    commit_file(&wt, "landed.txt", "landed\n", "work");
    let gitfile = std::fs::read_to_string(wt.join(".git")).unwrap();
    git(&root, &["checkout", "-q", "main"]);
    git(&root, &["merge", "-q", "--squash", branch]);
    git(&root, &["commit", "-q", "-m", "land work"]);
    std::fs::remove_dir_all(&wt).unwrap();

    let (_, tip) =
        gather_merge_facts_pinned(&root, Some("main"), branch, scope, false, true, |_| false);
    (tmp, root, wt, gitfile, tip)
}

/// Acceptance: a worktree recreated AFTER the final exists() check — inside
/// the window TASK-1543 could not close — keeps its registration. The atomic
/// claim fails (the path is occupied) and reap declines instead of orphaning
/// the live worktree's admin directory.
#[test]
fn bug_1694_reap_declines_registration_clear_when_path_reappears_after_final_check() {
    let (_tmp, root, wt, gitfile, tip) = landed_then_deleted_worktree("spec-1694", "BUG-1694");
    let admin_dir = root.join(".git").join("worktrees").join("wt-1694");
    assert!(admin_dir.exists(), "fixture registration must exist");

    let lease = fixture_lease(&wt, "spec-1694", "BUG-1694");
    let outcome = reap_one_with_clear_hooks(
        &root,
        &lease,
        tip.as_deref(),
        || {},
        || {
            std::fs::create_dir_all(&wt).unwrap();
            std::fs::write(wt.join(".git"), &gitfile).unwrap();
            std::fs::write(wt.join("precious.txt"), "preserve me").unwrap();
        },
    );
    assert!(
        admin_dir.exists(),
        "registration of the reappeared worktree must survive, got outcome: {outcome}"
    );
    assert!(
        wt.join("precious.txt").exists(),
        "reappeared worktree content must remain after reap_one"
    );
    assert!(
        outcome.contains("registration kept"),
        "expected a reported decline, got: {outcome}"
    );
}

/// Happy path: nothing reappears, the stale registration is cleared, and the
/// claim sentinel directory is not left behind.
#[test]
fn bug_1694_registration_cleared_and_claim_released_when_path_stays_missing() {
    let (_tmp, root, wt, _gitfile, tip) = landed_then_deleted_worktree("spec-1694b", "BUG-1694");
    let admin_dir = root.join(".git").join("worktrees").join("wt-1694");
    assert!(admin_dir.exists(), "fixture registration must exist");

    let lease = fixture_lease(&wt, "spec-1694b", "BUG-1694");
    let outcome = reap_one_with_clear_hooks(&root, &lease, tip.as_deref(), || {}, || {});
    assert!(
        !admin_dir.exists(),
        "stale registration must be cleared, got outcome: {outcome}"
    );
    assert!(
        !wt.exists(),
        "claim sentinel must be released after a successful clear"
    );
    assert!(
        outcome.contains("lease released"),
        "expected a reap, got: {outcome}"
    );
}

/// Fail closed: a relative lease path is declined outright — the claim would
/// otherwise resolve against the process cwd, diverging from the path the
/// `gitdir` identity match uses.
#[test]
fn bug_1694_relative_lease_path_declines_registration_clear() {
    let (_tmp, root, _wt, _gitfile, tip) = landed_then_deleted_worktree("spec-1694c", "BUG-1694");
    let admin_dir = root.join(".git").join("worktrees").join("wt-1694");

    let lease = fixture_lease(Path::new("wt-1694"), "spec-1694c", "BUG-1694");
    let outcome = reap_one_with_clear_hooks(&root, &lease, tip.as_deref(), || {}, || {});
    assert!(
        admin_dir.exists(),
        "a relative lease path must never clear a registration, got outcome: {outcome}"
    );
    assert!(
        outcome.contains("registration kept"),
        "expected a reported decline, got: {outcome}"
    );
}

/// Fail closed: unreadable repository state (no git repo at the root) cannot
/// establish that no registration names the path — the clear declines rather
/// than silently proceeding.
#[test]
fn bug_1694_unreadable_repository_state_declines_registration_clear() {
    let tmp = tempfile::tempdir().unwrap();
    let not_a_repo = tmp.path().join("not-a-repo");
    std::fs::create_dir_all(&not_a_repo).unwrap();
    let missing = tmp.path().join("missing-wt");
    let outcome = clear_missing_worktree_registration(&not_a_repo, &missing, || {}, || {});
    assert_eq!(outcome, RegistrationClearOutcome::Declined);
}

/// A repo with no worktrees dir at all has nothing to clear — that is the
/// common already-pruned case and must NOT decline (a decline would skip the
/// caller's branch cleanup).
#[test]
fn bug_1694_absent_worktrees_dir_reports_no_registration() {
    let (tmp, root) = create_fixture_repo();
    let missing = tmp.path().join("missing-wt");
    let outcome = clear_missing_worktree_registration(&root, &missing, || {}, || {});
    assert_eq!(outcome, RegistrationClearOutcome::NoRegistration);
}
