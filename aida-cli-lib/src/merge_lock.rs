//! STORY-1171: the branch merge-serialization lock (the "merge-lease").
//!
//! A per-target-branch lockfile at the MAIN clone's `.aida/merge-locks/<branch>`
//! that a merger ACQUIRES before the check→merge→pull critical section, holds
//! through it, and releases after — so no two mergers race to the same branch.
//! This SERIALIZES mergers; it is distinct from, and complements, the per-PR
//! `merge_hold` marker + Layer-2 branch protection (which GATE one PR but do not
//! serialize two green PRs). It is also distinct from git's transient
//! `index.lock` (which correctly does NOT block targeted single-spec store
//! writes). See ADR-38.
//!
//! Scope (ADR-38 decision 1): LOCAL file, same-machine mergers only — that is the
//! race we actually observed (advisor session + product session + drain, one
//! machine). Cross-host fleet serialization is a future extension.
//!
//! Contention (decision 2): bounded-wait-then-refuse. Stale-lock recovery mirrors
//! the drain-lock (coordination.rs) / index.lock (BUG-1164): a same-host DEAD pid
//! is reclaimed immediately; a TTL backstop covers any other wedge.
// trace:STORY-1171 | ai:claude

// The acquire-side API (acquire/MergeLease/DEFAULT_WAIT and its helpers) is
// exercised by the tests below and lands wired into the merge paths (pr ship +
// drain, spanning check→merge→pull) in the STORY-1171 follow-up; `status()` is
// live now via `aida merge-lock`. Allow dead_code until the wiring lands.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Bounded wait before a contending merger gives up and REFUSES (decision 2).
pub(crate) const DEFAULT_WAIT: Duration = Duration::from_secs(180);
/// Poll cadence while waiting for a held lock to free.
const POLL: Duration = Duration::from_secs(2);
/// TTL backstop: a lock older than this is reclaimable even when we cannot probe
/// the holder's pid. A merge is seconds-to-a-minute, so 10m is a generous crash
/// horizon (matches the drain-lock default).
const DEFAULT_TTL_SECS: u64 = 600;

fn locks_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("merge-locks")
}

/// Branch → single path segment (feature branches contain `/`).
fn branch_slug(branch: &str) -> String {
    branch
        .chars()
        .map(|c| {
            if c == '/' || c == std::path::MAIN_SEPARATOR {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// The lockfile path for one target branch. Public so callers/status can log it.
pub(crate) fn lock_path(project_root: &Path, branch: &str) -> PathBuf {
    locks_dir(project_root).join(branch_slug(branch))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The recorded holder of a merge-lease — enough for stale-recovery + `status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Holder {
    pub host: String,
    pub pid: u32,
    pub pr: Option<u64>,
    pub note: String,
    pub acquired_at: u64,
    pub ttl_secs: u64,
}

fn serialize(h: &Holder) -> String {
    format!(
        "host={}\npid={}\npr={}\nnote={}\nacquired_at={}\nttl_secs={}\n",
        h.host,
        h.pid,
        h.pr.map(|p| p.to_string()).unwrap_or_default(),
        h.note.replace('\n', " "),
        h.acquired_at,
        h.ttl_secs,
    )
}

/// Lenient parse — an unreadable/garbled field falls back to a safe default so a
/// corrupt lockfile still reclaims via the TTL rather than wedging forever.
fn parse(body: &str) -> Holder {
    let mut h = Holder {
        host: String::new(),
        pid: 0,
        pr: None,
        note: String::new(),
        acquired_at: 0,
        ttl_secs: DEFAULT_TTL_SECS,
    };
    for line in body.lines() {
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            match k.trim() {
                "host" => h.host = v.to_string(),
                "pid" => h.pid = v.parse().unwrap_or(0),
                "pr" => h.pr = v.parse().ok(),
                "note" => h.note = v.to_string(),
                "acquired_at" => h.acquired_at = v.parse().unwrap_or(0),
                "ttl_secs" => h.ttl_secs = v.parse().unwrap_or(DEFAULT_TTL_SECS),
                _ => {}
            }
        }
    }
    h
}

/// Is `pid` currently running on THIS machine? `kill(pid, 0)` == 0 (alive) or
/// EPERM (alive but not ours). ESRCH ⇒ dead. Non-unix cannot probe ⇒ assume
/// alive so only the TTL backstop reclaims (conservative).
#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: kill with signal 0 performs no signal delivery, only existence/perm check.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
#[cfg(not(unix))]
fn pid_alive(_pid: u32) -> bool {
    true
}

/// `Some(reason)` if a foreign holder is reclaimable: a DEAD pid on our host
/// (fast, exact), or any holder aged past its TTL (backstop). `None` ⇒ live.
fn stale_reason(h: &Holder, our_host: &str) -> Option<String> {
    if !h.host.is_empty() && h.host == our_host && !pid_alive(h.pid) {
        return Some(format!("holder pid {} is dead on this host", h.pid));
    }
    let ttl = if h.ttl_secs == 0 {
        DEFAULT_TTL_SECS
    } else {
        h.ttl_secs
    };
    let age = now_secs().saturating_sub(h.acquired_at);
    if age > ttl {
        return Some(format!("holder aged {age}s past its {ttl}s TTL"));
    }
    None
}

/// RAII guard: releasing (explicitly or on drop) removes the lockfile IF we hold
/// it. A no-op if already released.
#[must_use = "the merge-lease releases when dropped; hold it across the critical section"]
#[derive(Debug)]
pub(crate) struct MergeLease {
    path: PathBuf,
    held: bool,
}

impl MergeLease {
    /// Explicit release (idempotent). Dropping does the same.
    pub(crate) fn release(mut self) {
        self.do_release();
    }
    fn do_release(&mut self) {
        if self.held {
            let _ = std::fs::remove_file(&self.path);
            self.held = false;
        }
    }
}

impl Drop for MergeLease {
    fn drop(&mut self) {
        self.do_release();
    }
}

/// Acquire the merge-lease for `branch`, waiting up to `wait` on contention
/// (decision 2). Steals a stale holder (dead-same-host-pid or TTL-expired).
/// Returns `WouldBlock` if a live holder outlasts the wait — the caller REFUSES.
pub(crate) fn acquire(
    project_root: &Path,
    branch: &str,
    pr: Option<u64>,
    note: &str,
    wait: Duration,
) -> std::io::Result<MergeLease> {
    let dir = locks_dir(project_root);
    std::fs::create_dir_all(&dir)?;
    let path = lock_path(project_root, branch);
    let our_host = crate::coordination::hostname();
    let deadline = Instant::now() + wait;
    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                let h = Holder {
                    host: our_host.clone(),
                    pid: std::process::id(),
                    pr,
                    note: note.to_string(),
                    acquired_at: now_secs(),
                    ttl_secs: DEFAULT_TTL_SECS,
                };
                f.write_all(serialize(&h).as_bytes())?;
                return Ok(MergeLease { path, held: true });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let reclaimable = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|b| stale_reason(&parse(&b), &our_host));
                if reclaimable.is_some() {
                    // Steal the stale lock and retry immediately.
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                if Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        format!(
                            "merge-lease for '{branch}' is held by a live merger; waited {}s",
                            wait.as_secs()
                        ),
                    ));
                }
                std::thread::sleep(POLL);
            }
            Err(e) => return Err(e),
        }
    }
}

/// Every active merge-lease as `(branch, holder)`, ascending by branch — the
/// `aida merge-lock status` surface.
pub(crate) fn status(project_root: &Path) -> Vec<(String, Holder)> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(locks_dir(project_root)) {
        for e in entries.flatten() {
            if let Ok(body) = std::fs::read_to_string(e.path()) {
                out.push((e.file_name().to_string_lossy().to_string(), parse(&body)));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_serializes_and_bounded_wait_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let held = acquire(root, "main", Some(1), "first", DEFAULT_WAIT).unwrap();
        // A second acquire with a tiny wait must REFUSE (WouldBlock) while held.
        let start = Instant::now();
        let err = acquire(root, "main", Some(2), "second", Duration::from_millis(300)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
        assert!(
            start.elapsed() >= Duration::from_millis(250),
            "must actually wait the bound"
        );
        // Release → the branch is free again.
        held.release();
        acquire(root, "main", Some(3), "third", Duration::from_millis(300)).unwrap();
    }

    #[test]
    fn drop_releases() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        {
            let _g = acquire(root, "main", None, "x", DEFAULT_WAIT).unwrap();
            assert!(lock_path(root, "main").exists());
        }
        assert!(
            !lock_path(root, "main").exists(),
            "drop must remove the lockfile"
        );
    }

    #[test]
    fn stale_dead_pid_same_host_is_reclaimed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(locks_dir(root)).unwrap();
        // A holder on OUR host with a pid that cannot be alive (pid 999999999 → not running).
        let h = Holder {
            host: crate::coordination::hostname(),
            pid: 999_999_999,
            pr: Some(9),
            note: "dead".into(),
            acquired_at: now_secs(),
            ttl_secs: DEFAULT_TTL_SECS,
        };
        std::fs::write(lock_path(root, "main"), serialize(&h)).unwrap();
        // Fresh timestamp but dead same-host pid → reclaimed immediately (no wait).
        let start = Instant::now();
        let _g = acquire(root, "main", Some(10), "steal", Duration::from_millis(200)).unwrap();
        assert!(
            start.elapsed() < Duration::from_millis(150),
            "dead-pid steal is immediate"
        );
    }

    #[test]
    fn stale_ttl_expired_is_reclaimed_any_host() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(locks_dir(root)).unwrap();
        // A holder on a DIFFERENT host (can't probe pid) but aged past TTL → reclaimable.
        let h = Holder {
            host: "some-other-host".into(),
            pid: std::process::id(), // alive here, but host differs so pid path is skipped
            pr: Some(1),
            note: "old".into(),
            acquired_at: now_secs().saturating_sub(10_000),
            ttl_secs: 600,
        };
        std::fs::write(lock_path(root, "main"), serialize(&h)).unwrap();
        acquire(root, "main", Some(2), "steal", Duration::from_millis(200)).unwrap();
    }

    #[test]
    fn status_lists_active_leases() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _a = acquire(root, "main", Some(1), "on main", DEFAULT_WAIT).unwrap();
        let _b = acquire(root, "release/2", Some(2), "on release", DEFAULT_WAIT).unwrap();
        let st = status(root);
        let branches: Vec<_> = st.iter().map(|(b, _)| b.clone()).collect();
        assert_eq!(branches, vec!["main".to_string(), "release_2".to_string()]);
        assert_eq!(st[0].1.pr, Some(1));
    }
}
