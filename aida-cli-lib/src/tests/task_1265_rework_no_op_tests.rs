use super::*;
use std::path::Path;
use std::process::Command;

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "AIDA Test")
        .env("GIT_AUTHOR_EMAIL", "aida@example.test")
        .env("GIT_COMMITTER_NAME", "AIDA Test")
        .env("GIT_COMMITTER_EMAIL", "aida@example.test")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit_file(repo: &Path, name: &str, content: &str, message: &str) {
    std::fs::write(repo.join(name), content).unwrap();
    git(repo, &["add", name]);
    git(repo, &["commit", "-m", message]);
}

fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-b", "main"]);
    commit_file(tmp.path(), "base.txt", "base\n", "base");
    tmp
}

// trace:BUG-1445 | ai:codex
#[test]
fn pr_keyed_reviewer_verdict_arms_rework_guard() {
    let tmp = tempfile::tempdir().unwrap();
    review_verdict::record_verdict(
        tmp.path(),
        "STORY-1350",
        Some("Approved"),
        Some("older"),
        Some("story-1350-work"),
        Some("stale spec-keyed verdict"),
        &[],
        "codex",
    )
    .unwrap();
    review_verdict::record_verdict(
        tmp.path(),
        "PR-1974",
        Some("RequestChanges"),
        Some("deadbeef"),
        Some("story-1350-work"),
        Some("two findings remain"),
        &["share the marker constant".to_string()],
        "codex",
    )
    .unwrap();

    let verdict = blocking_rework_verdict(tmp.path(), "STORY-1350", 1974)
        .expect("the canonical PR-N reviewer artifact must arm the guard");
    assert_eq!(verdict.kind, review_verdict::VerdictKind::RequestChanges);
}

// trace:TASK-1265 trace:BUG-1445 | ai:codex
#[test]
fn rebased_branch_without_new_work_is_a_rework_no_op() {
    let tmp = fixture();
    let repo = tmp.path();
    git(repo, &["checkout", "-b", "topic"]);
    commit_file(repo, "topic.txt", "reviewed patch\n", "reviewed patch");
    let before = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["checkout", "main"]);
    commit_file(repo, "main.txt", "new base work\n", "advance main");
    git(repo, &["checkout", "topic"]);
    git(repo, &["rebase", "main"]);
    let after = git(repo, &["rev-parse", "HEAD"]);

    assert_ne!(before, after, "the fixture must rewrite the topic SHA");
    assert_ne!(
        git(repo, &["diff", "--stat", &before, &after]),
        "",
        "the moving base must make a tree comparison report changes"
    );
    assert_eq!(
        classify_rework_head_change(repo, &before, &after),
        Some(ReworkHeadChange::RebaseOnly)
    );
    let message = rework_no_op_message(
        1974,
        &before,
        &after,
        "fix both open findings",
        2,
        ReworkHeadChange::RebaseOnly,
    );
    assert!(
        message.contains("patch-ids are unchanged (rebase-only)"),
        "{message}"
    );
}

// trace:TASK-1265 | ai:codex
#[test]
fn genuine_new_commit_is_not_a_rework_no_op() {
    let tmp = fixture();
    let repo = tmp.path();
    git(repo, &["checkout", "-b", "topic"]);
    commit_file(repo, "topic.txt", "reviewed patch\n", "reviewed patch");
    let before = git(repo, &["rev-parse", "HEAD"]);
    commit_file(repo, "fix.txt", "addresses review\n", "address review");
    let after = git(repo, &["rev-parse", "HEAD"]);

    assert_eq!(
        classify_rework_head_change(repo, &before, &after),
        Some(ReworkHeadChange::ContentChanged)
    );
}

// trace:TASK-1265 | ai:codex
#[test]
fn no_op_failure_names_the_actual_rework_round() {
    let message = rework_no_op_message(
        42,
        "before",
        "after",
        "fix the guard",
        3,
        ReworkHeadChange::Unchanged,
    );
    assert!(message.starts_with("ROUND 3 rework"), "{message}");
    assert!(message.contains("fix the guard"), "{message}");
}
