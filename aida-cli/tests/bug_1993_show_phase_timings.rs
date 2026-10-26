// BUG-688: binary-driving e2e suites are Linux-gated like their siblings.
#![cfg(target_os = "linux")]
//! BUG-1993: `aida show` records per-phase timings in its usage event, so a
//! latency regression can be attributed to cache open, spec lookup, cache
//! context, or git linkage rather than only the total. Other commands record
//! no phases.
// trace:BUG-1993 | ai:claude

#[path = "support/test_git.rs"]
mod test_git;

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
    let status = test_git::git_command()
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

/// BUG-2221: the four BUG-1993 aggregate phases, in first-seen order, with
/// the finer attribution phases filtered out — the aggregate meanings the
/// guard and operators read are unchanged by the added sub-phases.
// trace:BUG-2221 | ai:claude
fn aggregate_phase_names(record: &serde_json::Value) -> Vec<String> {
    phase_names(record)
        .into_iter()
        .filter(|n| matches!(n.as_str(), "open" | "lookup" | "context" | "linkage"))
        .collect()
}

/// A fixture project with one spec; returns (tempdir guard, repo, home, spec).
fn fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    String,
) {
    let base = tempfile::tempdir().expect("tempdir");
    let base_dir = base.path().canonicalize().expect("canonicalize tempdir");
    let repo = base_dir.join("repo");
    let home = base_dir.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    // trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
    test_git::enroll_synthetic_home(&repo);
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
    (base, repo, home, spec)
}

#[test]
fn show_usage_event_records_phase_timings() {
    let (_base, repo, home, spec) = fixture();

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
            aggregate_phase_names(&record),
            ["open", "lookup", "context", "linkage"],
            "aida {args:?} recorded {record}"
        );
    }

    // `--no-git` skips linkage, so it records no linkage phase.
    let record = usage_record_of(&repo, &home, &["show", &spec, "--no-git"]);
    assert_eq!(
        aggregate_phase_names(&record),
        ["open", "lookup", "context"],
        "{record}"
    );
    assert!(
        phase_names(&record)
            .iter()
            .all(|n| !n.starts_with("linkage")),
        "--no-git records no linkage sub-phase either: {record}"
    );

    // Commands that never arm the recorder keep their record unchanged.
    let record = usage_record_of(&repo, &home, &["list"]);
    assert!(
        record.get("phases").is_none(),
        "list recorded phases: {record}"
    );
}

/// BUG-2221: the finer attribution phases land in the same usage-event
/// `phases` vec — pre-dispatch (`argv`), linkage sub-spans (`linkage.log`
/// runs in parallel with the trace walk's `linkage.enum`/`linkage.content`,
/// so sub-spans overlap and are NOT summed into the aggregate), and the JSON
/// surface's `relationships`/`render` — while the four aggregate phases keep
/// their BUG-1993 names and order.
// trace:BUG-2221 | ai:claude
#[test]
fn show_phase_accounting_preserves_aggregate_semantics() {
    let (_base, repo, home, spec) = fixture();
    let record = usage_record_of(&repo, &home, &["show", &spec, "--format", "json"]);
    let names = phase_names(&record);
    assert_eq!(
        aggregate_phase_names(&record),
        ["open", "lookup", "context", "linkage"],
        "{record}"
    );
    for sub in [
        "argv",
        "linkage.log",
        "linkage.enum",
        "linkage.content",
        "relationships",
        "render",
    ] {
        assert!(
            names.iter().any(|n| n == sub),
            "missing `{sub}` phase: {record}"
        );
    }
    let ms = |name: &str| -> u64 {
        record["phases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .and_then(|p| p["ms"].as_u64())
            .unwrap()
    };
    // Sub-spans of the parallel linkage section each fit inside the
    // aggregate (a structural relation, not a timing threshold); the 1 ms
    // slack absorbs independent millisecond truncation.
    for sub in ["linkage.log", "linkage.enum", "linkage.content"] {
        assert!(
            ms(sub) <= ms("linkage") + 1,
            "`{sub}` exceeds `linkage`: {record}"
        );
    }
}
