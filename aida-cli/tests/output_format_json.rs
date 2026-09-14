// BUG-688: binary-driving e2e tests share the Linux-only gate used by the
// existing real-CLI suites.
// trace:BUG-688 | ai:claude
#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::Command;

fn aida(repo: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(repo);
    cmd.env("HOME", home);
    cmd.env("AIDA_TELEMETRY", "0");
    cmd.env_remove("AIDA_SESSION_ROLE");
    cmd.env_remove("AIDA_PERMISSION_MODE");
    cmd
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

fn init_repo() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let base = tempfile::tempdir().expect("tempdir");
    let base_dir = base.path().canonicalize().expect("canonicalize tempdir");
    let repo = base_dir.join("repo");
    let home = base_dir.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();

    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@t.t"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);

    let init = aida(&repo, &home)
        .args([
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ])
        .output()
        .expect("run aida init");
    assert!(
        init.status.success(),
        "aida init failed (exit {:?}):\n--- stdout ---\n{}\n--- stderr ---\n{}",
        init.status.code(),
        String::from_utf8_lossy(&init.stdout),
        String::from_utf8_lossy(&init.stderr)
    );
    (base, repo, home)
}

#[test]
fn unsupported_format_json_on_focus_warns_instead_of_silent_fallback() {
    let (_base, repo, home) = init_repo();

    let out = aida(&repo, &home)
        .args(["--format", "json", "focus", "show"])
        .output()
        .expect("run aida focus show");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "focus show should remain a successful human fallback:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stdout.contains("No focus set."),
        "review repro should still reach the focus-show human body:\n{stdout}"
    );
    assert!(
        stderr.contains("has no JSON projection"),
        "--format json must not silently fall back with empty stderr:\n{stderr}"
    );
}
