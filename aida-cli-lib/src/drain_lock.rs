//! Drain-instance lock (BUG-538) — `.aida/drain.lock`.
//!
//! # The problem
//!
//! `aida burndown run` and `aida queue work --auto-complete` both INTEGRATE on
//! `main`: they merge PRs, push, create/remove worktrees, and share the same
//! `target/` build dir. Two of them running against the same repo tree at once
//! double-drive it — two integrators racing git ops, two sets of worktrees,
//! corrupted builds. Nothing prevented it; the "one drain at a time" invariant
//! relied on a human remembering which drains were live.
//!
//! # The lock (substrate-as-bouncer)
//!
//! On launch, every drain entry point acquires a GLOBAL lock — one drain per
//! repo, regardless of which command started it (a `burndown run` and a
//! `queue work --auto-complete` are mutually exclusive against each other, not
//! just against their own kind). The lock is a single JSON file at
//! `.aida/drain.lock` carrying the holder's `pid`, `started_at_utc`, the
//! `command` that launched it, and the `host`. A second launch reads it and:
//!
//! - **live + fresh** → REFUSE with the holder's pid / start / command.
//! - **stale** (pid dead, or older than [`stale_secs`]) → reclaim and proceed.
//! - **`AIDA_DRAIN_FORCE=1`** → bypass the check entirely (the rare intentional
//!   concurrent case, or a known-dead holder whose pid was recycled).
//!
//! Release is RAII: [`DrainGuard`]'s `Drop` removes the file on a clean exit.
//! Drain handlers that terminate via `std::process::exit` skip `Drop`, but that
//! is harmless — their pid is dead the instant they exit, so the next launch
//! stale-reclaims. The age backstop ([`AIDA_DRAIN_LOCK_STALE_SECS`], default
//! 1800s) catches the pathological pid-recycle case.
//!
//! # Granularity (out of scope)
//!
//! Per-scope parallel drains on non-overlapping batches are explicitly NOT
//! supported — the lock is global because that matches how drains are run
//! today. Revisit if real demand for concurrent non-overlapping drains appears.
//!
//! trace:BUG-538 | ai:claude

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::process_probe;

/// How often the background heartbeat thread refreshes the shared drain claim.
/// Comfortably under the TTL ([`DEFAULT_STALE_SECS`]) so a long drain phase
/// never lets the claim age out and get reclaimed by another clone.
/// trace:STORY-638 | ai:claude
const HEARTBEAT_INTERVAL_SECS: u64 = 300;

/// File name under `.aida/` holding the live drain's lock. Gitignored by the
/// deny-by-default `.aida/*` rule — pure per-clone runtime state.
const DRAIN_LOCK_FILE: &str = "drain.lock";

/// Env override: any non-empty / truthy value bypasses the concurrency check.
const FORCE_ENV: &str = "AIDA_DRAIN_FORCE";

/// Internal env override: a subprocess delegated by a live drain should observe
/// the parent's lock, not overwrite and release it. User-facing force remains
/// [`FORCE_ENV`]; this is only for child drives launched by AIDA itself.
const BORROW_ENV: &str = "AIDA_DRAIN_BORROW";

/// Env override: age (seconds) past which a still-claimed lock is treated as
/// stale even if its pid happens to be alive (pid-recycle backstop).
const STALE_SECS_ENV: &str = "AIDA_DRAIN_LOCK_STALE_SECS";

/// Default staleness horizon: a lock older than this is reclaimable regardless
/// of pid liveness. 30 minutes — comfortably longer than any single phase, far
/// shorter than a wedged drain a human would want auto-cleared.
const DEFAULT_STALE_SECS: u64 = 1800;

/// Path of the drain lock under `project_root`.
pub(crate) fn drain_lock_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join(DRAIN_LOCK_FILE)
}

// BUG-712: DrainGuard::drop removes the local lock (pid-checked), but an
// autonomous drive exits via std::process::exit (30+ sites), which skips Drop —
// leaving .aida/drain.lock on disk recording the now-dead drive pid. Register a
// libc atexit hook once, when a guard is acquired, so the lock is best-effort
// removed on ANY exit path (normal return OR process::exit). Idempotent with
// Drop: on a normal return Drop removes it first and the hook then no-ops (its
// pid-checked read finds nothing); on process::exit only the hook runs. The hook
// touches just the local file — the shared cross-clone claim (STORY-638) is
// covered by its own staleness, and the heartbeat thread dies with the process.
// trace:BUG-712 | ai:claude
static ATEXIT_LOCK_PATH: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);
// BUG-688: `libc` is a cfg(unix)-only dependency, so the atexit hook (registered
// below via `libc::atexit`) and the state it drives are Unix-only. On Windows
// the `#[cfg(unix)]` gate makes these compile out, so the crate builds there.
// trace:BUG-688 | ai:claude
#[cfg(unix)]
static ATEXIT_REGISTERED: std::sync::OnceLock<()> = std::sync::OnceLock::new();

#[cfg(unix)]
extern "C" fn drain_lock_atexit() {
    if let Ok(slot) = ATEXIT_LOCK_PATH.lock() {
        if let Some(path) = slot.as_ref() {
            remove_lock_if_ours(path);
        }
    }
}

/// Best-effort remove the lock file ONLY if it still records our pid — the same
/// ownership guard `DrainGuard::drop` applies, so we never delete a lock a
/// different drive reclaimed under a recycled pid. trace:BUG-712 | ai:claude
// BUG-688: on non-Unix the sole non-test caller (`drain_lock_atexit`) is
// cfg(unix)-gated out, so a Windows release build sees this as unused; the
// tests still exercise it on every platform. trace:BUG-688 | ai:claude
#[cfg_attr(not(unix), allow(dead_code))]
fn remove_lock_if_ours(path: &Path) {
    if read_lock(path).map(|l| l.pid) == Some(std::process::id()) {
        let _ = std::fs::remove_file(path);
    }
}

/// Arm the process-exit cleanup for the just-acquired drain lock (BUG-712).
fn register_atexit_cleanup(path: &Path) {
    if let Ok(mut slot) = ATEXIT_LOCK_PATH.lock() {
        *slot = Some(path.to_path_buf());
    }
    // BUG-688: the `libc::atexit` cleanup hook is Unix-only (`libc` is a
    // cfg(unix)-only dep). On Windows the RAII `DrainGuard::drop` still removes
    // the lock on a normal return, and the shared claim's staleness backstop
    // (STORY-638) covers a `process::exit` that skips Drop — so gating this out
    // for Windows loses only the best-effort process::exit cleanup, and keeps
    // the crate compiling cross-platform. trace:BUG-688 | ai:claude
    #[cfg(unix)]
    ATEXIT_REGISTERED.get_or_init(|| {
        // SAFETY: `drain_lock_atexit` is a no-arg extern "C" fn that only locks a
        // process-global Mutex and does best-effort fs — safe to run at exit.
        unsafe {
            libc::atexit(drain_lock_atexit);
        }
    });
}

/// On-disk lock record. Serialized as JSON via [`aida_core::write_atomic`] so a
/// concurrent reader never sees a torn file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DrainLock {
    /// PID of the drain process holding the lock.
    pub(crate) pid: u32,
    /// RFC3339 UTC timestamp of when the lock was taken.
    pub(crate) started_at_utc: String,
    /// The command that launched the drain (e.g. `burndown run --status approved`).
    pub(crate) command: String,
    /// Host the drain runs on — informational, for cross-machine shared clones.
    pub(crate) host: String,
    // BUG-759: the spec set the drain set out to work (the burndown-blessed
    // ready set). Read-side tooling (`aida drain status`) names it when the
    // launcher holds the lock but writes no per-phase drain-state file. Serde
    // default so a pre-existing lock written by an older binary still parses.
    // trace:BUG-759 | ai:claude
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) specs: Vec<String>,
}

impl DrainLock {
    /// Age in seconds relative to `now`, parsed from [`Self::started_at_utc`].
    /// An unparseable timestamp returns `None` (treated as "age unknown" — the
    /// pid-liveness check then decides staleness on its own).
    fn age_secs(&self, now: DateTime<Utc>) -> Option<u64> {
        let started = DateTime::parse_from_rfc3339(&self.started_at_utc)
            .ok()?
            .with_timezone(&Utc);
        let secs = now.signed_duration_since(started).num_seconds();
        Some(secs.max(0) as u64)
    }
}

/// The pure decision: given the lock currently on disk (if any), should a new
/// drain ACQUIRE (write its own, possibly reclaiming a stale one) or be
/// REFUSED? Liveness is injected as a predicate so the three paths
/// (no-lock / live-refuse / stale-reclaim) are unit-testable without spawning
/// real processes — mirrors `triage_lease::live_leases_reaping`.
/// trace:BUG-538 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LockDecision {
    /// No live lock stands in the way — write ours.
    Acquire,
    /// A live, non-stale drain holds the lock — refuse, surfacing the holder.
    Refuse(DrainLock),
}

/// Decide ACQUIRE vs REFUSE. `force` short-circuits to ACQUIRE (the
/// `AIDA_DRAIN_FORCE=1` escape). An absent / unparseable on-disk lock is always
/// ACQUIRE. A present lock is reclaimable (→ ACQUIRE) when forced, when its pid
/// is dead, or when it is older than `stale_secs`; otherwise REFUSE.
pub(crate) fn decide_lock(
    existing: Option<DrainLock>,
    now: DateTime<Utc>,
    stale_secs: u64,
    force: bool,
    is_alive: impl Fn(u32) -> bool,
) -> LockDecision {
    if force {
        return LockDecision::Acquire;
    }
    let Some(lock) = existing else {
        return LockDecision::Acquire;
    };
    let pid_dead = !is_alive(lock.pid);
    let aged_out = lock.age_secs(now).map(|a| a > stale_secs).unwrap_or(false);
    if pid_dead || aged_out {
        LockDecision::Acquire
    } else {
        LockDecision::Refuse(lock)
    }
}

/// Read + parse the on-disk lock, if present and well-formed. A missing file or
/// a parse error both yield `None` — a corrupt lock must never wedge a drain
/// (it is treated as "no lock", i.e. reclaimable).
fn read_lock(path: &Path) -> Option<DrainLock> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Resolve the staleness horizon from `AIDA_DRAIN_LOCK_STALE_SECS`, falling
/// back to [`DEFAULT_STALE_SECS`]. A non-numeric value falls back too.
fn stale_secs() -> u64 {
    std::env::var(STALE_SECS_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_STALE_SECS)
}

/// Is the `AIDA_DRAIN_FORCE` escape set to a truthy value?
fn force_requested() -> bool {
    std::env::var(FORCE_ENV)
        .map(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Is this process an internal child drive borrowing its parent's drain lock?
fn borrow_requested() -> bool {
    std::env::var(BORROW_ENV)
        .map(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Acquire the global drain lock for `project_root`, launched as `command`.
///
/// On success returns a [`DrainGuard`] that removes the lock on `Drop`. On a
/// live, non-stale conflict returns an `Err` whose message names the holder and
/// the recovery paths (wait, remove the file, or `AIDA_DRAIN_FORCE=1`).
///
/// Both drain entry points (`aida burndown run` and `aida queue work
/// --auto-complete`) call this at the top so they are mutually exclusive
/// against each other — see the module docs. trace:BUG-538 | ai:claude
pub(crate) fn acquire_drain_lock(project_root: &Path, command: &str) -> Result<DrainGuard> {
    // trace:BUG-759 | ai:claude — thin wrapper; the specs-aware acquire is the
    // one implementation so the lock-lifetime rules can't fork.
    acquire_drain_lock_with_specs(project_root, command, &[])
}

/// Like [`acquire_drain_lock`], additionally recording the drain's spec set in
/// the lock so `aida drain status` can name what a launcher-held drain (which
/// writes no per-phase drain-state file) is working — pid, started, command
/// AND specs stay probe-visible for the launcher's entire wall-clock.
// trace:BUG-759 | ai:claude
pub(crate) fn acquire_drain_lock_with_specs(
    project_root: &Path,
    command: &str,
    specs: &[String],
) -> Result<DrainGuard> {
    let path = drain_lock_path(project_root);
    let existing = read_lock(&path);

    // BUG-748: internal delegate drives (notably `queue integrate` shelling out
    // to `queue work --auto-complete --from-pr`) run while a parent drain still
    // owns `.aida/drain.lock`. Before this borrow path, those children used
    // AIDA_DRAIN_FORCE=1, overwrote the parent lock with their own pid, and then
    // their atexit cleanup removed it. The parent batch kept running without a
    // live lock, tripping BUG-716's implementer invariant on the next member.
    // A borrowed guard neither writes nor releases the lock; if no live parent
    // lock exists, fall through to the normal acquire/refuse path.
    // trace:BUG-748 | ai:codex
    if borrow_requested() {
        if let LockStatus::Running(lock) =
            classify_lock(existing.clone(), process_probe::pid_is_alive)
        {
            return Ok(DrainGuard {
                path,
                pid: lock.pid,
                store_root: None,
                project_root: project_root.to_path_buf(),
                heartbeat: None,
                borrowed: true,
            });
        }
    }

    let forced = force_requested();

    // STORY-638: BEFORE the local lock, consult the SHARED drain claim on the
    // `aida-store` branch so a second CLONE is refused while one holds an active
    // drain (MU-505). The local lock only sees THIS clone's `.aida/drain.lock`.
    // Drain/solo are process-backed, so the shared claim's same-host pid liveness
    // is authoritative; the TTL folds in `AIDA_DRAIN_LOCK_STALE_SECS`. A live
    // cross-clone drain errors here (naming the holder) before we touch the local
    // lock; no remote / unreachable store WARNs and proceeds local-only — a drain
    // must never be brittle on the network. trace:STORY-638 | ai:claude
    let store_root = project_root.join(".aida-store");
    let store_claimed = {
        match crate::coordination::acquire_lock_claim(
            &store_root,
            crate::coordination::LockKind::Drain,
            project_root,
            command,
            stale_secs(),
            forced,
        ) {
            Ok(crate::coordination::LockAcquireOutcome::Acquired) => true,
            Ok(crate::coordination::LockAcquireOutcome::Reclaimed(reason)) => {
                eprintln!(
                    "  {} reclaiming a stale cross-clone drain claim ({reason})",
                    crate::glyphs::get(
                        crate::glyphs::Glyph::InfoAlt,
                        crate::find_project_root().ok().as_deref(),
                    )
                );
                true
            }
            Ok(crate::coordination::LockAcquireOutcome::Unavailable(reason)) => {
                eprintln!(
                    "  {} cross-clone coordination unavailable: {reason}, proceeding",
                    crate::glyphs::get(
                        crate::glyphs::Glyph::Warning,
                        crate::find_project_root().ok().as_deref(),
                    )
                );
                false
            }
            Err(e) => anyhow::bail!("{e}"),
        }
    };

    match decide_lock(
        existing.clone(),
        Utc::now(),
        stale_secs(),
        forced,
        process_probe::pid_is_alive,
    ) {
        LockDecision::Refuse(holder) => {
            anyhow::bail!(
                "a drain is already running (pid {}, started {}, cmd `{}`{}).\n  \
                 Drives are serialized per repo — to drive several specs at once, use \
                 the `aida burndown` fan-out (one orchestrator over worktree-isolated \
                 subagents) instead of a second drive.\n  \
                 Otherwise wait for it to finish, or — if you're certain it's dead — remove {} \
                 or set AIDA_DRAIN_FORCE=1 to override.",
                holder.pid,
                holder.started_at_utc,
                holder.command,
                if holder.host.is_empty() {
                    String::new()
                } else {
                    format!(", host {}", holder.host)
                },
                path.display(),
            );
        }
        LockDecision::Acquire => {
            // A reclaim (stale lock present) is worth a one-line note so a fast
            // re-run after a crash isn't silently surprising. Forced runs say so
            // too. trace:BUG-538 | ai:claude
            if forced && existing.is_some() {
                // trace:TASK-840 | ai:claude — route the warning marker through the registry.
                let warn = crate::glyphs::get(
                    crate::glyphs::Glyph::Warning,
                    crate::find_project_root().ok().as_deref(),
                );
                eprintln!(
                    "  {warn} AIDA_DRAIN_FORCE=1 — overriding the existing drain lock at {}",
                    path.display()
                );
            } else if let Some(stale) = &existing {
                eprintln!(
                    "  {} reclaiming a stale drain lock (pid {}, started {} — not running)",
                    crate::glyphs::get(
                        crate::glyphs::Glyph::InfoAlt,
                        crate::find_project_root().ok().as_deref(),
                    ),
                    stale.pid,
                    stale.started_at_utc
                );
            }

            // Best-effort parent-dir create — the `.aida/` dir normally already
            // exists, but a freshly-attached clone might be racing it.
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let record = DrainLock {
                pid: std::process::id(),
                started_at_utc: Utc::now().to_rfc3339(),
                command: command.to_string(),
                host: hostname(),
                // trace:BUG-759 | ai:claude
                specs: specs.to_vec(),
            };
            let json = serde_json::to_string_pretty(&record).unwrap_or_else(|_| "{}".to_string());
            aida_core::write_atomic(&path, json).map_err(|e| {
                anyhow::anyhow!("could not write drain lock {}: {e}", path.display())
            })?;
            // STORY-638: when we hold the shared claim, spawn a background
            // heartbeat thread that refreshes `heartbeat_at` periodically so a
            // long drain (a single phase can outlast the TTL) never looks stale
            // to another clone. The thread stops when the guard drops. Threading
            // the guard through `burndown::run`'s deep per-spec loop would be
            // invasive; a lifetime-scoped thread is the low-risk equivalent of
            // "refresh on the loop tick". trace:STORY-638 | ai:claude
            let (store_for_guard, heartbeat) = if store_claimed {
                let stop = Arc::new(AtomicBool::new(false));
                let stop_t = Arc::clone(&stop);
                let store_t = store_root.clone();
                let project_t = project_root.to_path_buf();
                let handle = std::thread::spawn(move || {
                    let step = std::time::Duration::from_secs(1);
                    let mut elapsed = 0u64;
                    while !stop_t.load(Ordering::Relaxed) {
                        std::thread::sleep(step);
                        elapsed += 1;
                        if elapsed >= HEARTBEAT_INTERVAL_SECS {
                            elapsed = 0;
                            crate::coordination::heartbeat_lock_claim(
                                &store_t,
                                crate::coordination::LockKind::Drain,
                                &project_t,
                            );
                        }
                    }
                });
                (Some(store_root), Some(Heartbeat { stop, handle }))
            } else {
                (None, None)
            };
            // BUG-712: arm the process-exit cleanup before `path` is moved into
            // the guard, so the lock is removed even when the drive process::exits.
            register_atexit_cleanup(&path);
            Ok(DrainGuard {
                path,
                pid: record.pid,
                store_root: store_for_guard,
                project_root: project_root.to_path_buf(),
                heartbeat,
                borrowed: false,
            })
        }
    }
}

/// Best-effort host name for the lock record (informational only).
fn hostname() -> String {
    sysinfo::System::host_name().unwrap_or_default()
}

/// Liveness-corroborated read of the drain lock for read-side tooling
/// (`aida burndown status`, TASK-806). Mirrors [`crate::drain_state::probe`]:
/// the on-disk lock is classified against a PID-liveness probe so callers can
/// tell a live drain from a crashed one without re-implementing the read.
/// trace:TASK-806 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LockStatus {
    /// No lock file — no drain is (or recently was) running.
    None,
    /// The lock is present and its `pid` is alive — a drain is running.
    Running(DrainLock),
    /// The lock is present but its `pid` is dead — a drain crashed or exited
    /// without releasing it. The next `burndown run` / `queue work
    /// --auto-complete` stale-reclaims it.
    Stale(DrainLock),
}

/// Read `.aida/drain.lock` and corroborate the recorded `pid` against a
/// liveness probe. A missing or corrupt lock is [`LockStatus::None`] — the same
/// fail-safe `read_lock` applies on the acquire path. trace:TASK-806 | ai:claude
pub(crate) fn probe_lock(project_root: &Path) -> LockStatus {
    classify_lock(
        read_lock(&drain_lock_path(project_root)),
        process_probe::pid_is_alive,
    )
}

/// Pure classifier: split from [`probe_lock`] so the three paths
/// (none / running / stale) are unit-testable without a real lock or pid.
/// trace:TASK-806 | ai:claude
fn classify_lock(existing: Option<DrainLock>, is_alive: impl Fn(u32) -> bool) -> LockStatus {
    match existing {
        None => LockStatus::None,
        Some(lock) => {
            if is_alive(lock.pid) {
                LockStatus::Running(lock)
            } else {
                LockStatus::Stale(lock)
            }
        }
    }
}

/// BUG-716 invariant: an orchestrated implementer must run under a LIVE drain
/// lock. The `aida pr ship` self-merge gate keys on `probe_lock == Running` to
/// tell a drive from a plain session; if a drive ever spawned an implementer
/// WITHOUT the lock, that gate would silently stop firing and the reviewer
/// bypass (BUG-716) would reopen. A `Running` lock satisfies the invariant;
/// `None` / `Stale` (a crashed drive's leftover, BUG-712) do not. Pure so the
/// invariant is unit-testable, and asserted at the single orchestrated
/// implementer chokepoint (`RealPhaseDriver::run_implementer`) so a future
/// spawn path that drops the lock trips loudly in dev/CI instead of at a user.
// trace:BUG-716 | ai:claude
pub(crate) fn drive_lock_invariant_holds(status: &LockStatus) -> bool {
    matches!(status, LockStatus::Running(_))
}

/// RAII handle: while held, this process owns the global drain lock. `Drop`
/// removes the lock file — but ONLY if it still records THIS process's pid, so
/// a guard that outlives a stale-reclaim by a successor never deletes the
/// successor's live lock. trace:BUG-538 | ai:claude
/// Background heartbeat thread handle for the shared drain claim (STORY-638).
/// Its `stop` flag is flipped on guard drop so the thread exits promptly.
#[derive(Debug)]
struct Heartbeat {
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

#[derive(Debug)]
pub(crate) struct DrainGuard {
    path: PathBuf,
    pid: u32,
    /// Set when we hold the SHARED cross-clone drain claim on the store (STORY-638).
    /// `None` when cross-clone coordination was unavailable (local-only). The
    /// project root is kept separately so a moved cwd doesn't break release.
    /// trace:STORY-638 | ai:claude
    store_root: Option<PathBuf>,
    project_root: PathBuf,
    /// Background thread refreshing the shared claim's heartbeat. `None` when
    /// local-only. trace:STORY-638 | ai:claude
    heartbeat: Option<Heartbeat>,
    /// BUG-748: internal child drives can borrow a parent drain lock. Borrowed
    /// guards are read-only handles and must never release local/shared state.
    borrowed: bool,
}

impl Drop for DrainGuard {
    fn drop(&mut self) {
        if self.borrowed {
            return;
        }
        // Stop the heartbeat thread before releasing, so it can't re-write the
        // claim after we delete it. trace:STORY-638 | ai:claude
        if let Some(hb) = self.heartbeat.take() {
            hb.stop.store(true, Ordering::Relaxed);
            let _ = hb.handle.join();
        }
        // Release the shared cross-clone claim (best-effort; staleness covers a
        // crash). trace:STORY-638 | ai:claude
        if let Some(store_root) = &self.store_root {
            crate::coordination::release_lock_claim(
                store_root,
                crate::coordination::LockKind::Drain,
                &self.project_root,
            );
        }
        // Only remove the local file if it's still ours. A different drain may
        // have reclaimed it (e.g. our pid was wrongly judged stale); deleting its
        // lock would reopen the double-drive hole.
        if read_lock(&self.path).map(|l| l.pid) == Some(self.pid) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock(pid: u32, started_at_utc: &str) -> DrainLock {
        DrainLock {
            pid,
            started_at_utc: started_at_utc.to_string(),
            command: "queue work --auto-complete".to_string(),
            host: "testhost".to_string(),
            specs: Vec::new(),
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-14T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn atexit_removes_our_lock_but_preserves_a_foreign_one() {
        // BUG-712: the process-exit hook must clean up OUR leftover lock, but
        // never delete a lock a different (live) drive holds — the same pid guard
        // Drop applies. This is the safety property the atexit path relies on.
        let tmp = tempfile::tempdir().unwrap();
        // Ours (current pid) → removed.
        let mine = tmp.path().join("mine.lock");
        std::fs::write(
            &mine,
            serde_json::to_string(&lock(std::process::id(), "2026-01-01T00:00:00Z")).unwrap(),
        )
        .unwrap();
        remove_lock_if_ours(&mine);
        assert!(!mine.exists(), "our own lock should be removed on exit");
        // Foreign (different pid) → preserved.
        let theirs = tmp.path().join("theirs.lock");
        std::fs::write(
            &theirs,
            serde_json::to_string(&lock(
                std::process::id().wrapping_add(1),
                "2026-01-01T00:00:00Z",
            ))
            .unwrap(),
        )
        .unwrap();
        remove_lock_if_ours(&theirs);
        assert!(
            theirs.exists(),
            "a foreign drive's lock must NOT be removed"
        );
    }

    #[test]
    fn drive_lock_invariant_holds_only_for_a_running_lock() {
        // BUG-716: the pr-ship self-merge gate depends on a LIVE (Running) drain
        // lock being present whenever an orchestrated implementer runs. Only
        // Running satisfies the invariant — a missing lock (plain session) and a
        // Stale lock (crashed drive's leftover, BUG-712) must NOT, or the gate
        // would falsely believe it is outside a drive and allow the self-merge.
        assert!(drive_lock_invariant_holds(&LockStatus::Running(lock(
            1234,
            "2026-06-14T12:00:00Z"
        ))));
        assert!(!drive_lock_invariant_holds(&LockStatus::None));
        assert!(!drive_lock_invariant_holds(&LockStatus::Stale(lock(
            1234,
            "2026-06-14T12:00:00Z"
        ))));
    }

    #[test]
    fn no_existing_lock_acquires() {
        let d = decide_lock(None, now(), 1800, false, |_| true);
        assert_eq!(d, LockDecision::Acquire);
    }

    #[test]
    fn live_fresh_lock_refuses() {
        // started 60s ago, pid alive → refuse.
        let l = lock(4242, "2026-06-14T11:59:00Z");
        let d = decide_lock(Some(l.clone()), now(), 1800, false, |_| true);
        assert_eq!(d, LockDecision::Refuse(l));
    }

    #[test]
    fn dead_pid_lock_reclaims() {
        // pid dead → reclaim even though it's fresh.
        let l = lock(4242, "2026-06-14T11:59:30Z");
        let d = decide_lock(Some(l), now(), 1800, false, |_| false);
        assert_eq!(d, LockDecision::Acquire);
    }

    #[test]
    fn aged_out_lock_reclaims_even_when_pid_alive() {
        // started 3600s ago > 1800 horizon, pid alive → reclaim (pid-recycle backstop).
        let l = lock(4242, "2026-06-14T11:00:00Z");
        let d = decide_lock(Some(l), now(), 1800, false, |_| true);
        assert_eq!(d, LockDecision::Acquire);
    }

    #[test]
    fn force_overrides_a_live_fresh_lock() {
        let l = lock(4242, "2026-06-14T11:59:00Z");
        let d = decide_lock(Some(l), now(), 1800, true, |_| true);
        assert_eq!(d, LockDecision::Acquire);
    }

    #[test]
    fn unparseable_timestamp_falls_back_to_pid_liveness() {
        // age unknown → only pid liveness decides. Alive → refuse.
        let l = lock(4242, "not-a-timestamp");
        let alive = decide_lock(Some(l.clone()), now(), 1800, false, |_| true);
        assert_eq!(alive, LockDecision::Refuse(l));
        // Dead → reclaim.
        let l2 = lock(4242, "not-a-timestamp");
        let dead = decide_lock(Some(l2), now(), 1800, false, |_| false);
        assert_eq!(dead, LockDecision::Acquire);
    }

    #[test]
    fn age_secs_clamps_future_timestamps_to_zero() {
        let l = lock(1, "2026-06-14T13:00:00Z"); // 1h in the future vs now()
        assert_eq!(l.age_secs(now()), Some(0));
    }

    // ── acquire_drain_lock / DrainGuard round-trip (real fs + real pid) ──

    #[test]
    fn acquire_writes_lock_and_guard_drop_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = drain_lock_path(root);
        {
            let _guard = acquire_drain_lock(root, "burndown run (test)").unwrap();
            assert!(path.exists(), "lock file should exist while held");
            let on_disk = read_lock(&path).expect("a parseable lock");
            assert_eq!(on_disk.pid, std::process::id());
            assert_eq!(on_disk.command, "burndown run (test)");
        }
        // Guard dropped → file removed (it still recorded our pid).
        assert!(
            !path.exists(),
            "lock file should be gone after the guard drops"
        );
    }

    #[test]
    fn second_acquire_refuses_while_our_live_pid_holds_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _held = acquire_drain_lock(root, "first drain").unwrap();
        // A second acquire sees a fresh lock recording THIS (alive) pid → refuse.
        let err = acquire_drain_lock(root, "second drain")
            .expect_err("a live lock must refuse the second drain");
        let msg = err.to_string();
        assert!(msg.contains("a drain is already running"), "msg was: {msg}");
        assert!(
            msg.contains("AIDA_DRAIN_FORCE"),
            "msg should name the override: {msg}"
        );
    }

    #[test]
    fn acquire_reclaims_a_dead_pid_lock() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = drain_lock_path(root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // A lock recording a pid that is not alive, written "just now".
        let dead = DrainLock {
            pid: 999_999_999,
            started_at_utc: Utc::now().to_rfc3339(),
            command: "crashed drain".to_string(),
            host: "ghost".to_string(),
            specs: Vec::new(),
        };
        std::fs::write(&path, serde_json::to_string(&dead).unwrap()).unwrap();
        // Reclaim succeeds and rewrites the file with OUR pid.
        let _guard = acquire_drain_lock(root, "fresh drain").unwrap();
        assert_eq!(read_lock(&path).unwrap().pid, std::process::id());
    }

    #[test]
    fn guard_drop_leaves_a_successor_lock_intact() {
        // If our guard's file was reclaimed by another drain (different pid),
        // Drop must NOT delete the successor's live lock.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = drain_lock_path(root);
        let guard = acquire_drain_lock(root, "ours").unwrap();
        // Simulate a successor stomping the file with a different pid.
        let successor = DrainLock {
            pid: std::process::id().wrapping_add(1),
            started_at_utc: Utc::now().to_rfc3339(),
            command: "successor".to_string(),
            host: "h".to_string(),
            specs: Vec::new(),
        };
        std::fs::write(&path, serde_json::to_string(&successor).unwrap()).unwrap();
        drop(guard);
        // The successor's lock survives our Drop.
        let on_disk = read_lock(&path).expect("successor lock should remain");
        assert_eq!(on_disk.command, "successor");
    }

    #[test]
    fn borrow_guard_preserves_parent_lock_on_drop() {
        // BUG-748: a nested internal drive must be able to run under a parent
        // drain without overwriting the parent's lock or releasing it when the
        // child exits. This is the batch/integrator failure mode that left the
        // next orchestrated implementer without a live lock.
        let _env = crate::test_env::EnvVarsGuard::set(&[(BORROW_ENV, "1"), (FORCE_ENV, "1")]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = drain_lock_path(root);
        let parent = acquire_drain_lock(root, "parent batch drain").unwrap();
        let parent_lock = read_lock(&path).expect("parent lock exists");

        let borrowed = acquire_drain_lock(root, "internal child drive").unwrap();
        assert_eq!(
            read_lock(&path).expect("borrowed child must not rewrite lock"),
            parent_lock
        );
        drop(borrowed);
        assert_eq!(
            read_lock(&path).expect("borrowed child must not release lock"),
            parent_lock
        );

        drop(parent);
        assert!(
            !path.exists(),
            "owning parent guard still releases the lock"
        );
    }

    #[test]
    fn borrow_without_live_parent_falls_back_to_normal_acquire() {
        // A leaked AIDA_DRAIN_BORROW must not bypass the lock if there is no
        // live parent to borrow; the command becomes the owner as usual.
        let _env = crate::test_env::EnvVarGuard::set(BORROW_ENV, "1");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = drain_lock_path(root);
        let guard = acquire_drain_lock(root, "standalone drain").unwrap();
        let on_disk = read_lock(&path).expect("standalone borrow fallback owns lock");
        assert_eq!(on_disk.command, "standalone drain");
        drop(guard);
        assert!(!path.exists(), "fallback owner releases normally");
    }

    // ── probe_lock / classify_lock (read-side, TASK-806) ──

    #[test]
    fn classify_lock_none_when_no_file() {
        assert_eq!(classify_lock(None, |_| true), LockStatus::None);
    }

    #[test]
    fn classify_lock_running_when_pid_alive() {
        let l = lock(4242, "2026-06-14T11:59:00Z");
        assert_eq!(
            classify_lock(Some(l.clone()), |_| true),
            LockStatus::Running(l)
        );
    }

    #[test]
    fn classify_lock_stale_when_pid_dead() {
        let l = lock(4242, "2026-06-14T11:59:00Z");
        assert_eq!(
            classify_lock(Some(l.clone()), |_| false),
            LockStatus::Stale(l)
        );
    }

    #[test]
    fn probe_lock_reads_a_just_written_lock() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // A live lock recording OUR pid round-trips through probe_lock as Running.
        let _guard = acquire_drain_lock(root, "burndown run --status approved").unwrap();
        match probe_lock(root) {
            LockStatus::Running(l) => {
                assert_eq!(l.pid, std::process::id());
                assert_eq!(l.command, "burndown run --status approved");
            }
            other => panic!("expected Running, got {other:?}"),
        }
    }

    #[test]
    fn probe_lock_none_on_empty_root() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(probe_lock(dir.path()), LockStatus::None);
    }

    // ── BUG-759: lock lifetime == launcher lifetime, probe-visible spec set ──

    // Regression (BUG-759): the launcher-held lock must be probe-visible as a
    // live drain — pid, started, command, spec set — for the ENTIRE time the
    // guard is held, and gone the moment it drops. This is the read-side
    // contract `aida drain status` relies on to never print "No drain in
    // progress" while a `burndown run` launcher is alive.
    #[test]
    fn lock_with_specs_is_probe_visible_for_the_guards_whole_lifetime() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let specs = vec!["BUG-101".to_string(), "TASK-202".to_string()];
        {
            let _guard =
                acquire_drain_lock_with_specs(root, "burndown run (status=approved)", &specs)
                    .unwrap();
            match probe_lock(root) {
                LockStatus::Running(l) => {
                    assert_eq!(l.pid, std::process::id());
                    assert_eq!(l.command, "burndown run (status=approved)");
                    assert_eq!(l.specs, specs, "the blessed spec set must be probe-visible");
                }
                other => panic!("expected Running while the guard is held, got {other:?}"),
            }
            // A second drain launched during the window is refused (the lock is
            // coextensive with the launcher, so this holds for its whole run).
            let err = acquire_drain_lock(root, "second burndown run")
                .expect_err("a live launcher-held lock must refuse a second drain");
            assert!(err.to_string().contains("a drain is already running"));
        }
        // Guard dropped (launcher exited) → the lock is gone, probe reads None.
        assert_eq!(probe_lock(root), LockStatus::None);
    }

    // BUG-759: a lock written by an older binary (no `specs` field) still
    // parses — the field defaults to empty so a mid-upgrade drain stays
    // observable and reclaimable.
    #[test]
    fn pre_specs_lock_json_parses_with_empty_spec_set() {
        let body = r#"{
          "pid": 4242,
          "started_at_utc": "2026-06-14T11:59:00Z",
          "command": "burndown run (status=approved)",
          "host": "h"
        }"#;
        let parsed: DrainLock = serde_json::from_str(body).unwrap();
        assert!(parsed.specs.is_empty());
        assert_eq!(parsed.pid, 4242);
    }
}
