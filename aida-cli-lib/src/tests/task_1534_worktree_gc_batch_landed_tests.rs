//! TASK-1534 — aida worktree gc: use the landing-commit trailer check so
//! batch-landed branches are no longer 'no merge signal'.
//!
//! Acceptance criteria:
//! 1. worktree gc computes the field with session_reap's spec_landing_commit
//!    and applies the same content proof (patch-id or merge-tree no-op).
//! 2. Fixture test: a batch-landed branch is reported reclaimable; one with an
//!    unshipped commit is not.
//
// trace:TASK-1534 | ai:antigravity

use super::*;
use std::path::Path;

#[test]
// trace:BUG-1718 | ai:codex
// trace:BUG-1719 | ai:codex
fn bug_1718_unmerged_without_landing_signal_still_needs_operator_action() {
    let batched = AgentWorktreeFacts {
        dirty: false,
        ancestor_of_main: false,
        pr_merged: false,
        unique_unmerged_commits: 1,
        content_fully_landed: false,
        spec_trailer_on_main: true,
    };
    let AgentWorktreeVerdict::Keep {
        reason: no_action,
        actionable,
    } = classify_agent_worktree(&batched)
    else {
        panic!("batched undecidable work must be kept");
    };
    assert!(!actionable, "batched landed work is non-actionable");
    assert!(no_action.contains("no action required"), "{no_action}");

    let unmerged = AgentWorktreeFacts {
        spec_trailer_on_main: false,
        ..batched
    };
    let AgentWorktreeVerdict::Keep {
        reason: actionable,
        actionable: needs_action,
    } = classify_agent_worktree(&unmerged)
    else {
        panic!("genuinely unmerged work must be kept");
    };
    assert!(needs_action, "genuinely unmerged work remains actionable");
    assert!(actionable.contains("operator decision"), "{actionable}");
    assert!(!actionable.contains("no action required"), "{actionable}");
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
    commit_file(
        &wt_path,
        "unshipped.txt",
        "never shipped\n",
        "fix(core): follow-up work that never shipped (TASK-9002)",
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
        finding.summary.contains("no action required"),
        "unexpected summary: {}",
        finding.summary
    );
    assert_eq!(
        finding.action, "no action required — kept because batched content is undecidable",
        "batch-landed but content-undecidable work should not ask for operator adjudication"
    );
    assert!(
        !finding.action.contains("--yes --force"),
        "unshipped branch must not offer destructive auto-heal action"
    );
}
