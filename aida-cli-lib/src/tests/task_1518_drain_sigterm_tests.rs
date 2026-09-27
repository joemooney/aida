//! Tests for TASK-1518: the drain's SIGTERM handler releases the drain lock,
//! marks in-flight leases interrupted, and exits non-zero within a bounded
//! time; nothing changes without a signal.
//!
//! The protocol tests drive `run_handler` through an `mpsc` channel with a
//! recording exit and a private term flag, so the whole first-signal → grace
//! → second-signal/deadline sequence runs in-process without a real signal
//! and without touching process-wide state. The one REAL-signal test re-execs
//! this test binary as a child (the "fake drain"), which installs the handler
//! and SIGTERMs itself; the parent only inspects the child's exit status and
//! the files it left. Sibling tests in this process never see the signal.
//
// trace:TASK-1518 | ai:claude

use super::*;
use crate::drain_lock::{drain_lock_path, DrainLock};
use std::sync::mpsc;
use std::time::Instant;

const SIGTERM: i32 = 15;

/// Env var carrying the fake drain's project root to the re-exec'd child.
const CHILD_ROOT_ENV: &str = "AIDA_TEST_TASK_1518_SIGTERM_ROOT";

/// Populate `root` as a fake drain: the drain lock recorded for `pid`, two
/// leases (one created by `pid`, one by another process) and no stop request.
fn populate_fake_drain(root: &Path, pid: u32) {
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
}

/// A fresh temp root populated by [`populate_fake_drain`].
fn fake_drain(pid: u32) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    populate_fake_drain(tmp.path(), pid);
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

/// The stamp as the production lease reader (`list_leases` → `SessionLease`)
/// sees it: `(interrupted_at, interrupted_reason)`, or `None` when the lease
/// is unstamped or does not parse.
fn lease_interruption(path: &Path) -> Option<(String, String)> {
    let body = std::fs::read_to_string(path).ok()?;
    let lease: crate::SessionLease = toml::from_str(&body).ok()?;
    let at = lease.interrupted_at?;
    Some((
        at.to_rfc3339(),
        lease.interrupted_reason.unwrap_or_default(),
    ))
}

/// A private term flag per test, so no test flips the process-wide one.
fn private_flag() -> &'static AtomicBool {
    Box::leak(Box::new(AtomicBool::new(false)))
}

/// A context over an empty (but live) slot; the slot is returned so the
/// caller keeps it alive for the handler's lifetime.
fn ctx(
    root: &Path,
    grace: Duration,
    term_flag: &'static AtomicBool,
) -> (GuardSlot, DrainTermContext) {
    let slot: GuardSlot = Arc::new(Mutex::new(None));
    let ctx = DrainTermContext {
        project_root: root.to_path_buf(),
        drain_pid: std::process::id(),
        guard: Arc::downgrade(&slot),
        grace,
        term_flag,
        borrowed: false,
    };
    (slot, ctx)
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
fn task_1518_first_term_writes_stop_request_and_marks_leases_but_keeps_the_lock() {
    let pid = std::process::id();
    let tmp = fake_drain(pid);
    let root = tmp.path();

    let report = on_first_term(root, pid, false);
    assert!(report.stop_requested);
    assert!(stop_path(root).exists());
    let stop = std::fs::read_to_string(stop_path(root)).unwrap();
    assert!(stop.contains("\"sigterm\""), "{stop}");
    assert_eq!(report.leases_marked, vec!["aaaa11112222".to_string()]);
    assert!(
        drain_lock_path(root).exists(),
        "the first SIGTERM must NOT release the drain lock: the in-flight phase may \
         be integrating on main and a free lock would let another driver take it"
    );

    // Idempotent: a second pass marks nothing new and still keeps the lock.
    let again = on_first_term(root, pid, false);
    assert!(again.leases_marked.is_empty());
    assert!(drain_lock_path(root).exists());
}

/// The release step (run just before the exit) removes a lock file that
/// records our pid, and is idempotent.
#[test]
fn task_1518_release_step_releases_a_lock_file_of_ours_once() {
    let pid = std::process::id();
    let tmp = fake_drain(pid);
    let root = tmp.path();
    let slot: GuardSlot = Arc::new(Mutex::new(None));

    assert!(release_drain_lock(root, &Arc::downgrade(&slot)));
    assert!(
        !drain_lock_path(root).exists(),
        "the drain lock must be released"
    );
    // Idempotent: nothing left to release.
    assert!(!release_drain_lock(root, &Arc::downgrade(&slot)));
}

/// A lock recorded for ANOTHER pid is never removed (the same ownership rule
/// as the guard's Drop and the atexit hook).
#[test]
fn task_1518_release_step_never_removes_a_lock_it_does_not_own() {
    let other = std::process::id().wrapping_add(7_000_000);
    let tmp = fake_drain(other);
    let root = tmp.path();
    let slot: GuardSlot = Arc::new(Mutex::new(None));

    assert!(!release_drain_lock(root, &Arc::downgrade(&slot)));
    assert!(drain_lock_path(root).exists());
    // And the bookkeeping stamps nothing of another drain's either.
    let report = on_first_term(root, std::process::id(), false);
    assert!(report.leases_marked.is_empty());
    assert!(drain_lock_path(root).exists());
}

/// With a real guard in the slot, the release goes through the guard (slot
/// emptied, file gone).
#[test]
fn task_1518_release_step_drops_the_guard_in_the_slot() {
    let _env = crate::drain_lock::test_env_isolation();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    let guard = crate::drain_lock::acquire_drain_lock(root, "queue work --auto-complete").unwrap();
    assert!(drain_lock_path(root).exists());
    let slot: GuardSlot = Arc::new(Mutex::new(Some(guard)));

    assert!(release_drain_lock(root, &Arc::downgrade(&slot)));
    assert!(
        slot.lock().unwrap().is_none(),
        "the guard must have been dropped"
    );
    assert!(!drain_lock_path(root).exists());
}

/// The handler only holds a Weak handle: once the dispatch arm has dropped
/// the slot (normal return — the guard's own Drop already released the
/// lock), the release step finds nothing to release and removes nothing.
#[test]
fn task_1518_release_step_after_the_slot_is_gone_releases_nothing() {
    let _env = crate::drain_lock::test_env_isolation();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    let guard = crate::drain_lock::acquire_drain_lock(root, "queue work --auto-complete").unwrap();
    let slot: GuardSlot = Arc::new(Mutex::new(Some(guard)));
    let handle = Arc::downgrade(&slot);
    drop(slot);
    assert!(
        !drain_lock_path(root).exists(),
        "dropping the slot drops the guard exactly as before"
    );
    assert!(handle.upgrade().is_none());

    assert!(!release_drain_lock(root, &handle));
}

/// PROXY DECISION (a): a BORROWED child (`AIDA_DRAIN_BORROW`) stamps its own
/// leases but never writes the shared stop request — a manual kill of one
/// child must not stop the parent wave.
#[test]
fn task_1518_borrowed_child_marks_leases_but_writes_no_stop_request() {
    let pid = std::process::id();
    let tmp = fake_drain(pid);
    let root = tmp.path();

    let report = on_first_term(root, pid, true);
    assert!(!report.stop_requested);
    assert!(
        !stop_path(root).exists(),
        "the parent wave's stop file is untouched"
    );
    assert_eq!(report.leases_marked, vec!["aaaa11112222".to_string()]);
    assert!(drain_lock_path(root).exists());
}

// --- `aida drain stop --now` ------------------------------------------------

fn lock_of(root: &Path) -> DrainLock {
    serde_json::from_str(&std::fs::read_to_string(drain_lock_path(root)).unwrap()).unwrap()
}

/// The lock file survives `stop --now` while the signalled pid is alive: the
/// drain may still be integrating under it, and removing the file would let
/// `aida drain start` / the next tick double-drive main.
#[test]
fn task_1518_stop_now_leaves_the_lock_while_the_drain_pid_is_alive() {
    let pid = std::process::id();
    let tmp = fake_drain(pid);
    let root = tmp.path();
    let lock = lock_of(root);

    let outcome = crate::drain_cmd::release_lock_after_stop_now(
        root,
        &lock,
        |_, _| true,
        Duration::from_millis(300),
        Duration::from_millis(20),
    );
    assert_eq!(outcome, crate::drain_cmd::StopNowLock::LeftToDrain);
    assert!(
        drain_lock_path(root).exists(),
        "never remove the drain lock under a live drain"
    );
}

/// Once the pid is gone, `stop --now` removes a lock that still records it —
/// and never one a successor drain has since taken.
#[test]
fn task_1518_stop_now_releases_only_the_dead_pids_own_lock() {
    let pid = std::process::id();
    let tmp = fake_drain(pid);
    let root = tmp.path();
    let lock = lock_of(root);

    // Alive for the first two polls, then gone.
    let polls = std::sync::atomic::AtomicUsize::new(0);
    let outcome = crate::drain_cmd::release_lock_after_stop_now(
        root,
        &lock,
        |_, _| polls.fetch_add(1, Ordering::SeqCst) < 2,
        Duration::from_secs(5),
        Duration::from_millis(10),
    );
    assert_eq!(outcome, crate::drain_cmd::StopNowLock::ReleasedAfterExit);
    assert!(!drain_lock_path(root).exists());

    // The drain released it itself: nothing left for stop --now.
    let outcome = crate::drain_cmd::release_lock_after_stop_now(
        root,
        &lock,
        |_, _| false,
        Duration::from_secs(1),
        Duration::from_millis(10),
    );
    assert_eq!(outcome, crate::drain_cmd::StopNowLock::ReleasedByDrain);

    // A successor took the lock under a different pid: left alone.
    populate_fake_drain(root, pid.wrapping_add(4242));
    let outcome = crate::drain_cmd::release_lock_after_stop_now(
        root,
        &lock,
        |_, _| false,
        Duration::from_secs(1),
        Duration::from_millis(10),
    );
    assert_eq!(outcome, crate::drain_cmd::StopNowLock::ReleasedByDrain);
    assert!(
        drain_lock_path(root).exists(),
        "a successor's lock is never removed"
    );
}

// --- Protocol ---------------------------------------------------------------

/// Acceptance (1): SIGTERM to a fake drain marks the leases at once and
/// releases the lock on the way out; acceptance (2): a second SIGTERM forces
/// the exit, non-zero. The lock is HELD for the whole grace window.
#[test]
fn task_1518_second_sigterm_forces_exit_after_bookkeeping() {
    let pid = std::process::id();
    let tmp = fake_drain(pid);
    let root = tmp.path().to_path_buf();
    let (tx, rx) = mpsc::channel::<i32>();
    let (exit_code, exit) = recording_exit();
    let flag = private_flag();
    let (_slot, ctx) = ctx(&root, Duration::from_secs(3600), flag);

    let handler = std::thread::spawn(move || run_handler(rx, ctx, exit));
    tx.send(SIGTERM).unwrap();

    // Bookkeeping lands well before the grace window ends.
    let started = Instant::now();
    while lease_interruption(&lease_path(&root, "aaaa11112222")).is_none()
        && started.elapsed() < Duration::from_secs(5)
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(lease_interruption(&lease_path(&root, "aaaa11112222")).is_some());
    assert!(stop_path(&root).exists());
    assert!(
        drain_lock_path(&root).exists(),
        "the lock must be HELD during the grace window (BUG-538)"
    );
    assert!(
        exit_code.lock().unwrap().is_none(),
        "no exit before the second signal"
    );

    tx.send(SIGTERM).unwrap();
    handler.join().unwrap();
    assert_eq!(*exit_code.lock().unwrap(), Some(SIGTERM_EXIT_CODE));
    assert!(
        !drain_lock_path(&root).exists(),
        "the lock is released on the way out"
    );
    assert_ne!(SIGTERM_EXIT_CODE, 0);
    assert!(flag.load(Ordering::SeqCst));
    assert_eq!(stop_exit_code_for(flag, 0), SIGTERM_EXIT_CODE);
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
    let (_slot, ctx) = ctx(&root, Duration::from_millis(300), private_flag());

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
    let flag = private_flag();
    let (_slot, ctx) = ctx(&root, Duration::from_millis(200), flag);

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
    assert!(!flag.load(Ordering::SeqCst));
}

/// Acceptance (3): no behaviour change without a signal — an installed
/// handler whose source closes untouched does nothing to lock, leases or
/// stop request, never exits, and leaves the stop exit code alone.
#[test]
fn task_1518_no_signal_means_no_change() {
    let pid = std::process::id();
    let tmp = fake_drain(pid);
    let root = tmp.path().to_path_buf();
    let (tx, rx) = mpsc::channel::<i32>();
    let (exit_code, exit) = recording_exit();
    let flag = private_flag();
    let (_slot, ctx) = ctx(&root, Duration::from_millis(100), flag);

    let handler = std::thread::spawn(move || run_handler(rx, ctx, exit));
    drop(tx);
    handler.join().unwrap();

    assert!(exit_code.lock().unwrap().is_none());
    assert!(drain_lock_path(&root).exists());
    assert!(lease_interruption(&lease_path(&root, "aaaa11112222")).is_none());
    assert!(!stop_path(&root).exists());
    assert_eq!(stop_exit_code_for(flag, 0), 0);
    assert_eq!(stop_exit_code_for(flag, 7), 7);
}

/// The grace window comes from the env, clamped, with a sane fallback.
#[test]
fn task_1518_grace_env_parses_and_falls_back() {
    let env = crate::test_env::EnvVarsGuard::set(&[("AIDA_DRAIN_TERM_GRACE_SECS", "7")]);
    assert_eq!(grace_from_env(), Duration::from_secs(7));
    drop(env);
    let env = crate::test_env::EnvVarsGuard::set(&[("AIDA_DRAIN_TERM_GRACE_SECS", "600")]);
    assert_eq!(
        grace_from_env(),
        Duration::from_secs(MAX_GRACE_SECS),
        "the grace window is clamped under systemd's stop timeout"
    );
    drop(env);
    let env = crate::test_env::EnvVarsGuard::set(&[("AIDA_DRAIN_TERM_GRACE_SECS", "soon")]);
    assert_eq!(grace_from_env(), Duration::from_secs(DEFAULT_GRACE_SECS));
    drop(env);
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_DRAIN_TERM_GRACE_SECS", None)]);
    assert_eq!(grace_from_env(), Duration::from_secs(DEFAULT_GRACE_SECS));
}

// --- The reader: `aida ps` --------------------------------------------------

/// The stamp survives the production lease parser, and the `aida ps` row
/// built over it reads STOPPED (wave stopped, worktree intact, plain resume)
/// where the same unstamped lease reads STALLED with the dead-process hint.
#[test]
fn task_1518_ps_reads_a_marked_lease_as_stopped_not_dead() {
    let pid = std::process::id();
    let tmp = fake_drain(pid);
    let root = tmp.path();
    // Give the drain-created lease a real (clean, existing) worktree so the
    // dispatch probe has something to classify.
    let wt = root.join("wt-task-1");
    std::fs::create_dir_all(&wt).unwrap();
    let raw = std::fs::read_to_string(lease_path(root, "aaaa11112222")).unwrap();
    let raw = raw.replace(
        "worktree_path = \"/tmp/none\"",
        &format!("worktree_path = \"{}\"", wt.display()),
    );
    std::fs::write(lease_path(root, "aaaa11112222"), raw).unwrap();

    let before = crate::list_leases(root)
        .into_iter()
        .find(|l| l.id == "aaaa11112222")
        .unwrap();
    assert!(before.interrupted_at.is_none());

    assert_eq!(
        mark_in_flight_leases_interrupted(root, pid),
        vec!["aaaa11112222".to_string()]
    );
    let after = crate::list_leases(root)
        .into_iter()
        .find(|l| l.id == "aaaa11112222")
        .expect("the stamped lease still parses");
    assert!(after.interrupted_at.is_some(), "the reader sees the mark");
    assert_eq!(after.interrupted_reason.as_deref(), Some(REASON_SIGTERM));

    let specs = vec![crate::RunningWorkSpec {
        disp: "TASK-1".into(),
        agreed_id: Some("TASK-1".into()),
        spec_id: Some("TASK-1".into()),
        title: "stopped wave".into(),
        in_progress: true,
        orphan_excluded_type: false,
    }];
    let now = chrono::Utc::now();
    let rows_for = |lease: crate::SessionLease| {
        let (rows, _orphans) = crate::build_running_work(
            &specs,
            &[lease],
            &[],
            now,
            |_| crate::dispatch_health_ps::WorktreeGitProbe::default(),
            |_| None,
            |_| None,
            |_| None,
            |_| None,
            |_| crate::MailIdentityStatus::Unknown,
            |_, _| crate::SeatActivity::Unknown,
        );
        rows
    };

    // No live process backs either lease (creator_pid is this test's pid,
    // but a plain session lease is classified by its worktree, where no
    // live claude sits), so the only difference is the stamp.
    let stopped = rows_for(after);
    assert_eq!(stopped.len(), 1);
    let d = stopped[0].dispatch.as_ref().unwrap();
    assert_eq!(d.state, crate::dispatch_health_ps::DispatchState::Stopped);
    assert_eq!(d.state.label(), "stopped");
    let hint = d.hint.as_deref().unwrap();
    assert!(hint.contains("drain wave stopped"), "{hint}");
    assert!(hint.contains("aida queue work TASK-1"), "{hint}");
    assert!(!hint.contains("dead process"), "{hint}");

    let stalled = rows_for(before);
    let d = stalled[0].dispatch.as_ref().unwrap();
    assert_eq!(d.state, crate::dispatch_health_ps::DispatchState::Stalled);
}

/// The post-filter's matrix: only the dead-process, clean-tree reading is
/// re-framed; a dirty tree keeps its salvage urgency, a live or unknown
/// process keeps its reading, and an unstamped lease is untouched.
#[test]
fn task_1518_interruption_only_reframes_the_dead_clean_stalled_arm() {
    use crate::dispatch_health_ps::{apply_interruption, DispatchState as S};
    assert_eq!(
        apply_interruption(S::Stalled, Some(false), true),
        S::Stopped
    );
    assert_eq!(
        apply_interruption(S::Stalled, Some(false), false),
        S::Stalled
    );
    assert_eq!(apply_interruption(S::Stalled, Some(true), true), S::Stalled);
    assert_eq!(apply_interruption(S::Stalled, None, true), S::Stalled);
    assert_eq!(
        apply_interruption(S::Salvageable, Some(false), true),
        S::Salvageable
    );
    assert_eq!(apply_interruption(S::Moving, Some(true), true), S::Moving);
    assert_eq!(apply_interruption(S::Unknown, None, true), S::Unknown);
    assert_eq!(
        apply_interruption(S::AwaitingAgent, Some(false), true),
        S::AwaitingAgent
    );
    let hint = crate::dispatch_health_ps::next_command_hint(
        S::Stopped,
        Path::new("/wt/x"),
        "claude/x",
        None,
        Some("TASK-9"),
        false,
    )
    .unwrap();
    assert!(hint.contains("aida queue work TASK-9"), "{hint}");
}

// --- A real signal, in a child process --------------------------------------

/// The child half of the real-signal test. Inert unless the parent set
/// [`CHILD_ROOT_ENV`]; then this process IS the fake drain: it installs the
/// handler, SIGTERMs itself, checks the bookkeeping landed with the lock
/// still held, and waits for the grace deadline to release the lock and
/// force the exit (status 143). Reaching the end of the function means the
/// forced exit never came.
#[cfg(unix)]
#[test]
fn task_1518_real_sigterm_child_body() {
    let Some(root) = std::env::var_os(CHILD_ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let pid = std::process::id();
    populate_fake_drain(&root, pid);
    let slot: GuardSlot = Arc::new(Mutex::new(None));
    install(DrainTermContext {
        project_root: root.clone(),
        drain_pid: pid,
        guard: Arc::downgrade(&slot),
        grace: Duration::from_secs(4),
        term_flag: process_term_flag(),
        borrowed: false,
    })
    .unwrap();

    // SAFETY: signalling our own pid with a signal we have just registered a
    // handler for, in a process that exists only for this test.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }

    let started = Instant::now();
    while lease_interruption(&lease_path(&root, "aaaa11112222")).is_none()
        && started.elapsed() < Duration::from_secs(10)
    {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(lease_interruption(&lease_path(&root, "aaaa11112222")).is_some());
    assert!(
        drain_lock_path(&root).exists(),
        "the lock is held across the grace window"
    );
    assert!(lease_interruption(&lease_path(&root, "bbbb33334444")).is_none());
    assert!(stop_path(&root).exists());
    assert!(process_term_flag().load(Ordering::SeqCst));

    // The grace deadline must now release the lock and force
    // process::exit(143) from the handler.
    std::thread::sleep(Duration::from_secs(15));
    panic!("the grace deadline did not force the exit");
}

/// Acceptance (1) and (2) with a REAL signal, isolated from every sibling
/// test: re-exec this test binary filtered to the child body above, with
/// the fake drain's root in the env. The child catches its own SIGTERM,
/// releases the lock, marks the lease, and is forced out by the grace
/// deadline with status 143; an uncaught SIGTERM would instead end it by
/// signal, and a failed assertion would end it with the harness's 101.
#[cfg(unix)]
#[test]
fn task_1518_real_sigterm_in_a_child_process_releases_lock_and_marks_leases() {
    use std::os::unix::process::ExitStatusExt as _;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let exe = std::env::current_exe().unwrap();
    let output = std::process::Command::new(exe)
        .args(["real_sigterm_child_body", "--nocapture", "--test-threads=1"])
        .env(CHILD_ROOT_ENV, &root)
        .env_remove("AIDA_DRAIN_TERM_GRACE_SECS")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(SIGTERM_EXIT_CODE),
        "child must be forced out with 143 (signal: {:?})\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.signal()
    );
    assert!(
        !drain_lock_path(&root).exists(),
        "the child's real SIGTERM must release the lock"
    );
    let (_, reason) = lease_interruption(&lease_path(&root, "aaaa11112222")).unwrap();
    assert_eq!(reason, REASON_SIGTERM);
    assert!(lease_interruption(&lease_path(&root, "bbbb33334444")).is_none());
    assert!(stop_path(&root).exists());
    assert!(
        stderr.contains("SIGTERM: stop requested"),
        "the child must report the bookkeeping:\n{stderr}"
    );
    assert!(
        stderr.contains("drain lock released"),
        "the child must report the release on the way out:\n{stderr}"
    );
}
