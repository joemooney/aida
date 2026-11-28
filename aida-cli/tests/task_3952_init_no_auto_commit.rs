#![cfg(target_os = "linux")]
//! TASK-3952: `aida init` in a fresh repository never commits to the user's
//! branch unless asked. Driven through the real binary, without a terminal
//! (the CI / script path that used to commit silently).
// trace:TASK-3952 | ai:claude

#[path = "support/test_git.rs"]
mod test_git;

use std::path::Path;
use std::process::{Command, Output};

fn git(repo: &Path, args: &[&str]) -> String {
    let out = test_git::git_command()
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A new user's repository: one README commit on `main`, isolated HOME.
fn fresh_repo(base: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    test_git::enroll_synthetic_home(&repo);
    git(&repo, &["config", "user.email", "t@t.t"]);
    git(&repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("README.md"), "hi\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    (repo, home)
}

fn aida_init(repo: &Path, home: &Path, extra: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(repo)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .env_remove("AIDA_INIT_COMMIT_SCAFFOLD")
        .env_remove("AIDA_SESSION_ROLE")
        .env_remove("AIDA_AGENT_OUTPUT")
        .env_remove("AIDA_OUTPUT_FORMAT")
        .arg("init")
        .args(["--no-roles", "--no-agent-config"])
        .args(extra)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run aida init");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Printed so `--nocapture` shows what a new user sees.
    eprintln!("--- aida init {extra:?} stdout ---\n{stdout}\n--- stderr ---\n{stderr}");
    assert!(
        out.status.success(),
        "aida init failed:\n{stdout}\n{stderr}"
    );
    out
}

/// AC1 + AC2: the default init leaves HEAD where it was, lists what it wrote
/// with line counts, and says plainly that nothing was committed.
#[test]
fn default_init_in_a_fresh_repo_creates_no_commit() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (repo, home) = fresh_repo(tmp.path());
    let head_before = git(&repo, &["rev-parse", "HEAD"]);

    let out = aida_init(&repo, &home, &[]);

    assert_eq!(
        git(&repo, &["rev-parse", "HEAD"]),
        head_before,
        "a default `aida init` must not commit to the user's branch"
    );
    assert_eq!(git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("AIDA wrote these files"), "{stdout}");
    assert!(stdout.contains(" lines)"), "line counts shown: {stdout}");
    assert!(stdout.contains("They are not committed"), "{stdout}");
    assert!(
        !git(&repo, &["status", "--porcelain"]).is_empty(),
        "init's files are left in the working tree for review"
    );

    // The suggested next step works as printed.
    let hint = stdout
        .lines()
        .skip_while(|l| !l.contains("save them with:"))
        .nth(1)
        .expect("a git command follows the hint")
        .trim();
    let ran = Command::new("sh")
        .args(["-c", hint])
        .current_dir(&repo)
        .env("HOME", &home)
        .output()
        .unwrap();
    assert!(
        ran.status.success(),
        "`{hint}` failed: {}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert_eq!(
        git(&repo, &["log", "-1", "--format=%s"]),
        "chore: scaffold AIDA"
    );
}

/// AC1 + AC3: `--commit` makes the scaffold commit, and says so.
#[test]
fn init_with_commit_flag_commits_and_says_so() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (repo, home) = fresh_repo(tmp.path());
    let head_before = git(&repo, &["rev-parse", "HEAD"]);

    let out = aida_init(&repo, &home, &["--commit"]);

    assert_ne!(git(&repo, &["rev-parse", "HEAD"]), head_before);
    assert_eq!(
        git(&repo, &["log", "-1", "--format=%s"]),
        "chore: scaffold AIDA"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("committed them to `main`"), "{stdout}");
}
