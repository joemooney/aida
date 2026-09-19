use super::{
    align_reused_pr_branch_with_status, pr_branch_alignment, should_reuse_branch, PrBranchAlignment,
};
use std::process::Command;

fn git(repo: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn explicit_reuse_flag_always_reuses() {
    // --reuse-branch wins even for an auto-derived / absent branch.
    assert!(should_reuse_branch(true, false, false));
    assert!(should_reuse_branch(true, true, true));
}

#[test]
fn explicit_branch_that_exists_auto_reuses() {
    assert!(should_reuse_branch(false, true, true));
}

#[test]
fn explicit_branch_that_is_new_forks() {
    assert!(!should_reuse_branch(false, true, false));
}

#[test]
fn auto_derived_branch_never_reuses() {
    // branch_explicit == false → always fork, even on a (spurious)
    // preexists signal.
    assert!(!should_reuse_branch(false, false, true));
    assert!(!should_reuse_branch(false, false, false));
}

// trace:BUG-1270 | ai:codex
#[test]
fn pr_branch_alignment_covers_behind_diverged_equal_and_no_pr() {
    assert_eq!(
        pr_branch_alignment(true, Some(true), Some(false)),
        PrBranchAlignment::FastForward
    );
    assert_eq!(
        pr_branch_alignment(true, Some(false), Some(false)),
        PrBranchAlignment::RefuseDiverged
    );
    assert_eq!(
        pr_branch_alignment(true, Some(true), Some(true)),
        PrBranchAlignment::Proceed
    );
    assert_eq!(
        pr_branch_alignment(false, Some(false), Some(false)),
        PrBranchAlignment::Proceed
    );
}

// trace:BUG-1270 | ai:codex
#[test]
fn diverged_pr_branch_refuses_before_creating_worktree_or_side_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "aida@example.invalid"]);
    git(&repo, &["config", "user.name", "AIDA Test"]);
    std::fs::write(repo.join("base"), "base").unwrap();
    git(&repo, &["add", "base"]);
    git(&repo, &["commit", "-qm", "base"]);
    git(&repo, &["branch", "pr-branch"]);
    git(&repo, &["checkout", "-qb", "remote-tip"]);
    std::fs::write(repo.join("remote"), "remote").unwrap();
    git(&repo, &["add", "remote"]);
    git(&repo, &["commit", "-qm", "remote"]);
    git(
        &repo,
        &["update-ref", "refs/remotes/origin/pr-branch", "HEAD"],
    );
    git(&repo, &["checkout", "-q", "pr-branch"]);
    std::fs::write(repo.join("local"), "local").unwrap();
    git(&repo, &["add", "local"]);
    git(&repo, &["commit", "-qm", "local"]);

    let worktree = tmp.path().join("repo-pr-branch");
    let err = align_reused_pr_branch_with_status(&repo, "pr-branch", true).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("git fetch origin pr-branch && git branch -f pr-branch origin/pr-branch"));
    assert!(!worktree.exists(), "refusal must precede worktree creation");
    let side = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/pr-branch-work",
        ])
        .status()
        .unwrap();
    assert!(!side.success(), "must not create a -work side branch");
}
