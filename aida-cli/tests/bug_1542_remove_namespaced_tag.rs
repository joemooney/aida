#![cfg(target_os = "linux")]
//! Black-box regression coverage for exact `--remove-tag` semantics.
// trace:BUG-1542 | ai:codex

use aida_core::Storage;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run git")
}

fn aida(fixture: &Fixture, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(&fixture.repo)
        .env("HOME", &fixture.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .args(args)
        .output()
        .expect("run aida")
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let fixture = Fixture {
        repo: base.join("repo"),
        home: base.join("home"),
        _tmp: tmp,
    };
    std::fs::create_dir_all(&fixture.repo).unwrap();
    std::fs::create_dir_all(&fixture.home).unwrap();
    assert!(git(&fixture.repo, &["init", "-q", "-b", "main"])
        .status
        .success());
    assert!(
        git(&fixture.repo, &["config", "user.email", "test@example.com"])
            .status
            .success()
    );
    assert!(
        git(&fixture.repo, &["config", "user.name", "BUG-1542 Test"])
            .status
            .success()
    );
    assert!(git(
        &fixture.repo,
        &["commit", "-q", "--allow-empty", "-m", "init"]
    )
    .status
    .success());
    let init = aida(
        &fixture,
        &[
            "init",
            "--force",
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
    fixture
}

fn add_task(fixture: &Fixture) -> String {
    let out = aida(
        fixture,
        &[
            "add",
            "--title",
            "tag removal fixture",
            "--type",
            "task",
            "--status",
            "approved",
            "--tags",
            "severity:cosmetic,severity:functional,batch:probe,aida:queue:work,ordinary",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|token| token.starts_with("TASK-") && token[5..].chars().all(|c| c.is_ascii_digit()))
        .unwrap_or_else(|| panic!("missing task id in {stdout}"))
        .to_string()
}

fn stored_tags(fixture: &Fixture, id: &str) -> Vec<String> {
    let storage = Storage::new(fixture.repo.join(".aida-store"));
    let store = storage.load().expect("load canonical store");
    let mut tags: Vec<String> = store
        .requirements
        .iter()
        .find(|req| req.spec_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("missing {id}"))
        .tags
        .iter()
        .cloned()
        .collect();
    tags.sort();
    tags
}

// trace:BUG-1542 | ai:codex
#[test]
fn cli_removes_exact_namespaced_and_simple_tags_without_clobbering_neighbors() {
    let fixture = fixture();
    let id = add_task(&fixture);

    for tag in [
        "severity:cosmetic",
        "batch:probe",
        "aida:queue:work",
        "ordinary",
    ] {
        let out = aida(&fixture, &["edit", &id, "--remove-tag", tag]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains(&format!("removed 1 tag: {tag}")),
            "{stdout}"
        );
        assert!(!stored_tags(&fixture, &id).contains(&tag.to_string()));
    }

    assert_eq!(stored_tags(&fixture, &id), vec!["severity:functional"]);

    // A second removal is an explicit, idempotent no-op, not a misleading
    // "no changes specified" response.
    let absent = aida(&fixture, &["edit", &id, "--remove-tag", "batch:probe"]);
    assert!(absent.status.success());
    let stdout = String::from_utf8_lossy(&absent.stdout);
    assert!(
        stdout.contains("no matching tag to remove: batch:probe"),
        "{stdout}"
    );
    assert_eq!(stored_tags(&fixture, &id), vec!["severity:functional"]);

    // Exercise the read/cache surface after canonical-store writes.
    let shown = aida(&fixture, &["show", &id, "--json"]);
    assert!(
        shown.status.success(),
        "{}",
        String::from_utf8_lossy(&shown.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&shown.stdout).expect("show JSON");
    assert_eq!(json["tags"], serde_json::json!(["severity:functional"]));
}
