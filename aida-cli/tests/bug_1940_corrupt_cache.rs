#![cfg(target_os = "linux")]
//! Black-box coverage for a corrupt `.aida/cache.db`: `aida cache rebuild`
//! must not depend on the old file being readable (it moves the damaged file
//! and its sidecars aside, says where, and rebuilds from the git store), and a
//! read command / `aida cache verify` must print the one-line fix naming
//! `aida cache rebuild` instead of the raw sqlite error.
// trace:BUG-1940 | ai:claude

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap()
        .success());
}

fn aida(fixture: &Fixture, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(&fixture.repo)
        .env("HOME", &fixture.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_OUTPUT_FORMAT", "human")
        .args(args)
        .output()
        .expect("run aida")
}

fn text(out: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn ok(out: &Output) -> String {
    let (stdout, stderr) = text(out);
    assert!(out.status.success(), "stdout:\n{stdout}\nstderr:\n{stderr}");
    stdout
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let repo = base.join("repo");
    let home = base.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Corrupt Cache Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let fixture = Fixture {
        _tmp: tmp,
        repo,
        home,
    };
    ok(&aida(
        &fixture,
        &[
            "init",
            "--force",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ],
    ));
    ok(&aida(
        &fixture,
        &[
            "add",
            "--title",
            "searchable widget",
            "--type",
            "task",
            "--description",
            "d",
        ],
    ));
    ok(&aida(&fixture, &["cache", "rebuild"]));
    fixture
}

fn cache_path(fixture: &Fixture) -> PathBuf {
    fixture.repo.join(".aida/cache.db")
}

/// The reported shape: the FTS table's shadow config table is gone, so every
/// FTS access (and `PRAGMA integrity_check`) fails with "vtable constructor
/// failed: requirements_fts".
fn break_fts(fixture: &Fixture) {
    let conn = rusqlite::Connection::open(cache_path(fixture)).unwrap();
    conn.execute_batch("DROP TABLE requirements_fts_config")
        .unwrap();
    drop(conn);
}

fn quarantined(fixture: &Fixture) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(fixture.repo.join(".aida"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("cache.db.corrupt-"))
        .collect();
    names.sort();
    names
}

// Acceptance (a): rebuild quarantines the broken file and rebuilds from git.
#[test]
fn rebuild_quarantines_a_corrupt_cache_and_rebuilds_from_the_store() {
    let fx = fixture();
    break_fts(&fx);

    let stdout = ok(&aida(&fx, &["cache", "rebuild"]));
    assert!(stdout.contains("moved it aside to"), "{stdout}");
    assert!(stdout.contains("Cache rebuilt"), "{stdout}");

    let moved = quarantined(&fx);
    assert!(
        moved
            .iter()
            .any(|n| !n.ends_with("-wal") && !n.ends_with("-shm")),
        "the main cache file is quarantined: {moved:?}"
    );
    let main = moved
        .iter()
        .find(|n| !n.ends_with("-wal") && !n.ends_with("-shm"))
        .unwrap();
    assert!(
        stdout.contains(main.as_str()),
        "notice names the path: {stdout}"
    );

    // The fresh cache is healthy: FTS search and verify both work.
    let found = ok(&aida(&fx, &["search", "widget"]));
    assert!(found.contains("searchable widget"), "{found}");
    ok(&aida(&fx, &["cache", "verify"]));
}

// Acceptance (a), non-sqlite bytes: still recovers, and moves the bytes aside
// rather than deleting them.
#[test]
fn rebuild_recovers_from_a_garbage_cache_file() {
    let fx = fixture();
    let path = cache_path(&fx);
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    std::fs::write(&path, b"this is not a sqlite database".repeat(200)).unwrap();

    let out = aida(&fx, &["cache", "rebuild"]);
    let stdout = ok(&out);
    assert!(stdout.contains("Cache rebuilt"), "{stdout}");
    assert!(!quarantined(&fx).is_empty(), "garbage bytes are kept aside");
    let found = ok(&aida(&fx, &["search", "widget"]));
    assert!(found.contains("searchable widget"), "{found}");
}

// Acceptance (b): read commands and verify print the one-line fix.
#[test]
fn read_command_and_verify_print_the_rebuild_hint_not_the_raw_error() {
    let fx = fixture();
    break_fts(&fx);

    for args in [&["search", "widget"][..], &["cache", "verify"][..]] {
        let out = aida(&fx, args);
        let (stdout, stderr) = text(&out);
        assert!(
            !out.status.success(),
            "{args:?} must fail: {stdout}{stderr}"
        );
        let all = format!("{stdout}{stderr}");
        assert!(
            all.contains("Fix: aida cache rebuild"),
            "{args:?} names the fix:\n{all}"
        );
        assert!(
            !all.contains("Caused by"),
            "{args:?} must not surface the raw sqlite error:\n{all}"
        );
    }
    assert!(
        quarantined(&fx).is_empty(),
        "a read never quarantines; only rebuild does"
    );

    // The hint's fix actually works.
    ok(&aida(&fx, &["cache", "rebuild"]));
    ok(&aida(&fx, &["cache", "verify"]));
}

// BUSY is not corruption: a healthy cache locked by another writer is never
// quarantined by verify or rebuild.
#[test]
fn a_locked_healthy_cache_is_not_quarantined() {
    let fx = fixture();
    let holder = rusqlite::Connection::open(cache_path(&fx)).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();

    let verify = aida(&fx, &["cache", "verify"]);
    let (stdout, stderr) = text(&verify);
    assert!(
        !format!("{stdout}{stderr}").contains("corrupt"),
        "a locked cache is not reported corrupt:\n{stdout}{stderr}"
    );
    let rebuild = Command::new(env!("CARGO_BIN_EXE_aida"))
        .current_dir(&fx.repo)
        .env("HOME", &fx.home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_OUTPUT_FORMAT", "human")
        .env("AIDA_CACHE_RETRY_COUNT", "0")
        .args(["cache", "rebuild"])
        .output()
        .unwrap();
    let (stdout, stderr) = text(&rebuild);
    assert!(
        !format!("{stdout}{stderr}").contains("moved it aside"),
        "a locked cache is never moved aside:\n{stdout}{stderr}"
    );
    assert!(quarantined(&fx).is_empty(), "nothing quarantined");
    holder.execute_batch("ROLLBACK").unwrap();
    ok(&aida(&fx, &["cache", "verify"]));
}

/// Hold the cache refresh lock (`.aida/cache.db.refresh.lock`, a flock) from
/// another process, as a concurrent refresh would.
struct HeldRefreshLock(std::process::Child);

impl HeldRefreshLock {
    fn hold(fixture: &Fixture) -> Self {
        let lock = fixture.repo.join(".aida/cache.db.refresh.lock");
        let ready = fixture.repo.join(".aida/lock-held");
        // `--close`: the lock fd is not inherited by the `sh`/`sleep` child,
        // so killing `flock` itself releases the lock.
        let child = Command::new("flock")
            .arg("--close")
            .arg(&lock)
            .arg("sh")
            .arg("-c")
            .arg(format!("touch '{}'; sleep 60", ready.display()))
            .spawn()
            .expect("spawn flock");
        let start = std::time::Instant::now();
        while !ready.exists() {
            assert!(start.elapsed().as_secs() < 10, "flock never acquired");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Self(child)
    }
}

impl Drop for HeldRefreshLock {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn write_garbage_cache(fx: &Fixture) {
    let path = cache_path(fx);
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    std::fs::write(&path, b"this is not a sqlite database".repeat(200)).unwrap();
}

// Acceptance (b), SQLITE_NOTADB: when the open-time auto-heal cannot take the
// refresh lock (another process holds it), a read prints the fix and the cause
// instead of the raw error, and leaves the file where it is.
#[test]
fn notadb_read_prints_the_rebuild_hint_when_auto_heal_cannot_lock() {
    let fx = fixture();
    write_garbage_cache(&fx);
    let held = HeldRefreshLock::hold(&fx);
    let out = aida(&fx, &["list"]);
    drop(held);
    let (stdout, stderr) = text(&out);
    let all = format!("{stdout}{stderr}");
    assert!(!out.status.success(), "{all}");
    assert!(all.contains("is corrupt. Fix: aida cache rebuild"), "{all}");
    assert!(
        all.contains("cause: "),
        "original cause stays visible:\n{all}"
    );
    assert!(!all.contains("Caused by"), "{all}");
    assert!(
        quarantined(&fx).is_empty(),
        "no quarantine without the lock"
    );

    // Lock released: the next read auto-heals and says where the file went.
    let out = aida(&fx, &["list"]);
    let (stdout, stderr) = text(&out);
    assert!(out.status.success(), "{stdout}{stderr}");
    assert!(stderr.contains("moved it aside to"), "{stderr}");
    assert_eq!(quarantined(&fx).len(), 1);
}

// Rebuild respects the refresh lock: while another process holds it, rebuild
// does not move the (corrupt) cache aside and says why.
#[test]
fn rebuild_waits_for_and_respects_the_refresh_lock() {
    let fx = fixture();
    break_fts(&fx);
    let held = HeldRefreshLock::hold(&fx);
    let out = aida(&fx, &["cache", "rebuild"]);
    drop(held);
    let (stdout, stderr) = text(&out);
    let all = format!("{stdout}{stderr}");
    assert!(!out.status.success(), "{all}");
    assert!(all.contains("another aida process is refreshing"), "{all}");
    assert!(quarantined(&fx).is_empty(), "nothing moved while locked");

    let stdout = ok(&aida(&fx, &["cache", "rebuild"]));
    assert!(stdout.contains("moved it aside to"), "{stdout}");
}
