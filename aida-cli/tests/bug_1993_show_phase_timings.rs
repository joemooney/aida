// BUG-688: binary-driving e2e suites are Linux-gated like their siblings.
#![cfg(target_os = "linux")]
//! BUG-1993: `aida show` records per-phase timings in its usage event, so a
//! latency regression can be attributed to cache open, spec lookup, cache
//! context, or git linkage rather than only the total. Other commands record
//! no phases.
// trace:BUG-1993 | ai:claude

use std::path::Path;
use std::process::Command;

mod support;

fn aida(repo: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(repo);
    cmd.env("HOME", home);
    cmd.env("AIDA_HOME", home);
    cmd.env("AIDA_TELEMETRY", "0");
    for var in [
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_GRANT",
        "AIDA_PERMISSION_MODE",
        "AIDA_AGENT_OUTPUT",
        "AIDA_OUTPUT_FORMAT",
        "AIDA_HEADLESS_VENDOR",
    ] {
        cmd.env_remove(var);
    }
    cmd
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

fn run_ok(cmd: &mut Command) -> String {
    let out = cmd.output().expect("run aida");
    assert!(
        out.status.success(),
        "aida failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Run `aida <args>` with telemetry on and return the usage record it wrote.
fn usage_record_of(repo: &Path, home: &Path, args: &[&str]) -> serde_json::Value {
    let log = home.join(".aida").join("usage.jsonl");
    let _ = std::fs::remove_file(&log);
    run_ok(aida(repo, home).env_remove("AIDA_TELEMETRY").args(args));
    let text = std::fs::read_to_string(&log).expect("usage.jsonl written");
    let line = text
        .lines()
        .rev()
        .find(|l| l.contains("\"cmd\""))
        .expect("a usage record");
    serde_json::from_str(line).expect("usage record is JSON")
}

fn phase_names(record: &serde_json::Value) -> Vec<String> {
    record
        .get("phases")
        .and_then(|p| p.as_array())
        .map(|phases| {
            phases
                .iter()
                .map(|p| {
                    assert!(p["ms"].is_u64(), "phase ms is an integer: {p}");
                    p["name"].as_str().expect("phase name").to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn show_usage_event_records_phase_timings() {
    let base = tempfile::tempdir().expect("tempdir");
    let base_dir = base.path().canonicalize().expect("canonicalize tempdir");
    let repo = base_dir.join("repo");
    let home = base_dir.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@t.t"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    run_ok(aida(&repo, &home).args([
        "init",
        "--no-skills",
        "--no-hooks",
        "--no-agent-config",
        "--no-roles",
    ]));
    let grant = support::ensure_seat(&home, &repo, "advisor", &[]).expect("advisor grant");
    let add_out = run_ok(
        aida(&repo, &home)
            .env("AIDA_SESSION_ROLE", "advisor")
            .env("AIDA_SESSION_GRANT", grant)
            .args(["add", "--type", "bug", "--title", "phase timing fixture"]),
    );
    let spec = add_out
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|t| t.starts_with("BUG-"))
        .unwrap_or_else(|| panic!("no spec id in add output:\n{add_out}"))
        .to_string();

    // Every render surface times the same phases, linkage included.
    for args in [
        vec!["show", spec.as_str()],
        vec!["show", spec.as_str(), "--json"],
        vec!["show", spec.as_str(), "--format", "toon"],
        vec!["show", spec.as_str(), "--card"],
    ] {
        let record = usage_record_of(&repo, &home, &args);
        assert_eq!(record["cmd"], "show", "{record}");
        assert_eq!(
            phase_names(&record),
            ["open", "lookup", "context", "linkage"],
            "aida {args:?} recorded {record}"
        );
    }

    // `--no-git` skips linkage, so it records no linkage phase.
    let record = usage_record_of(&repo, &home, &["show", &spec, "--no-git"]);
    assert_eq!(
        phase_names(&record),
        ["open", "lookup", "context"],
        "{record}"
    );

    // Commands that never arm the recorder keep their record unchanged.
    let record = usage_record_of(&repo, &home, &["list"]);
    assert!(
        record.get("phases").is_none(),
        "list recorded phases: {record}"
    );
}
