#![cfg(target_os = "linux")]
//! Black-box coverage for ambiguous spec ids: one id that resolves to two
//! requirements (a merge-gate agreed_id equal to another spec's native id).
//! Writes and `aida show` must refuse with a non-zero exit, naming each
//! candidate's unambiguous handle; `aida list` must not emit two rows under
//! one id.
// trace:BUG-1535 | ai:claude

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap()
        .success());
}

fn aida(fixture: &Fixture, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(&fixture.repo)
        .env("HOME", &fixture.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_OUTPUT_FORMAT", "human")
        .args(args)
        .output()
        .expect("run aida")
}

fn ok(out: &Output) -> String {
    assert!(
        out.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn yaml(fixture: &Fixture, id: &str) -> String {
    std::fs::read_to_string(
        fixture
            .repo
            .join(".aida-store/objects/TASK/000")
            .join(format!("{id}.yaml")),
    )
    .unwrap()
}

fn uuid_of(fixture: &Fixture, id: &str) -> String {
    yaml(fixture, id)
        .lines()
        .find_map(|l| l.strip_prefix("id: "))
        .map(|s| s.trim().trim_matches('\'').trim_matches('"').to_string())
        .expect("uuid line")
}

/// TASK-1 (native) and TASK-2 (agreed id rewritten to TASK-1) — the live
/// store's BUG-34 shape.
fn colliding_fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Ambiguous Id Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let fixture = Fixture {
        _tmp: tmp,
        repo,
        home,
    };
    ok(&aida(
        &fixture,
        &[
            "init",
            "--force",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ],
    ));
    for title in ["native owner", "agreed holder"] {
        ok(&aida(
            &fixture,
            &[
                "add",
                "--title",
                title,
                "--type",
                "task",
                "--description",
                "d",
            ],
        ));
    }
    let path = fixture
        .repo
        .join(".aida-store/objects/TASK/000/TASK-2.yaml");
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(body.contains("agreed_id: TASK-2"), "{body}");
    std::fs::write(
        &path,
        body.replace("agreed_id: TASK-2", "agreed_id: TASK-1"),
    )
    .unwrap();
    git(
        &fixture.repo.join(".aida-store"),
        &["commit", "-qam", "collide"],
    );
    ok(&aida(&fixture, &["cache", "rebuild"]));
    fixture
}

#[test]
fn writes_on_an_ambiguous_id_refuse_and_name_each_handle() {
    let fixture = colliding_fixture();
    let native_uuid = uuid_of(&fixture, "TASK-1");
    let before_1 = yaml(&fixture, "TASK-1");
    let before_2 = yaml(&fixture, "TASK-2");

    for args in [
        vec!["edit", "TASK-1", "--title", "changed"],
        vec!["edit", "task-1", "--status", "rejected"],
        vec!["comment", "add", "TASK-1", "hello"],
        vec!["rel", "add", "TASK-1", "TASK-2", "--type", "related"],
    ] {
        let out = aida(&fixture, &args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !out.status.success(),
            "{args:?} must refuse; stderr:\n{stderr}"
        );
        assert!(stderr.contains("ambiguous"), "{args:?}: {stderr}");
        assert!(stderr.contains(&native_uuid), "{args:?}: {stderr}");
        assert!(stderr.contains("TASK-2"), "{args:?}: {stderr}");
    }
    // Neither object was touched.
    assert_eq!(yaml(&fixture, "TASK-1"), before_1);
    assert_eq!(yaml(&fixture, "TASK-2"), before_2);

    // Each unambiguous handle still works.
    ok(&aida(
        &fixture,
        &["edit", &native_uuid, "--title", "native renamed"],
    ));
    assert!(yaml(&fixture, "TASK-1").contains("native renamed"));
    ok(&aida(
        &fixture,
        &["edit", "TASK-2", "--title", "holder renamed"],
    ));
    assert!(yaml(&fixture, "TASK-2").contains("holder renamed"));
}

#[test]
fn show_on_an_ambiguous_id_exits_non_zero_listing_candidates() {
    let fixture = colliding_fixture();
    let native_uuid = uuid_of(&fixture, "TASK-1");
    let out = aida(&fixture, &["show", "TASK-1", "--no-git"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "stderr:\n{stderr}");
    assert!(stderr.contains("native owner"), "{stderr}");
    assert!(stderr.contains("agreed holder"), "{stderr}");
    assert!(stderr.contains(&native_uuid), "{stderr}");

    let by_uuid = ok(&aida(&fixture, &["show", &native_uuid, "--no-git"]));
    assert!(by_uuid.contains("native owner"), "{by_uuid}");
}

#[test]
fn list_does_not_emit_two_rows_under_one_id() {
    let fixture = colliding_fixture();
    let out = aida(&fixture, &["list", "--all", "--json"]);
    let stdout = ok(&out);
    let rows: serde_json::Value = serde_json::from_str(&stdout).expect("list json");
    let rows = rows
        .as_array()
        .cloned()
        .or_else(|| rows.get("requirements").and_then(|r| r.as_array()).cloned())
        .expect("row array");
    let ids: Vec<String> = rows
        .iter()
        .filter(|r| {
            r.get("title")
                .and_then(|t| t.as_str())
                .is_some_and(|t| t == "native owner" || t == "agreed holder")
        })
        .map(|r| {
            r.get("agreed_id")
                .and_then(|v| v.as_str())
                .or_else(|| r.get("spec_id").and_then(|v| v.as_str()))
                .or_else(|| r.get("id").and_then(|v| v.as_str()))
                .unwrap()
                .to_ascii_uppercase()
        })
        .collect();
    assert_eq!(ids.len(), 2, "{stdout}");
    assert_ne!(ids[0], ids[1], "two rows under one id: {ids:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("ambiguous"));
}
