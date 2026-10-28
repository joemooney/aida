// BUG-688: binary-driving e2e suites are Linux-gated like their siblings.
#![cfg(target_os = "linux")]
//! BUG-1993: `aida show` records per-phase timings in its usage event, so a
//! latency regression can be attributed to cache open, spec lookup, cache
//! context, or git linkage rather than only the total. Other commands record
//! no phases. BUG-b6b1278890774ef993d2ebc327862470-008 adds the bounds those
//! phases must keep: no unattributed residual, a bounded content walk, and
//! open/lookup/notice degrading behind a held writer lock.
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

fn phase_ms(record: &serde_json::Value, name: &str) -> u64 {
    record["phases"]
        .as_array()
        .and_then(|phases| phases.iter().find(|p| p["name"] == name))
        .and_then(|p| p["ms"].as_u64())
        .unwrap_or_else(|| panic!("no `{name}` phase: {record}"))
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

/// BUG-b6b1278890774ef993d2ebc327862470-008: every span of a `show` is
/// attributed to a named phase — the pre-dispatch hooks (`preflight`), the
/// hygiene guards (`guard`), the role-activity write (`activity`), backend
/// teardown (`close`), post-command work (`exit`) — and whatever no phase
/// measured lands in `residual`, so the top-level phases account for the
/// whole `duration_ms` (the live trip had ~21 s unattributed). The bounded
/// content-linkage walk still finds a traced source file on a healthy tree.
// trace:BUG-b6b1278890774ef993d2ebc327862470-008 | ai:claude
#[test]
fn show_phases_account_for_the_whole_duration_and_content_linkage_finds_traces() {
    let (_base, repo, home, spec) = fixture();
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(
        repo.join("src").join("lib.rs"),
        format!("// trace:{spec} | ai:claude\nfn traced_fixture() {{}}\n"),
    )
    .unwrap();

    for args in [
        vec!["show", spec.as_str(), "--format", "json"],
        vec!["show", spec.as_str(), "--format", "toon"],
    ] {
        let record = usage_record_of(&repo, &home, &args);
        let names = phase_names(&record);
        for named in [
            "preflight",
            "guard",
            "activity",
            "close",
            "exit",
            "residual",
        ] {
            assert!(
                names.iter().any(|n| n == named),
                "aida {args:?}: missing `{named}` phase: {record}"
            );
        }
        // `residual` is duration_ms minus every measured top-level phase, so
        // it is the unattributed time itself. A new unmeasured wait (the live
        // trip's ~21 s) lands there; a healthy fixture leaves only scheduling
        // noise. The bound is a share of the call, not a timing threshold, so
        // a slow host scales both sides.
        let duration = record["duration_ms"].as_u64().unwrap();
        let residual = phase_ms(&record, "residual");
        assert!(
            residual <= (duration / 2).max(250),
            "aida {args:?}: {residual} ms of duration_ms ({duration} ms) is \
             attributed to no named phase: {record}"
        );
        // (a): the show-only content walk stops at its 500 ms budget and
        // degrades to a partial file list, so on any host it stays far below
        // the 3 s shared budget that cost the live trip ~3 s per call.
        if args.contains(&"json") {
            let content = phase_ms(&record, "linkage.content");
            assert!(
                content < 2_000,
                "aida {args:?}: linkage.content took {content} ms: {record}"
            );
        }
    }

    let out = run_ok(aida(&repo, &home).args(["show", &spec, "--format", "json"]));
    let shown: serde_json::Value = serde_json::from_str(&out).expect("show JSON");
    let files = shown["git_linkage"]["files"]
        .as_array()
        .expect("files array");
    assert!(
        files.iter().any(|f| f["file"] == "src/lib.rs"),
        "content linkage missed the traced file: {}",
        shown["git_linkage"]
    );
}

/// BUG-b6b1278890774ef993d2ebc327862470-008 (b): every cache wait in `show`
/// degrades instead of blocking behind a concurrent writer. The shape is the
/// one where each wait really needs the writer's lock: the cache's schema
/// stamp is older than the binary (a migration is pending, so the snapshot
/// cannot be served without a rebuild) and another connection holds
/// `BEGIN IMMEDIATE` for the whole call, with the SQLite retry ladder
/// stretched so riding it cannot finish quickly. Each wait must degrade:
/// * open — the refresh fails fast and `show` opens the committed snapshot
///   (before: ~3 s of read allowance, then a "re-run shortly" failure);
/// * lookup — the spec is served from its authoritative object;
/// * notice — the advisory upstream check is skipped (its marker is not
///   recorded, so the next command checks again).
// trace:BUG-b6b1278890774ef993d2ebc327862470-008 | ai:claude
#[test]
fn show_open_lookup_and_notice_degrade_under_a_held_writer_lock() {
    let (_base, repo, home, spec) = fixture();
    let cache = aida_core::CachedGitBackend::default_cache_path(&repo.join(".aida-store"));
    let conn = rusqlite::Connection::open(&cache).expect("open cache");
    conn.execute(
        "UPDATE cache_meta SET value = '1' WHERE key = 'schema_version'",
        [],
    )
    .expect("age the schema stamp");
    let marker = repo.join(".aida").join("upstream-report-notice-version");
    let _ = std::fs::remove_file(&marker);
    conn.execute_batch("BEGIN IMMEDIATE")
        .expect("hold the writer lock");

    let log = home.join(".aida").join("usage.jsonl");
    let _ = std::fs::remove_file(&log);
    let out = aida(&repo, &home)
        .env_remove("AIDA_TELEMETRY")
        .env_remove("AIDA_CACHE_RETRY_COUNT")
        .env("AIDA_CACHE_RETRY_MS", "120000")
        .args(["show", &spec])
        .output()
        .expect("run aida show");
    let marker_written = marker.exists();
    conn.execute_batch("ROLLBACK").unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "open/lookup must degrade, not fail, behind the writer\nstdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("phase timing fixture"),
        "lookup must still serve the spec from its object\nstdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !marker_written,
        "the notice must skip itself behind the writer, not record a check"
    );
    let text = std::fs::read_to_string(&log).expect("usage.jsonl written");
    let record: serde_json::Value = serde_json::from_str(
        text.lines()
            .rev()
            .find(|l| l.contains("\"cmd\""))
            .expect("a usage record"),
    )
    .unwrap();
    // Structural, not a timing threshold: the stretched ladder makes a
    // single uncapped sleep 120 s, and the old capped path spent its full 3 s
    // read allowance in `open` alone.
    for wait in ["open", "lookup", "notice"] {
        let ms = phase_ms(&record, wait);
        assert!(
            ms < 3_000,
            "`{wait}` waited {ms} ms behind the writer: {record}"
        );
    }
}

/// BUG-b6b1278890774ef993d2ebc327862470-008 (b), the shape with nothing to
/// degrade to: the cache file has no schema yet and a writer holds its lock,
/// so the open must write before any row can be read. The open's schema
/// write rides show's fast-fail ladder and refuses with the usual
/// lock-holder guidance instead of the full retry ladder (stretched here so
/// a single uncapped sleep would blow the ceiling; the live binary took
/// ~16 s on the unstretched ladder).
///
/// Failing fast IS the ADR-53 outcome here, not a missed degrade: every
/// degrade path `show` has (the committed snapshot, the targeted object
/// read) runs on an open cache, and opening one means writing its schema —
/// the very write the writer blocks. With no schema there is no snapshot to
/// serve, so the bounded outcome is a prompt refusal naming the lock and
/// its remedy. The test pins the refusal inside ADR-53's 5 s ceiling (plus
/// a margin for process start-up on a loaded CI runner), so a 6-59 s
/// regression fails instead of passing under a loose watchdog.
// trace:BUG-b6b1278890774ef993d2ebc327862470-008 | ai:claude
// trace:TASK-b6b1278890774ef993d2ebc327862470-030 | ai:claude
#[test]
fn show_open_refuses_fast_when_a_writer_blocks_the_first_schema_write() {
    const ADR_53_CEILING: std::time::Duration = std::time::Duration::from_secs(5);
    const SPAWN_MARGIN: std::time::Duration = std::time::Duration::from_secs(1);
    let (_base, repo, home, spec) = fixture();
    let cache = aida_core::CachedGitBackend::default_cache_path(&repo.join(".aida-store"));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", cache.display()));
    }
    let conn = rusqlite::Connection::open(&cache).expect("create cache file");
    conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))
        .expect("WAL cache");
    conn.execute_batch("BEGIN IMMEDIATE")
        .expect("hold the writer lock");

    let started = std::time::Instant::now();
    let mut child = aida(&repo, &home)
        .env_remove("AIDA_CACHE_RETRY_COUNT")
        .env("AIDA_CACHE_RETRY_MS", "120000")
        .args(["show", &spec])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn aida show");
    while child.try_wait().expect("poll aida show").is_none() {
        if started.elapsed() >= ADR_53_CEILING + SPAWN_MARGIN {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "show was still waiting on the writer after {:?}, past ADR-53's \
                 {ADR_53_CEILING:?} ceiling",
                started.elapsed()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let out = child.wait_with_output().unwrap();
    conn.execute_batch("ROLLBACK").unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "no schema, no rows: {stdout}");
    assert!(
        stdout.contains("database is locked"),
        "the refusal names the lock and its remedy:\n{stdout}"
    );
}
