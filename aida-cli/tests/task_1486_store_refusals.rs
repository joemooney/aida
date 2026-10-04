#![cfg(target_os = "linux")]
//! Black-box coverage for the store-resolution refusals, all under a fake HOME
//! and temp directories:
//! - agent-mode output of the no-project refusal keeps every explicit-store
//!   option (not just `aida init`);
//! - tracker commands inside a distributed project resolve through distributed
//!   detection instead of claiming there is no `.aida/config.toml`;
//! - `init --refresh` in a distributed project whose store isn't attached does
//!   not seed a stale local `requirements.db`.
// trace:TASK-1486 | ai:claude

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let root = tmp.path().to_path_buf();
    Fixture {
        _tmp: tmp,
        home,
        root,
    }
}

fn aida_in(f: &Fixture, dir: &Path, format: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(dir)
        .env("HOME", &f.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_OUTPUT_FORMAT", format)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env_remove("AIDA_STORE")
        .env_remove("REQ_DB_NAME")
        .env_remove("AIDA_REGISTRY_PATH")
        .env_remove("REQ_REGISTRY_PATH")
        .env_remove("AIDA_AGENT_OUTPUT")
        .stdin(std::process::Stdio::null())
        .args(args)
        .output()
        .expect("run aida")
}

fn text(out: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A git repo whose `.aida/config.toml` declares distributed mode, with no
/// `aida-store` branch and (unless `attached`) no `.aida-store/` worktree.
fn distributed_project(f: &Fixture, attached: bool) -> PathBuf {
    let proj = f.root.join("proj");
    std::fs::create_dir_all(proj.join(".aida")).unwrap();
    let init = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&proj)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .unwrap();
    assert!(init.success());
    std::fs::write(proj.join(".aida/config.toml"), "mode = \"distributed\"\n").unwrap();
    if attached {
        std::fs::create_dir_all(proj.join(".aida-store/objects")).unwrap();
    }
    proj
}

#[test]
fn agent_mode_no_project_refusal_lists_the_explicit_store_options() {
    let f = fixture();
    let outside = f.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let legacy = f.root.join("other/requirements.db");
    std::fs::write(
        f.home.join(".aida.config"),
        format!(
            "projects:\n  other:\n    path: {}\n    description: unrelated\n\
             default_project: other\n",
            legacy.display()
        ),
    )
    .unwrap();

    let out = aida_in(&f, &outside, "toon", &["list"]);
    assert!(!out.status.success(), "must refuse: {}", text(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("error: \"No AIDA project found here"),
        "{}",
        text(&out)
    );
    assert!(stdout.contains("help[5]:"), "{}", text(&out));
    for needle in [
        "aida init",
        "--file <path>",
        "-p <project>",
        "REQ_DB_NAME",
        "AIDA_STORE",
        "-p other",
    ] {
        assert!(
            stdout.contains(needle),
            "missing {needle:?}: {}",
            text(&out)
        );
    }
    assert!(!stdout.contains("BUG-"), "no internal ids: {}", text(&out));
    assert!(!stdout.contains("TASK-"), "no internal ids: {}", text(&out));
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

#[test]
fn tracker_commands_in_an_unattached_distributed_project_name_the_real_problem() {
    let f = fixture();
    let proj = distributed_project(&f, false);
    for tracker in ["github", "gitlab", "jira"] {
        let out = aida_in(&f, &proj, "human", &[tracker, "list"]);
        assert!(!out.status.success(), "{tracker}: {}", text(&out));
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            !err.contains("there is no `.aida/config.toml`"),
            "{tracker} must not deny the config that exists: {}",
            text(&out)
        );
        assert!(
            err.contains("This is a distributed AIDA project"),
            "{tracker}: {}",
            text(&out)
        );
    }
}

#[test]
fn tracker_commands_in_an_attached_distributed_project_resolve_the_store() {
    let f = fixture();
    let proj = distributed_project(&f, true);
    // Resolution succeeds and the command reaches its own tracker checks
    // (no repository configured), instead of any store refusal.
    let out = aida_in(&f, &proj, "human", &["github", "push", "TASK-1"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("No AIDA project found"), "{}", text(&out));
    assert!(!err.contains("store isn't set up"), "{}", text(&out));
    assert!(err.contains("aida github config"), "{}", text(&out));
}

#[test]
fn init_refresh_in_an_unattached_distributed_project_leaves_a_stale_local_db_alone() {
    let f = fixture();
    let proj = distributed_project(&f, false);
    let stale = proj.join("requirements.db");
    let seeded = aida_in(
        &f,
        &proj,
        "human",
        &[
            "--file",
            stale.to_str().unwrap(),
            "add",
            "stale spec",
            "--type",
            "task",
        ],
    );
    assert!(seeded.status.success(), "{}", text(&seeded));
    let before = std::fs::read(&stale).unwrap();

    let out = aida_in(&f, &proj, "human", &["init", "--refresh"]);
    assert!(out.status.success(), "{}", text(&out));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("type protocols not seeded"), "{}", text(&out));
    assert!(!err.contains("TASK-"), "no internal ids: {}", text(&out));
    assert_eq!(
        std::fs::read(&stale).unwrap(),
        before,
        "the stale local requirements.db must not be seeded"
    );
}
