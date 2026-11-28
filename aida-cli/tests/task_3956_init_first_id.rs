// Linux-only like the other binary-driving first-run suites (BUG-688).
#![cfg(target_os = "linux")]
//! TASK-3956: the first-run loop `aida init` prints names the ID `aida add`
//! really issues next, instead of a hard-coded `TASK-1`.
// trace:TASK-3956 | ai:claude

#[path = "support/test_git.rs"]
mod test_git;

use std::path::{Path, PathBuf};
use std::process::Command;

fn aida(repo: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(repo);
    cmd.env("HOME", home);
    cmd.env("AIDA_TELEMETRY", "0");
    cmd.env("AIDA_AGENT_OUTPUT", "0");
    cmd.env("NO_COLOR", "1");
    cmd.env_remove("AIDA_SESSION_ROLE");
    cmd.env_remove("AIDA_PERMISSION_MODE");
    cmd
}

fn git(repo: &Path, args: &[&str]) {
    let status = test_git::git_command()
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

/// A fresh repo plus HOME; `enroll` opts into the authoritative-ID fixture.
fn fresh_project(enroll: bool) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let base = tempfile::tempdir().expect("tempdir");
    let dir = base.path().canonicalize().unwrap();
    let (repo, home) = (dir.join("repo"), dir.join("home"));
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    if enroll {
        test_git::enroll_synthetic_home(&repo);
    }
    git(&repo, &["config", "user.email", "t@t.t"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    (base, repo, home)
}

/// Run init, then add a task; return (init stdout, predicted ID, issued ID).
fn init_then_add(repo: &Path, home: &Path) -> (String, Option<String>, String) {
    let init = aida(repo, home)
        .args([
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ])
        .output()
        .expect("run aida init");
    let out = String::from_utf8_lossy(&init.stdout).to_string();
    assert!(init.status.success(), "aida init failed:\n{out}");
    let predicted = out
        .lines()
        .find_map(|l| l.split("AIDA will give it the ID ").nth(1))
        .map(|id| id.trim().to_string());

    let add = aida(repo, home)
        .args(["add", "--type", "task", "--title", "Build login page"])
        .output()
        .expect("run aida add");
    let add_out = String::from_utf8_lossy(&add.stdout).to_string();
    assert!(add.status.success(), "aida add failed:\n{add_out}");
    let issued = add_out
        .lines()
        .find_map(|l| l.strip_prefix("Added: "))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no `Added:` line:\n{add_out}"))
        .to_string();
    (out, predicted, issued)
}

fn assert_loop_uses(out: &str, id: &str) {
    for cmd in [
        format!("aida done {id}"),
        format!("aida show {id}"),
        format!("({id})"),
    ] {
        assert!(out.contains(&cmd), "init loop lacks `{cmd}`:\n{out}");
    }
}

#[test]
fn local_only_init_names_the_long_id_add_really_issues() {
    let (_tmp, repo, home) = fresh_project(false);
    let (out, predicted, issued) = init_then_add(&repo, &home);
    assert_eq!(predicted.as_deref(), Some(issued.as_str()), "{out}");
    assert!(
        aida_core::id_identity::IdKey::parse(&issued).is_some_and(|k| k.is_node_local()),
        "a local-only clone issues a node-local ID, got {issued}"
    );
    assert_loop_uses(&out, &issued);
    assert!(
        !out.contains("TASK-1 ") && !out.contains("(TASK-1)"),
        "{out}"
    );
    assert!(out.contains("names this copy of the project"), "{out}");
}

/// An enrolled clone dispenses from an ID block claimed at add time, which
/// init cannot predict: it must then promise no ID at all, never a wrong one.
#[test]
fn block_allocating_init_promises_no_id_it_cannot_predict() {
    let (_tmp, repo, home) = fresh_project(true);
    let (out, predicted, issued) = init_then_add(&repo, &home);
    assert_eq!(
        predicted, None,
        "init guessed an ID before {issued} was claimed:\n{out}"
    );
    assert!(out.contains("prints the new task's ID"), "{out}");
    assert_loop_uses(&out, "<ID>");
    assert!(
        !out.contains("TASK-1 ") && !out.contains("(TASK-1)"),
        "{out}"
    );
}
