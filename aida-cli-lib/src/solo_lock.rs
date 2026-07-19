//! Solo-loop lock (STORY-627) — `.aida/solo.lock`.
//!
//! # The problem
//!
//! `aida solo run` is a long-running foreground loop that INTEGRATES on `main`
//! (it shells out to `intake --apply` / `burndown run` / `queue integrate`). Two
//! solo loops running against the same repo tree double-drive it the same way two
//! drains would. Worse, before this lock there was no way to tell a *live* loop
//! from a *dead* one: a Ctrl-C-killed loop left the `~/.aida/solo.toml` flag ON,
//! so `aida solo status` reported "ON" with no process behind it, and a second
//! `aida solo run` happily started alongside a still-running one. trace:STORY-627
//!
//! # The lock (mirrors `drain_lock`, BUG-538)
//!
//! On `aida solo run`, the loop acquires a per-repo lock: a single JSON file at
//! `.aida/solo.lock` carrying the holder's `pid`, `started_at_utc`, and `host`. A
//! second `aida solo run` reads it and:
//!
//! - **live** (pid alive) → REFUSE, surfacing the holder's pid / start.
//! - **stale** (pid dead) → reclaim and proceed.
//!
//! Release is RAII: [`SoloGuard`]'s `Drop` removes the file on a clean exit (or
//! when the flag-poll breaks the loop). A loop killed by Ctrl-C skips `Drop`, but
//! the next `aida solo run` stale-reclaims it (its pid is dead). `aida solo stop`
//! reads the lock and, if a live pid is found, SIGTERMs it (best-effort) in
//! addition to clearing the flag, so stop works even mid-step.
//!
//! Liveness is the [`process_probe::pid_is_alive`] probe the drain lock uses; the
//! pure decision (`decide` / `classify`) is split out so the paths are
//! unit-testable without spawning real processes. trace:STORY-627 | ai:claude

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::process_probe;

/// File name under `.aida/` holding the live solo loop's lock. Gitignored by the
/// deny-by-default `.aida/*` rule — pure per-clone runtime state.
const SOLO_LOCK_FILE: &str = "solo.lock";

/// Path of the solo lock under `project_root`.
pub(crate) fn solo_lock_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join(SOLO_LOCK_FILE)
}

/// On-disk lock record. Serialized as JSON via [`aida_core::write_atomic`] so a
/// concurrent reader never sees a torn file. Mirrors `drain_lock::DrainLock`
/// minus the `command` field (there is only one solo command).
/// trace:STORY-627 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SoloLock {
    /// PID of the solo-loop process holding the lock.
    pub(crate) pid: u32,
    /// RFC3339 UTC timestamp of when the lock was taken.
    pub(crate) started_at_utc: String,
    /// Host the loop runs on — informational, for cross-machine shared clones.
    pub(crate) host: String,
}

/// The pure decision: given the lock currently on disk (if any), should a new
/// solo loop ACQUIRE (write its own, possibly reclaiming a stale one) or be
/// REFUSED? Liveness is injected as a predicate so both paths (no-lock /
/// live-refuse / stale-reclaim) are unit-testable without real processes —
/// mirrors `drain_lock::decide_lock`. trace:STORY-627 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LockDecision {
    /// No live lock stands in the way — write ours.
    Acquire,
    /// A live loop holds the lock — refuse, surfacing the holder.
    Refuse(SoloLock),
}

/// Decide ACQUIRE vs REFUSE. An absent / unparseable on-disk lock is ACQUIRE. A
/// present lock is reclaimable (→ ACQUIRE) when its pid is dead; otherwise
/// REFUSE. Unlike the drain lock there is no age backstop — the solo loop is
/// expected to run for hours (up to its TTL), so age says nothing about
/// staleness; only pid-liveness does. trace:STORY-627 | ai:claude
pub(crate) fn decide_lock(
    existing: Option<SoloLock>,
    is_alive: impl Fn(u32) -> bool,
) -> LockDecision {
    match existing {
        None => LockDecision::Acquire,
        Some(lock) => {
            if is_alive(lock.pid) {
                LockDecision::Refuse(lock)
            } else {
                LockDecision::Acquire
            }
        }
    }
}

/// Read + parse the on-disk lock, if present and well-formed. A missing file or
/// a parse error both yield `None` — a corrupt lock must never wedge the loop
/// (it is treated as "no lock", i.e. reclaimable).
fn read_lock(path: &Path) -> Option<SoloLock> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Best-effort host name for the lock record (informational only).
fn hostname() -> String {
    sysinfo::System::host_name().unwrap_or_default()
}

/// Acquire the solo-loop lock for `project_root`.
///
/// On success returns a [`SoloGuard`] that removes the lock on `Drop`. On a live
/// conflict returns an `Err` whose message names the holder and the recovery
/// path (`aida solo stop`, or — if certain it's dead — remove the file).
/// trace:STORY-627 | ai:claude
pub(crate) fn acquire_solo_lock(project_root: &Path) -> Result<SoloGuard> {
    let path = solo_lock_path(project_root);
    let existing = read_lock(&path);

    // STORY-638: consult the SHARED solo claim on the `aida-store` branch BEFORE
    // the local lock, so a second CLONE is refused while one holds an active solo
    // loop (MU-506). Solo is process-backed → same-host pid liveness is the
    // authoritative reclaim signal (no age backstop; the loop runs for hours, so
    // a long TTL is the cross-host floor). A live cross-clone solo loop errors
    // here; no remote / unreachable store WARNs and proceeds local-only.
    // `AIDA_DRAIN_FORCE=1` is the shared escape (one drain/solo override knob).
    // trace:STORY-638 | ai:claude
    let store_root = project_root.join(".aida-store");
    let forced = std::env::var("AIDA_DRAIN_FORCE")
        .map(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    let store_claimed = match crate::coordination::acquire_lock_claim(
        &store_root,
        crate::coordination::LockKind::Solo,
        project_root,
        "solo run",
        crate::coordination::DEFAULT_TTL_SECS,
        forced,
    ) {
        Ok(crate::coordination::LockAcquireOutcome::Acquired) => true,
        Ok(crate::coordination::LockAcquireOutcome::Reclaimed(reason)) => {
            eprintln!(
                "  {} reclaiming a stale cross-clone solo claim ({reason})",
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
    };

    match decide_lock(existing.clone(), process_probe::pid_is_alive) {
        LockDecision::Refuse(holder) => {
            anyhow::bail!(
                "a solo loop is already running (pid {}, started {}{}).\n  \
                 Stop it with `aida solo stop`, or — if you're certain it's dead — remove {}.",
                holder.pid,
                holder.started_at_utc,
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
            // re-run after a Ctrl-C isn't silently surprising. trace:STORY-627
            if let Some(stale) = &existing {
                eprintln!(
                    "  {} reclaiming a stale solo lock (pid {}, started {} — not running)",
                    crate::glyphs::get(
                        crate::glyphs::Glyph::InfoAlt,
                        crate::find_project_root().ok().as_deref(),
                    ),
                    stale.pid,
                    stale.started_at_utc
                );
            }
            // Best-effort parent-dir create — `.aida/` normally already exists.
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let record = SoloLock {
                pid: std::process::id(),
                started_at_utc: chrono::Utc::now().to_rfc3339(),
                host: hostname(),
            };
            let json = serde_json::to_string_pretty(&record).unwrap_or_else(|_| "{}".to_string());
            aida_core::write_atomic(&path, json).map_err(|e| {
                anyhow::anyhow!("could not write solo lock {}: {e}", path.display())
            })?;
            Ok(SoloGuard {
                path,
                pid: record.pid,
                store_root: if store_claimed {
                    Some(store_root)
                } else {
                    None
                },
                project_root: project_root.to_path_buf(),
            })
        }
    }
}

/// Liveness-corroborated read of the solo lock for `aida solo stop` / `status`.
/// Mirrors `drain_lock::LockStatus`: the on-disk lock is classified against a
/// PID-liveness probe so callers can tell a live loop from a crashed one.
/// trace:STORY-627 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LockStatus {
    /// No lock file — no loop is (or recently was) running.
    None,
    /// The lock is present and its `pid` is alive — a loop is running.
    Running(SoloLock),
    /// The lock is present but its `pid` is dead — a loop crashed or was
    /// Ctrl-C'd without releasing it. The next `aida solo run` stale-reclaims it.
    Stale(SoloLock),
}

/// Read `.aida/solo.lock` and corroborate the recorded `pid` against a liveness
/// probe. A missing or corrupt lock is [`LockStatus::None`]. trace:STORY-627
pub(crate) fn probe_lock(project_root: &Path) -> LockStatus {
    classify_lock(
        read_lock(&solo_lock_path(project_root)),
        process_probe::pid_is_alive,
    )
}

/// Pure classifier: split from [`probe_lock`] so the three paths
/// (none / running / stale) are unit-testable without a real lock or pid.
/// trace:STORY-627 | ai:claude
fn classify_lock(existing: Option<SoloLock>, is_alive: impl Fn(u32) -> bool) -> LockStatus {
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

/// Best-effort SIGTERM to `pid` (cross-platform via `sysinfo`, mirroring
/// `exit_signal::signal_process_tree`). Returns `true` if the signal was
/// delivered. Used by `aida solo stop` to terminate a live loop mid-step so stop
/// takes effect even when the loop is blocked inside a long `claude -p` step.
/// trace:STORY-627 | ai:claude
pub(crate) fn signal_stop(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, Signal, System};
    let mut sys =
        System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::new()));
    sys.refresh_processes_specifics(ProcessRefreshKind::new());
    match sys.process(Pid::from_u32(pid)) {
        Some(proc_) => {
            // kill_with(Term) returns None on a platform without SIGTERM
            // (Windows) — fall back to a hard kill there.
            if proc_.kill_with(Signal::Term).is_none() {
                proc_.kill()
            } else {
                true
            }
        }
        None => false,
    }
}

/// RAII handle: while held, this process owns the solo lock. `Drop` removes the
/// lock file — but ONLY if it still records THIS process's pid, so a guard that
/// outlives a stale-reclaim by a successor never deletes the successor's live
/// lock. trace:STORY-627 | ai:claude
#[derive(Debug)]
pub(crate) struct SoloGuard {
    path: PathBuf,
    pid: u32,
    /// Set when we hold the SHARED cross-clone solo claim on the store (STORY-638).
    /// `None` when cross-clone coordination was unavailable (local-only).
    /// trace:STORY-638 | ai:claude
    store_root: Option<PathBuf>,
    project_root: PathBuf,
}

impl SoloGuard {
    /// Refresh the shared solo claim's heartbeat on each loop tick so a
    /// long-running solo loop never ages past its TTL. No-op when cross-clone
    /// coordination is unavailable. Best-effort. trace:STORY-638 | ai:claude
    pub(crate) fn heartbeat(&self) {
        if let Some(store_root) = &self.store_root {
            crate::coordination::heartbeat_lock_claim(
                store_root,
                crate::coordination::LockKind::Solo,
                &self.project_root,
            );
        }
    }
}

impl Drop for SoloGuard {
    fn drop(&mut self) {
        // Release the shared cross-clone claim first (best-effort; staleness
        // covers a crash). trace:STORY-638 | ai:claude
        if let Some(store_root) = &self.store_root {
            crate::coordination::release_lock_claim(
                store_root,
                crate::coordination::LockKind::Solo,
                &self.project_root,
            );
        }
        if read_lock(&self.path).map(|l| l.pid) == Some(self.pid) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock(pid: u32, started_at_utc: &str) -> SoloLock {
        SoloLock {
            pid,
            started_at_utc: started_at_utc.to_string(),
            host: "testhost".to_string(),
        }
    }

    // ── decide_lock (pure) ──

    #[test]
    fn no_existing_lock_acquires() {
        assert_eq!(decide_lock(None, |_| true), LockDecision::Acquire);
    }

    #[test]
    fn live_lock_refuses() {
        let l = lock(4242, "2026-06-15T11:59:00Z");
        assert_eq!(
            decide_lock(Some(l.clone()), |_| true),
            LockDecision::Refuse(l)
        );
    }

    #[test]
    fn dead_pid_lock_reclaims() {
        let l = lock(4242, "2026-06-15T11:59:00Z");
        assert_eq!(decide_lock(Some(l), |_| false), LockDecision::Acquire);
    }

    // ── classify_lock (read-side) ──

    #[test]
    fn classify_lock_none_when_no_file() {
        assert_eq!(classify_lock(None, |_| true), LockStatus::None);
    }

    #[test]
    fn classify_lock_running_when_pid_alive() {
        let l = lock(4242, "2026-06-15T11:59:00Z");
        assert_eq!(
            classify_lock(Some(l.clone()), |_| true),
            LockStatus::Running(l)
        );
    }

    #[test]
    fn classify_lock_stale_when_pid_dead() {
        let l = lock(4242, "2026-06-15T11:59:00Z");
        assert_eq!(
            classify_lock(Some(l.clone()), |_| false),
            LockStatus::Stale(l)
        );
    }

    // ── acquire_solo_lock / SoloGuard round-trip (real fs + real pid) ──

    #[test]
    fn acquire_writes_lock_and_guard_drop_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = solo_lock_path(root);
        {
            let _guard = acquire_solo_lock(root).unwrap();
            assert!(path.exists(), "lock file should exist while held");
            let on_disk = read_lock(&path).expect("a parseable lock");
            assert_eq!(on_disk.pid, std::process::id());
        }
        assert!(
            !path.exists(),
            "lock file should be gone after the guard drops"
        );
    }

    #[test]
    fn second_acquire_refuses_while_our_live_pid_holds_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _held = acquire_solo_lock(root).unwrap();
        let err = acquire_solo_lock(root).expect_err("a live lock must refuse the second loop");
        let msg = err.to_string();
        assert!(
            msg.contains("a solo loop is already running"),
            "msg was: {msg}"
        );
        assert!(
            msg.contains("aida solo stop"),
            "msg should name the recovery path: {msg}"
        );
    }

    #[test]
    fn acquire_reclaims_a_dead_pid_lock() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = solo_lock_path(root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let dead = SoloLock {
            pid: 999_999_999,
            started_at_utc: chrono::Utc::now().to_rfc3339(),
            host: "ghost".to_string(),
        };
        std::fs::write(&path, serde_json::to_string(&dead).unwrap()).unwrap();
        let _guard = acquire_solo_lock(root).unwrap();
        assert_eq!(read_lock(&path).unwrap().pid, std::process::id());
    }

    #[test]
    fn guard_drop_leaves_a_successor_lock_intact() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = solo_lock_path(root);
        let guard = acquire_solo_lock(root).unwrap();
        let successor = SoloLock {
            pid: std::process::id().wrapping_add(1),
            started_at_utc: chrono::Utc::now().to_rfc3339(),
            host: "h".to_string(),
        };
        std::fs::write(&path, serde_json::to_string(&successor).unwrap()).unwrap();
        drop(guard);
        let on_disk = read_lock(&path).expect("successor lock should remain");
        assert_eq!(on_disk.pid, successor.pid);
    }

    #[test]
    fn probe_lock_reads_a_just_written_lock() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _guard = acquire_solo_lock(root).unwrap();
        match probe_lock(root) {
            LockStatus::Running(l) => assert_eq!(l.pid, std::process::id()),
            other => panic!("expected Running, got {other:?}"),
        }
    }

    #[test]
    fn probe_lock_none_on_empty_root() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(probe_lock(dir.path()), LockStatus::None);
    }

    #[test]
    fn signal_stop_returns_false_for_dead_pid() {
        // A PID near u32::MAX is not in use → no signal delivered.
        assert!(!signal_stop(u32::MAX - 1));
    }
}
