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
    aida_env(repo, home, args, &[])
}

fn aida_env(repo: &Path, home: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
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
    cmd.envs(env.iter().copied());
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

/// A fake driver/agent process: a `sleep 600` child that is killed and
/// reaped on drop, so a failing assertion never leaks it.
struct Sleeper(Child);

impl Sleeper {
    fn id(&self) -> u32 {
        self.0.id()
    }

    /// Kill and reap (idempotent).
    fn kill(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for Sleeper {
    fn drop(&mut self) {
        self.kill();
    }
}

fn sleeper() -> Sleeper {
    Sleeper(
        Command::new("sleep")
            .arg("600")
            .spawn()
            .expect("spawn sleep"),
    )
}

/// A pid that is guaranteed dead: spawn, kill, reap.
fn dead_pid() -> u32 {
    let mut c = sleeper();
    let pid = c.id();
    c.kill();
    pid
}

/// Write a session lease the way pre-driver binaries did: NO `driver` key
/// unless `driver` is given — old lease files must keep parsing.
fn write_lease(repo: &Path, id: &str, spec: &str, active_pid: u32, driver: Option<&str>) {
    write_lease_at(
        repo,
        &repo.join("no-such-worktree"),
        id,
        spec,
        active_pid,
        driver,
    );
}

fn write_lease_at(
    repo: &Path,
    worktree: &Path,
    id: &str,
    spec: &str,
    active_pid: u32,
    driver: Option<&str>,
) {
    let dir = repo.join(".aida").join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let slug = spec.to_ascii_lowercase();
    let mut lease = toml::map::Map::new();
    for (k, v) in [
        ("id", id.to_string()),
        ("scope", spec.to_string()),
        ("slug", slug.clone()),
        ("owner", "tester".to_string()),
        ("worktree_path", worktree.to_string_lossy().into_owned()),
        ("branch", slug),
        ("started_at", chrono::Utc::now().to_rfc3339()),
        ("hostname", "h".to_string()),
        ("role", "implementer".to_string()),
    ] {
        lease.insert(k.into(), toml::Value::String(v));
    }
    lease.insert("active_pid".into(), toml::Value::Integer(active_pid.into()));
    if let Some(d) = driver {
        lease.insert("driver".into(), toml::Value::String(d.into()));
    }
    let body = toml::to_string(&toml::Value::Table(lease)).unwrap();
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

/// Registered relay + spec in its batch + a dead lease pid on a lease that
/// names the relay → `relay:<run>` everywhere, the row reads live (no
/// stale/stalled wording). Killing the relay degrades the spec to
/// `queued (batch:X)` without deleting anything.
#[test]
fn relay_drives_batch_spec_then_degrades_to_queued_when_killed() {
    let (_tmp, repo, home) = init_repo();
    let spec = add_in_progress(&repo, &home, Some("batch:next"));
    write_lease(
        &repo,
        "aaaabbbbcccc",
        &spec,
        dead_pid(),
        Some("relay:lib-split"),
    );

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
    relay.kill();
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
    relay.kill();
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
    agent.kill();
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

/// A live relay that covers the spec only through its batch tag must NOT
/// vouch for a dead session: with uncommitted work in the dead session's
/// worktree the row still reads salvageable (and is not lifted to live),
/// while the driver label still names the relay.
#[test]
fn tag_covered_dead_session_with_dirty_worktree_stays_salvageable() {
    let (tmp, repo, home) = init_repo();
    std::fs::write(repo.join("work.txt"), "base\n").unwrap();
    git(&repo, &["add", "work.txt"]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    let spec = add_in_progress(&repo, &home, Some("batch:next"));
    let wt = tmp.path().canonicalize().unwrap().join("wt-dirty");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "dirty-branch",
            wt.to_str().unwrap(),
        ],
    );
    std::fs::write(wt.join("work.txt"), "uncommitted edit\n").unwrap();
    // Age the edit past the fresh-dirty-movement window, which would
    // otherwise read the tree as still moving.
    assert!(Command::new("touch")
        .args(["-d", "2 hours ago"])
        .arg(wt.join("work.txt"))
        .status()
        .expect("run touch")
        .success());
    write_lease_at(&repo, &wt, "5a1a5a1a5a1a", &spec, dead_pid(), None);

    let relay = sleeper();
    let pid = relay.id().to_string();
    let reg = aida(
        &repo,
        &home,
        &[
            "driver",
            "register",
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

    let (section, row) = ps_entry(&repo, &home, &spec);
    assert_eq!(section, "sessions", "{row}");
    assert_eq!(row["dispatch_state"], "salvageable", "{row}");
    assert_ne!(row["liveness"], "live", "a tag match never lifts: {row}");
    assert_eq!(row["driver"], "relay:lib-split", "{row}");
    drop(relay);
}

/// `AIDA_RELAY_RUN` in the environment of `aida claim` is recorded as the
/// minted lease's `driver` — the attribution is written by the CLI itself.
#[test]
fn relay_run_env_is_stamped_on_the_claim_lease() {
    let (_tmp, repo, home) = init_repo();
    let spec = add_in_progress(&repo, &home, None);
    let out = aida_env(
        &repo,
        &home,
        &["claim", &spec],
        &[("AIDA_RELAY_RUN", "nightly")],
    );
    assert!(
        out.status.success(),
        "claim: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let dir = repo.join(".aida").join("sessions");
    let leases: Vec<toml::Value> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|b| toml::from_str(&b).ok())
        .filter(|v: &toml::Value| v.get("scope").and_then(|s| s.as_str()) == Some(spec.as_str()))
        .collect();
    assert_eq!(leases.len(), 1, "one claim lease: {leases:?}");
    assert_eq!(
        leases[0].get("driver").and_then(|d| d.as_str()),
        Some("relay:nightly"),
        "{:?}",
        leases[0]
    );
}

/// A tag-covered spec whose session crashed on a clean worktree: the row is
/// not lifted (a tag match never vouches for a dead session), so it stays
/// stale and is hidden by default — but the spec must still be visible in
/// default `aida ps`, under "Driven", and counted as driven (not orphaned)
/// in the TOON scalars.
#[test]
fn tag_covered_crashed_session_is_listed_under_driven_by_default() {
    let (tmp, repo, home) = init_repo();
    let spec = add_in_progress(&repo, &home, Some("batch:next"));
    let wt = tmp.path().canonicalize().unwrap().join("wt-clean");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "clean-branch",
            wt.to_str().unwrap(),
        ],
    );
    write_lease_at(&repo, &wt, "c1ea5c1ea5c1", &spec, dead_pid(), None);

    let relay = sleeper();
    let pid = relay.id().to_string();
    let reg = aida(
        &repo,
        &home,
        &[
            "driver",
            "register",
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

    let human = aida(&repo, &home, &["ps", "--format", "human"]);
    let text = String::from_utf8_lossy(&human.stdout);
    let driven = text
        .split("Driven")
        .nth(1)
        .unwrap_or_else(|| panic!("no Driven section in default ps: {text}"));
    assert!(
        driven.contains(&spec) && driven.contains("relay:lib-split"),
        "{text}"
    );
    assert!(!text.contains("Orphaned"), "{text}");

    let toon = aida(&repo, &home, &["ps", "--format", "toon"]);
    let toon = String::from_utf8_lossy(&toon.stdout);
    assert!(toon.lines().any(|l| l == "orphaned: 0"), "{toon}");
    assert!(toon.lines().any(|l| l == "driven: 1"), "{toon}");
    drop(relay);
}

/// Register a relay over `batch:next` with the given extra args, asserting
/// success. Runs from `cwd` (the primary checkout or a linked worktree).
fn register_relay(cwd: &Path, home: &Path, run: &str, pid: u32, extra: &[&str]) {
    let pid = pid.to_string();
    let mut args = vec![
        "driver",
        "register",
        "--run",
        run,
        "--tag",
        "batch:next",
        "--pid",
        pid.as_str(),
    ];
    args.extend_from_slice(extra);
    let reg = aida(cwd, home, &args);
    assert!(
        reg.status.success(),
        "register: {}",
        String::from_utf8_lossy(&reg.stderr)
    );
}

/// Move a record's heartbeat `secs` into the past — equivalent to resolving
/// at `now = heartbeat + secs` without a clock seam in the binary.
fn backdate_heartbeat(repo: &Path, run: &str, secs: i64) {
    let path = repo
        .join(".aida")
        .join("drivers")
        .join(format!("relay-{run}.json"));
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read record")).unwrap();
    let then = (chrono::Utc::now() - chrono::Duration::seconds(secs)).to_rfc3339();
    v["heartbeat_at"] = then.clone().into();
    v["started_at"] = then.into();
    std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

/// The relay registers once per claude session (30–90 min) with no `--ttl`:
/// an hour after its last heartbeat, with its pid still alive, it must still
/// drive the spec, and a lease naming it whose own pid is dead stays lifted
/// out of stalled.
// trace:STORY-1653 | ai:claude
#[test]
fn relay_without_ttl_stays_live_an_hour_past_its_heartbeat() {
    let (_tmp, repo, home) = init_repo();
    let spec = add_in_progress(&repo, &home, Some("batch:next"));
    write_lease(
        &repo,
        "11112222aaaa",
        &spec,
        dead_pid(),
        Some("relay:lib-split"),
    );
    let relay = sleeper();
    register_relay(&repo, &home, "lib-split", relay.id(), &[]);
    backdate_heartbeat(&repo, "lib-split", 60 * 60);

    let listed = json(
        &aida(&repo, &home, &["driver", "list", "--json"]),
        "driver list",
    );
    assert_eq!(listed["drivers"][0]["live"], true, "{listed}");
    let (section, row) = ps_entry(&repo, &home, &spec);
    assert_eq!(section, "sessions", "{row}");
    assert_eq!(row["driver"], "relay:lib-split", "{row}");
    assert_eq!(row["liveness"], "live", "still lifted: {row}");
    assert!(row["dispatch_state"].is_null(), "no stalled framing: {row}");
    assert_eq!(list_row(&repo, &home, &spec)["driver"], "relay:lib-split");
    drop(relay);
}

/// An explicit `--ttl` still expires the record on its heartbeat even while
/// the pid is alive: the spec falls back to `queued (batch:X)`.
// trace:STORY-1653 | ai:claude
#[test]
fn explicit_ttl_expires_a_live_pid_record() {
    let (_tmp, repo, home) = init_repo();
    let spec = add_in_progress(&repo, &home, Some("batch:next"));
    let relay = sleeper();
    register_relay(&repo, &home, "wedged", relay.id(), &["--ttl", "60"]);
    assert_eq!(
        list_row(&repo, &home, &spec)["driver"],
        "relay:wedged",
        "fresh heartbeat within --ttl is live"
    );
    backdate_heartbeat(&repo, "wedged", 120);
    let listed = json(
        &aida(&repo, &home, &["driver", "list", "--json"]),
        "driver list",
    );
    assert_eq!(listed["drivers"][0]["live"], false, "{listed}");
    assert_eq!(listed["drivers"][0]["ttl_secs"], 60, "{listed}");
    let lrow = list_row(&repo, &home, &spec);
    assert_eq!(lrow["driver"], "queued (batch:next)", "{lrow}");
    drop(relay);
}

/// A driver registered from inside a linked worktree lands in the MAIN
/// checkout's registry (like session leases), so `aida ps` / `aida list` /
/// `aida driver list` from the primary checkout see it, and `deregister`
/// from the worktree removes it.
// trace:STORY-1653 | ai:claude
#[test]
fn driver_registered_from_linked_worktree_is_seen_from_primary_checkout() {
    let (tmp, repo, home) = init_repo();
    let spec = add_in_progress(&repo, &home, Some("batch:next"));
    let wt = tmp.path().canonicalize().unwrap().join("wt-relay");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "relay-branch",
            wt.to_str().unwrap(),
        ],
    );
    let relay = sleeper();
    register_relay(&wt, &home, "from-wt", relay.id(), &[]);

    let record = repo
        .join(".aida")
        .join("drivers")
        .join("relay-from-wt.json");
    assert!(record.exists(), "record lands under the main checkout");
    assert!(
        !wt.join(".aida").join("drivers").exists(),
        "nothing is written under the linked worktree"
    );
    let listed = json(
        &aida(&repo, &home, &["driver", "list", "--json"]),
        "driver list",
    );
    assert_eq!(listed["drivers"][0]["driver"], "relay:from-wt", "{listed}");
    assert_eq!(listed["drivers"][0]["live"], true, "{listed}");
    assert_eq!(list_row(&repo, &home, &spec)["driver"], "relay:from-wt");
    let (_, row) = ps_entry(&repo, &home, &spec);
    assert_eq!(row["driver"], "relay:from-wt", "{row}");

    let dereg = aida(&wt, &home, &["driver", "deregister", "--run", "from-wt"]);
    assert!(
        dereg.status.success(),
        "deregister: {}",
        String::from_utf8_lossy(&dereg.stderr)
    );
    assert!(!record.exists(), "deregister from the worktree hits main");
    drop(relay);
}
