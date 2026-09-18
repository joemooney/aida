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

// trace:TASK-1265 | ai:codex
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
    assert_eq!(
        rework_heads_content_changed(repo, &before, &after),
        Some(false)
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
        rework_heads_content_changed(repo, &before, &after),
        Some(true)
    );
}

// trace:TASK-1265 | ai:codex
#[test]
fn no_op_failure_names_the_actual_rework_round() {
    let message = rework_no_op_message(42, "before", "after", "fix the guard", 3);
    assert!(
        message.starts_with("ROUND 3 rework implementer"),
        "{message}"
    );
    assert!(message.contains("fix the guard"), "{message}");
}
