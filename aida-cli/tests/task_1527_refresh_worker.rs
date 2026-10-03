#![cfg(target_os = "linux")]
//! Black-box acceptance for STORY-1484 slice C (TASK-1527): the durable
//! refresh request and the detached refresh worker.
//!
//! Story acceptance #3 is the load-bearing test here: killing the worker
//! mid-refresh leaves the previous committed cache readable and the request
//! file present, and the next `--if-requested` run completes the refresh. The
//! kill is made deterministic by holding `BEGIN IMMEDIATE` on the fixture
//! cache, which parks the worker inside the SQLite retry ladder (~25 s of
//! budget) while the signal lands.
// trace:TASK-1527 | ai:claude

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
    aida_command(fixture, args).output().expect("run aida")
}

fn aida_command(fixture: &Fixture, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aida"));
    command
        .current_dir(&fixture.repo)
        .env("HOME", &fixture.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .args(args);
    command
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
    assert!(git(&repo, &["config", "user.name", "TASK-1527 Test"])
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

fn store_path(fixture: &Fixture) -> PathBuf {
    fixture.repo.join(".aida-store")
}

fn cache_path(fixture: &Fixture) -> PathBuf {
    aida_core::CachedGitBackend::default_cache_path(&store_path(fixture))
}

/// Move the store HEAD without touching the cache: the staleness every test
/// here starts from, created the way a foreign writer creates it.
fn make_store_stale(fixture: &Fixture) {
    let store = store_path(fixture);
    assert!(
        git(&store, &["commit", "-q", "--allow-empty", "-m", "external"])
            .status
            .success(),
        "external store commit failed"
    );
}

fn store_head(fixture: &Fixture) -> String {
    let out = git(&store_path(fixture), &["rev-parse", "HEAD"]);
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn seeded_fixture() -> Fixture {
    let fixture = init_fixture();
    let add = aida(&fixture, &["add", "Durable refresh seed", "--type", "task"]);
    assert!(add.status.success(), "add failed");
    // A read builds the cache so every test starts from a committed snapshot.
    assert!(aida(&fixture, &["list"]).status.success());
    fixture
}

/// Story acceptance #3. The worker is killed while it owns the refresh flock
/// and is waiting in the SQLite write ladder; the previous committed cache
/// stays readable, the request survives, and the next schedule-tick run
/// (`--if-requested`) finishes the job and clears it.
// trace:TASK-1527 | ai:claude
#[test]
fn killed_worker_leaves_cache_readable_and_request_present_then_next_run_completes() {
    let fixture = seeded_fixture();
    let store = store_path(&fixture);
    let cache = cache_path(&fixture);
    make_store_stale(&fixture);
    let target = store_head(&fixture);
    aida_core::db::refresh_request::file_request(&cache, &target).unwrap();

    // Park every cache writer: BEGIN IMMEDIATE holds SQLite's write lock while
    // WAL readers keep flowing.
    let conn = rusqlite::Connection::open(&cache).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();

    let mut worker = aida_command(
        &fixture,
        &[
            "cache",
            "refresh",
            "--worker",
            "--store",
            store.to_str().unwrap(),
            "--cache",
            cache.to_str().unwrap(),
        ],
    )
    .spawn()
    .expect("spawn worker");
    // Give it time to take the flock and enter the ladder (~25 s of budget
    // remains, so this cannot race the ladder's end), then OBSERVE the flock
    // is actually held before killing it there (codex review: without this
    // observation the test would still pass if acquisition were removed).
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert!(
        aida_core::db::cache_refresh::RefreshLock::try_acquire(&cache)
            .unwrap()
            .is_none(),
        "the worker must hold the refresh flock when the kill lands"
    );
    worker.kill().expect("kill worker");
    let status = worker.wait().unwrap();
    assert!(
        !status.success(),
        "the worker must die by signal, not finish"
    );
    // The kernel released the dead worker's flock: no wedge for the next run.
    assert!(
        aida_core::db::cache_refresh::RefreshLock::try_acquire(&cache)
            .unwrap()
            .is_some(),
        "a dead worker must not leave the refresh flock held"
    );

    // Previous committed cache: readable. Request: present.
    assert!(
        aida_core::db::refresh_request::load(&cache).is_some(),
        "the request file must survive a dead worker"
    );
    let list = aida(&fixture, &["list"]);
    assert!(
        list.status.success(),
        "a read after a killed worker must serve the committed snapshot:\n{}",
        String::from_utf8_lossy(&list.stderr)
    );

    conn.execute_batch("ROLLBACK").unwrap();

    // The next tick completes the refresh and clears the request (A7).
    let tick = aida(&fixture, &["cache", "refresh", "--if-requested"]);
    assert!(
        tick.status.success(),
        "--if-requested failed:\n{}",
        String::from_utf8_lossy(&tick.stderr)
    );
    assert!(
        aida_core::db::refresh_request::load(&cache).is_none(),
        "a completed refresh at the current head must clear the request"
    );
    let status_out = aida(&fixture, &["cache", "status"]);
    let text = String::from_utf8_lossy(&status_out.stdout).to_string();
    assert!(
        text.contains("FRESH"),
        "cache must be fresh after the tick:\n{text}"
    );
}

/// `--if-requested` with no pending request is a silent no-op, so an enabled
/// schedule job trains nobody to ignore it.
// trace:TASK-1527 | ai:claude
#[test]
fn if_requested_without_a_request_is_a_silent_noop() {
    let fixture = seeded_fixture();
    let out = aida(&fixture, &["cache", "refresh", "--if-requested"]);
    assert!(out.status.success());
    assert!(
        out.stdout.is_empty(),
        "no-request tick must print nothing, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// Plain `aida cache refresh` refreshes strictly inline and clears a pending
/// request — the operator's recovery verb when the backoff has suppressed
/// spawning.
// trace:TASK-1527 | ai:claude
#[test]
fn plain_refresh_freshens_and_clears_the_request() {
    let fixture = seeded_fixture();
    let cache = cache_path(&fixture);
    make_store_stale(&fixture);
    aida_core::db::refresh_request::file_request(&cache, &store_head(&fixture)).unwrap();
    let out = aida(&fixture, &["cache", "refresh"]);
    assert!(
        out.status.success(),
        "refresh failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(aida_core::db::refresh_request::load(&cache).is_none());
    let status_out = aida(&fixture, &["cache", "status"]);
    assert!(String::from_utf8_lossy(&status_out.stdout).contains("FRESH"));
}

/// Story acceptance #5: `aida cache status` shows the pending request, the
/// worker failures, the suppression stand-down, and the refresh-lock holder.
// trace:TASK-1527 | ai:claude
#[test]
fn cache_status_shows_request_failures_suppression_and_lock_holder() {
    let fixture = seeded_fixture();
    let cache = cache_path(&fixture);
    aida_core::db::refresh_request::file_request(&cache, "feedfacefeedface").unwrap();
    for n in 0..3 {
        aida_core::db::refresh_request::record_failed_attempt(&cache, &format!("boom {n}"))
            .unwrap();
    }
    let out = aida(&fixture, &["cache", "status"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        text.contains("Refresh request:  pending for feedfacefeedface"),
        "missing pending request line:\n{text}"
    );
    assert!(
        text.contains("3 failed attempt(s), last: boom 2"),
        "missing failures line:\n{text}"
    );
    assert!(
        text.contains("SUPPRESSED after repeated worker failures"),
        "missing suppression line:\n{text}"
    );
    assert!(
        text.contains("Refresh lock:     free"),
        "lock line:\n{text}"
    );

    // Hold the flock from this process: status (another process) must call
    // it held.
    let _guard = aida_core::db::cache_refresh::RefreshLock::try_acquire(&cache)
        .unwrap()
        .expect("acquire refresh flock");
    let held = aida(&fixture, &["cache", "status"]);
    let text = String::from_utf8_lossy(&held.stdout).to_string();
    assert!(
        text.contains("held (a refresh is running)"),
        "missing held lock line:\n{text}"
    );
}

/// A suppressed request (three failed attempts in the window) makes the tick
/// report and stand down instead of spawning a fourth worker (A7).
// trace:TASK-1527 | ai:claude
#[test]
fn suppressed_request_makes_the_tick_stand_down() {
    let fixture = seeded_fixture();
    let cache = cache_path(&fixture);
    make_store_stale(&fixture);
    aida_core::db::refresh_request::file_request(&cache, &store_head(&fixture)).unwrap();
    for n in 0..3 {
        aida_core::db::refresh_request::record_failed_attempt(&cache, &format!("crash {n}"))
            .unwrap();
    }
    let out = aida(&fixture, &["cache", "refresh", "--if-requested"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        text.contains("suppressed after repeated worker failures"),
        "the stand-down must be visible to the tick log:\n{text}"
    );
    assert!(
        aida_core::db::refresh_request::load(&cache).is_some(),
        "standing down must not clear the request"
    );
}
