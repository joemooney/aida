//! Tests for TASK-1518: the drain's SIGTERM handler releases the drain lock,
//! marks in-flight leases interrupted, and exits non-zero within a bounded
//! time; nothing changes without a signal.
//!
//! The protocol tests drive `run_handler` through an `mpsc` channel with a
//! recording exit so the whole first-signal → grace → second-signal/deadline
//! sequence runs in-process. One Unix test sends a REAL SIGTERM to this
//! process (the "fake drain") through `install`.
//
// trace:TASK-1518 | ai:claude

use super::*;
use crate::drain_lock::{drain_lock_path, DrainLock};
use std::sync::mpsc;
use std::time::Instant;

const SIGTERM: i32 = 15;

/// A project root with the drain lock recorded for `pid`, two leases (one
/// created by `pid`, one by another process) and no stop request.
fn fake_drain(pid: u32) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida").join("sessions")).unwrap();
    let lock = DrainLock {
        pid,
        pid_start_time: None,
        started_at_utc: chrono::Utc::now().to_rfc3339(),
        command: "queue work --auto-complete --drain".to_string(),
        host: "testhost".to_string(),
        wave_id: String::new(),
        binary_sha: String::new(),
        binary_mtime_secs: None,
        binary_path: String::new(),
        launched_stale: false,
        specs: vec!["TASK-1".to_string()],
    };
    std::fs::write(
        drain_lock_path(root),
        serde_json::to_string_pretty(&lock).unwrap(),
    )
    .unwrap();
    write_lease(root, "aaaa11112222", pid, "TASK-1");
    write_lease(root, "bbbb33334444", pid.wrapping_add(100_000), "TASK-2");
    tmp
}

fn write_lease(root: &Path, id: &str, creator_pid: u32, scope: &str) {
    let body = format!(
        "id = \"{id}\"\nscope = \"{scope}\"\nslug = \"{}\"\nowner = \"t\"\n\
         worktree_path = \"/tmp/none\"\nbranch = \"claude/{}\"\n\
         started_at = \"2026-09-26T00:00:00Z\"\nhostname = \"h\"\n\
         creator_pid = {creator_pid}\ncustom_key = \"kept\"\n",
        scope.to_ascii_lowercase(),
        scope.to_ascii_lowercase()
    );
    std::fs::write(
        root.join(".aida")
            .join("sessions")
            .join(format!("{id}.toml")),
        body,
    )
    .unwrap();
}

fn lease_path(root: &Path, id: &str) -> PathBuf {
    root.join(".aida")
        .join("sessions")
        .join(format!("{id}.toml"))
}

fn stop_path(root: &Path) -> PathBuf {
    crate::drain_cmd::drain_stop_path(root)
}

fn ctx(root: &Path, grace: Duration) -> DrainTermContext {
    DrainTermContext {
        project_root: root.to_path_buf(),
        drain_pid: std::process::id(),
        guard: Arc::new(Mutex::new(None)),
        grace,
    }
}

/// Recording exit: the handler's `exit` stores the code instead of leaving
/// the process.
fn recording_exit() -> (Arc<Mutex<Option<i32>>>, impl Fn(i32)) {
    let slot = Arc::new(Mutex::new(None));
    let for_closure = Arc::clone(&slot);
    (slot, move |code| {
        *for_closure.lock().unwrap() = Some(code);
    })
}

// --- Bookkeeping ------------------------------------------------------------

#[test]
fn task_1518_first_term_marks_only_this_drains_leases_and_keeps_unknown_keys() {
    let pid = std::process::id();
    let tmp = fake_drain(pid);
    let root = tmp.path();

    let marked = mark_in_flight_leases_interrupted(root, pid);
    assert_eq!(marked, vec!["aaaa11112222".to_string()]);

    let (at, reason) = lease_interruption(&lease_path(root, "aaaa11112222")).unwrap();
    assert!(!at.is_empty());
    assert_eq!(reason, REASON_SIGTERM);
    let body = std::fs::read_to_string(lease_path(root, "aaaa11112222")).unwrap();
    assert!(
        body.contains("custom_key = \"kept\""),
        "unknown keys must survive: {body}"
    );
    assert!(body.contains("creator_pid = "), "{body}");

    assert!(lease_interruption(&lease_path(root, "bbbb33334444")).is_none());

    // Idempotent: a second pass stamps nothing new.
    assert!(mark_in_flight_leases_interrupted(root, pid).is_empty());
}

#[test]
fn task_1518_first_term_writes_stop_request_and_releases_lock_file() {
    let pid = std::process::id();
    let tmp = fake_drain(pid);
    let root = tmp.path();
    let slot: GuardSlot = Arc::new(Mutex::new(None));

    let report = on_first_term(root, pid, &slot);
    assert!(report.stop_requested);
    assert!(stop_path(root).exists());
    let stop = std::fs::read_to_string(stop_path(root)).unwrap();
    assert!(stop.contains("\"sigterm\""), "{stop}");
    assert_eq!(report.leases_marked, vec!["aaaa11112222".to_string()]);
    assert!(report.lock_released);
    assert!(
        !drain_lock_path(root).exists(),
        "the drain lock must be released"
    );
}

/// A lock recorded for ANOTHER pid is never removed (the same ownership rule
/// as the guard's Drop and the atexit hook).
#[test]
fn task_1518_first_term_never_removes_a_lock_it_does_not_own() {
    let other = std::process::id().wrapping_add(7_000_000);
    let tmp = fake_drain(other);
    let root = tmp.path();
    let slot: GuardSlot = Arc::new(Mutex::new(None));

    let report = on_first_term(root, std::process::id(), &slot);
    assert!(!report.lock_released);
    assert!(drain_lock_path(root).exists());
    assert!(report.leases_marked.is_empty());
}

/// With a real guard in the slot, the release goes through the guard (slot
/// emptied, file gone).
#[test]
fn task_1518_first_term_drops_the_guard_in_the_slot() {
    let _env = crate::drain_lock::test_env_isolation();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    let guard = crate::drain_lock::acquire_drain_lock(root, "queue work --auto-complete").unwrap();
    assert!(drain_lock_path(root).exists());
    let slot: GuardSlot = Arc::new(Mutex::new(Some(guard)));

    let report = on_first_term(root, std::process::id(), &slot);
    assert!(report.lock_released);
    assert!(
        slot.lock().unwrap().is_none(),
        "the guard must have been dropped"
    );
    assert!(!drain_lock_path(root).exists());
}

// --- Protocol ---------------------------------------------------------------

/// Acceptance (1): SIGTERM to a fake drain releases the lock and marks the
/// leases; acceptance (2): a second SIGTERM forces the exit, non-zero.
#[test]
fn task_1518_second_sigterm_forces_exit_after_bookkeeping() {
    let pid = std::process::id();
    let tmp = fake_drain(pid);
    let root = tmp.path().to_path_buf();
    let (tx, rx) = mpsc::channel::<i32>();
    let (exit_code, exit) = recording_exit();
    let ctx = ctx(&root, Duration::from_secs(3600));

    let handler = std::thread::spawn(move || run_handler(rx, ctx, exit));
    tx.send(SIGTERM).unwrap();

    // Bookkeeping lands well before the grace window ends.
    let started = Instant::now();
    while drain_lock_path(&root).exists() && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !drain_lock_path(&root).exists(),
        "lock must be released on the first SIGTERM"
    );
    assert!(lease_interruption(&lease_path(&root, "aaaa11112222")).is_some());
    assert!(stop_path(&root).exists());
    assert!(
        exit_code.lock().unwrap().is_none(),
        "no exit before the second signal"
    );

    tx.send(SIGTERM).unwrap();
    handler.join().unwrap();
    assert_eq!(*exit_code.lock().unwrap(), Some(SIGTERM_EXIT_CODE));
    assert_ne!(SIGTERM_EXIT_CODE, 0);
    assert!(term_requested());
    assert_eq!(stop_exit_code(0), SIGTERM_EXIT_CODE);
}

/// Acceptance (2): the grace deadline forces the exit without a second
/// signal, and within the bound.
#[test]
fn task_1518_grace_deadline_forces_exit() {
    let pid = std::process::id();
    let tmp = fake_drain(pid);
    let root = tmp.path().to_path_buf();
    let (tx, rx) = mpsc::channel::<i32>();
    let (exit_code, exit) = recording_exit();
    let ctx = ctx(&root, Duration::from_millis(300));

    let started = Instant::now();
    let handler = std::thread::spawn(move || run_handler(rx, ctx, exit));
    tx.send(SIGTERM).unwrap();
    handler.join().unwrap();
    let elapsed = started.elapsed();

    assert_eq!(*exit_code.lock().unwrap(), Some(SIGTERM_EXIT_CODE));
    assert!(
        elapsed < Duration::from_secs(5),
        "the forced exit must be bounded, took {elapsed:?}"
    );
    assert!(!drain_lock_path(&root).exists());
    drop(tx);
}

/// Signals other than SIGTERM never trigger the protocol.
#[test]
fn task_1518_other_signals_are_ignored() {
    let pid = std::process::id();
    let tmp = fake_drain(pid);
    let root = tmp.path().to_path_buf();
    let (tx, rx) = mpsc::channel::<i32>();
    let (exit_code, exit) = recording_exit();
    let ctx = ctx(&root, Duration::from_millis(200));

    let handler = std::thread::spawn(move || run_handler(rx, ctx, exit));
    tx.send(1).unwrap(); // SIGHUP
    tx.send(2).unwrap(); // SIGINT
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        drain_lock_path(&root).exists(),
        "no bookkeeping without SIGTERM"
    );
    assert!(exit_code.lock().unwrap().is_none());
    drop(tx); // close the source: the handler returns without exiting
    handler.join().unwrap();
    assert!(exit_code.lock().unwrap().is_none());
    assert!(drain_lock_path(&root).exists());
}

/// Acceptance (3): no behaviour change without a signal — an installed
/// handler whose source closes untouched does nothing to lock, leases or
/// stop request, and never exits.
#[test]
fn task_1518_no_signal_means_no_change() {
    let pid = std::process::id();
    let tmp = fake_drain(pid);
    let root = tmp.path().to_path_buf();
    let (tx, rx) = mpsc::channel::<i32>();
    let (exit_code, exit) = recording_exit();
    let ctx = ctx(&root, Duration::from_millis(100));

    let handler = std::thread::spawn(move || run_handler(rx, ctx, exit));
    drop(tx);
    handler.join().unwrap();

    assert!(exit_code.lock().unwrap().is_none());
    assert!(drain_lock_path(&root).exists());
    assert!(lease_interruption(&lease_path(&root, "aaaa11112222")).is_none());
    assert!(!stop_path(&root).exists());
}

/// The grace window comes from the env, with a sane fallback.
#[test]
fn task_1518_grace_env_parses_and_falls_back() {
    let _env = crate::test_env::EnvVarsGuard::set(&[("AIDA_DRAIN_TERM_GRACE_SECS", "7")]);
    assert_eq!(grace_from_env(), Duration::from_secs(7));
    drop(_env);
    let _env = crate::test_env::EnvVarsGuard::set(&[("AIDA_DRAIN_TERM_GRACE_SECS", "soon")]);
    assert_eq!(grace_from_env(), Duration::from_secs(DEFAULT_GRACE_SECS));
    drop(_env);
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_DRAIN_TERM_GRACE_SECS", None)]);
    assert_eq!(grace_from_env(), Duration::from_secs(DEFAULT_GRACE_SECS));
}

/// Acceptance (1) with a REAL signal: `install` registers the handler, a
/// SIGTERM to this process (the fake drain) releases the lock and marks the
/// lease. The grace is an hour so the forced exit never reaches the test
/// binary; signal-hook keeps SIGTERM caught for the rest of the process, so
/// the default terminate action can no longer fire either.
#[cfg(unix)]
#[test]
fn task_1518_real_sigterm_releases_lock_and_marks_leases() {
    let pid = std::process::id();
    let tmp = fake_drain(pid);
    let root = tmp.path().to_path_buf();
    install(DrainTermContext {
        project_root: root.clone(),
        drain_pid: pid,
        guard: Arc::new(Mutex::new(None)),
        grace: Duration::from_secs(3600),
    })
    .unwrap();

    // SAFETY: signalling our own pid with a signal we have just registered a
    // handler for.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }

    let started = Instant::now();
    while drain_lock_path(&root).exists() && started.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        !drain_lock_path(&root).exists(),
        "the real SIGTERM must release the lock"
    );
    assert!(lease_interruption(&lease_path(&root, "aaaa11112222")).is_some());
    assert!(lease_interruption(&lease_path(&root, "bbbb33334444")).is_none());
    assert!(stop_path(&root).exists());
}
