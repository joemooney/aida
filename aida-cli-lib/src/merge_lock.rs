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
    /// Kernel start time of `pid` when it took the lease, so a recycled pid
    /// does not read as the live holder. `None` for a lockfile written before
    /// TASK-1750; the liveness check then degrades to a bare pid probe.
    // trace:TASK-1750 | ai:claude
    pub pid_start: Option<String>,
    pub pr: Option<u64>,
    pub note: String,
    pub acquired_at: u64,
    pub ttl_secs: u64,
}

fn serialize(h: &Holder) -> String {
    format!(
        "host={}\npid={}\npid_start={}\npr={}\nnote={}\nacquired_at={}\nttl_secs={}\n",
        h.host,
        h.pid,
        h.pid_start.as_deref().unwrap_or_default(),
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
        pid_start: None,
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
                "pid_start" => h.pid_start = Some(v.to_string()).filter(|v| !v.is_empty()),
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

/// Is the holder's process still running on THIS machine? Routed through the
/// canonical `aida_core::liveness::process_identity_is_alive` (BUG-1741,
/// TASK-1750): a defunct (zombie) holder reads dead instead of pinning the
/// lock until its TTL, and a pid the OS recycled for an unrelated process
/// reads dead because its kernel start time no longer matches `pid_start`.
// trace:BUG-1741 | ai:claude
// trace:TASK-1750 | ai:claude
fn holder_alive(h: &Holder) -> bool {
    aida_core::liveness::process_identity_is_alive(h.pid, h.pid_start.as_deref())
}

/// `Some(reason)` if a foreign holder is reclaimable: a DEAD pid on our host
/// (fast, exact), or any holder aged past its TTL (backstop). `None` ⇒ live.
fn stale_reason(h: &Holder, our_host: &str) -> Option<String> {
    if !h.host.is_empty() && h.host == our_host && !holder_alive(h) {
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

/// RAII guard: dropping it removes the lockfile IF we hold it. Release early
/// with `drop(lease)`.
#[must_use = "the merge-lease releases when dropped; hold it across the critical section"]
#[derive(Debug)]
pub(crate) struct MergeLease {
    path: PathBuf,
    held: bool,
}

impl MergeLease {
    // trace:TASK-1581 | ai:antigravity — the explicit `release(self)` had no
    // caller; `drop(lease)` is the same operation.
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
                    pid_start: aida_core::liveness::process_start_identity(std::process::id()),
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
        // trace:STORY-1171.ac991dc3 | ai:codex
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
        drop(held);
        let _ = acquire(root, "main", Some(3), "third", Duration::from_millis(300)).unwrap();
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

    #[cfg(unix)]
    #[test]
    fn stale_dead_pid_same_host_is_reclaimed() {
        // trace:STORY-1171.acf58627 | ai:codex
        // trace:BUG-1184 | ai:codex
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(locks_dir(root)).unwrap();
        // A holder on OUR host with a pid that cannot be alive (pid 999999999 → not running).
        let h = Holder {
            host: crate::coordination::hostname(),
            pid: 999_999_999,
            pid_start: None,
            pr: Some(9),
            note: "dead".into(),
            acquired_at: now_secs(),
            ttl_secs: DEFAULT_TTL_SECS,
        };
        std::fs::write(lock_path(root, "main"), serialize(&h)).unwrap();
        // Fresh timestamp but dead same-host pid → reclaimed immediately (no wait).
        let start = Instant::now();
        let _g = acquire(root, "main", Some(10), "steal", Duration::from_millis(200)).unwrap();
        // The `unwrap` above carries "reclaimed": a holder not recognised as
        // stale makes `acquire` spin to its 200ms deadline and return Err. So
        // this is only the "no wait" signal — and at 150ms against a 200ms
        // deadline it had 33% margin over a host condition, which is how a
        // correct steal went red under load. trace:BUG-1731 | ai:claude
        crate::test_timing::assert_within_budget(
            start,
            Duration::from_millis(150),
            Duration::from_secs(60),
            "steal of a merge lock held by a dead same-host pid",
        );
    }

    #[cfg(not(unix))]
    #[test]
    fn same_host_live_pid_waits_until_ttl_on_non_unix() {
        // trace:BUG-1184 | ai:codex
        // trace:BUG-1777 | ai:codex
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(locks_dir(root)).unwrap();
        // Non-Unix probes the OS process table, so use this live process to
        // test the wait path. A bogus PID is correctly reclaimed immediately.
        let h = Holder {
            host: crate::coordination::hostname(),
            pid: std::process::id(),
            pid_start: None,
            pr: Some(9),
            note: "fresh live pid".into(),
            acquired_at: now_secs(),
            ttl_secs: DEFAULT_TTL_SECS,
        };
        std::fs::write(lock_path(root, "main"), serialize(&h)).unwrap();

        let err = acquire(
            root,
            "main",
            Some(10),
            "blocked",
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);

        let mut expired = h;
        expired.note = "expired bogus pid".into();
        expired.acquired_at = now_secs().saturating_sub(10_000);
        expired.ttl_secs = 600;
        std::fs::write(lock_path(root, "main"), serialize(&expired)).unwrap();
        let _g = acquire(
            root,
            "main",
            Some(11),
            "ttl steal",
            Duration::from_millis(200),
        )
        .unwrap();
    }

    /// TASK-1750: a same-host holder whose pid is alive but whose recorded
    /// start time differs is a recycled pid — reclaimed at once, not after
    /// the TTL. The matching start time keeps the live holder's lock.
    // trace:TASK-1750 | ai:claude
    #[test]
    fn recycled_pid_same_host_is_reclaimed_but_true_holder_waits() {
        let Some(our_start) = aida_core::liveness::process_start_identity(std::process::id())
        else {
            return; // platform cannot report start times; nothing to distinguish
        };
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(locks_dir(root)).unwrap();
        let mut h = Holder {
            host: crate::coordination::hostname(),
            pid: std::process::id(),
            pid_start: Some(our_start),
            pr: Some(9),
            note: "true holder".into(),
            acquired_at: now_secs(),
            ttl_secs: DEFAULT_TTL_SECS,
        };
        std::fs::write(lock_path(root, "main"), serialize(&h)).unwrap();
        let err = acquire(
            root,
            "main",
            Some(10),
            "blocked",
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);

        h.pid_start = Some("2000-01-01T00:00:00+00:00".into());
        h.note = "recycled pid".into();
        std::fs::write(lock_path(root, "main"), serialize(&h)).unwrap();
        let _g = acquire(root, "main", Some(11), "steal", Duration::from_millis(200))
            .expect("a recycled pid must not hold the merge-lease");
    }

    #[test]
    fn pid_start_round_trips_and_legacy_lockfile_parses_without_it() {
        // trace:TASK-1750 | ai:claude
        let h = Holder {
            host: "h".into(),
            pid: 42,
            pid_start: Some("2026-10-14T16:00:00+00:00".into()),
            pr: None,
            note: String::new(),
            acquired_at: 1,
            ttl_secs: 2,
        };
        assert_eq!(parse(&serialize(&h)).pid_start, h.pid_start);
        let legacy = parse("host=h\npid=42\npr=\nnote=\nacquired_at=1\nttl_secs=2\n");
        assert_eq!(legacy.pid, 42);
        assert_eq!(legacy.pid_start, None);
        let empty = Holder {
            pid_start: None,
            ..h
        };
        assert_eq!(parse(&serialize(&empty)).pid_start, None);
    }

    #[test]
    fn stale_ttl_expired_is_reclaimed_any_host() {
        // trace:STORY-1171.acf58627 | ai:codex
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(locks_dir(root)).unwrap();
        // A holder on a DIFFERENT host (can't probe pid) but aged past TTL → reclaimable.
        let h = Holder {
            host: "some-other-host".into(),
            pid: std::process::id(), // alive here, but host differs so pid path is skipped
            pid_start: None,
            pr: Some(1),
            note: "old".into(),
            acquired_at: now_secs().saturating_sub(10_000),
            ttl_secs: 600,
        };
        std::fs::write(lock_path(root, "main"), serialize(&h)).unwrap();
        let _ = acquire(root, "main", Some(2), "steal", Duration::from_millis(200)).unwrap();
    }

    #[test]
    fn status_lists_active_leases() {
        // trace:STORY-1171.ac117c5a | ai:codex
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
