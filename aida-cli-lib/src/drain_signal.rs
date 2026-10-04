//! SIGTERM handling for the unattended drain (`aida queue work --auto-complete`).
//!
//! # The problem
//!
//! A drain wave is stopped from outside more often than it finishes: systemd
//! `RuntimeMaxSec` / `OOMPolicy=stop` on the transient wave unit (TASK-1510),
//! `aida drain stop --now`, or an operator's `kill`. Until this module the
//! drain had no SIGTERM handler, so the default action killed it mid-phase
//! and left `.aida/drain.lock` plus its phase leases on disk for the next
//! tick's reap / stale-lock paths to clean up — the same recovery class as a
//! crash, and indistinguishable from one.
//!
//! # The protocol
//!
//! On the FIRST SIGTERM the handler thread, in this order and bounded by
//! plain file operations:
//!
//! 1. writes the cooperative stop request (`aida drain stop` shape, mode
//!    `sigterm`) so the dispatch loop picks up no further head;
//! 2. forwards SIGTERM to every in-flight vendor child this drain launched
//!    (TASK-1542) — the `active_pid` of each lease it created, and only when
//!    that pid's recorded kernel start identity still matches, so a pid
//!    recycled since the lease was written can never be signalled;
//! 3. stamps `interrupted_at` / `interrupted_reason = "sigterm"` on every
//!    lease this drain created (`creator_pid` == the drain pid) — an
//!    *interrupted* lease, not an abandoned one, so `aida ps` and the reaper
//!    can tell a stopped wave from a crashed one;
//! 4. records `stopped_at` / `stopped_reason` on `.aida/drain-state.json`
//!    (TASK-1542), so once this process is gone `aida drain status` reports a
//!    *stopped* drain rather than the *stale* one a crash would leave. The
//!    record is re-stamped just before the forced exit, because a phase write
//!    during the grace window persists a state value read before the first
//!    stamp and would otherwise drop it.
//!
//! The vendor children get the signal and then the SAME grace window the drain
//! itself gets — there is no second timer, and no SIGKILL escalation: a child
//! that outlives the window is reparented and reaped by the next tick exactly
//! as it was before TASK-1542.
//!
//! It then waits up to a grace window for the drain to finish on its own
//! (the in-flight phase may land, and the batch loop then returns through
//! the stop request with [`SIGTERM_EXIT_CODE`]). The drain lock is HELD for
//! that whole window: the in-flight phase may be integrating on `main`, and
//! a freed lock would let another driver (`queue integrate --watch`,
//! `burndown run`, a manual `queue work`) take it and double-drive the tree
//! — the BUG-538 hole the lock exists to close. A SECOND SIGTERM, or the end
//! of the grace window, releases the lock (guard dropped: heartbeat stopped,
//! shared claim released, local file removed only when it still records our
//! pid) and immediately forces `process::exit(SIGTERM_EXIT_CODE)`. That
//! release may do one store round trip (the shared cross-clone claim) before
//! the exit, which is why it sits inside the grace budget.
//!
//! A drain whose lock is BORROWED (an internal child drive under
//! `AIDA_DRAIN_BORROW`) still stamps its own leases and still signals its own
//! children, but writes neither the stop request nor the stopped record: both
//! files are ONE per project root and belong to the parent wave, and a manual
//! kill of one child must not stop the whole wave — nor make
//! `aida drain status` report it as stopped.
//!
//! Without a signal nothing here runs: installing the handler only spawns a
//! parked thread. Unix only; on Windows [`install`] is a documented no-op
//! (there is no SIGTERM; the wave is stopped by the job-object / console
//! event paths the launcher already owns).
//!
//! trace:TASK-1518 | ai:claude
//! trace:TASK-1542 | ai:claude

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use crate::drain_lock::DrainGuard;

/// Exit status of a drain stopped by SIGTERM: the conventional 128 + 15.
pub(crate) const SIGTERM_EXIT_CODE: i32 = 143;

/// How long the first SIGTERM waits for the drain to finish on its own
/// before the handler forces the exit. Override with
/// `AIDA_DRAIN_TERM_GRACE_SECS`, clamped to [`MAX_GRACE_SECS`]. Both sit
/// under systemd's default `TimeoutStopSec` of 90 s (the transient wave unit
/// sets none, so the default applies) so the lock release and the forced
/// exit both land before the SIGKILL.
pub(crate) const DEFAULT_GRACE_SECS: u64 = 30;

/// Upper bound on the grace window, whatever the env says: past this the
/// held drain lock would outlive systemd's stop timeout and the SIGKILL
/// would leave it for the next tick's stale-reclaim after all.
pub(crate) const MAX_GRACE_SECS: u64 = 60;

const GRACE_ENV: &str = "AIDA_DRAIN_TERM_GRACE_SECS";

/// The reason string stamped on interrupted leases and the stop request.
pub(crate) const REASON_SIGTERM: &str = "sigterm";

/// Set once the first SIGTERM has been handled. Read by the batch loop's
/// stop-request return so a drain that winds down cooperatively still exits
/// non-zero (a stopped wave is not a clean drain). The handler reaches it
/// through [`DrainTermContext::term_flag`] so tests drive a private flag and
/// never flip this process-wide one under sibling tests.
static TERM_REQUESTED: AtomicBool = AtomicBool::new(false);

/// The process-wide "a SIGTERM was handled" flag, for the production context.
pub(crate) fn process_term_flag() -> &'static AtomicBool {
    &TERM_REQUESTED
}

/// The exit code a cooperative stop should report: `default` normally,
/// [`SIGTERM_EXIT_CODE`] once a SIGTERM has been handled in this process.
pub(crate) fn stop_exit_code(default: i32) -> i32 {
    stop_exit_code_for(process_term_flag(), default)
}

/// [`stop_exit_code`] over an explicit flag (the testable form).
pub(crate) fn stop_exit_code_for(term_flag: &AtomicBool, default: i32) -> i32 {
    if term_flag.load(Ordering::SeqCst) {
        SIGTERM_EXIT_CODE
    } else {
        default
    }
}

/// The grace window, from `AIDA_DRAIN_TERM_GRACE_SECS` or the default,
/// clamped to [`MAX_GRACE_SECS`]. A non-numeric value falls back to the
/// default; `0` means "exit as soon as the bookkeeping is done".
pub(crate) fn grace_from_env() -> Duration {
    let secs = std::env::var(GRACE_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_GRACE_SECS)
        .min(MAX_GRACE_SECS);
    Duration::from_secs(secs)
}

/// The shared slot the dispatch arm keeps its [`DrainGuard`] in, so the
/// handler thread can release the lock properly (drop = heartbeat stop +
/// shared-claim release + pid-checked file removal) instead of deleting a
/// file out from under a live guard.
pub(crate) type GuardSlot = Arc<Mutex<Option<DrainGuard>>>;

/// The handler's view of the slot: a `Weak` so the parked handler thread
/// never keeps the guard alive. When the dispatch arm returns normally the
/// slot's only strong reference drops, the guard drops exactly as it did
/// before this module existed, and a later signal finds nothing to release.
pub(crate) type GuardHandle = Weak<Mutex<Option<DrainGuard>>>;

/// What the first SIGTERM's bookkeeping did — returned so a test (and the
/// stderr line) can say exactly what was cleaned up.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct TermReport {
    /// The cooperative stop request was written.
    pub(crate) stop_requested: bool,
    /// Lease ids stamped `interrupted_at`.
    pub(crate) leases_marked: Vec<String>,
    /// Pids of the in-flight vendor children this drain forwarded SIGTERM to.
    // trace:TASK-1542 | ai:codex
    pub(crate) children_signalled: Vec<u32>,
}

/// One in-flight vendor child launched by this drain. The kernel start
/// identity travels WITH the pid: every liveness re-check between collection
/// and the kill must be identity-aware, and a bare `pid` field invites the
/// `process_identity_is_alive(pid, None)` spelling that silently degrades to a
/// pid-only check — which is the recycled-pid hazard the collector exists to
/// avoid. Always `Some` by construction; the collector skips a lease without it.
// trace:TASK-1542 | ai:codex
pub(crate) struct VendorChild {
    pub(crate) lease_id: String,
    pub(crate) pid: u32,
    pub(crate) start_time: String,
}

/// Collect drain-owned children independently of lease interruption stamping.
// trace:TASK-1542 | ai:codex
pub(crate) fn in_flight_vendor_children(project_root: &Path, drain_pid: u32) -> Vec<VendorChild> {
    let dir = project_root.join(".aida").join("sessions");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut children = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = toml::from_str::<toml::Value>(&body) else {
            continue;
        };
        let Some(table) = value.as_table() else {
            continue;
        };
        let creator = table
            .get("creator_pid")
            .and_then(|v| v.as_integer())
            .and_then(|v| u32::try_from(v).ok());
        if creator != Some(drain_pid) {
            continue;
        }
        // Do not skip `interrupted_at`: an earlier partial stop may have stamped this lease while its child remains live.
        let Some(pid) = table
            .get("active_pid")
            .and_then(|v| v.as_integer())
            .and_then(|v| u32::try_from(v).ok())
        else {
            continue;
        };
        if pid <= 1 || pid == drain_pid {
            continue;
        }
        // A missing identity must skip: a bare PID check could signal an unrelated process after PID reuse.
        let Some(start) = table.get("active_pid_start_time").and_then(|v| v.as_str()) else {
            continue;
        };
        if !aida_core::liveness::process_identity_is_alive(pid, Some(start)) {
            continue;
        }
        let lease_id = table
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                path.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
        children.push(VendorChild {
            lease_id,
            pid,
            start_time: start.to_string(),
        });
    }
    children.sort_by(|a, b| a.lease_id.cmp(&b.lease_id));
    children
}

/// Forward SIGTERM to each collected child. Returns the pids actually signalled.
// trace:TASK-1542 | ai:codex
#[cfg(unix)]
pub(crate) fn forward_term_to_children(children: &[VendorChild]) -> Vec<u32> {
    children
        .iter()
        .filter_map(|child| {
            debug_assert!(child.pid > 1);
            // The re-check is IDENTITY-aware, like the collector's: a bare
            // `None` here would accept a pid recycled between collection and
            // this line and SIGTERM an unrelated process of the operator's.
            if child.pid <= 1
                || !aida_core::liveness::process_identity_is_alive(
                    child.pid,
                    Some(&child.start_time),
                )
            {
                return None;
            }
            // SAFETY: `pid` is a live, identity-corroborated child collected
            // from a lease this drain created and re-checked on the line above;
            // pids 0 and 1 (whole-process-group and init) are excluded, so this
            // can only reach the intended single process.
            (unsafe { libc::kill(child.pid as libc::pid_t, libc::SIGTERM) } == 0)
                .then_some(child.pid)
        })
        .collect()
}

/// Windows has no SIGTERM signal.
// trace:TASK-1542 | ai:codex
#[cfg(not(unix))]
pub(crate) fn forward_term_to_children(_children: &[VendorChild]) -> Vec<u32> {
    Vec::new()
}

/// Everything the handler needs; built by the dispatch arm.
pub(crate) struct DrainTermContext {
    pub(crate) project_root: PathBuf,
    pub(crate) drain_pid: u32,
    pub(crate) guard: GuardHandle,
    pub(crate) grace: Duration,
    /// Raised after the first SIGTERM's bookkeeping; production passes
    /// [`process_term_flag`].
    pub(crate) term_flag: &'static AtomicBool,
    /// This drain borrows its parent's lock (`AIDA_DRAIN_BORROW`): stamp
    /// leases, but never write the shared stop request.
    pub(crate) borrowed: bool,
}

/// Where the handler's signals come from. Production wraps a signal-hook
/// iterator; tests feed an `mpsc` channel so the whole protocol (first
/// signal → bookkeeping → grace → second signal or deadline → exit) runs
/// in-process without a real signal.
// trace:TASK-1518 | ai:claude
pub(crate) trait SignalSource {
    /// Block until a signal arrives; `None` once the source is closed.
    fn wait(&mut self) -> Option<i32>;
    /// Wait up to `timeout` for a signal; `None` on timeout or close.
    fn wait_timeout(&mut self, timeout: Duration) -> Option<i32>;
}

impl SignalSource for std::sync::mpsc::Receiver<i32> {
    fn wait(&mut self) -> Option<i32> {
        self.recv().ok()
    }
    fn wait_timeout(&mut self, timeout: Duration) -> Option<i32> {
        self.recv_timeout(timeout).ok()
    }
}

/// signal-hook backed source. `wait` blocks on the iterator; `wait_timeout`
/// polls `pending()` on a short cadence so the grace deadline is honoured.
#[cfg(unix)]
struct HookSource(signal_hook::iterator::Signals);

#[cfg(unix)]
impl SignalSource for HookSource {
    fn wait(&mut self) -> Option<i32> {
        loop {
            if let Some(sig) = self.0.wait().next() {
                return Some(sig);
            }
            if self.0.is_closed() {
                return None;
            }
        }
    }
    fn wait_timeout(&mut self, timeout: Duration) -> Option<i32> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(sig) = self.0.pending().next() {
                return Some(sig);
            }
            if self.0.is_closed() || std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Install the SIGTERM handler for this drain. Spawns one parked thread;
/// nothing else changes until a signal arrives. Errors only when the signal
/// iterator cannot be registered, which the caller treats as advisory.
// trace:TASK-1518 | ai:claude
#[cfg(unix)]
pub(crate) fn install(ctx: DrainTermContext) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let signals = signal_hook::iterator::Signals::new([signal_hook::consts::SIGTERM])
        .context("installing the drain SIGTERM handler")?;
    std::thread::Builder::new()
        .name("aida-drain-sigterm".into())
        .spawn(move || {
            run_handler(HookSource(signals), ctx, |code| std::process::exit(code));
        })
        .context("spawning the drain SIGTERM handler thread")?;
    Ok(())
}

/// Windows has no SIGTERM: the wave is stopped through the launcher's own
/// process-tree paths, and the next tick's reap / stale-lock recovery
/// applies exactly as before this module existed. Deliberately a no-op
/// rather than a console-control-event port, so the Unix contract above is
/// the only one to reason about.
// trace:TASK-1518 | ai:claude
#[cfg(not(unix))]
pub(crate) fn install(_ctx: DrainTermContext) -> anyhow::Result<()> {
    Ok(())
}

/// The signal number this handler acts on.
const SIGTERM_NUM: i32 = 15;

/// The handler loop: the FIRST SIGTERM runs the bookkeeping (stop request,
/// lease stamps) and arms the grace deadline; a SECOND SIGTERM, or the
/// deadline, releases the drain lock and calls `exit` with
/// [`SIGTERM_EXIT_CODE`]. The lock is held across the grace window on
/// purpose — see the module docs. Signals other than SIGTERM are ignored.
/// Generic over the source and the exit so a test can drive it in-process.
// trace:TASK-1518 | ai:claude
pub(crate) fn run_handler<S, F>(mut source: S, ctx: DrainTermContext, exit: F)
where
    S: SignalSource,
    F: Fn(i32),
{
    // Park until the first SIGTERM.
    loop {
        match source.wait() {
            Some(sig) if sig == SIGTERM_NUM => break,
            Some(_) => continue,
            // Source closed without a signal: nothing to do, ever.
            None => return,
        }
    }

    ctx.term_flag.store(true, Ordering::SeqCst);
    let report = on_first_term(&ctx.project_root, ctx.drain_pid, ctx.borrowed);
    eprintln!(
        "  {} SIGTERM: stop requested; {} vendor child(ren) signalled; {} lease(s) marked interrupted; drain lock held \
         while the in-flight phase finishes; exiting {} within {}s (a second SIGTERM exits now)",
        crate::glyph(crate::glyphs::Glyph::Warning),
        report.children_signalled.len(), report.leases_marked.len(),
        SIGTERM_EXIT_CODE,
        ctx.grace.as_secs()
    );

    // Grace: a second SIGTERM or the deadline, whichever comes first.
    let deadline = std::time::Instant::now() + ctx.grace;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match source.wait_timeout(remaining) {
            Some(sig) if sig == SIGTERM_NUM => break,
            Some(_) => continue,
            // Timeout, or the source closed: either way the window is over.
            None => break,
        }
    }

    // The window is over: release the lock LAST, immediately before the
    // exit, so no other driver can take it while this drain may still be
    // integrating (BUG-538).
    let released = release_drain_lock(&ctx.project_root, &ctx.guard);
    eprintln!(
        "  {} SIGTERM: drain lock {}; exiting {}",
        crate::glyph(crate::glyphs::Glyph::Warning),
        if released { "released" } else { "not held" },
        SIGTERM_EXIT_CODE
    );
    // A phase write during the grace window may have overwritten the first
    // stop stamp (it persists a state value read BEFORE the stamp landed).
    // Re-stamp after the lock release and directly before the exit, when the
    // last of those writes is behind us; idempotence keeps the original time.
    // Borrowed children skip it — see `on_first_term`.
    if !ctx.borrowed {
        crate::drain_state::record_stopped(&ctx.project_root, REASON_SIGTERM);
    }
    exit(SIGTERM_EXIT_CODE);
}

/// The first SIGTERM's bookkeeping, pure over the filesystem: stop request
/// (skipped for a `borrowed` child, whose request file belongs to the parent
/// wave) and lease stamps. Deliberately NOT the lock release — that is
/// [`release_drain_lock`], run just before the exit. Every step is
/// best-effort and independent so a failure in one never skips the other.
// trace:TASK-1518 | ai:claude
pub(crate) fn on_first_term(project_root: &Path, drain_pid: u32, borrowed: bool) -> TermReport {
    let stop_requested = !borrowed
        && crate::drain_cmd::write_stop_request(project_root, REASON_SIGTERM, Some(drain_pid))
            .is_ok();
    let children_signalled =
        forward_term_to_children(&in_flight_vendor_children(project_root, drain_pid));
    let leases_marked = mark_in_flight_leases_interrupted(project_root, drain_pid);
    // Skipped for a BORROWED child for the same reason as the stop request
    // above: `.aida/drain-state.json` is ONE file per project root, owned by
    // the parent wave, and a manual kill of one internal child drive must not
    // make `aida drain status` report the whole wave as deliberately stopped.
    if !borrowed {
        crate::drain_state::record_stopped(project_root, REASON_SIGTERM);
    }
    TermReport {
        stop_requested,
        leases_marked,
        children_signalled,
    }
}

/// The release step, run immediately before the forced exit: drop the guard
/// if the slot is still alive and still holds it (the proper release), then
/// remove a local lock file that still records our pid (covers a borrowed or
/// already-taken slot). Idempotent: a second call finds an empty slot and no
/// file of ours. True when either step released something. Never touches a
/// lock recorded for another pid.
// trace:TASK-1518 | ai:claude
pub(crate) fn release_drain_lock(project_root: &Path, guard: &GuardHandle) -> bool {
    let dropped = match guard.upgrade() {
        Some(slot) => {
            let taken = match slot.lock() {
                Ok(mut slot) => slot.take(),
                Err(poisoned) => poisoned.into_inner().take(),
            };
            // Dropped here, outside the slot's critical section: the guard's
            // Drop joins the heartbeat thread and releases the shared claim.
            taken.is_some()
        }
        None => false,
    };
    let removed = crate::drain_lock::release_lock_if_ours(project_root);
    dropped || removed
}

/// Stamp `interrupted_at` / `interrupted_reason` on every lease under
/// `.aida/sessions/` whose `creator_pid` is `drain_pid` and that carries no
/// stamp yet. Patched as generic TOML key inserts (the `manual_enter_at`
/// pattern) so keys this binary does not model survive the round-trip.
/// Returns the ids stamped, sorted. Unreadable or unparseable leases are
/// skipped, never deleted. The reader is `SessionLease::interrupted_at`:
/// `aida ps` classifies a stamped lease with no live process as `stopped`
/// (a wave that was stopped, resumable) rather than a dead agent.
// trace:TASK-1518 | ai:claude
pub(crate) fn mark_in_flight_leases_interrupted(
    project_root: &Path,
    drain_pid: u32,
) -> Vec<String> {
    let dir = project_root.join(".aida").join("sessions");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let now = chrono::Utc::now().to_rfc3339();
    let mut marked = Vec::new();
    // Read-modify-write per lease: a concurrent writer (the session's own
    // `session end`, a `worktree enter` re-stamp) landing in the microseconds
    // between our read and our atomic write loses its update. Accepted: the
    // drain is stopping and every other lease writer is idempotent.
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut value) = toml::from_str::<toml::Value>(&body) else {
            continue;
        };
        let Some(table) = value.as_table_mut() else {
            continue;
        };
        let creator = table
            .get("creator_pid")
            .and_then(|v| v.as_integer())
            .and_then(|v| u32::try_from(v).ok());
        if creator != Some(drain_pid) || table.contains_key("interrupted_at") {
            continue;
        }
        table.insert(
            "interrupted_at".to_string(),
            toml::Value::String(now.clone()),
        );
        table.insert(
            "interrupted_reason".to_string(),
            toml::Value::String(REASON_SIGTERM.to_string()),
        );
        let id = table
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                path.file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default()
            });
        let Ok(content) = toml::to_string_pretty(&value) else {
            continue;
        };
        if aida_core::write_atomic(&path, &content).is_ok() {
            marked.push(id);
        }
    }
    marked.sort();
    marked
}

#[cfg(test)]
#[path = "tests/task_1518_drain_sigterm_tests.rs"]
mod task_1518_drain_sigterm_tests;

#[cfg(test)]
#[path = "tests/task_1542_drain_stop_tests.rs"]
mod task_1542_drain_stop_tests;
