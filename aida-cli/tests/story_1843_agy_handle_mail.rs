#![cfg(target_os = "linux")]
// Black-box coverage: a raw Antigravity session started through the shipped
// agy hook wiring gets a handle; a Claude session mails that handle; the agy
// session sees it on its next turn through the PreInvocation hook, once per
// turn, until it reads its inbox.
// trace:STORY-1843 | ai:claude

#[path = "support/test_git.rs"]
mod test_git;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const AGY_CONV: &str = "5f0c2a8e-1843-4b7e-9d10-agy0000000001";

fn templates() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../aida-core/templates")
}

/// The shipped agy wiring for `key`'s `event` handler.
fn agy_hook_command(key: &str, event: &str) -> String {
    let raw = std::fs::read_to_string(templates().join("antigravity-hooks.json")).unwrap();
    let wiring: serde_json::Value = serde_json::from_str(&raw).unwrap();
    wiring[key][event][0]["command"]
        .as_str()
        .unwrap_or_else(|| panic!("no {key}/{event} handler in antigravity-hooks.json"))
        .to_string()
}

/// A clean environment: no ambient Claude/Codex session evidence leaks in.
fn base_env(cmd: &mut Command, home: &Path, path: &str) {
    cmd.env_clear()
        .env("HOME", home)
        .env("PATH", path)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_USER", "joe")
        .env("GIT_CONFIG_NOSYSTEM", "1");
}

fn aida(repo: &Path, home: &Path, path: &str, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    base_env(&mut cmd, home, path);
    cmd.current_dir(repo).args(args).output().unwrap()
}

/// Run an agy hook command the way the client does: payload on stdin, one
/// JSON object on stdout.
fn agy_hook(
    repo: &Path,
    home: &Path,
    path: &str,
    command: &str,
    payload: &str,
) -> serde_json::Value {
    let mut cmd = Command::new("sh");
    base_env(&mut cmd, home, path);
    cmd.env("AIDA_SESSION_ROLE", "implementer");
    let mut child = cmd
        .current_dir(repo)
        .args(["-c", command])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write as _;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("agy hook stdout is not one JSON object ({e}): {stdout}"))
}

fn ephemeral(out: &serde_json::Value) -> Option<String> {
    out["injectSteps"][0]["ephemeralMessage"]
        .as_str()
        .map(str::to_string)
}

struct Kill(std::process::Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

// trace:STORY-1843.ac0eef56 | ai:claude
#[test]
fn claude_mails_an_agy_session_by_handle_and_agy_sees_it_next_turn() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let home = temp.path().join("home");
    let bin = temp.path().join("bin");
    for dir in [&repo, &home, &bin] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let aida_dir = Path::new(env!("CARGO_BIN_EXE_aida")).parent().unwrap();
    let path = format!("{}:{}:/usr/bin:/bin", bin.display(), aida_dir.display());
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "test@example.com"],
        &["config", "user.name", "STORY-1843 Test"],
        &["commit", "-q", "--allow-empty", "-m", "init"],
    ] {
        assert!(test_git::git_command()
            .current_dir(&repo)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let init = aida(
        &repo,
        &home,
        &path,
        &[
            "init",
            "--force",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ],
    );
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    // Install the shipped agy hook script where the shipped wiring looks.
    let hooks = repo.join(".agents/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let script = hooks.join("aida-mail-notice.sh");
    std::fs::copy(templates().join("hooks/aida-mail-notice.sh"), &script).unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    // The agy client: a long-lived process whose argv0 is `agy`, under which
    // its SessionStart hook runs (the registry records the client process).
    std::os::unix::fs::symlink("/bin/sh", bin.join("agy")).unwrap();
    let start_out = temp.path().join("session-start.json");
    let payload_file = temp.path().join("session-start-payload.json");
    std::fs::write(
        &payload_file,
        format!(
            r#"{{"conversationId":"{AGY_CONV}","workspacePaths":["{}"],"modelName":"auto"}}"#,
            repo.display()
        ),
    )
    .unwrap();
    let start_cmd = agy_hook_command("aida-mail-notice", "SessionStart");
    let mut agy = Command::new(bin.join("agy"));
    base_env(&mut agy, &home, &path);
    agy.env("AIDA_SESSION_ROLE", "implementer")
        .env("HOOK", &start_cmd)
        .env("PAYLOAD", &payload_file)
        .env("OUT", &start_out)
        .current_dir(&repo)
        .args([
            "-c",
            r#"sh -c "$HOOK" < "$PAYLOAD" > "$OUT.tmp"; mv "$OUT.tmp" "$OUT"; exec sleep 300"#,
        ]);
    let _agy = Kill(agy.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(60);
    while !start_out.exists() {
        assert!(
            Instant::now() < deadline,
            "agy SessionStart hook never finished"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let start: serde_json::Value =
        serde_json::from_str(std::fs::read_to_string(&start_out).unwrap().trim()).unwrap();
    assert!(start.is_object(), "{start}");

    // The session holds a generated agy handle, listed with its real vendor.
    let handle = "agy-implementer-01";
    let ls = aida(&repo, &home, &path, &["agent", "ls", "--format", "human"]);
    let ls = String::from_utf8_lossy(&ls.stdout);
    let row = ls
        .lines()
        .find(|l| l.starts_with(handle))
        .unwrap_or_else(|| panic!("{handle} not listed: {ls}"));
    assert!(row.contains("implementer"), "{row}");
    let record = std::fs::read_dir(repo.join(".aida/agents"))
        .unwrap()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .find(|body| body.contains(&format!("name = \"{handle}\"")))
        .unwrap_or_else(|| panic!("no registry record for {handle}"));
    assert!(record.contains("agent_type = \"antigravity\""), "{record}");

    // A Claude session addresses the agy session by handle.
    let mut send = Command::new(env!("CARGO_BIN_EXE_aida"));
    base_env(&mut send, &home, &path);
    let sent = send
        .current_dir(&repo)
        .env("CLAUDE_CODE_SESSION_ID", "claude-sender-1")
        .env("AIDA_AGENT_TYPE", "claude")
        .env("AIDA_SESSION_ROLE", "advisor")
        .args([
            "mailbox",
            "send",
            "--to",
            handle,
            "--subject",
            "rebase PR-1",
            "please rebase",
        ])
        .output()
        .unwrap();
    assert!(
        sent.status.success(),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    // An unheld handle is refused rather than opening a phantom inbox.
    let refused = aida(
        &repo,
        &home,
        &path,
        &["mailbox", "send", "--to", "agy-implementer-99", "x"],
    );
    assert!(!refused.status.success());

    // The agy session's next turn surfaces it, addressed to its handle.
    let turn = agy_hook_command("aida-mail-turn-notice", "PreInvocation");
    let payload = |steps: u32, invocation: u32| {
        format!(
            r#"{{"conversationId":"{AGY_CONV}","invocationNum":{invocation},"initialNumSteps":{steps},"modelName":"auto"}}"#
        )
    };
    let first = agy_hook(&repo, &home, &path, &turn, &payload(4, 1));
    let message = ephemeral(&first).unwrap_or_else(|| panic!("no mail injected: {first}"));
    assert!(message.contains("rebase PR-1"), "{message}");
    assert!(message.contains(handle), "{message}");
    // Not repeated on later model calls within the same turn...
    let same_turn = agy_hook(&repo, &home, &path, &turn, &payload(4, 2));
    assert_eq!(ephemeral(&same_turn), None, "{same_turn}");
    // ...but shown again next turn while it stays unread.
    let next_turn = agy_hook(&repo, &home, &path, &turn, &payload(9, 1));
    assert!(ephemeral(&next_turn).is_some_and(|m| m.contains("rebase PR-1")));
    // Another conversation never sees this session's mail.
    let other = agy_hook(
        &repo,
        &home,
        &path,
        &turn,
        r#"{"conversationId":"other-conv","invocationNum":1,"initialNumSteps":2}"#,
    );
    assert!(
        ephemeral(&other).is_none_or(|m| !m.contains("rebase PR-1")),
        "{other}"
    );
}
