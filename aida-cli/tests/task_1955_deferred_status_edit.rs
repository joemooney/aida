// TASK-1955 (advisor finding, 2026-10-21 09:27): `aida edit <ID> --status X`
// on a deferred spec printed `Updated` while the spec kept reading
// `deferred` until `aida undefer`. The edit is now refused with the undefer
// hint, and the stored status is left alone. Drives the real binary with a
// scratch HOME.
// trace:TASK-1955 | ai:claude
#![cfg(target_os = "linux")]

#[path = "support/test_git.rs"]
mod test_git;

use std::path::Path;
use std::process::{Command, Stdio};

fn aida(repo: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aida"));
    command
        .current_dir(repo)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .stdin(Stdio::null());
    for name in [
        "AIDA_STORE",
        "AIDA_PROJECT_ROOT",
        "AIDA_DRIVE_ROOT",
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_GRANT",
        "AIDA_AGENT_OUTPUT",
        "GIT_DIR",
        "GIT_WORK_TREE",
    ] {
        command.env_remove(name);
    }
    command.args(args).output().expect("run aida")
}

fn ok(out: &std::process::Output, what: &str) -> String {
    assert!(
        out.status.success(),
        "{what}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn git(repo: &Path, args: &[&str]) {
    let status = test_git::git_command()
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?}");
}

#[test]
fn status_edit_of_a_deferred_spec_is_refused_with_the_undefer_hint() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "TASK-1955 Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    ok(
        &aida(
            &repo,
            &home,
            &[
                "init",
                "--no-skills",
                "--no-hooks",
                "--no-agent-config",
                "--no-roles",
            ],
        ),
        "init",
    );
    ok(
        &aida(
            &repo,
            &home,
            &[
                "add",
                "--title",
                "probe",
                "--type",
                "task",
                "--description",
                "x",
            ],
        ),
        "add",
    );
    ok(&aida(&repo, &home, &["defer", "TASK-1"]), "defer");

    let edit = aida(&repo, &home, &["edit", "TASK-1", "--status", "rejected"]);
    let stdout = String::from_utf8_lossy(&edit.stdout);
    let stderr = String::from_utf8_lossy(&edit.stderr);
    assert!(!edit.status.success(), "a no-op status edit must fail");
    assert!(
        !stdout.contains("Updated"),
        "never `Updated` for a no-op: {stdout}"
    );
    // Off a terminal the error is rendered on stdout; either stream counts.
    assert!(
        format!("{stdout}{stderr}").contains("aida undefer TASK-1"),
        "stderr: {stderr}\nstdout: {stdout}"
    );

    // The stored status is untouched: undeferring shows the original draft.
    ok(&aida(&repo, &home, &["undefer", "TASK-1"]), "undefer");
    let show = ok(&aida(&repo, &home, &["show", "TASK-1"]), "show");
    assert!(
        show.lines().any(|l| l.trim() == "status: draft"),
        "the refused edit must not have changed the status: {show}"
    );
}
