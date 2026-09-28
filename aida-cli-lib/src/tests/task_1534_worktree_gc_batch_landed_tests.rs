//! TASK-1534 — aida worktree gc: use the landing-commit trailer check so
//! batch-landed branches are no longer 'no merge signal'.
//!
//! Acceptance criteria:
//! 1. worktree gc computes the field with session_reap's spec_trailer_landed_on
//!    and applies the same content proof (patch-id or merge-tree no-op).
//! 2. Fixture test: a batch-landed branch is reported reclaimable; one with an
//!    unshipped commit is not.
//
// trace:TASK-1534 | ai:antigravity

use super::*;
use std::path::Path;

#[test]
// trace:BUG-1718 | ai:codex
fn bug_1718_classify_agent_worktree_requires_landing_recency_guard() {
    let facts = AgentWorktreeFacts {
        dirty: false,
        ancestor_of_main: false,
        pr_merged: false,
        unique_unmerged_commits: 1,
        content_fully_landed: false,
        spec_trailer_on_main: true,
        no_commits_after_landing: true,
    };
    assert!(matches!(
        classify_agent_worktree(&facts),
        AgentWorktreeVerdict::Removable(_)
    ));
    let later_work = AgentWorktreeFacts {
        no_commits_after_landing: false,
        ..facts
    };
    assert!(matches!(
        classify_agent_worktree(&later_work),
        AgentWorktreeVerdict::Keep(_)
    ));
}

#[test]
// trace:BUG-1718 | ai:codex
fn bug_1718_missing_agent_worktree_registration_is_left_untouched_silently() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    commit_file(&root, "README.md", "base\n", "init");
    git(&root, &["branch", "-M", "main"]);
    let wt = root.join(".claude/worktrees/gone");
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "worktree-agent-gone-9010",
            wt.to_str().unwrap(),
            "main",
        ],
    );
    std::fs::remove_dir_all(&wt).unwrap();
    let findings = scan_merged_agent_worktrees(&root);
    assert!(
        findings.is_empty(),
        "missing worktree emitted findings: {findings:?}"
    );
    assert!(
        git(&root, &["worktree", "list", "--porcelain"]).contains("prunable"),
        "read-only scan must leave stale git registration untouched"
    );
}

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
    let path = root.join(file);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, content).unwrap();
    git(root, &["add", file]);
    git(root, &["commit", "-q", "-m", message]);
}

// trace:BUG-1718 | ai:codex
fn commit_file_at(root: &Path, file: &str, content: &str, message: &str, date: &str) {
    let path = root.join(file);
    std::fs::write(&path, content).unwrap();
    git(root, &["add", file]);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["commit", "-q", "-m", message])
        .env("GIT_COMMITTER_DATE", date)
        .env("GIT_AUTHOR_DATE", date)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "dated commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
// trace:BUG-1718 | ai:codex
fn bug_1718_oldest_landing_match_keeps_branch_commit_between_mentions() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    commit_file(&root, "README.md", "base\n", "init");
    git(&root, &["branch", "-M", "main"]);
    let wt = root.join(".claude/worktrees/agent-oldest");
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "worktree-agent-oldest-9010",
            wt.to_str().unwrap(),
            "main",
        ],
    );
    commit_file_at(
        &wt,
        "branch.txt",
        "branch\n",
        "feat: ship (TASK-9010)",
        "2020-03-01T00:00:00Z",
    );

    git(&root, &["checkout", "-q", "main"]);
    commit_file_at(
        &root,
        "landed.txt",
        "landed\n",
        "feat: batch land (TASK-9010)",
        "2020-02-01T00:00:00Z",
    );
    commit_file_at(
        &root,
        "later.txt",
        "later\n",
        "chore: repeat trailer (TASK-9010)",
        "2020-04-01T00:00:00Z",
    );

    let findings = scan_merged_agent_worktrees(&root);
    assert_eq!(
        findings.len(),
        1,
        "expected one worktree finding: {findings:?}"
    );
    assert!(
        findings[0].summary.contains("flagged:"),
        "branch commit between landing and later mention must be kept: {}",
        findings[0].summary
    );
}

#[test]
// trace:BUG-1718 | ai:codex
fn bug_1718_plan_commit_is_not_a_landing_signal_and_worktree_is_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    commit_file(&root, "README.md", "base\n", "init");
    git(&root, &["branch", "-M", "main"]);
    let wt = root.join(".claude/worktrees/agent-plan");
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "worktree-agent-plan-9011",
            wt.to_str().unwrap(),
            "main",
        ],
    );
    commit_file(&wt, "branch.txt", "branch\n", "feat: implement (TASK-9011)");
    git(&root, &["checkout", "-q", "main"]);
    commit_file(
        &root,
        "plan.md",
        "plan\n",
        "docs(plans): TASK-9011 plan (TASK-9011)",
    );

    assert_eq!(
        crate::session_reap::spec_landing_commit(
            &root,
            "main",
            "worktree-agent-plan-9011",
            "TASK-9011"
        ),
        None
    );
    let findings = scan_merged_agent_worktrees(&root);
    assert_eq!(
        findings.len(),
        1,
        "expected one worktree finding: {findings:?}"
    );
    assert!(
        findings[0].summary.contains("flagged:"),
        "plan-only reference must keep worktree: {}",
        findings[0].summary
    );
}

#[test]
// trace:BUG-1718 | ai:codex
fn bug_1718_batch_landed_branch_is_reported_reclaimable_until_later_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    commit_file(&root, "README.md", "base\n", "init");
    git(&root, &["branch", "-M", "main"]);

    // Create an agent-managed worktree for batch work.
    let wt_path = root.join(".claude/worktrees/agent-batch");
    std::fs::create_dir_all(wt_path.parent().unwrap()).unwrap();
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "worktree-agent-batch-9001",
            wt_path.to_str().unwrap(),
            "main",
        ],
    );

    // Make commits on the branch that implement TASK-9001.
    commit_file(
        &wt_path,
        "feature.txt",
        "feature code\n",
        "feat(core): implement feature (TASK-9001)",
    );

    // Simulate batch integration commit on main combining TASK-9001 and TASK-9002.
    git(&root, &["checkout", "-q", "main"]);
    std::fs::write(root.join("feature.txt"), "feature code\n").unwrap();
    std::fs::write(root.join("other.txt"), "other code\n").unwrap();
    git(&root, &["add", "feature.txt", "other.txt"]);
    git(
        &root,
        &[
            "commit",
            "-q",
            "-m",
            "[AI:claude] chore(integrate): batch 12 - TASK-9001 TASK-9002 (#50)",
            "-m",
            "Ship feature (TASK-9001)\nShip other (TASK-9002)",
        ],
    );
    // Unrelated commit on main afterwards.
    commit_file(&root, "after.txt", "after\n", "chore: later update");
    // Make the branch's file differ after landing, forcing BUG-1718's date guard.
    // trace:BUG-1718 | ai:codex
    commit_file(
        &root,
        "feature.txt",
        "later main edit\n",
        "chore: edit landed file later",
    );

    // Scan merged agent worktrees via doctor_cmd.
    let findings = scan_merged_agent_worktrees(&root);
    assert_eq!(
        findings.len(),
        1,
        "expected exactly one finding for the agent worktree, got: {findings:?}"
    );
    let finding = &findings[0];
    assert_eq!(finding.category, "merged-agent-worktrees");
    assert!(
        finding.summary.contains("is mergeable-and-gone"),
        "expected reclaimable/removable summary, got: {}",
        finding.summary
    );
    assert!(
        finding
            .summary
            .contains("its spec landed on origin/main through a batched integration merge"),
        "expected integration merge reason in summary, got: {}",
        finding.summary
    );
    assert!(
        finding.summary.contains("postdates that landing commit"),
        "expected date-guard removal: {}",
        finding.summary
    );
    assert!(
        finding
            .action
            .contains("--category merged-agent-worktrees --yes --force"),
        "expected reclaimable heal action, got: {}",
        finding.action
    );

    // A real branch commit dated after landing must keep the worktree.
    // trace:BUG-1718 | ai:codex
    let future_commit = std::process::Command::new("git")
        .arg("-C")
        .arg(&wt_path)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-q",
            "-m",
            "post landing work",
        ])
        .env("GIT_COMMITTER_DATE", "2035-01-01T00:00:00Z")
        .env("GIT_AUTHOR_DATE", "2035-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(
        future_commit.status.success(),
        "future commit failed: {}",
        String::from_utf8_lossy(&future_commit.stderr)
    );
    let findings = scan_merged_agent_worktrees(&root);
    assert!(
        findings[0].summary.contains("flagged:"),
        "post-landing commit must stay kept: {}",
        findings[0].summary
    );
}

#[test]
fn task_1534_branch_with_unshipped_commit_is_not_reclaimable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    commit_file(&root, "README.md", "base\n", "init");
    git(&root, &["branch", "-M", "main"]);

    // Create an agent-managed worktree for batch work.
    let wt_path = root.join(".claude/worktrees/agent-unshipped");
    std::fs::create_dir_all(wt_path.parent().unwrap()).unwrap();
    git(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            "worktree-agent-unshipped-9002",
            wt_path.to_str().unwrap(),
            "main",
        ],
    );

    // Initial commit for TASK-9002.
    commit_file(
        &wt_path,
        "shipped.txt",
        "part that lands\n",
        "feat(core): initial part (TASK-9002)",
    );

    // Land that part in a batched integration commit on main.
    git(&root, &["checkout", "-q", "main"]);
    std::fs::write(root.join("shipped.txt"), "part that lands\n").unwrap();
    git(&root, &["add", "shipped.txt"]);
    git(
        &root,
        &[
            "commit",
            "-q",
            "-m",
            "[AI:claude] chore(integrate): batch 13 - TASK-9002 (#51)",
            "-m",
            "Ship initial part (TASK-9002)",
        ],
    );

    // Add an unshipped follow-up commit to the agent worktree.
    commit_file_at(
        &wt_path,
        "unshipped.txt",
        "never shipped\n",
        "fix(core): follow-up work that never shipped (TASK-9002)",
        "2035-01-01T00:00:00Z",
    );

    // Scan merged agent worktrees via doctor_cmd.
    let findings = scan_merged_agent_worktrees(&root);
    assert_eq!(
        findings.len(),
        1,
        "expected exactly one finding for the agent worktree, got: {findings:?}"
    );
    let finding = &findings[0];
    assert_eq!(finding.category, "merged-agent-worktrees");
    assert!(
        finding.summary.contains("flagged:"),
        "expected flagged (kept) summary, got: {}",
        finding.summary
    );
    assert!(
        finding
            .summary
            .contains("unique unmerged commit(s) — keep, operator decision"),
        "expected unmerged commit notice in summary, got: {}",
        finding.summary
    );
    assert_eq!(
        finding.action, "operator decision: review and keep, or remove by hand",
        "unshipped commit must require operator decision, not auto-reclaim"
    );
    assert!(
        !finding.action.contains("--yes --force"),
        "unshipped branch must not offer destructive auto-heal action"
    );
}
