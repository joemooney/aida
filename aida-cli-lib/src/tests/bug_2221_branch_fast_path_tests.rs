//! BUG-2221 P2: the name-matched in-flight branch fast path gives exactly
//! the unchanged `--contains` scan's answer on the fixtures the scan's own
//! bugs were filed for (BUG-553 stacked sibling, BUG-1528 branch-crossing,
//! BUG-1591 remote-only newer branch, BUG-720 orphan store), and a miss falls
//! through to the scan. Real temporary git repos; structural equality, no
//! wall-clock assertions.
// trace:BUG-2221 | ai:claude
use super::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn git_env(repo: &Path, args: &[&str], date: Option<i64>) -> String {
    let mut cmd = crate::test_git::git_command();
    cmd.arg("-C").arg(repo).args(args);
    if let Some(t) = date {
        let d = format!("@{t} +0000");
        cmd.env("GIT_COMMITTER_DATE", &d).env("GIT_AUTHOR_DATE", &d);
    }
    let out = cmd.output().expect("git on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git(repo: &Path, args: &[&str]) -> String {
    git_env(repo, args, None)
}

fn init_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    git(&root, &["init", "--initial-branch=main", "--quiet"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "Test"]);
    git_env(
        &root,
        &["commit", "-q", "--allow-empty", "-m", "chore: init"],
        Some(1_700_000_000),
    );
    (tmp, root)
}

/// An empty commit with a pinned committer date; returns its OID.
fn commit_at(root: &Path, subject: &str, t: i64) -> String {
    git_env(
        root,
        &["commit", "-q", "--allow-empty", "-m", subject],
        Some(t),
    );
    git(root, &["rev-parse", "HEAD"])
}

/// The referencing commits as the collector orders them (`--all
/// --date-order`, newest first).
fn referencing_commits(root: &Path, id: &str) -> Vec<(String, String, String)> {
    let out = git(
        root,
        &[
            "log",
            "--all",
            "-F",
            "--date-order",
            "--format=%H%x1f%h%x1f%s",
            "--grep",
            id,
        ],
    );
    out.lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let mut p = l.splitn(3, '\u{1f}');
            (
                p.next().unwrap().to_string(),
                p.next().unwrap().to_string(),
                p.next().unwrap().to_string(),
            )
        })
        .collect()
}

fn far() -> Instant {
    Instant::now() + Duration::from_secs(600)
}

/// The fast path hits and equals the unchanged scan; returns the answer.
fn assert_equivalent(root: &Path, id: &str) -> crate::linkage_branch::BranchScan {
    let ids = vec![id.to_string()];
    let commits = referencing_commits(root, id);
    assert!(!commits.is_empty(), "fixture has referencing commits");
    let scan = crate::linkage_branch::contains_branch_scan(root, &ids, &commits, far(), true);
    let fast = crate::linkage_branch::name_matched_branch_scan(root, &ids, &commits, far())
        .expect("an id-named ref contains a referencing commit: the fast path hits");
    assert_eq!(fast, scan, "fast path must equal the --contains scan");
    fast
}

#[test]
fn branch_name_match_equals_contains_scan_branch_crossing() {
    // BUG-1528/1591: local `bug-1420-work` (older) plus a NEWER remote-only
    // `origin/bug-1420-round2`.
    let (_tmp, root) = init_repo();
    git(&root, &["checkout", "-q", "-b", "bug-1420-work"]);
    commit_at(&root, "fix: first pass (BUG-1420)", 1_700_000_100);
    git(&root, &["checkout", "-q", "main"]);
    git(&root, &["checkout", "-q", "-b", "bug-1420-round2"]);
    let sha = commit_at(&root, "fix: round 2 (BUG-1420)", 1_700_000_200);
    git(&root, &["checkout", "-q", "main"]);
    git(&root, &["branch", "-D", "bug-1420-round2"]);
    git(
        &root,
        &["update-ref", "refs/remotes/origin/bug-1420-round2", &sha],
    );
    let got = assert_equivalent(&root, "BUG-1420");
    assert_eq!(got.branch.as_deref(), Some("bug-1420-work"));
    assert_eq!(got.other_branches, vec!["bug-1420-round2".to_string()]);
    assert_eq!(got.truncated, None);
}

#[test]
fn branch_name_match_equals_contains_scan_stacked_sibling() {
    // BUG-553: a sibling spec's branch stacked on this spec's unmerged
    // commit, named so it lists FIRST.
    let (_tmp, root) = init_repo();
    git(&root, &["checkout", "-q", "-b", "task-806"]);
    commit_at(&root, "feat: own work (TASK-806)", 1_700_000_100);
    git(&root, &["checkout", "-q", "-b", "aaa-task-900"]);
    commit_at(&root, "feat: stacked sibling (TASK-900)", 1_700_000_200);
    git(&root, &["checkout", "-q", "main"]);
    let got = assert_equivalent(&root, "TASK-806");
    assert_eq!(got.branch.as_deref(), Some("task-806"));
    assert!(got.other_branches.is_empty());
}

#[test]
fn branch_name_match_equals_contains_scan_local_and_remote_same_tip() {
    // BUG-1591: the same branch locally and on origin, plus a newer
    // remote-only id-named branch; the local one ranks first.
    let (_tmp, root) = init_repo();
    git(&root, &["checkout", "-q", "-b", "bug-77-a"]);
    let a = commit_at(&root, "fix: a (BUG-77)", 1_700_000_100);
    git(&root, &["update-ref", "refs/remotes/origin/bug-77-a", &a]);
    git(&root, &["checkout", "-q", "main"]);
    git(&root, &["checkout", "-q", "-b", "bug-77-b"]);
    let b = commit_at(&root, "fix: b (BUG-77)", 1_700_000_300);
    git(&root, &["checkout", "-q", "main"]);
    git(&root, &["branch", "-D", "bug-77-b"]);
    git(&root, &["update-ref", "refs/remotes/origin/bug-77-b", &b]);
    let got = assert_equivalent(&root, "BUG-77");
    assert_eq!(got.branch.as_deref(), Some("bug-77-a"));
    assert_eq!(got.other_branches, vec!["bug-77-b".to_string()]);
}

#[test]
fn branch_name_match_equals_contains_scan_orphan_store() {
    // BUG-720: the orphan store also names the spec; it is never offered.
    let (_tmp, root) = init_repo();
    git(&root, &["checkout", "-q", "--orphan", "aida-store"]);
    commit_at(&root, "update store (TASK-808)", 1_700_000_300);
    let store = git(&root, &["rev-parse", "HEAD"]);
    git(
        &root,
        &["update-ref", "refs/remotes/gitlab/aida-store", &store],
    );
    git(&root, &["checkout", "-q", "main"]);
    git(&root, &["checkout", "-q", "-b", "task-808-work"]);
    commit_at(&root, "feat: work (TASK-808)", 1_700_000_100);
    git(&root, &["checkout", "-q", "main"]);
    let got = assert_equivalent(&root, "TASK-808");
    assert_eq!(got.branch.as_deref(), Some("task-808-work"));
    assert!(got.other_branches.is_empty());
}

#[test]
fn branch_name_match_equals_contains_scan_commit_cap() {
    // BUG-1594: more referencing commits than the scan examines; both
    // report the same truncation, and the newest-first cap is honoured.
    let (_tmp, root) = init_repo();
    git(&root, &["checkout", "-q", "-b", "bug-88-old"]);
    commit_at(&root, "fix: oldest (BUG-88)", 1_700_000_050);
    git(&root, &["checkout", "-q", "main"]);
    git(&root, &["checkout", "-q", "-b", "bug-88-work"]);
    for i in 0..(LINKAGE_BRANCH_SCAN_MAX_COMMITS as i64 + 2) {
        commit_at(&root, &format!("fix: step {i} (BUG-88)"), 1_700_000_100 + i);
    }
    git(&root, &["checkout", "-q", "main"]);
    let got = assert_equivalent(&root, "BUG-88");
    assert_eq!(got.branch.as_deref(), Some("bug-88-work"));
    assert!(
        got.other_branches.is_empty(),
        "bug-88-old holds only a commit past the cap"
    );
    let total = LINKAGE_BRANCH_SCAN_MAX_COMMITS + 3;
    assert_eq!(
        got.truncated,
        Some((LINKAGE_BRANCH_SCAN_MAX_COMMITS, total))
    );
}

#[test]
fn branch_no_name_match_falls_back_unchanged() {
    // No id-named ref: the fast path proves nothing; the combined search is
    // the unchanged scan (first candidate fallback).
    let (_tmp, root) = init_repo();
    git(&root, &["checkout", "-q", "-b", "feature/y"]);
    commit_at(&root, "feat: flying (TASK-806)", 1_700_000_100);
    git(&root, &["checkout", "-q", "main"]);
    let ids = vec!["TASK-806".to_string()];
    let commits = referencing_commits(&root, "TASK-806");
    assert_eq!(
        crate::linkage_branch::name_matched_branch_scan(&root, &ids, &commits, far()),
        None
    );
    let combined = crate::linkage_branch::in_flight_branch_scan(&root, &ids, &commits, far(), true);
    let scan = crate::linkage_branch::contains_branch_scan(&root, &ids, &commits, far(), true);
    assert_eq!(combined, scan);
    assert_eq!(combined.branch.as_deref(), Some("feature/y"));
}

#[test]
fn branch_name_match_miss_falls_back_unchanged() {
    // An id-named ref that holds none of the referencing commits is a miss,
    // not a "no branch" answer.
    let (_tmp, root) = init_repo();
    git(&root, &["branch", "task-806-stale"]);
    git(&root, &["checkout", "-q", "-b", "feature/z"]);
    commit_at(&root, "feat: work (TASK-806)", 1_700_000_100);
    git(&root, &["checkout", "-q", "main"]);
    let ids = vec!["TASK-806".to_string()];
    let commits = referencing_commits(&root, "TASK-806");
    assert_eq!(
        crate::linkage_branch::name_matched_branch_scan(&root, &ids, &commits, far()),
        None
    );
    let combined = crate::linkage_branch::in_flight_branch_scan(&root, &ids, &commits, far(), true);
    assert_eq!(combined.branch.as_deref(), Some("feature/z"));
}

#[test]
fn branch_name_match_expired_deadline_is_a_miss() {
    let (_tmp, root) = init_repo();
    git(&root, &["checkout", "-q", "-b", "bug-9-work"]);
    commit_at(&root, "fix: x (BUG-9)", 1_700_000_100);
    git(&root, &["checkout", "-q", "main"]);
    let ids = vec!["BUG-9".to_string()];
    let commits = referencing_commits(&root, "BUG-9");
    assert_eq!(
        crate::linkage_branch::name_matched_branch_scan(&root, &ids, &commits, Instant::now()),
        None
    );
}
