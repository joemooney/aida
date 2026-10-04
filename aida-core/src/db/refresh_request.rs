//! Durable cache refresh request sidecar (STORY-1484 slice C).
//!
//! The request file is a HINT, never the source of truth: "needs refresh" is
//! always re-derivable from `is_stale(HEAD)`, so a lost or corrupt request only
//! delays the refresh to the next reader or schedule tick. Per amendment A7 the
//! request is cleared only when the cache is fresh at the CURRENT head under
//! the refresh flock — never by ancestor comparison, which would spin forever
//! after a force-push — and after three failed worker attempts inside ten
//! minutes readers stop spawning and fall back to the strict inline path.
// trace:TASK-1527 | ai:claude

use std::path::{Path, PathBuf};

use super::cache_lock::cache_sidecar_path;

/// Spawning is suppressed once this many FAILED attempts land in the window.
pub const SUPPRESS_AFTER_FAILURES: usize = 3;
/// The sliding window those failures must share, in seconds.
pub const SUPPRESS_WINDOW_SECS: i64 = 600;
/// Attempt history is capped so a long crash loop cannot grow the file.
const MAX_RECORDED_ATTEMPTS: usize = 10;

/// One worker attempt. `error: None` records a spawn whose outcome is not yet
/// known (the worker overwrites its own slot only on failure); suppression
/// counts only attempts that FAILED, so a slow-but-alive worker never counts
/// against the backoff.
// trace:TASK-1527 | ai:claude
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct WorkerAttempt {
    pub at: String,
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The durable refresh request. Every field after `target_head` is diagnostic;
/// unknown fields from newer binaries are ignored on read (serde default).
// trace:TASK-1527 | ai:claude
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct RefreshRequest {
    #[serde(default)]
    pub target_head: String,
    #[serde(default)]
    pub requested_at: String,
    #[serde(default)]
    pub requester_pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_ns: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_version: Option<String>,
    #[serde(default)]
    pub attempts: Vec<WorkerAttempt>,
}

/// The sidecar address, via the single BUG-1644 derivation (amendment A12).
// trace:TASK-1527 | ai:claude
pub fn refresh_request_path(cache_path: &Path) -> PathBuf {
    cache_sidecar_path(cache_path, "refresh-request")
}

/// The worker log sidecar, same derivation (amendment A12).
// trace:TASK-1527 | ai:claude
pub fn refresh_log_path(cache_path: &Path) -> PathBuf {
    cache_sidecar_path(cache_path, "refresh.log")
}

/// Read the pending request. A missing OR unparseable file is `None`: the file
/// is a hint, and the next `file_request` rewrites it whole, so a corrupt hint
/// heals itself instead of wedging readers.
// trace:TASK-1527 | ai:claude
pub fn load(cache_path: &Path) -> Option<RefreshRequest> {
    let body = std::fs::read_to_string(refresh_request_path(cache_path)).ok()?;
    serde_json::from_str(&body).ok()
}

// trace:BUG-1779 | ai:antigravity
struct RequestLock(#[allow(dead_code)] std::fs::File);

impl RequestLock {
    fn acquire(cache_path: &Path) -> std::io::Result<Self> {
        let path = cache_sidecar_path(cache_path, "refresh-request.lock");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        fs2::FileExt::lock_exclusive(&file)?;
        Ok(Self(file))
    }
}

/// File (or re-target) the request for `target_head`, preserving the recorded
/// attempt history so backoff survives re-filing by later readers.
// trace:TASK-1527 | ai:claude
pub fn file_request(cache_path: &Path, target_head: &str) -> std::io::Result<RefreshRequest> {
    let _guard = RequestLock::acquire(cache_path)?;
    let mut request = load(cache_path).unwrap_or_default();
    request.target_head = target_head.to_string();
    request.requested_at = chrono::Utc::now().to_rfc3339();
    request.requester_pid = std::process::id();
    request.hostname = std::env::var("HOSTNAME").ok();
    request.boot_id = super::cache_lock::local_boot_id();
    request.pid_ns = super::cache_lock::local_pid_ns();
    request.binary = std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string());
    request.binary_version = option_env!("CARGO_PKG_VERSION").map(str::to_string);
    write(cache_path, &request)?;
    Ok(request)
}

/// Append a FAILED attempt (amendment A7's backoff input), keeping at most
/// [`MAX_RECORDED_ATTEMPTS`]. A request that no longer exists is not recreated:
/// failure history without a pending request would suppress nothing.
// trace:TASK-1527 | ai:claude
pub fn record_failed_attempt(cache_path: &Path, error: &str) -> std::io::Result<()> {
    let _guard = RequestLock::acquire(cache_path)?;
    let Some(mut request) = load(cache_path) else {
        return Ok(());
    };
    request.attempts.push(WorkerAttempt {
        at: chrono::Utc::now().to_rfc3339(),
        pid: std::process::id(),
        error: Some(error.to_string()),
    });
    if request.attempts.len() > MAX_RECORDED_ATTEMPTS {
        let excess = request.attempts.len() - MAX_RECORDED_ATTEMPTS;
        request.attempts.drain(..excess);
    }
    write(cache_path, &request)
}

/// Remove the request. Callers clear only when the cache is fresh at the
/// CURRENT head under the refresh flock (amendment A7); this function does not
/// re-check that, so it stays testable without a store.
// trace:TASK-1527 | ai:claude
pub fn clear(cache_path: &Path) -> std::io::Result<()> {
    let _guard = RequestLock::acquire(cache_path)?;
    match std::fs::remove_file(refresh_request_path(cache_path)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

pub fn clear_if_target_matches(cache_path: &Path, expected_head: &str) -> std::io::Result<()> {
    let _guard = RequestLock::acquire(cache_path)?;
    let Some(request) = load(cache_path) else {
        return Ok(());
    };
    if request.target_head == expected_head {
        match std::fs::remove_file(refresh_request_path(cache_path)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => (),
        }
    }
    Ok(())
}

impl RefreshRequest {
    /// Amendment A7: after [`SUPPRESS_AFTER_FAILURES`] failed workers within
    /// [`SUPPRESS_WINDOW_SECS`], readers stop spawning and take the strict
    /// inline path. Only attempts carrying an error count.
    // trace:TASK-1527 | ai:claude
    pub fn spawning_suppressed(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        let failed_in_window = self
            .attempts
            .iter()
            .filter(|a| a.error.is_some())
            .filter(|a| {
                chrono::DateTime::parse_from_rfc3339(&a.at)
                    .map(|at| (now - at.with_timezone(&chrono::Utc)).num_seconds())
                    .map(|age| (0..=SUPPRESS_WINDOW_SECS).contains(&age))
                    .unwrap_or(false)
            })
            .count();
        failed_in_window >= SUPPRESS_AFTER_FAILURES
    }

    /// The most recent recorded failure, for `cache status` and doctor.
    // trace:TASK-1527 | ai:claude
    pub fn last_error(&self) -> Option<&str> {
        self.attempts.iter().rev().find_map(|a| a.error.as_deref())
    }
}

// Same tmp+rename shape as the lock-info sidecar: readers never observe a
// truncated hint, and a crashed writer leaves at worst a stale `.tmp`.
// trace:TASK-1527 | ai:claude
fn write(cache_path: &Path, request: &RefreshRequest) -> std::io::Result<()> {
    let path = refresh_request_path(cache_path);
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    let body = serde_json::to_string_pretty(request)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_path(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("cache.db")
    }

    #[test]
    fn missing_and_corrupt_requests_read_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_path(&dir);
        assert!(load(&cache).is_none());
        std::fs::write(refresh_request_path(&cache), "{not json").unwrap();
        assert!(
            load(&cache).is_none(),
            "a corrupt hint must heal, not wedge"
        );
        // A re-file replaces the corrupt hint wholesale.
        file_request(&cache, "abc123").unwrap();
        assert_eq!(load(&cache).unwrap().target_head, "abc123");
    }

    #[test]
    fn refiling_preserves_attempt_history() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_path(&dir);
        file_request(&cache, "head-one").unwrap();
        record_failed_attempt(&cache, "worker exited 101").unwrap();
        let refiled = file_request(&cache, "head-two").unwrap();
        assert_eq!(refiled.target_head, "head-two");
        assert_eq!(
            refiled.attempts.len(),
            1,
            "backoff history must survive a re-target"
        );
        assert_eq!(refiled.last_error(), Some("worker exited 101"));
    }

    #[test]
    fn clear_is_idempotent_and_removes_the_hint() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_path(&dir);
        clear(&cache).unwrap();
        file_request(&cache, "abc").unwrap();
        clear(&cache).unwrap();
        assert!(load(&cache).is_none());
        clear(&cache).unwrap();
    }

    #[test]
    fn failed_attempt_without_a_request_records_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_path(&dir);
        record_failed_attempt(&cache, "boom").unwrap();
        assert!(load(&cache).is_none());
    }

    #[test]
    fn suppression_needs_three_failures_inside_the_window() {
        let now = chrono::Utc::now();
        let attempt = |age_secs: i64, error: Option<&str>| WorkerAttempt {
            at: (now - chrono::Duration::seconds(age_secs)).to_rfc3339(),
            pid: 1,
            error: error.map(str::to_string),
        };
        let mut request = RefreshRequest {
            attempts: vec![attempt(30, Some("a")), attempt(20, Some("b"))],
            ..Default::default()
        };
        assert!(
            !request.spawning_suppressed(now),
            "two failures are not a loop"
        );
        request.attempts.push(attempt(10, None));
        assert!(
            !request.spawning_suppressed(now),
            "a spawn with no recorded failure must not count toward the backoff"
        );
        request.attempts.push(attempt(5, Some("c")));
        assert!(request.spawning_suppressed(now));
        // The same three failures spread past the window do not suppress.
        let aged = RefreshRequest {
            attempts: vec![
                attempt(SUPPRESS_WINDOW_SECS + 60, Some("a")),
                attempt(SUPPRESS_WINDOW_SECS + 30, Some("b")),
                attempt(5, Some("c")),
            ],
            ..Default::default()
        };
        assert!(!aged.spawning_suppressed(now));
    }

    #[test]
    fn attempt_history_is_capped() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_path(&dir);
        file_request(&cache, "abc").unwrap();
        for i in 0..15 {
            record_failed_attempt(&cache, &format!("failure {i}")).unwrap();
        }
        let request = load(&cache).unwrap();
        assert_eq!(request.attempts.len(), 10);
        assert_eq!(request.last_error(), Some("failure 14"));
        assert_eq!(
            request.attempts.first().unwrap().error.as_deref(),
            Some("failure 5"),
            "the OLDEST attempts are the ones dropped"
        );
    }

    #[test]
    fn concurrent_record_failed_attempt_is_serialized() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_path(&dir);
        file_request(&cache, "abc").unwrap();
        let mut threads = vec![];
        for i in 0..10 {
            let path = cache.clone();
            threads.push(std::thread::spawn(move || {
                for j in 0..10 {
                    record_failed_attempt(&path, &format!("error {i}-{j}")).unwrap();
                }
            }));
        }
        for t in threads {
            t.join().unwrap();
        }
        let request = load(&cache).unwrap();
        // MAX_RECORDED_ATTEMPTS is 10, so it should retain exactly 10 valid entries
        // without corruption or panics.
        assert_eq!(request.attempts.len(), 10);
    }
}
