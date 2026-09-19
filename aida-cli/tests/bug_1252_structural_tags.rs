#![cfg(target_os = "linux")]
//! Black-box acceptance coverage for BUG-1252. These tests drive the shipped
//! CLI rather than the guard/healer helpers, then inspect the canonical store.
// trace:BUG-1252 | ai:codex

use aida_core::{Relationship, RelationshipType, Requirement, Storage};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const INCIDENT_IDS: [&str; 13] = [
    "BUG-1222",
    "BUG-1224",
    "BUG-1226",
    "BUG-1227",
    "BUG-1229",
    "BUG-1230",
    "BUG-1233",
    "SPIKE-82",
    "STORY-1221",
    "TASK-1271",
    "TASK-1276",
    "TASK-1278",
    "TASK-1-140",
];

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

fn init_fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    assert!(git(&repo, &["init", "-q", "-b", "main"]).status.success());
    assert!(git(&repo, &["config", "user.email", "test@example.com"])
        .status
        .success());
    assert!(git(&repo, &["config", "user.name", "BUG-1252 Test"])
        .status
        .success());
    assert!(git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"])
        .status
        .success());

    let fixture = Fixture {
        _tmp: tmp,
        repo,
        home,
    };
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
        "init failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&init.stdout),
        String::from_utf8_lossy(&init.stderr)
    );
    fixture
}

fn add_tagged_task(fixture: &Fixture) -> String {
    let out = aida(
        fixture,
        &[
            "add",
            "--title",
            "structural tag fixture",
            "--type",
            "task",
            "--status",
            "approved",
            "--tags",
            "parent:EPIC-28,batch:incident,lane:research,severity:high,lifecycle:blocked,aida:owned,ordinary",
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

#[test]
fn cli_refuses_every_structural_family_and_force_ledgers_dropped_tags() {
    let fixture = init_fixture();
    let id = add_tagged_task(&fixture);

    let refused = Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(&fixture.repo)
        .env("HOME", &fixture.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_AGENT_OUTPUT", "1")
        .args(["edit", &id, "--tags", "ordinary"])
        .output()
        .expect("run refusing edit");
    assert_eq!(refused.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&refused.stdout);
    for tag in [
        "aida:owned",
        "batch:incident",
        "lane:research",
        "lifecycle:blocked",
        "parent:EPIC-28",
        "severity:high",
    ] {
        assert!(
            stdout.contains(tag),
            "agent refusal omitted {tag}: {stdout}"
        );
    }
    assert!(stdout.contains("structural_tags_dropped=["), "{stdout}");
    assert!(
        stdout.contains("use_--add-tag/--remove-tag_or_pass_--force"),
        "{stdout}"
    );

    let forced = aida(&fixture, &["edit", &id, "--tags", "ordinary", "--force"]);
    assert!(
        forced.status.success(),
        "forced edit failed: {}",
        String::from_utf8_lossy(&forced.stderr)
    );
    let subject = git(
        &fixture.repo.join(".aida-store"),
        &["log", "-1", "--format=%s"],
    );
    let subject = String::from_utf8_lossy(&subject.stdout);
    assert!(subject.contains("replaced tags, dropped:"), "{subject}");
    for tag in [
        "aida:owned",
        "batch:incident",
        "lane:research",
        "lifecycle:blocked",
        "parent:EPIC-28",
        "severity:high",
    ] {
        assert!(
            subject.contains(tag),
            "audit subject omitted {tag}: {subject}"
        );
    }
}

#[test]
fn doctor_cli_lists_and_heals_the_thirteen_incident_ids() {
    let fixture = init_fixture();
    let storage = Storage::new(fixture.repo.join(".aida-store"));
    let mut store = storage.load().expect("load initialized store");
    let mut parent = Requirement::new("Incident parent".into(), String::new());
    parent.spec_id = Some("EPIC-28".into());
    let parent_uuid = parent.id;
    store.requirements.push(parent);
    for id in INCIDENT_IDS {
        let mut child = Requirement::new(format!("incident {id}"), String::new());
        child.spec_id = Some(id.into());
        child.relationships.push(Relationship {
            rel_type: RelationshipType::Child,
            target_id: parent_uuid,
            created_at: None,
            created_by: None,
        });
        store.requirements.push(child);
    }
    storage
        .save_with_commit_subject(&store, "test: seed BUG-1252 incident fixture")
        .expect("save incident fixture");

    let detected = aida(
        &fixture,
        &["doctor", "--category", "parent-tag-drift", "--json"],
    );
    assert!(
        detected.status.success(),
        "doctor failed: {}",
        String::from_utf8_lossy(&detected.stderr)
    );
    let detected = String::from_utf8_lossy(&detected.stdout);
    for id in INCIDENT_IDS {
        assert!(detected.contains(id), "doctor omitted {id}: {detected}");
    }

    let healed = aida(
        &fixture,
        &[
            "doctor",
            "--heal",
            "--yes",
            "--category",
            "parent-tag-drift",
        ],
    );
    assert!(
        healed.status.success(),
        "doctor --heal failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&healed.stdout),
        String::from_utf8_lossy(&healed.stderr)
    );
    let healed_store = storage.load().expect("reload healed store");
    for id in INCIDENT_IDS {
        let req = healed_store.get_requirement_by_spec_id(id).unwrap();
        assert!(req.tags.contains("parent:EPIC-28"), "{id} was not healed");
    }

    let clean = aida(
        &fixture,
        &["doctor", "--category", "parent-tag-drift", "--json"],
    );
    assert!(clean.status.success());
    let clean = String::from_utf8_lossy(&clean.stdout);
    for id in INCIDENT_IDS {
        assert!(
            !clean.contains(id),
            "healed id still reported: {id}: {clean}"
        );
    }
}
