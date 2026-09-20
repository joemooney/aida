// BUG-1289: `--format json` must return parseable JSON on `aida status`
// (bare and per-spec) and `aida ps`, must match the dedicated `--json` flag
// where both exist, and the per-spec view must carry the flag-only liveness
// case explicitly (the TASK-163 regression class) rather than collapsing it
// into live/stale. Binary-driving e2e tests share the Linux-only gate used
// by the existing real-CLI suites.
// trace:BUG-1289 | ai:claude
#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::{Command, Output};

fn aida(repo: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(repo)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .env_remove("AIDA_PERMISSION_MODE")
        .args(args)
        .output()
        .expect("run aida")
}

fn git(repo: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git")
        .success());
}

fn init_repo() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let base = tmp.path().canonicalize().expect("canonical tempdir");
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "BUG-1289 Test"]);
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
    (tmp, repo, home)
}

fn add_spec(repo: &Path, home: &Path, status: &str) -> String {
    let add = aida(
        repo,
        home,
        &[
            "add",
            "--title",
            "BUG-1289 format json fixture",
            "--type",
            "task",
            "--status",
            status,
        ],
    );
    assert!(
        add.status.success(),
        "add: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    let stdout = String::from_utf8_lossy(&add.stdout);
    stdout
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|token| token.starts_with("TASK-") && token[5..].chars().all(|c| c.is_ascii_digit()))
        .unwrap_or_else(|| panic!("missing task id in {stdout}"))
        .to_string()
}

fn parse_json(out: &Output, label: &str) -> serde_json::Value {
    assert!(
        out.status.success(),
        "{label} exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{label} did not produce parseable JSON: {e}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        )
    })
}

/// AC1: `aida status <spec> --format json` returns parseable JSON — the
/// exact repro from the spec ("human output, starting TASK-1289 In
/// Progress").
#[test]
fn status_spec_format_json_parses_and_matches_dedicated_json_flag() {
    let (_tmp, repo, home) = init_repo();
    let spec = add_spec(&repo, &home, "approved");

    let via_flag = aida(&repo, &home, &["status", &spec, "--json"]);
    let via_format = aida(&repo, &home, &["status", &spec, "--format", "json"]);
    let mut a = parse_json(&via_flag, "status <spec> --json");
    let mut b = parse_json(&via_format, "status <spec> --format json");
    // `idle_secs` is a live clock read between the two invocations — strip it
    // before the equality check so a 1s scheduling wobble isn't a false fail.
    a["idle_secs"] = serde_json::Value::Null;
    b["idle_secs"] = serde_json::Value::Null;
    assert_eq!(a, b, "--json and --format json must be the same document");
    assert_eq!(a["spec"], spec);
    assert_eq!(a["status"], "Approved");
}

/// AC1: `aida status --format json` (bare, project-wide) returns parseable
/// JSON.
#[test]
fn status_bare_format_json_parses() {
    let (_tmp, repo, home) = init_repo();
    let out = aida(&repo, &home, &["status", "--format", "json", "--no-ci"]);
    let value = parse_json(&out, "status --format json");
    assert!(
        value.get("requirements").is_some() || value.get("agents").is_some(),
        "expected the project-wide status shape, got {value}"
    );
}

/// AC1: `aida ps --format json` returns parseable JSON, and matches `--json`.
#[test]
fn ps_format_json_parses_and_matches_dedicated_json_flag() {
    let (_tmp, repo, home) = init_repo();
    let via_flag = aida(&repo, &home, &["ps", "--json"]);
    let via_format = aida(&repo, &home, &["ps", "--format", "json"]);
    let a = parse_json(&via_flag, "ps --json");
    let b = parse_json(&via_format, "ps --format json");
    assert_eq!(a, b, "--json and --format json must be the same document");
}

/// BINDING GUARD (Part B): flag-only liveness (status In-Progress, no live
/// session linked) must be a DISTINCT `liveness` value — never collapsed
/// into "live" or "stale". This is the exact TASK-163 regression class.
#[test]
fn status_spec_flag_only_liveness_is_a_distinct_json_value() {
    let (_tmp, repo, home) = init_repo();
    let spec = add_spec(&repo, &home, "approved");
    let edit = aida(&repo, &home, &["edit", &spec, "--status", "in-progress"]);
    assert!(
        edit.status.success(),
        "edit: {}",
        String::from_utf8_lossy(&edit.stderr)
    );

    let out = aida(&repo, &home, &["status", &spec, "--format", "json"]);
    let value = parse_json(&out, "status <flag-only spec> --format json");
    assert_eq!(value["in_progress"], true, "{value}");
    assert_eq!(
        value["liveness"], "flag-only",
        "flag-only must not collapse into live/stale: {value}"
    );
    assert_eq!(value["live"], false, "{value}");
    assert!(
        value["session"].is_null(),
        "flag-only has no backing session: {value}"
    );
}

/// Part B: the per-spec JSON carries active pid + drain round ("phase and
/// round") when a live drain owns the spec — from the same local
/// `drain-state.json` / `drain.lock` fixture TASK-163 uses.
#[test]
fn status_spec_reports_active_pid_and_drain_round() {
    let (_tmp, repo, home) = init_repo();
    let spec = add_spec(&repo, &home, "approved");
    let edit = aida(&repo, &home, &["edit", &spec, "--status", "in-progress"]);
    assert!(edit.status.success());

    let runtime = repo.join(".aida");
    let pid = std::process::id();
    std::fs::write(
        runtime.join("drain.lock"),
        format!(
            r#"{{"pid":{pid},"started_at_utc":"2999-01-01T00:00:00+00:00","command":"aida queue work --auto-complete","host":"test","specs":["{spec}"]}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        runtime.join("drain-state.json"),
        format!(
            r#"{{"command":"aida queue work {spec} --auto-complete","mode":"single","members":[{{"spec":"{spec}","state":"in-phase-3"}}],"pipeline_depth":1,"current":"{spec}","current_phase":"3","phase_attempt":2,"orchestrator_pid":{pid},"started_at":"2026-09-19T00:00:00+00:00","on_drain_complete":"done"}}"#
        ),
    )
    .unwrap();

    let out = aida(&repo, &home, &["status", &spec, "--format", "json"]);
    let value = parse_json(&out, "status <live-drain spec> --format json");
    assert_eq!(value["liveness"], "live", "{value}");
    assert_eq!(value["active_pid"], pid, "{value}");
    assert_eq!(value["drain"]["phase"], "3/6 (reviewer)", "{value}");
    assert_eq!(value["drain"]["round"], 2, "{value}");
    assert_eq!(value["drain"]["orchestrator_pid"], pid, "{value}");
}

/// Part B: a punted (NeedsAttention) spec surfaces the parked reason with
/// its category under the unified `parked` key.
#[test]
fn status_spec_reports_parked_reason_for_a_punt() {
    let (_tmp, repo, home) = init_repo();
    let spec = add_spec(&repo, &home, "approved");
    let edit = aida(&repo, &home, &["edit", &spec, "--status", "in-progress"]);
    assert!(edit.status.success());
    let punt = aida(
        &repo,
        &home,
        &[
            "punt",
            &spec,
            "--category",
            "design-fork",
            "--reason",
            "two equally defensible approaches, no spec guidance",
        ],
    );
    assert!(
        punt.status.success(),
        "punt: {}",
        String::from_utf8_lossy(&punt.stderr)
    );

    let out = aida(&repo, &home, &["status", &spec, "--format", "json"]);
    let value = parse_json(&out, "status <punted spec> --format json");
    assert_eq!(value["status"], "Needs Attention", "{value}");
    assert_eq!(value["parked"]["source"], "punt", "{value}");
    assert_eq!(value["parked"]["category"], "design-fork", "{value}");
    assert!(
        value["parked"]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("defensible"),
        "{value}"
    );
}

/// AC2/AC3: the two contract-manifest surfaces that had no `--json` at all
/// (`queue progress`, `findings list`) now parse as JSON and match
/// `--format json`.
#[test]
fn queue_progress_and_findings_list_format_json_parse() {
    let (_tmp, repo, home) = init_repo();

    let qp_flag = aida(&repo, &home, &["queue", "progress", "--json"]);
    let qp_format = aida(&repo, &home, &["queue", "progress", "--format", "json"]);
    let a = parse_json(&qp_flag, "queue progress --json");
    let b = parse_json(&qp_format, "queue progress --format json");
    assert_eq!(a, b);
    assert!(a.get("queued").is_some(), "{a}");
    assert!(a.get("in_progress").is_some(), "{a}");
    assert!(a.get("done").is_some(), "{a}");

    let fl_flag = aida(&repo, &home, &["findings", "list", "--json"]);
    let fl_format = aida(&repo, &home, &["findings", "list", "--format", "json"]);
    let a = parse_json(&fl_flag, "findings list --json");
    let b = parse_json(&fl_format, "findings list --format json");
    assert_eq!(a, b);
    assert!(a["findings"].is_array(), "{a}");
}
