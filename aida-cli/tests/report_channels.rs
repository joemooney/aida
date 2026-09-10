// BUG-688: gated to Linux like the other binary-driving e2e suites that depend
// on `aida init` in a throwaway git repo.
// trace:BUG-688 | ai:claude
#![cfg(target_os = "linux")]
//! STORY-1004: `aida report bug|idea` keeps stdout paste-ready.
//!
//! The composed Subject/body report is the stdout contract. Local filing is a
//! side effect, so its confirmation belongs on stderr.
// trace:STORY-1004

use std::path::Path;
use std::process::Command;

fn aida(repo: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(repo);
    cmd.env("HOME", home);
    cmd.env("AIDA_TELEMETRY", "0");
    cmd.env("AIDA_AGENT_OUTPUT", "0");
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
fn report_bug_stdout_is_only_paste_ready_report_and_filing_notice_is_stderr() {
    let (_base, repo, home) = init_repo();

    let out = aida(&repo, &home)
        .args([
            "report",
            "bug",
            "--title",
            "picker lists disabled agent",
            "--description",
            "Steps:\n1. Open picker\n2. See disabled agent",
        ])
        .output()
        .expect("run aida report bug");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "aida report failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );

    assert!(
        stdout.starts_with("Subject: aida: picker lists disabled agent\n\n"),
        "stdout must begin with the paste-ready subject/body report:\n{stdout}"
    );
    assert!(
        stdout
            .contains("Description:\nSteps:\n1. Open picker\n2. See disabled agent\n\nContext:\n"),
        "stdout must contain the composed report body:\n{stdout}"
    );
    assert!(
        !stdout.contains("Filed local upstream AIDA report"),
        "filing confirmation must not be appended to stdout:\n{stdout}"
    );
    assert!(
        stderr.contains("Filed local upstream AIDA report BUG-"),
        "filing confirmation must be visible on stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("AIDA: picker lists disabled agent"),
        "stderr confirmation should name the locally filed report:\n{stderr}"
    );
}
