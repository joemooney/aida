//! Tests for BUG-1680:
//! 1. Prevent `aida ps` from recommending `salvage-commit` for dead sessions
//!    whose worktree is the protected main checkout (or any protected branch);
//!    instead suggest inspecting files (`git -C <wt> status`).
//! 2. Distinguish untracked-only files from salvageable changes (saying "untracked
//!    files only" instead of "uncommitted work").
//! 3. Prevent the same main-checkout warning repeating once per stale session by
//!    collapsing duplicate rows for the same worktree path with a count `(N sessions)`.
//! 4. Preserve salvage guidance for ordinary feature worktrees.
//
// trace:BUG-1680 | ai:antigravity

use super::*;
use crate::dispatch_health_ps::{
    is_protected_branch, next_command_hint_with_untracked, probe_untracked_only, DispatchState,
};
use std::path::{Path, PathBuf};
use std::process::Command;

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .expect("git command failed");
    assert!(status.success(), "git {:?} failed", args);
}

fn mock_lease(id: &str, scope: &str, worktree: PathBuf) -> SessionLease {
    SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: "tester".into(),
        worktree_path: worktree,
        branch: scope.to_ascii_lowercase(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: None,
        creator_pid_start_time: None,
        active_pid: None,
        active_pid_start_time: None,
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
    }
}

fn mock_ps_row(id: &str, spec: &str, worktree: PathBuf) -> PsRow {
    PsRow {
        lease: mock_lease(id, spec, worktree),
        state: LeaseState::Stale,
        role: Some("implementer".into()),
        lease_role: Some("implementer".into()),
        pid: None,
        pid_started_at: None,
        elapsed_secs: 100,
        spec: Some(spec.into()),
        dispatch: None,
        locked_by: None,
        mail_identity: None,
        activity: None,
    }
}

/// 1. Protected/default branch gives inspect-only hint and never recommends salvage commit.
/// Ensures feature/foo/main is NOT treated as a protected branch.
#[test]
fn test_protected_and_default_branch_gives_inspect_only_hint() {
    let protected_branches = &[
        "main",
        "master",
        "trunk",
        "develop",
        "aida-store",
        "HEAD",
        "MAIN",
        " master ",
        "refs/heads/main",
        "refs/heads/master",
        "refs/remotes/origin/main",
        "origin/main",
        "upstream/trunk",
    ];

    for branch in protected_branches {
        assert!(
            is_protected_branch(branch),
            "expected {branch} to be protected"
        );

        let wt = Path::new("/home/joe/ai/aida");
        let hint = next_command_hint_with_untracked(
            DispatchState::Salvageable,
            wt,
            branch,
            Some("fix: something"),
            Some("TASK-100"),
            false,
            false,
        )
        .expect("salvageable state must produce a hint");

        assert!(
            hint.contains("inspect the files: git -C /home/joe/ai/aida status"),
            "expected inspect-only hint on {branch}, got: {hint}"
        );
        assert!(
            !hint.contains("salvage-commit"),
            "hint must not recommend salvage-commit on protected branch {branch}: {hint}"
        );
        assert!(
            !hint.contains("add -A"),
            "hint must not recommend add -A on protected branch {branch}: {hint}"
        );
    }

    // Ensure feature/foo/main is NOT treated as a protected branch
    assert!(
        !is_protected_branch("feature/foo/main"),
        "feature/foo/main must NOT be treated as protected"
    );
    assert!(
        !is_protected_branch("feature/main"),
        "feature/main must NOT be treated as protected"
    );
}

/// 2. Ordinary feature branch retains salvage hint with git add -A and git commit.
#[test]
fn test_feature_branch_retains_salvage_hint() {
    let feature_branches = &["task-123", "bug-1680", "feature/foo/main", "feature/main"];

    for branch in feature_branches {
        assert!(
            !is_protected_branch(branch),
            "expected {branch} NOT to be protected"
        );

        let wt = Path::new("/home/joe/ai/aida-feature");
        let hint = next_command_hint_with_untracked(
            DispatchState::Salvageable,
            wt,
            branch,
            Some("fix: something"),
            Some("TASK-100"),
            false,
            false,
        )
        .expect("salvageable state must produce a hint");

        let expected_cmd = format!(
            "salvage-commit then rebrief: git -C /home/joe/ai/aida-feature add -A && git -C /home/joe/ai/aida-feature commit -m \"wip: salvage {branch}\""
        );
        assert!(
            hint.contains(&expected_cmd),
            "expected salvage hint on {branch}, got: {hint}"
        );
    }
}

/// 3. Untracked-only vs tracked-change detection:
/// Distinguishes untracked files outside tracked directories from tracked changes,
/// and includes a case for untracked files in nested tracked directories.
#[test]
fn test_untracked_only_vs_tracked_change_detection() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();

    run_git(repo, &["init"]);
    run_git(repo, &["config", "user.email", "tester@example.com"]);
    run_git(repo, &["config", "user.name", "Tester"]);

    // Create a tracked nested directory structure
    std::fs::create_dir_all(repo.join("src/nested")).unwrap();
    std::fs::write(repo.join("src/nested/tracked.rs"), "fn foo() {}\n").unwrap();
    run_git(repo, &["add", "src/nested/tracked.rs"]);
    run_git(repo, &["commit", "-m", "initial commit"]);

    // Case A: Untracked files in untracked tool directories (e.g. .agents/, .codegraph/, target-review/)
    std::fs::create_dir_all(repo.join(".codegraph")).unwrap();
    std::fs::write(repo.join(".codegraph/index.db"), "data").unwrap();
    std::fs::create_dir_all(repo.join("target-review")).unwrap();
    std::fs::write(repo.join("target-review/test.log"), "log").unwrap();

    // With only untracked files in untracked dirs, probe_untracked_only returns true.
    assert!(
        probe_untracked_only(repo),
        "expected untracked_only to be true for tool directories outside tracked tree"
    );

    // Verify hint wording uses "untracked files only" instead of "uncommitted work"
    let hint_untracked = next_command_hint_with_untracked(
        DispatchState::Salvageable,
        repo,
        "main",
        Some("commit"),
        Some("BUG-1680"),
        false,
        true,
    )
    .unwrap();
    assert!(
        hint_untracked.contains("untracked files only"),
        "expected 'untracked files only' in hint, got: {hint_untracked}"
    );
    assert!(
        !hint_untracked.contains("uncommitted work"),
        "did not expect 'uncommitted work' when untracked_only is true, got: {hint_untracked}"
    );

    // Case B: Untracked file inside a nested tracked directory (src/nested/untracked.rs)
    std::fs::write(repo.join("src/nested/untracked.rs"), "fn bar() {}\n").unwrap();

    // Because it is in a tracked directory, it represents session work, so probe_untracked_only is false.
    assert!(
        !probe_untracked_only(repo),
        "expected untracked_only to be false when untracked file is inside a tracked directory"
    );

    // Case C: Tracked file modified
    std::fs::remove_file(repo.join("src/nested/untracked.rs")).unwrap();
    std::fs::write(
        repo.join("src/nested/tracked.rs"),
        "fn foo() { /* mod */ }\n",
    )
    .unwrap();

    assert!(
        !probe_untracked_only(repo),
        "expected untracked_only to be false when tracked file is modified"
    );
}

/// 4. Duplicate worktree rows collapse with count:
/// Multiple stale session rows pointing to the same worktree (e.g. main checkout)
/// collapse into one row displaying `(N sessions)`.
#[test]
fn test_duplicate_worktree_rows_collapse_with_count() {
    let main_wt = PathBuf::from("/home/joe/ai/aida");
    let feature_wt = PathBuf::from("/home/joe/ai/aida-task-123");

    let row1 = mock_ps_row("sess-1", "TASK-101", main_wt.clone());
    let row2 = mock_ps_row("sess-2", "TASK-102", main_wt.clone());
    let row3 = mock_ps_row("sess-3", "TASK-103", main_wt.clone());
    let row4 = mock_ps_row("sess-4", "TASK-104", feature_wt.clone());

    let input_rows = vec![&row1, &row2, &row3, &row4];
    let collapsed = collapse_salvageable_by_worktree(&input_rows);

    assert_eq!(
        collapsed.len(),
        2,
        "expected 2 collapsed rows (1 for main, 1 for feature)"
    );

    // First group is main_wt with count 3
    assert_eq!(collapsed[0].row.lease.worktree_path, main_wt);
    assert_eq!(collapsed[0].count, 3);

    // Second group is feature_wt with count 1
    assert_eq!(collapsed[1].row.lease.worktree_path, feature_wt);
    assert_eq!(collapsed[1].count, 1);
}
