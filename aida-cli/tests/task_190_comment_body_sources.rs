#![cfg(target_os = "linux")]
// Black-box coverage for mutually exclusive, lossless comment body sources.
// trace:TASK-190 | ai:codex

use std::io::Write;
use std::process::{Command, Stdio};

fn run(cwd: &std::path::Path, home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(cwd)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn comment_body_sources_conflict_and_empty_explicit_sources_fail() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let home = temp.path().join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "test@example.com"],
        &["config", "user.name", "TASK-190 Test"],
    ] {
        assert!(Command::new("git")
            .current_dir(&repo)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    assert!(Command::new("git")
        .current_dir(&repo)
        .args(["commit", "-q", "--allow-empty", "-m", "init"])
        .status()
        .unwrap()
        .success());
    assert!(run(
        &repo,
        &home,
        &[
            "init",
            "--force",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles"
        ]
    )
    .status
    .success());
    let added = run(&repo, &home, &["add", "fixture", "--type", "task"]);
    let stdout = String::from_utf8_lossy(&added.stdout);
    let id = stdout
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|token| token.starts_with("TASK-") && token[5..].chars().all(|c| c.is_ascii_digit()))
        .unwrap();
    let body = repo.join("body.md");
    std::fs::write(&body, "literal `date` and $(pwd)\n").unwrap();
    let body_path = body.to_str().unwrap();

    for args in [
        vec!["comment", "add", id, "positional", "--content", "legacy"],
        vec!["comment", "add", id, "positional", "--body-file", body_path],
        vec!["comment", "add", id, "positional", "--stdin"],
        vec![
            "comment",
            "add",
            id,
            "--content",
            "legacy",
            "--body-file",
            body_path,
        ],
        vec!["comment", "add", id, "--content", "legacy", "--stdin"],
    ] {
        assert!(
            !run(&repo, &home, &args).status.success(),
            "mixed sources must fail"
        );
    }

    for args in [
        vec!["comment", "add", id, "positional", "--interactive"],
        vec!["comment", "add", id, "--content", "legacy", "--interactive"],
        vec![
            "comment",
            "add",
            id,
            "--body-file",
            body_path,
            "--interactive",
        ],
        vec!["comment", "add", id, "--stdin", "--interactive"],
        vec![
            "comment",
            "edit",
            "--req-id",
            id,
            "--comment-id",
            "abc",
            "--content",
            "replacement",
            "--interactive",
        ],
        vec![
            "comment",
            "edit",
            "--req-id",
            id,
            "--comment-id",
            "abc",
            "--body-file",
            body_path,
            "--interactive",
        ],
        vec![
            "comment",
            "edit",
            "--req-id",
            id,
            "--comment-id",
            "abc",
            "--stdin",
            "--interactive",
        ],
    ] {
        assert!(
            !run(&repo, &home, &args).status.success(),
            "interactive and noninteractive sources must conflict"
        );
    }

    std::fs::write(&body, " \n").unwrap();
    assert!(!run(
        &repo,
        &home,
        &["comment", "add", id, "--body-file", body_path]
    )
    .status
    .success());

    let mut child = Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(&repo)
        .env("HOME", &home)
        .env("AIDA_TELEMETRY", "0")
        .args(["comment", "add", id, "--stdin"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"").unwrap();
    assert!(!child.wait().unwrap().success());
}
