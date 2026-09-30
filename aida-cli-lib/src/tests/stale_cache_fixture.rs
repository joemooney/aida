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
use std::sync::mpsc::{channel, Sender};
use std::thread::JoinHandle;

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

/// Hold the refresh flock for `cache_path` on another thread.
///
/// The holder must be a different thread, not merely a different lock guard:
/// `RefreshLock` shares its descriptor with same-thread nested callers by
/// design, so a same-thread acquire would read as a nested winner rather than
/// as contention.
// trace:TASK-1526 | ai:claude
pub(crate) fn hold_foreign_refresh(cache_path: &Path) -> ForeignRefresh {
    let path = cache_path.to_path_buf();
    let (ready_tx, ready_rx) = channel();
    let (stop_tx, stop_rx) = channel::<()>();
    let thread = std::thread::spawn(move || {
        let lock = aida_core::db::cache_refresh::RefreshLock::try_acquire(&path)
            .expect("fixture: acquiring the refresh flock must not error")
            .expect("fixture: the refresh flock must be free before the holder takes it");
        ready_tx.send(()).unwrap();
        let _ = stop_rx.recv();
        drop(lock);
    });
    ready_rx
        .recv()
        .expect("fixture: the refresh holder thread must report ready");
    ForeignRefresh {
        stop: Some(stop_tx),
        thread: Some(thread),
    }
}
