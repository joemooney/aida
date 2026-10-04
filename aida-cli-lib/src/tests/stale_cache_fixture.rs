//! Manufacture a stale cache read for the regression suites that must prove a
//! caller re-validates against the authoritative object.
//!
//! BUG-1664 and BUG-1670 originally produced the stale snapshot by planting a
//! `.lock-info` sidecar for a live foreign PID: the read path consulted that
//! sidecar and served the last committed rows instead of catching up. TASK-1526
//! deleted that shortcut — the sidecar is a diagnostic and never authorizes
//! stale serving — so the fixture now creates the condition the way the shipped
//! design does: another thread holds the refresh flock, this thread loses the
//! single flight, its bounded read budget expires, and the read is served from
//! the last committed snapshot and labelled stale.
// trace:TASK-1526 | ai:claude

use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use aida_core::db::cache_refresh::RefreshLock;

/// How long the holder keeps retrying the acquire before it gives up, and how
/// often it retries. BUG-1733: the acquire is not guaranteed to succeed on the
/// first attempt even though every test owns a private cache directory. A
/// sibling test thread that forks while this test's *previous* `ForeignRefresh`
/// still holds the flock leaves the child with an inherited descriptor to the
/// same open file description, so the flock outlives the guard that dropped it
/// (BUG-1729 established that mechanism here). The child is a short-lived
/// exec'd subprocess, so the condition clears on its own — wait for it on a
/// stated bound instead of asserting the lock is free the instant we ask.
// trace:BUG-1733 | ai:claude
const ACQUIRE_DEADLINE: Duration = Duration::from_secs(30);
// trace:BUG-1733 | ai:claude
const ACQUIRE_POLL: Duration = Duration::from_millis(25);

/// What the holder thread reports back through the ready channel. A plain
/// `()` cannot distinguish "the holder is armed" from "the holder died", which
/// is what made the original `RecvError` at the receive site the only thing the
/// test printed.
// trace:BUG-1733 | ai:claude
type Ready = Result<(), String>;

/// A foreign refresh holder. While this guard is alive, every cache read on the
/// test's own thread loses the single flight and serves the stale snapshot.
/// Dropping it releases the flock and joins the holder thread.
// trace:TASK-1526 | ai:claude
pub(crate) struct ForeignRefresh {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for ForeignRefresh {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Take the refresh flock, tolerating transient contention up to
/// [`ACQUIRE_DEADLINE`].
///
/// Returns the attempt count alongside the lock so a caller can assert the
/// retry actually engaged rather than only that the acquire eventually worked.
// trace:BUG-1733 | ai:claude
fn acquire_with_retry(path: &Path, deadline: Duration) -> Result<(RefreshLock, usize), String> {
    let started = Instant::now();
    let mut attempts = 0usize;
    loop {
        attempts += 1;
        match RefreshLock::try_acquire(path) {
            Ok(Some(lock)) => return Ok((lock, attempts)),
            Ok(None) => {}
            Err(e) => {
                return Err(format!(
                    "acquiring the refresh flock errored on attempt {attempts}: {e}"
                ))
            }
        }
        if started.elapsed() >= deadline {
            return Err(format!(
                "the refresh flock was still held after {attempts} attempts over {:?} \
                 (poll interval {ACQUIRE_POLL:?}); a sibling thread's forked child may be \
                 holding an inherited descriptor to it — see BUG-1729",
                started.elapsed()
            ));
        }
        std::thread::sleep(ACQUIRE_POLL);
    }
}

/// Hold the refresh flock for `cache_path` on another thread.
///
/// The holder must be a different thread, not merely a different lock guard:
/// `RefreshLock` shares its descriptor with same-thread nested callers by
/// design, so a same-thread acquire would read as a nested winner rather than
/// as contention.
// trace:TASK-1526 | ai:claude
pub(crate) fn hold_foreign_refresh(cache_path: &Path) -> ForeignRefresh {
    let path = cache_path.to_path_buf();
    let (ready_tx, ready_rx) = channel::<Ready>();
    let (stop_tx, stop_rx) = channel::<()>();
    let thread = std::thread::spawn(move || {
        let lock = match acquire_with_retry(&path, ACQUIRE_DEADLINE) {
            Ok((lock, _attempts)) => lock,
            Err(why) => {
                let _ = ready_tx.send(Err(why));
                return;
            }
        };
        if ready_tx.send(Ok(())).is_err() {
            return;
        }
        let _ = stop_rx.recv();
        drop(lock);
    });
    let thread = await_ready(ready_rx, thread);
    ForeignRefresh {
        stop: Some(stop_tx),
        thread: Some(thread),
    }
}

/// Block until the holder reports armed, and give the holder's own failure back
/// to the caller when it does not.
///
/// BUG-1733: a `RecvError` here means the holder thread ended without
/// reporting, which is almost always a panic inside it. Re-raising that panic's
/// own payload is the difference between naming the missing handshake and
/// naming the actual fault; the original fixture reported only the former, so a
/// contended acquire read as "the refresh holder thread must report ready:
/// RecvError" with the real message lost in a parallel run's output.
// trace:BUG-1733 | ai:claude
fn await_ready(ready_rx: Receiver<Ready>, thread: JoinHandle<()>) -> JoinHandle<()> {
    match ready_rx.recv() {
        Ok(Ok(())) => thread,
        Ok(Err(why)) => panic!("fixture: the refresh holder thread could not arm: {why}"),
        Err(_) => match thread.join() {
            Err(payload) => std::panic::resume_unwind(payload),
            Ok(()) => panic!(
                "fixture: the refresh holder thread exited without reporting ready and \
                 without panicking"
            ),
        },
    }
}

/// BUG-1733's own regressions. The fixture is test-support code, so these
/// assert on the fixture rather than on any shipped behaviour: that a contended
/// acquire is waited out instead of panicking, that giving up says how long it
/// waited and how many times it asked, and that a holder that dies before the
/// handshake reports its own panic rather than the missing handshake.
// trace:BUG-1733 | ai:claude
#[cfg(test)]
mod bug_1733_holder_diagnostics_tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn cache_path(dir: &tempfile::TempDir) -> std::path::PathBuf {
        dir.path().join("requirements-cache.db")
    }

    /// AC2/AC4: contention that clears within the bound is waited out, and the
    /// wait is a retry of the acquire — `attempts` is greater than one, so this
    /// fails if the retry loop is removed and only the first ask is made.
    // trace:BUG-1733 | ai:claude
    #[test]
    fn a_transiently_held_flock_is_waited_out_rather_than_panicking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = cache_path(&dir);
        let (armed_tx, armed_rx) = channel::<()>();
        let holder_path = path.clone();
        let holder = std::thread::spawn(move || {
            let lock = RefreshLock::try_acquire(&holder_path)
                .expect("holder: acquire must not error")
                .expect("holder: the flock is free in a private tempdir");
            armed_tx.send(()).expect("holder: report armed");
            std::thread::sleep(Duration::from_millis(250));
            drop(lock);
        });
        armed_rx.recv().expect("the holder must arm");

        let (lock, attempts) = acquire_with_retry(&path, Duration::from_secs(30))
            .expect("the acquire must succeed once the holder releases");
        assert!(
            attempts > 1,
            "the acquire must have been retried, not won outright; attempts={attempts}"
        );
        drop(lock);
        holder.join().expect("holder thread");
    }

    /// AC4: giving up is bounded and says so. The message names the attempt
    /// count and the elapsed wait, which is what a future occurrence needs in
    /// order not to cost another session.
    // trace:BUG-1733 | ai:claude
    #[test]
    fn giving_up_names_the_bound_the_elapsed_wait_and_the_attempt_count() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = cache_path(&dir);
        let (armed_tx, armed_rx) = channel::<()>();
        let (release_tx, release_rx) = channel::<()>();
        let holder_path = path.clone();
        let holder = std::thread::spawn(move || {
            let lock = RefreshLock::try_acquire(&holder_path)
                .expect("holder: acquire must not error")
                .expect("holder: the flock is free in a private tempdir");
            armed_tx.send(()).expect("holder: report armed");
            let _ = release_rx.recv();
            drop(lock);
        });
        armed_rx.recv().expect("the holder must arm");

        let why = acquire_with_retry(&path, Duration::from_millis(200))
            .err()
            .expect("a permanently held flock must exhaust the bound");
        assert!(
            why.contains("attempts") && why.contains("still held"),
            "the give-up message must name the attempt count and the condition: {why}"
        );
        assert!(
            why.contains("BUG-1729"),
            "the give-up message must point at the known mechanism: {why}"
        );
        let _ = release_tx.send(());
        holder.join().expect("holder thread");
    }

    /// AC1: a holder that panics before the handshake surfaces *its own* panic.
    /// Before this change the caller saw only `RecvError` from the ready
    /// channel, which named the handshake and discarded the real fault.
    // trace:BUG-1733 | ai:claude
    #[test]
    fn a_holder_that_dies_before_arming_reports_its_own_panic() {
        let (ready_tx, ready_rx) = channel::<Ready>();
        let thread = std::thread::spawn(move || {
            let _keep = ready_tx;
            panic!("holder: the original fault");
        });
        let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = await_ready(ready_rx, thread);
        }))
        .err()
        .expect("awaiting a dead holder must panic");
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("<non-string panic payload>");
        assert!(
            message.contains("the original fault"),
            "the holder's own panic must be re-raised, not the missing handshake: {message}"
        );
    }
}
