#![cfg(target_os = "linux")]
//! Black-box regression for terminal specs still owned by a live drain.
// trace:TASK-163 | ai:codex

use std::path::Path;
use std::process::{Command, Output};

fn git(repo: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git")
        .success());
}

fn aida(repo: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(repo)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .args(args)
        .output()
        .expect("run aida")
}

#[test]
fn completed_spec_reports_live_review_and_merge_drain_phases() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "TASK-163 Test"]);
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
        "init: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    let add = aida(
        &repo,
        &home,
        &[
            "add",
            "--title",
            "live drain terminal fixture",
            "--type",
            "task",
            "--status",
            "approved",
        ],
    );
    assert!(
        add.status.success(),
        "add: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    let stdout = String::from_utf8_lossy(&add.stdout);
    let spec = stdout
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|token| token.starts_with("TASK-") && token[5..].chars().all(|c| c.is_ascii_digit()))
        .unwrap_or_else(|| panic!("missing task id in {stdout}"))
        .to_string();
    let completed = aida(&repo, &home, &["edit", &spec, "--status", "completed"]);
    assert!(
        completed.status.success(),
        "complete: {}",
        String::from_utf8_lossy(&completed.stderr)
    );

    let runtime = repo.join(".aida");
    let pid = std::process::id();
    std::fs::write(
        runtime.join("drain.lock"),
        format!(r#"{{"pid":{pid},"started_at_utc":"2999-01-01T00:00:00+00:00","command":"aida queue work --auto-complete","host":"test","specs":["{spec}"]}}"#),
    )
    .unwrap();

    for (phase, expected) in [(3, "3/6 (reviewer)"), (4, "4/6 (merge)")] {
        std::fs::write(
            runtime.join("drain-state.json"),
            format!(
                r#"{{"command":"aida queue work {spec} --auto-complete","mode":"single","members":[{{"spec":"{spec}","state":"in-phase-{phase}"}}],"pipeline_depth":1,"current":"{spec}","current_phase":"{phase}","orchestrator_pid":{pid},"started_at":"2026-09-19T00:00:00+00:00","on_drain_complete":"done"}}"#
            ),
        )
        .unwrap();

        let why = aida(&repo, &home, &["why", &spec, "--json"]);
        assert!(
            why.status.success(),
            "why phase {phase}: {}",
            String::from_utf8_lossy(&why.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&why.stdout).expect("why JSON");
        assert_eq!(value["bucket"], "in-flight", "phase {phase}: {value}");
        assert_eq!(value["drain"]["phase"], expected, "phase {phase}: {value}");
        assert_eq!(
            value["drain"]["orchestrator_pid"], pid,
            "phase {phase}: {value}"
        );
    }
}
