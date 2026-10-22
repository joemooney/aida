#![cfg(target_os = "linux")]
// Black-box coverage: `aida comment add` with an unknown `@handle` warns and
// opens no mailbox for it, while a known role with a live reader is notified.
// trace:TASK-1955 | ai:claude (TASK-1918)
// trace:STORY-1843 | ai:codex

#[path = "support/test_git.rs"]
mod test_git;

use std::process::Command;

fn run(cwd: &std::path::Path, home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(cwd)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .args(args)
        .output()
        .unwrap()
}

/// Every message file under `dir`, whitespace stripped.
fn message_files(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(message_files(&path));
        } else if path.extension().is_some_and(|e| e == "json") {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            out.push(text.chars().filter(|c| !c.is_whitespace()).collect());
        }
    }
    out
}

#[test]
fn comment_add_with_an_unknown_mention_warns_and_opens_no_mailbox() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let home = temp.path().join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "test@example.com"],
        &["config", "user.name", "TASK-1955 Test"],
        &["commit", "-q", "--allow-empty", "-m", "init"],
    ] {
        assert!(test_git::git_command()
            .current_dir(&repo)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
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

    // C1 requires a real current reader, rather than merely a known role name.
    // Bind the fixture to this test process's kernel identity, so PID reuse
    // cannot turn an unrelated process into a reader.
    // trace:STORY-1843 | ai:codex
    let agents = repo.join(".aida/agents");
    std::fs::create_dir_all(&agents).unwrap();
    let reader = agents.join("codex-mention-reader.toml");
    std::fs::write(
        &reader,
        toml::to_string(&serde_json::json!({
            "id": "codex-mention-reader",
            "agent_type": "codex",
            "pid": std::process::id(),
            "pid_start": aida_core::liveness::process_start_identity(std::process::id())
                .expect("Linux test process has a kernel start identity"),
            "name": "advisor-mention-reader",
            "role": "advisor",
            "started_at": "2026-10-21T00:00:00Z",
            "last_active_at": "2026-10-21T00:00:00Z",
            "worktree_path": repo.to_string_lossy(),
            "source": "agent-launcher"
        }))
        .unwrap(),
    )
    .unwrap();

    let out = run(
        &repo,
        &home,
        &["comment", "add", id, "ping @nobody-here-1955 and @advisor"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("@nobody-here-1955: send refused: unknown recipient 'nobody-here-1955'"),
        "{stderr}"
    );
    assert!(stderr.contains("mention dropped"), "{stderr}");
    assert!(stderr.contains("advisor-mention-reader"), "{stderr}");

    let files = message_files(&repo.join(".aida").join("mailbox"));
    assert!(
        files.iter().any(|f| f.contains("\"agent\":\"advisor\"")),
        "the known role is notified: {files:?}"
    );
    assert!(
        !files
            .iter()
            .any(|f| f.contains("\"agent\":\"nobody-here-1955\"")),
        "no mailbox is opened for the unknown handle: {files:?}"
    );

    // Once the only reader is gone, even the known role must refuse delivery.
    // Retain the successful notification and prove the new comment adds none.
    // trace:STORY-1843 | ai:codex
    std::fs::remove_file(reader).unwrap();
    let out = run(&repo, &home, &["comment", "add", id, "ping @advisor"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("@advisor: send refused: 'advisor' has 0 live readers"),
        "{stderr}"
    );
    let mut before = files;
    let mut after = message_files(&repo.join(".aida/mailbox"));
    before.sort();
    after.sort();
    assert_eq!(after, before, "zero-reader mention must not append mail");
}
