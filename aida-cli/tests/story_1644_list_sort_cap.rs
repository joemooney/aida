#![cfg(target_os = "linux")]
//! Black-box acceptance coverage for STORY-1644: `aida list` sorts by natural
//! numeric ID by default (TASK-2 before TASK-11), `[list] default_sort`
//! overrides that and `--sort` beats it, and the agent-mode row cap is
//! configurable (`--limit` > `AIDA_LIST_LIMIT` > `[list] agent_limit` > 100)
//! and never cuts rows silently.
// trace:STORY-1644 | ai:claude

use std::path::Path;
use std::process::{Command, Output};

mod support;

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: std::path::PathBuf,
    home: std::path::PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run git")
}

fn aida_with(fixture: &Fixture, envs: &[(&str, &str)], args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(&fixture.repo)
        .env("HOME", &fixture.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .env("NO_COLOR", "1")
        .env_remove("AIDA_LIST_LIMIT")
        .env_remove("AIDA_OUTPUT_FORMAT")
        .env_remove("AIDA_AGENT_OUTPUT")
        .args(args);
    if let Some(grant) = support::ensure_seat(&fixture.home, &fixture.repo, "advisor", &[]) {
        cmd.env("AIDA_SESSION_GRANT", grant);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("run aida")
}

fn aida(fixture: &Fixture, args: &[&str]) -> Output {
    aida_with(fixture, &[], args)
}

fn ok(label: &str, out: Output) -> String {
    assert!(
        out.status.success(),
        "{label} failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
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
    assert!(git(&repo, &["config", "user.name", "Sort Test"])
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
    ok(
        "aida init",
        aida(
            &fixture,
            &[
                "init",
                "--force",
                "--no-skills",
                "--no-hooks",
                "--no-agent-config",
                "--no-roles",
            ],
        ),
    );
    fixture
}

fn task_id_from(stdout: &str) -> String {
    stdout
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|t| {
            t.strip_prefix("TASK-")
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        })
        .unwrap_or_else(|| panic!("missing TASK id in {stdout}"))
        .to_string()
}

/// Set `[section] key = value` in the project's `.aida/config.toml`.
fn set_config(fixture: &Fixture, section: &str, key: &str, value: toml::Value) {
    let path = fixture.repo.join(".aida/config.toml");
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml::Table = toml::from_str(&body).expect("config parses");
    let table = doc
        .entry(section.to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .expect("section is a table");
    table.insert(key.to_string(), value);
    std::fs::write(&path, toml::to_string(&doc).unwrap()).unwrap();
}

fn clear_config(fixture: &Fixture, section: &str, key: &str) {
    let path = fixture.repo.join(".aida/config.toml");
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml::Table = toml::from_str(&body).expect("config parses");
    if let Some(t) = doc.get_mut(section).and_then(|v| v.as_table_mut()) {
        t.remove(key);
    }
    std::fs::write(&path, toml::to_string(&doc).unwrap()).unwrap();
}

/// Row ids in output order from the machine JSON projection.
fn json_ids(fixture: &Fixture, envs: &[(&str, &str)], args: &[&str]) -> Vec<String> {
    let mut argv: Vec<&str> = vec!["list"];
    argv.extend_from_slice(args);
    argv.extend_from_slice(&["--format", "json"]);
    let stdout = ok("aida list --format json", aida_with(fixture, envs, &argv));
    let rows: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("list json: {e}\n{stdout}"));
    rows.as_array()
        .unwrap_or_else(|| panic!("not an array: {stdout}"))
        .iter()
        .map(|r| r["spec_id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The agent-mode TOON listing: (header line, row ids in order, full text).
fn toon_list(
    fixture: &Fixture,
    envs: &[(&str, &str)],
    args: &[&str],
) -> (String, Vec<String>, String) {
    let mut argv: Vec<&str> = vec!["list"];
    argv.extend_from_slice(args);
    argv.extend_from_slice(&["--format", "toon"]);
    let stdout = ok("aida list --format toon", aida_with(fixture, envs, &argv));
    let header = stdout
        .lines()
        .find(|l| l.starts_with("count: "))
        .unwrap_or_else(|| panic!("no count header:\n{stdout}"))
        .to_string();
    let ids = stdout
        .lines()
        .skip_while(|l| !l.starts_with("specs["))
        .skip(1)
        .take_while(|l| l.starts_with(' '))
        .map(|l| l.trim().split(',').next().unwrap_or_default().to_string())
        .collect();
    (header, ids, stdout)
}

fn tasks(n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("TASK-{i}")).collect()
}

#[test]
fn list_sort_and_agent_cap_follow_story_1644() {
    let fx = init_fixture();
    let mut created = Vec::new();
    for i in 1..=12 {
        let out = ok(
            "aida add",
            aida(
                &fx,
                &["add", "--type", "task", "--title", &format!("task {i}")],
            ),
        );
        created.push(task_id_from(&out));
    }
    assert_eq!(created, tasks(12), "fresh project allocates TASK-1..12");
    // Touch TASK-2 so recency order differs from ID order.
    ok(
        "aida edit",
        aida(&fx, &["edit", "TASK-2", "--priority", "high"]),
    );

    // 1. Default order: natural numeric ID (TASK-2 before TASK-11), on both
    //    the JSON and the agent TOON surface.
    assert_eq!(json_ids(&fx, &[], &[]), tasks(12));
    let (header, ids, _) = toon_list(&fx, &[], &[]);
    assert_eq!(ids, tasks(12));
    assert_eq!(header, "count: 12 of 12 open");

    // 2. `[list] default_sort` overrides the default; `--sort` beats config.
    set_config(&fx, "list", "default_sort", "modified".into());
    let by_modified = json_ids(&fx, &[], &[]);
    assert_eq!(by_modified.first().map(String::as_str), Some("TASK-2"));
    assert_eq!(json_ids(&fx, &[], &["--sort", "id"]), tasks(12));
    clear_config(&fx, "list", "default_sort");

    // 3. Agent cap from config, cut announced in the header.
    set_config(&fx, "list", "agent_limit", 3.into());
    let (header, ids, text) = toon_list(&fx, &[], &[]);
    assert_eq!(ids, tasks(3));
    assert_eq!(
        header,
        "count: 3 of 12 open — showing first 3 by id; newest hidden: --sort modified, or --limit 0"
    );
    assert!(text.contains("agent_limit"), "{text}");

    // 4. Env beats config.
    let (header, ids, _) = toon_list(&fx, &[("AIDA_LIST_LIMIT", "5")], &[]);
    assert_eq!(ids, tasks(5));
    assert!(header.starts_with("count: 5 of 12 open — "), "{header}");

    // 5. Flags beat env: `--limit N`, `--limit 0` (no cap), `--all`.
    let (header, ids, _) = toon_list(&fx, &[("AIDA_LIST_LIMIT", "5")], &["--limit", "7"]);
    assert_eq!(ids, tasks(7));
    assert!(header.starts_with("count: 7 of 12 open — "), "{header}");
    let (header, ids, _) = toon_list(&fx, &[("AIDA_LIST_LIMIT", "5")], &["--limit", "0"]);
    assert_eq!(ids, tasks(12));
    assert_eq!(header, "count: 12 of 12 open");
    let (_, ids, _) = toon_list(&fx, &[("AIDA_LIST_LIMIT", "5")], &["--all"]);
    assert_eq!(ids, tasks(12));

    // 6. `agent_limit = 0` is unbounded.
    set_config(&fx, "list", "agent_limit", 0.into());
    let (_, ids, _) = toon_list(&fx, &[], &[]);
    assert_eq!(ids, tasks(12));

    // 7. JSON stays a bare array and is never cut by the agent cap; an
    //    explicit `--limit` cut is announced on stderr.
    set_config(&fx, "list", "agent_limit", 2.into());
    assert_eq!(json_ids(&fx, &[], &[]), tasks(12));
    let out = aida(&fx, &["list", "--limit", "4", "--format", "json"]);
    let stdout = ok("json --limit 4", out.clone());
    let rows: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(rows.as_array().map(Vec::len), Some(4));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("truncated: true, total: 12"),
        "stderr should announce the cut:\n{stderr}"
    );
}

#[test]
fn history_limit_is_configurable() {
    let fx = init_fixture();
    for i in 1..=4 {
        ok(
            "aida add",
            aida(
                &fx,
                &["add", "--type", "task", "--title", &format!("h {i}")],
            ),
        );
    }
    let count_rows = |envs: &[(&str, &str)], args: &[&str]| -> usize {
        let mut argv = vec!["history"];
        argv.extend_from_slice(args);
        argv.extend_from_slice(&["--format", "json"]);
        let stdout = ok("aida history --json", aida_with(&fx, envs, &argv));
        let v: serde_json::Value =
            serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("history json: {e}\n{stdout}"));
        v.as_array()
            .or_else(|| v.get("events").and_then(|e| e.as_array()))
            .or_else(|| v.get("rows").and_then(|e| e.as_array()))
            .unwrap_or_else(|| panic!("unexpected history json shape: {stdout}"))
            .len()
    };
    let unbounded = count_rows(&[], &[]);
    assert!(
        unbounded >= 4,
        "expected >= 4 history rows, got {unbounded}"
    );
    set_config(&fx, "history", "limit", 2.into());
    assert_eq!(count_rows(&[], &[]), 2);
    assert_eq!(count_rows(&[("AIDA_HISTORY_LIMIT", "3")], &[]), 3);
    assert_eq!(
        count_rows(&[("AIDA_HISTORY_LIMIT", "3")], &["--limit", "1"]),
        1
    );
}
