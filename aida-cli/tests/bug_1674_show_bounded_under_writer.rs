#![cfg(target_os = "linux")]
//! Black-box acceptance for BUG-1674: `aida show <ID>` returns in under
//! 5000 ms wall-clock while another process holds the cache write lock or
//! the refresh flock on a stale cache — serving either the authoritative
//! YAML (the spec body itself) or a stale-labelled committed snapshot per
//! STORY-1484's reader protocol. It must never wait out the ~25 s SQLite
//! retry ladder, and it must never print a status that contradicts the
//! authoritative YAML without a stale label.
//!
//! The foreign writer is simulated the way TASK-1527's tests do it: a
//! `BEGIN IMMEDIATE` transaction on the shared cache parks every cache
//! writer while WAL readers keep flowing.
//!
//! How the bound is asserted (BUG-1967). A bare `elapsed < 5 s` measured the
//! host as much as the code: at load ~60 on 6 cores a correct `show` (which
//! never waits on the writer) still took 5-10 s just queueing for a CPU. So
//! the 5 s figure is a reported *budget*, and what fails a run is structural:
//!
//! * every lock the `show` could wait on is held for the whole call, so a
//!   `show` that waits for a holder never returns on its own; and
//! * the measured call runs with the SQLite retry ladder stretched to
//!   [`STRETCHED_LADDER_STEP_MS`] per step, so a `show` that rides the
//!   uncapped ladder sleeps at least that long before its first retry.
//!
//! Either regression therefore runs into [`HARD_CEILING`], which kills the
//! child and fails the test. A correct `show` sleeps only inside its bounded
//! read waits (the zero refresh-flock budget plus BUG-1956's 3 s ladder
//! allowance), so it can reach the ceiling only through an order-of-magnitude
//! scheduler stall. That separation holds under any plausible host load.
// trace:BUG-1674 | ai:claude
// trace:BUG-1967 | ai:claude
// trace:BUG-1995 | ai:antigravity

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// ADR-53's interaction ceiling. It is a performance budget, reported on
/// stderr when missed, never fatal: no in-test signal can prove the host was
/// quiet while the call ran (see `aida-cli-lib/src/test_timing.rs`).
// trace:BUG-1967 | ai:claude
const BUDGET: Duration = Duration::from_secs(5);

/// The fatal bound. A correct `show` stays far below it even when the host
/// is oversubscribed; a `show` that waits for a held lock or rides the
/// stretched ladder cannot finish before it.
// trace:BUG-1967 | ai:claude
const HARD_CEILING: Duration = Duration::from_secs(60);

/// Each step of the SQLite retry ladder during the measured `show`. It is
/// twice [`HARD_CEILING`], so a single uncapped ladder sleep (the ~25 s
/// shape BUG-1674 fixed) cannot finish before the ceiling.
// trace:BUG-1967 | ai:claude
const STRETCHED_LADDER_STEP_MS: u64 = 120_000;

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
    assert!(git(&repo, &["config", "user.name", "BUG-1674 Test"])
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
    let add = aida(&fixture, &["add", "bounded show seed", "--type", "task"]);
    assert!(add.status.success(), "add failed");
    // A read builds the cache so every test starts from a committed snapshot.
    assert!(aida(&fixture, &["list"]).status.success());
    fixture
}

fn store_path(fixture: &Fixture) -> PathBuf {
    fixture.repo.join(".aida-store")
}

fn cache_path(fixture: &Fixture) -> PathBuf {
    aida_core::CachedGitBackend::default_cache_path(&store_path(fixture))
}

/// Move the store HEAD without touching the cache, the way a foreign writer
/// does. An incremental refresh can still cover this shape.
fn make_store_stale(fixture: &Fixture) {
    assert!(git(
        &store_path(fixture),
        &["commit", "-q", "--allow-empty", "-m", "external"]
    )
    .status
    .success());
}

/// Rewrite the store's last commit so the recorded cache head is no longer
/// an ancestor: the next refresh needs a FULL rebuild (the shape that used
/// to ride the whole retry ladder).
fn make_store_rewritten(fixture: &Fixture) {
    assert!(git(
        &store_path(fixture),
        &[
            "commit",
            "-q",
            "--allow-empty",
            "--amend",
            "-m",
            "rewritten"
        ]
    )
    .status
    .success());
}

/// The 1-minute load average, for the budget report only.
#[cfg(target_os = "linux")]
fn load_average() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Run `aida show TASK-1` with the retry ladder stretched, killing it at
/// [`HARD_CEILING`]. Returns the output and the wall-clock it took.
// trace:BUG-1967 | ai:claude
fn timed_show(fixture: &Fixture, context: &str) -> (Output, Duration) {
    let out_path = fixture.home.join("show.stdout");
    let err_path = fixture.home.join("show.stderr");
    let start = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(&fixture.repo)
        .env("HOME", &fixture.home)
        .env("AIDA_TELEMETRY", "0")
        // The ladder must be the production one apart from its step length:
        // an inherited retry count of 0 would hide a ladder regression.
        .env_remove("AIDA_CACHE_RETRY_COUNT")
        .env_remove("AIDA_CACHE_FAST_FAIL")
        .env_remove("AIDA_CACHE_READ_WAIT_MS")
        .env("AIDA_CACHE_RETRY_MS", STRETCHED_LADDER_STEP_MS.to_string())
        .args(["show", "TASK-1"])
        .stdout(Stdio::from(std::fs::File::create(&out_path).unwrap()))
        .stderr(Stdio::from(std::fs::File::create(&err_path).unwrap()))
        .spawn()
        .expect("spawn aida show");
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll aida show") {
            break status;
        }
        if start.elapsed() >= HARD_CEILING {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "{context}: show was still running after the {HARD_CEILING:?} hard \
                 ceiling (1-minute load {}). It waited for a held lock or rode the \
                 uncapped retry ladder; no host load explains this.\nstdout={}\nstderr={}",
                load_average(),
                std::fs::read_to_string(&out_path).unwrap_or_default(),
                std::fs::read_to_string(&err_path).unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let elapsed = start.elapsed();
    let output = Output {
        status,
        stdout: std::fs::read(&out_path).unwrap(),
        stderr: std::fs::read(&err_path).unwrap(),
    };
    (output, elapsed)
}

/// Run `aida show TASK-1`, asserting the hard ceiling, exit 0, and the stale
/// label on the machine (TOON) surface; report a missed [`BUDGET`]. Returns
/// stdout.
fn show_bounded_and_labelled(fixture: &Fixture, context: &str) -> String {
    let (out, elapsed) = timed_show(fixture, context);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    // trace:BUG-1967 | ai:claude
    if elapsed >= BUDGET {
        eprintln!(
            "BUDGET-MISSED {context}: show took {elapsed:?}, over its {BUDGET:?} budget \
             but inside the {HARD_CEILING:?} ceiling (1-minute load {}). Reported, not \
             failed: a busy host explains this; a lock wait would have hit the ceiling. \
             trace:BUG-1967",
            load_average()
        );
    }
    assert!(
        out.status.success(),
        "{context}: show failed\nstdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("bounded show seed"),
        "{context}: show must still serve the spec\n{stdout}"
    );
    assert!(
        stdout.contains("stale: true"),
        "{context}: a read behind a held lock must carry the stale label\n{stdout}"
    );
    assert!(
        stderr.contains("note: showing results cached at"),
        "{context}: missing the stale note on stderr\n{stderr}"
    );
    stdout
}

/// Acceptance 1, incremental-able shape: stale cache + a live foreign writer
/// holding the SQLite write lock for at least 20 s.
// trace:BUG-1674.ac93280e | ai:claude
#[test]
fn show_is_bounded_behind_a_live_writer() {
    let fixture = init_fixture();
    make_store_stale(&fixture);
    let conn = rusqlite::Connection::open(cache_path(&fixture)).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    // The writer outlives the whole call: it is released only after the
    // assertions, so a show that waited for it would blow the ceiling.
    show_bounded_and_labelled(&fixture, "incremental shape");
    conn.execute_batch("ROLLBACK").unwrap();
}

/// Acceptance 1, full-rebuild shape: the recorded cache head is not an
/// ancestor of the store head, so no incremental path exists — this is the
/// shape that used to park in the ~25 s retry ladder and then fail.
// trace:BUG-1674.ac93280e | ai:claude
#[test]
fn show_is_bounded_when_the_stale_cache_needs_a_full_rebuild() {
    let fixture = init_fixture();
    make_store_rewritten(&fixture);
    let conn = rusqlite::Connection::open(cache_path(&fixture)).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    show_bounded_and_labelled(&fixture, "full-rebuild shape");
    conn.execute_batch("ROLLBACK").unwrap();
}

/// Acceptance 1, refresh-in-progress shape: a foreign holder owns the
/// refresh flock (a worker mid-refresh); the reader waits only its bounded
/// budget and then serves the labelled committed snapshot.
// trace:BUG-1674.ac93280e | ai:claude
#[test]
fn show_is_bounded_while_a_foreign_refresh_holds_the_flock() {
    let fixture = init_fixture();
    make_store_stale(&fixture);
    let cache = cache_path(&fixture);
    let _guard = aida_core::db::cache_refresh::RefreshLock::try_acquire(&cache)
        .unwrap()
        .expect("acquire the refresh flock as the foreign holder");
    show_bounded_and_labelled(&fixture, "flock-holder shape");
}

/// Acceptance 2: the spec body comes from authoritative YAML, so a status
/// changed by a foreign commit is shown correctly even while the cache is
/// stale and the write lock is held — never the cached value without a
/// label.
// trace:BUG-1674.ac08b40e | ai:claude
#[test]
fn show_serves_the_authoritative_status_behind_a_live_writer() {
    let fixture = init_fixture();
    let objects_root = store_path(&fixture).join("objects");
    let object = aida_core::object_store::object_path(&objects_root, "TASK-1")
        .expect("object path for TASK-1");
    let yaml = std::fs::read_to_string(&object).unwrap();
    assert!(
        yaml.contains("status: Draft"),
        "fixture shape changed:\n{yaml}"
    );
    std::fs::write(&object, yaml.replace("status: Draft", "status: Approved")).unwrap();
    let store = store_path(&fixture);
    assert!(git(&store, &["add", "."]).status.success());
    assert!(
        git(&store, &["commit", "-q", "-m", "external status change"])
            .status
            .success()
    );
    let conn = rusqlite::Connection::open(cache_path(&fixture)).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let stdout = show_bounded_and_labelled(&fixture, "authoritative status");
    assert!(
        stdout.contains("status: approved"),
        "show must display the authoritative YAML status, not the stale cached one:\n{stdout}"
    );
    conn.execute_batch("ROLLBACK").unwrap();
}
