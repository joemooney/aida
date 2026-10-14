#![cfg(target_os = "linux")]
//! Black-box acceptance for BUG-1956: the reader-side fallbacks BUG-1674 left
//! on the full SQLite retry ladder stay inside ADR-53's 5000 ms ceiling while
//! another process holds the cache write lock on a stale cache.
//!
//! Telemetry (2026-10-11 07:18-07:20Z) showed six `show`, two `list` and one
//! `comment list` calls each taking ~25.5 s (exactly the retry ladder) and
//! exiting 1 while one `aida edit` held the SQLite write lock for 157 s. The
//! refresh workers had already failed, so readers took the inline fallback
//! (spawning suppressed) and parked in the ladder. Each test here holds
//! `BEGIN IMMEDIATE` on cache.db from a separate connection, as BUG-1674's
//! tests do. The measured reads keep the production retry count but stretch
//! each ladder step (`AIDA_CACHE_RETRY_MS`), so a regression that rides the
//! uncapped ladder runs into the hard ceiling.
// trace:BUG-1956 | ai:claude
// trace:BUG-1995 | ai:antigravity

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// ADR-53's interaction ceiling. It is a performance budget, reported on
/// stderr when missed, never fatal: no in-test signal can prove the host was
/// quiet while the call ran (see `aida-cli-lib/src/test_timing.rs`).
// trace:BUG-1995 | ai:antigravity
const BUDGET: Duration = Duration::from_secs(5);

/// The fatal bound. A correct reader fallback stays far below it even when the
/// host is oversubscribed; a command that waits for a held lock or rides the
/// stretched ladder cannot finish before it.
// trace:BUG-1995 | ai:antigravity
const HARD_CEILING: Duration = Duration::from_secs(60);

/// Each step of the SQLite retry ladder during the measured call. It is twice
/// [`HARD_CEILING`], so a single uncapped ladder sleep (the ~25.5 s shape
/// BUG-1956 fixed) cannot finish before the ceiling.
// trace:BUG-1995 | ai:antigravity
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

fn aida_env(fixture: &Fixture, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(&fixture.repo)
        .env("HOME", &fixture.home)
        .env("AIDA_TELEMETRY", "0")
        .env_remove("AIDA_CACHE_RETRY_COUNT")
        .env_remove("AIDA_CACHE_RETRY_MS")
        .env_remove("AIDA_CACHE_NO_DETACH")
        .env_remove("AIDA_CACHE_FAST_FAIL")
        .env_remove("AIDA_CACHE_READ_WAIT_MS")
        .args(args);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.output().expect("run aida")
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
    assert!(git(&repo, &["config", "user.name", "BUG-1956 Test"])
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
    let init = aida_env(
        &fixture,
        &[
            "init",
            "--force",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ],
        &[],
    );
    assert!(
        init.status.success(),
        "init failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&init.stdout),
        String::from_utf8_lossy(&init.stderr)
    );
    let add = aida_env(
        &fixture,
        &["add", "bounded show seed", "--type", "task"],
        &[],
    );
    assert!(add.status.success(), "add failed");
    // A read builds the cache so every test starts from a committed snapshot.
    assert!(aida_env(&fixture, &["list"], &[]).status.success());
    fixture
}

fn store_path(fixture: &Fixture) -> PathBuf {
    fixture.repo.join(".aida-store")
}

fn cache_path(fixture: &Fixture) -> PathBuf {
    aida_core::CachedGitBackend::default_cache_path(&store_path(fixture))
}

/// Rewrite the store's last commit behind the cache's back, so the recorded
/// cache head is no longer an ancestor. No incremental refresh applies (its
/// one-attempt path was already bounded by BUG-1674); the reader reaches the
/// inline full-rebuild fallbacks this bug is about.
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

/// Record three failed refresh workers in the last 600 s, in the real
/// refresh-request sidecar, so readers stop spawning and take the inline path
/// (amendment A7): the state the overnight refresh log showed.
fn suppress_spawning(fixture: &Fixture) {
    let cache = cache_path(fixture);
    aida_core::db::refresh_request::file_request(&cache, "unused").unwrap();
    for _ in 0..aida_core::db::refresh_request::SUPPRESS_AFTER_FAILURES {
        aida_core::db::refresh_request::record_failed_attempt(
            &cache,
            "refresh worker exceeded its runtime cap",
        )
        .unwrap();
    }
    let request = aida_core::db::refresh_request::load(&cache).expect("request filed");
    assert!(request.spawning_suppressed(chrono::Utc::now()));
}

/// Hold the SQLite write lock the way a long `aida edit` does.
fn hold_writer(fixture: &Fixture) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(cache_path(fixture)).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    conn
}

/// The 1-minute load average, for the budget report only. The item-level
/// cfg is what the portability ratchet keys on, so it stays.
// trace:BUG-1995 | ai:antigravity
#[cfg(target_os = "linux")]
fn load_average() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Run `aida` with the retry ladder stretched, killing it at [`HARD_CEILING`].
/// Returns the output and the wall-clock it took.
// trace:BUG-1995 | ai:antigravity
fn timed_aida(
    fixture: &Fixture,
    args: &[&str],
    envs: &[(&str, &str)],
    context: &str,
) -> (Output, Duration) {
    let out_path = fixture.home.join("aida.stdout");
    let err_path = fixture.home.join("aida.stderr");
    let start = Instant::now();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(&fixture.repo)
        .env("HOME", &fixture.home)
        .env("AIDA_TELEMETRY", "0")
        .env_remove("AIDA_CACHE_RETRY_COUNT")
        .env_remove("AIDA_CACHE_NO_DETACH")
        .env_remove("AIDA_CACHE_FAST_FAIL")
        .env_remove("AIDA_CACHE_READ_WAIT_MS")
        .env("AIDA_CACHE_RETRY_MS", STRETCHED_LADDER_STEP_MS.to_string())
        .args(args);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let mut child = cmd
        .stdout(std::process::Stdio::from(
            std::fs::File::create(&out_path).unwrap(),
        ))
        .stderr(std::process::Stdio::from(
            std::fs::File::create(&err_path).unwrap(),
        ))
        .spawn()
        .expect("spawn aida");
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll aida") {
            break status;
        }
        if start.elapsed() >= HARD_CEILING {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "{context}: {args:?} was still running after the {HARD_CEILING:?} hard \
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

/// Run a read, asserting the hard ceiling, exit 0, and the writer-busy stale note;
/// report a missed [`BUDGET`]. Returns stdout.
// trace:BUG-1995 | ai:antigravity
fn read_bounded_and_labelled(
    fixture: &Fixture,
    args: &[&str],
    envs: &[(&str, &str)],
    context: &str,
) -> String {
    let (out, elapsed) = timed_aida(fixture, args, envs, context);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    // trace:BUG-1995 | ai:antigravity
    if elapsed >= BUDGET {
        eprintln!(
            "BUDGET-MISSED {context}: {args:?} took {elapsed:?}, over its {BUDGET:?} budget \
             but inside the {HARD_CEILING:?} ceiling (1-minute load {}). Reported, not \
             failed: a busy host explains this; a lock wait would have hit the ceiling. \
             trace:BUG-1995",
            load_average()
        );
    }
    assert!(
        out.status.success(),
        "{context}: {args:?} failed\nstdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("bounded show seed"),
        "{context}: the read must still serve the spec\n{stdout}"
    );
    assert!(
        stderr.contains("the cache write lock was busy"),
        "{context}: missing the writer-busy stale note on stderr\n{stderr}"
    );
    stdout
}

/// Once the writer is gone, the next show brings the cache current.
fn show_refreshes(fixture: &Fixture, envs: &[(&str, &str)], context: &str) {
    let out = aida_env(fixture, &["show", "TASK-1"], envs);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "{context}: show failed after release\n{stderr}"
    );
    assert!(
        !stdout.contains("stale: true") && !stderr.contains("note: showing results cached at"),
        "{context}: show must refresh once the writer is gone\nstdout={stdout}\nstderr={stderr}"
    );
}

/// (a) The telemetry shape: refresh workers kept failing, so readers stopped
/// spawning and refreshed inline, parking in the ~25.5 s ladder.
// trace:BUG-1956 | ai:claude
#[test]
fn reads_are_bounded_when_spawning_is_suppressed() {
    let fixture = init_fixture();
    make_store_rewritten(&fixture);
    suppress_spawning(&fixture);
    let conn = hold_writer(&fixture);
    read_bounded_and_labelled(&fixture, &["show", "TASK-1"], &[], "suppressed show");
    read_bounded_and_labelled(&fixture, &["list"], &[], "suppressed list");
    conn.execute_batch("ROLLBACK").unwrap();
    drop(conn);
    show_refreshes(&fixture, &[], "suppressed");
}

/// (b) `AIDA_CACHE_NO_DETACH=1` refreshes inline instead of detaching.
// trace:BUG-1956 | ai:claude
#[test]
fn show_is_bounded_with_no_detach() {
    let fixture = init_fixture();
    make_store_rewritten(&fixture);
    let envs = [("AIDA_CACHE_NO_DETACH", "1")];
    let conn = hold_writer(&fixture);
    read_bounded_and_labelled(&fixture, &["show", "TASK-1"], &envs, "no-detach show");
    conn.execute_batch("ROLLBACK").unwrap();
    drop(conn);
    show_refreshes(&fixture, &envs, "no-detach");
}

/// (c) A missing `schema_version` stamp makes `Cache::open` write before any
/// read. With usable tables that stamp is bookkeeping and must not wait out
/// the ladder behind a live writer.
// trace:BUG-1956 | ai:claude
#[test]
fn show_is_bounded_when_the_schema_stamp_is_missing() {
    let fixture = init_fixture();
    {
        let conn = rusqlite::Connection::open(cache_path(&fixture)).unwrap();
        let removed = conn
            .execute("DELETE FROM cache_meta WHERE key = 'schema_version'", [])
            .unwrap();
        assert_eq!(removed, 1, "fixture shape changed: no schema stamp");
    }
    make_store_rewritten(&fixture);
    // No detach keeps the run hermetic (no stray worker against the lock).
    let envs = [("AIDA_CACHE_NO_DETACH", "1")];
    let conn = hold_writer(&fixture);
    read_bounded_and_labelled(&fixture, &["show", "TASK-1"], &envs, "missing stamp");
    conn.execute_batch("ROLLBACK").unwrap();
    drop(conn);
    show_refreshes(&fixture, &envs, "missing stamp");
}

/// A mutation keeps the full ladder: with a schema migration pending (no
/// compatible snapshot) and a writer that outlasts the read cap, `aida edit`
/// waits for the writer instead of failing at the read cap.
// trace:BUG-1956 | ai:claude
#[test]
fn edit_outlasts_a_writer_when_no_compatible_snapshot_exists() {
    let fixture = init_fixture();
    rusqlite::Connection::open(cache_path(&fixture))
        .unwrap()
        .execute(
            "UPDATE cache_meta SET value = '1' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    make_store_rewritten(&fixture);
    let conn = hold_writer(&fixture);
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(4));
        conn.execute_batch("ROLLBACK").unwrap();
    });
    let start = Instant::now();
    let out = aida_env(
        &fixture,
        &["edit", "TASK-1", "--title", "edited behind a writer"],
        &[("AIDA_CACHE_NO_DETACH", "1")],
    );
    let elapsed = start.elapsed();
    release.join().unwrap();
    assert!(
        out.status.success(),
        "edit must wait out the writer, not fail at the read cap ({elapsed:?})\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        elapsed >= Duration::from_secs(3),
        "edit must have waited for the writer: {elapsed:?}"
    );
    let show = aida_env(&fixture, &["show", "TASK-1"], &[]);
    assert!(String::from_utf8_lossy(&show.stdout).contains("edited behind a writer"));
}
