#![cfg(target_os = "linux")]
//! Black-box acceptance coverage for safe `aida edit` description sources.
// trace:BUG-1234 | ai:codex

use aida_core::Storage;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
    id: String,
}

fn git(dir: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap()
        .success());
}

fn command(fixture: &Fixture) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aida"));
    command
        .current_dir(&fixture.repo)
        .env("HOME", &fixture.home)
        .env("AIDA_TELEMETRY", "0");
    command
}

fn aida(fixture: &Fixture, args: &[&str]) -> Output {
    command(fixture).args(args).output().expect("run aida")
}

fn init_fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "BUG-1234 Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);

    let mut fixture = Fixture {
        _tmp: tmp,
        repo,
        home,
        id: String::new(),
    };
    assert!(aida(
        &fixture,
        &[
            "init",
            "--force",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles"
        ]
    )
    .status
    .success());
    let added = aida(
        &fixture,
        &[
            "add",
            "--title",
            "fixture",
            "--type",
            "task",
            "--description",
            "original",
        ],
    );
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let stdout = String::from_utf8_lossy(&added.stdout);
    fixture.id = stdout
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|token| token.starts_with("TASK-") && token[5..].chars().all(|c| c.is_ascii_digit()))
        .unwrap()
        .to_string();
    fixture
}

fn description(fixture: &Fixture) -> String {
    Storage::new(fixture.repo.join(".aida-store"))
        .load()
        .unwrap()
        .requirements
        .into_iter()
        .find(|req| req.spec_id.as_deref() == Some(&fixture.id))
        .unwrap()
        .description
}

#[test]
fn edit_replaces_description_from_file_and_stdin() {
    let fixture = init_fixture();
    let path = fixture.repo.join("description.md");
    std::fs::write(&path, "from file\nwith `literal` and $dollar\n").unwrap();
    let edited = aida(
        &fixture,
        &[
            "edit",
            &fixture.id,
            "--description-from-file",
            path.to_str().unwrap(),
        ],
    );
    assert!(
        edited.status.success(),
        "{}",
        String::from_utf8_lossy(&edited.stderr)
    );
    assert_eq!(
        description(&fixture),
        "from file\nwith `literal` and $dollar\n"
    );

    let mut child = command(&fixture)
        .args(["edit", &fixture.id, "--description-stdin"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"from stdin\nsecond line\n")
        .unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(description(&fixture), "from stdin\nsecond line\n");
}

#[test]
fn edit_refuses_missing_and_empty_files_without_erasing_description() {
    let fixture = init_fixture();
    let missing = fixture.repo.join("missing.md");
    let result = aida(
        &fixture,
        &[
            "edit",
            &fixture.id,
            "--description-from-file",
            missing.to_str().unwrap(),
        ],
    );
    assert!(!result.status.success());
    assert_eq!(description(&fixture), "original");

    let empty = fixture.repo.join("empty.md");
    std::fs::write(&empty, "").unwrap();
    let result = aida(
        &fixture,
        &[
            "edit",
            &fixture.id,
            "--description-from-file",
            empty.to_str().unwrap(),
        ],
    );
    assert!(!result.status.success());
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output.contains("description input is empty"), "{output}");
    assert_eq!(description(&fixture), "original");
}

#[test]
fn edit_help_lists_all_sources_and_clap_rejects_combining_them() {
    let fixture = init_fixture();
    let help = aida(&fixture, &["edit", "--help"]);
    let help = String::from_utf8_lossy(&help.stdout);
    for flag in [
        "--description <DESCRIPTION>",
        "--description-from-file",
        "--description-stdin",
    ] {
        assert!(help.contains(flag), "missing {flag} in help:\n{help}");
    }

    let path = fixture.repo.join("description.md");
    std::fs::write(&path, "replacement").unwrap();
    let conflict = aida(
        &fixture,
        &[
            "edit",
            &fixture.id,
            "--description",
            "inline",
            "--description-from-file",
            path.to_str().unwrap(),
        ],
    );
    assert_eq!(conflict.status.code(), Some(2));
    assert_eq!(description(&fixture), "original");
}
