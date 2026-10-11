// The derived per-spec driver end to end: `aida driver register` + session
// leases + batch tags resolve every in-progress spec to relay / agent /
// queued / idle in `aida ps --json` and `aida list --json`. Fake drivers are
// spawned `sleep` children (killed to simulate a dead relay); HOME and the
// project are isolated tempdirs.
// trace:STORY-1653 | ai:claude
#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::{Child, Command, Output};

mod support;

fn aida(repo: &Path, home: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(repo)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .env_remove("AIDA_PERMISSION_MODE")
        .env_remove("AIDA_RELAY_RUN")
        .env_remove("AIDA_DRIVER");
    if let Some(grant) = support::ensure_seat(home, repo, "advisor", &[]) {
        cmd.env("AIDA_SESSION_GRANT", grant);
    }
    cmd.args(args).output().expect("run aida")
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
    git(&repo, &["config", "user.name", "Driver Test"]);
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

/// Add an In-Progress task (optionally tagged) and return its id.
fn add_in_progress(repo: &Path, home: &Path, tags: Option<&str>) -> String {
    let mut args = vec![
        "add",
        "--title",
        "driver fixture",
        "--type",
        "task",
        "--status",
        "approved",
    ];
    if let Some(t) = tags {
        args.extend(["--tags", t]);
    }
    let add = aida(repo, home, &args);
    assert!(
        add.status.success(),
        "add: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    let stdout = String::from_utf8_lossy(&add.stdout);
    let id = stdout
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|t| t.starts_with("TASK-") && t[5..].chars().all(|c| c.is_ascii_digit()))
        .unwrap_or_else(|| panic!("missing task id in {stdout}"))
        .to_string();
    let edit = aida(repo, home, &["edit", &id, "--status", "in-progress"]);
    assert!(
        edit.status.success(),
        "edit: {}",
        String::from_utf8_lossy(&edit.stderr)
    );
    id
}

fn sleeper() -> Child {
    Command::new("sleep")
        .arg("600")
        .spawn()
        .expect("spawn sleep")
}

/// A pid that is guaranteed dead: spawn, kill, reap.
fn dead_pid() -> u32 {
    let mut c = sleeper();
    let pid = c.id();
    c.kill().unwrap();
    c.wait().unwrap();
    pid
}

/// Write a session lease the way pre-driver binaries did: NO `driver` key
/// unless `driver` is given — old lease files must keep parsing.
fn write_lease(repo: &Path, id: &str, spec: &str, active_pid: u32, driver: Option<&str>) {
    let dir = repo.join(".aida").join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = format!(
        "id = \"{id}\"\nscope = \"{spec}\"\nslug = \"{slug}\"\nowner = \"tester\"\n\
         worktree_path = \"{wt}\"\nbranch = \"{slug}\"\nstarted_at = \"{now}\"\n\
         hostname = \"h\"\nrole = \"implementer\"\nactive_pid = {active_pid}\n",
        slug = spec.to_ascii_lowercase(),
        wt = repo.join("no-such-worktree").display(),
        now = chrono::Utc::now().to_rfc3339(),
    );
    if let Some(d) = driver {
        body.push_str(&format!("driver = \"{d}\"\n"));
    }
    std::fs::write(dir.join(format!("{id}.toml")), body).unwrap();
}

fn json(out: &Output, label: &str) -> serde_json::Value {
    assert!(
        out.status.success(),
        "{label}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{label} not JSON: {e}\n{}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn list_row(repo: &Path, home: &Path, spec: &str) -> serde_json::Value {
    let v = json(
        &aida(repo, home, &["list", "--json", "--all"]),
        "list --json",
    );
    v.as_array()
        .and_then(|rows| rows.iter().find(|r| r["spec_id"] == spec).cloned())
        .unwrap_or_else(|| panic!("{spec} missing from list json: {v}"))
}

/// The ps JSON entry for `spec` from sessions, driven or orphaned.
fn ps_entry(repo: &Path, home: &Path, spec: &str) -> (String, serde_json::Value) {
    let v = json(&aida(repo, home, &["ps", "--json", "--all"]), "ps --json");
    for section in ["sessions", "driven", "orphaned"] {
        if let Some(row) = v[section]
            .as_array()
            .and_then(|rows| rows.iter().find(|r| r["spec"] == spec))
        {
            return (section.to_string(), row.clone());
        }
    }
    panic!("{spec} missing from ps json: {v}");
}

/// Registered relay + spec in its batch + dead lease pid → `relay:<run>`
/// everywhere, the row reads live (no stale/stalled wording). Killing the
/// relay degrades the spec to `queued (batch:X)` without deleting anything.
#[test]
fn relay_drives_batch_spec_then_degrades_to_queued_when_killed() {
    let (_tmp, repo, home) = init_repo();
    let spec = add_in_progress(&repo, &home, Some("batch:next"));
    write_lease(&repo, "aaaabbbbcccc", &spec, dead_pid(), None);

    let mut relay = sleeper();
    let pid = relay.id().to_string();
    let reg = aida(
        &repo,
        &home,
        &[
            "driver",
            "register",
            "--kind",
            "relay",
            "--run",
            "lib-split",
            "--tag",
            "batch:next",
            "--pid",
            &pid,
        ],
    );
    assert!(
        reg.status.success(),
        "register: {}",
        String::from_utf8_lossy(&reg.stderr)
    );

    let listed = json(
        &aida(&repo, &home, &["driver", "list", "--json"]),
        "driver list",
    );
    assert_eq!(
        listed["drivers"][0]["driver"], "relay:lib-split",
        "{listed}"
    );
    assert_eq!(listed["drivers"][0]["live"], true, "{listed}");

    let (section, row) = ps_entry(&repo, &home, &spec);
    assert_eq!(section, "sessions", "{row}");
    assert_eq!(row["driver"], "relay:lib-split", "{row}");
    assert_eq!(row["driver_kind"], "relay", "{row}");
    assert_eq!(
        row["liveness"], "live",
        "a driven row is never stale: {row}"
    );
    assert!(row["dispatch_state"].is_null(), "no stalled framing: {row}");

    let lrow = list_row(&repo, &home, &spec);
    assert_eq!(lrow["driver"], "relay:lib-split", "{lrow}");
    assert_eq!(lrow["driver_kind"], "relay", "{lrow}");
    let filtered = json(
        &aida(&repo, &home, &["list", "--json", "--driver", "relay"]),
        "list --driver",
    );
    assert_eq!(filtered.as_array().map(Vec::len), Some(1), "{filtered}");

    // Human surfaces: the driver sits beside the status; nothing reads as
    // orphaned/stalled while the relay is live.
    let human_list = aida(&repo, &home, &["list", "--format", "human"]);
    let text = String::from_utf8_lossy(&human_list.stdout);
    eprintln!("{text}");
    assert!(text.contains("· relay:lib-split"), "{text}");
    let human_ps = aida(&repo, &home, &["ps", "--format", "human"]);
    let text = String::from_utf8_lossy(&human_ps.stdout);
    eprintln!("{text}");
    assert!(text.contains("relay:lib-split"), "{text}");
    for word in ["Orphaned", "stalled", "STALLED", "dormant", "Dormant"] {
        assert!(
            !text.contains(word),
            "driven row must not read {word}: {text}"
        );
    }

    // Kill the relay: nothing is cleaned up, the spec just stops being driven.
    relay.kill().unwrap();
    relay.wait().unwrap();
    let record = repo
        .join(".aida")
        .join("drivers")
        .join("relay-lib-split.json");
    assert!(record.exists(), "readers never reap driver records");

    let lrow = list_row(&repo, &home, &spec);
    assert_eq!(lrow["driver"], "queued (batch:next)", "{lrow}");
    assert_eq!(lrow["driver_kind"], "queued", "{lrow}");
    let (_, row) = ps_entry(&repo, &home, &spec);
    assert_eq!(row["driver_kind"], "queued", "{row}");
    assert!(record.exists(), "still no cleanup after ps/list reads");

    let listed = json(
        &aida(&repo, &home, &["driver", "list", "--json"]),
        "driver list",
    );
    assert_eq!(listed["drivers"][0]["live"], false, "{listed}");
    let dereg = aida(
        &repo,
        &home,
        &["driver", "deregister", "--run", "lib-split"],
    );
    assert!(dereg.status.success());
    assert!(
        !record.exists(),
        "deregister removes the caller's own record"
    );
}

/// A lease that names its relay (`AIDA_RELAY_RUN` at claim time) resolves to
/// that relay even with no batch tag.
#[test]
fn lease_driver_field_attributes_to_live_relay() {
    let (_tmp, repo, home) = init_repo();
    let spec = add_in_progress(&repo, &home, None);
    write_lease(
        &repo,
        "ddddeeeeffff",
        &spec,
        dead_pid(),
        Some("relay:nightly"),
    );
    let mut relay = sleeper();
    let pid = relay.id().to_string();
    let reg = aida(
        &repo,
        &home,
        &["driver", "register", "--run", "nightly", "--pid", &pid],
    );
    assert!(
        reg.status.success(),
        "register: {}",
        String::from_utf8_lossy(&reg.stderr)
    );

    let (section, row) = ps_entry(&repo, &home, &spec);
    assert_eq!(
        (section.as_str(), &row["driver"]),
        ("sessions", &"relay:nightly".into()),
        "{row}"
    );
    assert_eq!(list_row(&repo, &home, &spec)["driver"], "relay:nightly");
    relay.kill().unwrap();
    relay.wait().unwrap();
}

/// A lease whose own process is alive → `agent:<session>`.
#[test]
fn live_lease_pid_resolves_to_agent() {
    let (_tmp, repo, home) = init_repo();
    let spec = add_in_progress(&repo, &home, None);
    let mut agent = sleeper();
    write_lease(&repo, "0123456789ab", &spec, agent.id(), None);

    let (section, row) = ps_entry(&repo, &home, &spec);
    assert_eq!(section, "sessions", "{row}");
    assert_eq!(row["driver"], "agent:01234567", "{row}");
    let lrow = list_row(&repo, &home, &spec);
    assert_eq!(lrow["driver_kind"], "agent", "{lrow}");
    agent.kill().unwrap();
    agent.wait().unwrap();
}

/// No lease, no driver, no batch → `idle <age>`, reported as an orphan with
/// the idle reading (never "inactive"/"dormant").
#[test]
fn no_lease_no_driver_is_idle() {
    let (_tmp, repo, home) = init_repo();
    let spec = add_in_progress(&repo, &home, None);
    let lrow = list_row(&repo, &home, &spec);
    assert_eq!(lrow["driver_kind"], "idle", "{lrow}");
    assert!(
        lrow["driver"].as_str().unwrap_or("").starts_with("idle"),
        "{lrow}"
    );
    let (section, row) = ps_entry(&repo, &home, &spec);
    assert_eq!(section, "orphaned", "{row}");
    assert_eq!(row["driver_kind"], "idle", "{row}");
    let none = json(
        &aida(&repo, &home, &["list", "--json", "--driver", "relay"]),
        "list --driver",
    );
    assert_eq!(none.as_array().map(Vec::len), Some(0), "{none}");
}
