//! TASK-3961: a first `aida init` in a fresh HOME and repository prints a
//! short, jargon-free guide: no wall of host-readiness FAIL lines, one line
//! pointing at the details, and no role / advisor / bypass setup chatter.
// trace:TASK-3961 | ai:claude

#[path = "support/test_git.rs"]
mod test_git;

use std::path::Path;
use std::process::{Command, Output};

fn run(cmd: &mut Command) -> Output {
    cmd.output().expect("spawn")
}

fn git(repo: &Path, args: &[&str]) {
    let out = run(test_git::git_command().args(args).current_dir(repo));
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

#[test]
fn task3961_fresh_init_output_is_short_and_jargon_free() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let (repo, home) = (base.join("repo"), base.join("home"));
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    test_git::enroll_synthetic_home(&repo);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Task 3961 Test"]);
    std::fs::write(repo.join("greet.py"), "print('hi')\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "initial"]);

    // A plain first run: no opt-out flags, as a new user types it.
    let out = run(Command::new(env!("CARGO_BIN_EXE_aida"))
        .arg("init")
        .current_dir(&repo)
        .env("HOME", &home)
        .env("AIDA_TELEMETRY", "0")
        .env("NO_COLOR", "1")
        .env_remove("AIDA_OUTPUT_FORMAT")
        .env_remove("AIDA_SESSION_ROLE"));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "aida init: {stdout}\n{stderr}");
    let all = format!("{stdout}\n{stderr}");

    // AC1 / AC4: no FAIL wall; one summary line names where the details are.
    let fails: Vec<&str> = all
        .lines()
        .filter(|l| l.trim_start().starts_with("FAIL"))
        .collect();
    assert!(fails.is_empty(), "FAIL lines in first-run init: {fails:?}");
    assert!(!all.contains("UsernsBlocked"), "{all}");
    let pointers = all
        .lines()
        .filter(|l| l.contains("aida doctor --lane-host"))
        .count();
    assert_eq!(pointers, 1, "exactly one details pointer:\n{all}");

    // AC2: the first-run guide carries the loop, not role/seat jargon.
    assert!(all.contains("The loop:"), "{all}");
    for jargon in [
        "role enter advisor",
        "global roles installed",
        "starter global role",
        "Agent bypass confirmation",
        "set up as the implementer",
    ] {
        assert!(
            !all.contains(jargon),
            "first-run init mentions {jargon:?}:\n{all}"
        );
    }
}
