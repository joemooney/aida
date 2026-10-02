//! Detached cache refresh worker (STORY-1484 slice C, amendment A8).
//!
//! The worker is a separate `aida cache refresh --worker` process spawned from
//! `current_exe()` with EXPLICIT `--store`/`--cache` paths (never the cwd),
//! stdin null and both outputs appended to the `refresh.log` sidecar. It is
//! detached (setsid / DETACHED_PROCESS) so the requesting reader can exit; a
//! cgroup kill (systemd-run drain units) can still reap it, which the durable
//! request file covers — the next reader or schedule tick re-spawns.
// trace:TASK-1527 | ai:claude

use std::path::Path;
use std::time::Duration;

use anyhow::Result;

use super::cache_refresh::RefreshLock;
use super::cached_git_backend::CachedGitBackend;
use super::refresh_request;
use super::traits::DatabaseBackend as _;

/// Floor for the worker runtime cap.
pub const WORKER_CAP_FLOOR: Duration = Duration::from_secs(120);
/// How long a just-spawned worker polls for the refresh flock before
/// concluding another refresh owns the work. Covers the window where the
/// spawning reader has not yet dropped its own guard.
const WORKER_LOCK_PATIENCE: Duration = Duration::from_secs(3);

/// `max(120 s, 2x the last full rebuild)` (sketch; the floor covers a cache
/// that has never stamped a rebuild duration).
// trace:TASK-1527 | ai:claude
pub fn worker_runtime_cap(backend: &CachedGitBackend) -> Duration {
    let last = backend
        .cache()
        .last_full_rebuild_ms()
        .ok()
        .flatten()
        .unwrap_or(0);
    WORKER_CAP_FLOOR.max(Duration::from_millis(last.saturating_mul(2)))
}

/// What a worker run concluded. `LockHeld` is success-shaped: someone else is
/// doing the work, and exiting 0 keeps the schedule tick quiet.
// trace:TASK-1527 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub enum WorkerOutcome {
    /// The cache was stale and this process brought it current.
    Refreshed,
    /// The cache was already fresh; the request (if any) was cleared.
    AlreadyFresh,
    /// Another process holds the refresh flock.
    LockHeld,
}

/// The worker body, also run inline by `cache refresh --if-requested` under a
/// schedule tick. Amendment A7's clearing rule lives HERE and nowhere else:
/// the request is removed only when `is_stale(current HEAD)` is false while
/// the refresh flock is held. A head that moves during the refresh leaves the
/// request in place for the next pass rather than looping unbounded.
// trace:TASK-1527 | ai:claude
pub fn run_refresh_worker(backend: &CachedGitBackend) -> Result<WorkerOutcome> {
    let cache_path = backend.cache().path().to_path_buf();
    let deadline = std::time::Instant::now() + WORKER_LOCK_PATIENCE;
    let _guard = loop {
        match RefreshLock::try_acquire(&cache_path)? {
            Some(guard) => break guard,
            None if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            None => return Ok(WorkerOutcome::LockHeld),
        }
    };
    let head = crate::git_ops::head_sha(backend.path()).unwrap_or_default();
    if !backend.cache().is_stale(&head)? {
        refresh_request::clear(&cache_path)?;
        return Ok(WorkerOutcome::AlreadyFresh);
    }
    backend.ensure_cache_fresh_with_schema_retry()?;
    let head_now = crate::git_ops::head_sha(backend.path()).unwrap_or_default();
    if !backend.cache().is_stale(&head_now)? {
        refresh_request::clear(&cache_path)?;
    } else {
        // The store moved under the refresh: re-target the surviving request
        // so `cache status` names the head actually wanted.
        refresh_request::file_request(&cache_path, &head_now)?;
    }
    Ok(WorkerOutcome::Refreshed)
}

/// `AIDA_CACHE_NO_DETACH=1` (CI, tests) forbids spawning: a stale reader
/// refreshes inline strictly instead — today's behaviour (sketch Q1).
// trace:TASK-1527 | ai:claude
pub fn no_detach() -> bool {
    std::env::var("AIDA_CACHE_NO_DETACH").is_ok_and(|v| !v.is_empty() && v != "0")
}

#[cfg(test)]
thread_local! {
    /// Test seam: records would-be spawns instead of forking real processes,
    /// so reader-protocol tests stay hermetic (fixtures and fake HOME only).
    pub(super) static SPAWN_LOG: std::cell::RefCell<Option<Vec<std::path::PathBuf>>> =
        const { std::cell::RefCell::new(None) };
}

/// Spawn the detached worker for the store at `store_root`. Returns the child
/// pid. The caller decides WHETHER to spawn (request filed, backoff clear,
/// [`no_detach`] unset); this helper only refuses recursion: a worker never
/// spawns a worker.
// trace:TASK-1527 | ai:claude
pub fn spawn_detached_worker(store_root: &Path, cache_path: &Path) -> std::io::Result<u32> {
    if std::env::var_os("AIDA_CACHE_WORKER").is_some() {
        let _ = store_root;
        return Err(std::io::Error::other(
            "refusing to spawn a refresh worker from inside a refresh worker",
        ));
    }
    // Under cfg(test), current_exe() is the TEST binary: a real spawn would
    // re-enter the harness with `cache refresh --worker` args. Armed tests
    // observe the spawn; everything else gets a refusal that surfaces as a
    // recorded failed attempt (the Deferred path).
    #[cfg(test)]
    {
        let intercepted = SPAWN_LOG.with(|log| {
            if let Some(entries) = log.borrow_mut().as_mut() {
                entries.push(cache_path.to_path_buf());
                true
            } else {
                false
            }
        });
        if intercepted {
            Ok(0)
        } else {
            Err(std::io::Error::other(
                "detached spawn disabled under cfg(test); arm SPAWN_LOG or set AIDA_CACHE_NO_DETACH",
            ))
        }
    }
    #[cfg(not(test))]
    {
        use std::process::{Command, Stdio};
        rotate_log(cache_path)?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(refresh_request::refresh_log_path(cache_path))?;
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.arg("cache")
            .arg("refresh")
            .arg("--worker")
            .arg("--store")
            .arg(store_root)
            .arg("--cache")
            .arg(cache_path)
            .env("AIDA_CACHE_WORKER", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Safety: setsid is async-signal-safe; nothing else runs pre-exec.
            unsafe {
                cmd.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        }
        let mut child = cmd.spawn()?;
        let pid = child.id();
        // Reap in the background so a long-lived parent (the MCP server)
        // accumulates no zombies, without blocking this reader on the refresh.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(pid)
    }
}

/// The worker log is append-only; cap it at ~1 MB by rotating to `.1` so a
/// crash loop cannot grow it unbounded (sketch: C).
// trace:TASK-1527 | ai:claude
const LOG_ROTATE_BYTES: u64 = 1024 * 1024;
fn rotate_log(cache_path: &Path) -> std::io::Result<()> {
    let log = refresh_request::refresh_log_path(cache_path);
    match std::fs::metadata(&log) {
        Ok(meta) if meta.len() >= LOG_ROTATE_BYTES => {
            let rotated = log.with_extension("log.1");
            std::fs::rename(&log, rotated)
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_cap_floors_at_two_minutes_and_doubles_the_last_rebuild() {
        assert_eq!(WORKER_CAP_FLOOR, Duration::from_secs(120));
        // The doubling path is exercised through worker_runtime_cap in the
        // backend tests; the pure arithmetic is pinned here.
        assert_eq!(
            WORKER_CAP_FLOOR.max(Duration::from_millis(400_000u64)),
            Duration::from_millis(400_000)
        );
        assert_eq!(
            WORKER_CAP_FLOOR.max(Duration::from_millis(10)),
            WORKER_CAP_FLOOR
        );
    }

    #[test]
    fn a_worker_never_spawns_a_worker() {
        let dir = tempfile::tempdir().unwrap();
        let _env = crate::test_env::EnvVarGuard::set("AIDA_CACHE_WORKER", "1");
        let err = spawn_detached_worker(dir.path(), &dir.path().join("cache.db"))
            .expect_err("recursion must be refused");
        assert!(err.to_string().contains("inside a refresh worker"));
    }

    #[test]
    fn log_rotation_renames_at_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache.db");
        let log = refresh_request::refresh_log_path(&cache);
        std::fs::write(&log, vec![b'x'; (LOG_ROTATE_BYTES + 1) as usize]).unwrap();
        rotate_log(&cache).unwrap();
        assert!(!log.exists());
        assert!(log.with_extension("log.1").exists());
        // Under the cap: untouched.
        std::fs::write(&log, b"small").unwrap();
        rotate_log(&cache).unwrap();
        assert!(log.exists());
    }
}
