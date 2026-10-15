// BUG-688: binary-driving e2e suites are Linux-gated like their siblings.
#![cfg(target_os = "linux")]
//! BUG-2007: `aida agent new codex` launches codex off the shared app-server
//! daemon. Attached to the daemon, codex ran its SessionStart hooks with the
//! daemon's environment (another session's role and grant), so an implementer
//! child was told it was the advisor.
// trace:BUG-2007 | ai:claude

use std::path::Path;
use std::process::{Command, Output};

fn aida(repo: &Path, home: &Path, path: &str, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(repo)
        .env("HOME", home)
        .env("AIDA_HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .env("PATH", path)
        .args(args);
    for var in [
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_GRANT",
        "AIDA_PERMISSION_MODE",
        "AIDA_AGENT_OUTPUT",
        "AIDA_OUTPUT_FORMAT",
        "AIDA_HEADLESS_VENDOR",
    ] {
        cmd.env_remove(var);
    }
    cmd.output().expect("run aida")
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn codex_launch_command_runs_without_the_shared_daemon() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let (repo, home, bin) = (base.join("repo"), base.join("home"), base.join("bin"));
    for dir in [&repo, &home, &bin] {
        std::fs::create_dir_all(dir).unwrap();
    }
    // A stand-in `codex` so the launcher resolves a binary; --no-exec never
    // runs it.
    let codex = bin.join("codex");
    std::fs::write(&codex, "#!/bin/sh\nexit 0\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Codex Launch Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let init = aida(
        &repo,
        &home,
        &path,
        &[
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ],
    );
    assert!(
        init.status.success(),
        "init: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let out = aida(
        &repo,
        &home,
        &path,
        &[
            "agent",
            "new",
            "codex",
            "--role",
            "implementer",
            "--no-exec",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "agent new codex --no-exec: {}\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.split_whitespace().any(|arg| arg == "--no-daemon"),
        "the codex launch command must carry --no-daemon:\n{stdout}"
    );
}
