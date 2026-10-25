// TASK-1808 (folded into TASK-1955): `aida doctor` and `aida awaiting` print
// the solo-human mode line while `[review] solo_human_self_approval` is on in
// the user's global config, and omit it when the key is off or the project is
// the AIDA repository (STORY-1655 S3). Drives the real binary with a scratch
// HOME, so the global config never touches the developer's own.
// trace:TASK-1955 | ai:claude (TASK-1808)
#![cfg(target_os = "linux")]

#[path = "support/test_git.rs"]
mod test_git;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MODE_ON_LINE: &str = "review: solo-human mode ON (self-approval permitted for TTY humans)";
const SOLO_ON: &str = "[review]\nsolo_human_self_approval = true\n";

fn aida(repo: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aida"));
    command
        .current_dir(repo)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .stdin(Stdio::null());
    for name in [
        "AIDA_STORE",
        "AIDA_PROJECT_ROOT",
        "AIDA_DRIVE_ROOT",
        "AIDA_AGENT_OUTPUT",
        "GIT_DIR",
        "GIT_WORK_TREE",
    ] {
        command.env_remove(name);
    }
    let store = repo.join(".aida-store");
    if store.join("objects").is_dir() {
        command.env("AIDA_STORE", store);
    }
    command.args(args).output().expect("run aida")
}

fn git(repo: &Path, args: &[&str]) {
    let status = test_git::git_command()
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?}");
}

/// An initialised project; `aida_repo` makes it read as the AIDA repository.
fn fixture(aida_repo: bool) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    // trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
    test_git::enroll_synthetic_home(&repo);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "TASK-1808 Test"]);
    if aida_repo {
        std::fs::write(
            repo.join("Cargo.toml"),
            "[workspace]\n[workspace.package]\nrepository = \"https://github.com/joemooney/aida\"\n",
        )
        .unwrap();
        git(&repo, &["add", "Cargo.toml"]);
    }
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let init = aida(
        &repo,
        &home,
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
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    (tmp, repo, home)
}

fn prints_mode_line(repo: &Path, home: &Path, args: &[&str]) -> bool {
    let out = aida(repo, home, args);
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|l| l.trim() == MODE_ON_LINE)
}

fn surfaces() -> [&'static [&'static str]; 2] {
    // The human surfaces: off a terminal both default to compact agent output.
    [
        &["doctor", "--format", "human"],
        &["awaiting", "--no-ci", "--format", "human"],
    ]
}

#[test]
fn doctor_and_awaiting_print_the_mode_line_only_while_the_mode_is_on() {
    let (_tmp, repo, home) = fixture(false);
    for args in surfaces() {
        assert!(
            !prints_mode_line(&repo, &home, args),
            "{args:?}: off by default"
        );
    }
    std::fs::write(home.join(".aida").join("config.toml"), SOLO_ON).unwrap();
    for args in surfaces() {
        assert!(prints_mode_line(&repo, &home, args), "{args:?}: mode on");
    }
}

#[test]
fn doctor_and_awaiting_omit_the_mode_line_in_the_aida_repository() {
    let (_tmp, repo, home) = fixture(true);
    std::fs::write(home.join(".aida").join("config.toml"), SOLO_ON).unwrap();
    for args in surfaces() {
        assert!(
            !prints_mode_line(&repo, &home, args),
            "{args:?}: S3, never in the AIDA repository"
        );
    }
}
