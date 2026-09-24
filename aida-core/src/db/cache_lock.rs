//! Cache write-lock metadata: the `.aida/cache.db.lock-info` sidecar.
//!
//! SQLite owns the actual write lock; this sidecar is ADVISORY metadata that
//! names who is (or was) writing, so a waiter can explain contention and an
//! operator can recover from a crashed writer. BUG-1595 introduced the file and
//! `aida doctor heal stale-locks`; TASK-1484 makes it richer and safer:
//!
//! - richer fields (lock kind, resource, PID start identity, acquired time,
//!   expected duration / soft deadline, heartbeat, phase), all optional on read
//!   so a pre-TASK-1484 file still parses;
//! - owner liveness that survives PID reuse: a live PID whose kernel start
//!   identity differs from the recorded one is a DIFFERENT process, so the
//!   recorded owner is dead. When the identity cannot be read the owner is
//!   presumed alive (PRIN-5: fail closed);
//! - an expected-duration overrun is diagnostic evidence only. Nothing here
//!   deletes a live owner's sidecar or signals its process because an estimate
//!   expired.
//!
//! trace:TASK-1484 | ai:claude

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The only lock kind that writes this sidecar today.
pub const CACHE_WRITE_LOCK_KIND: &str = "cache-write";

/// Default expected duration of one cache write (a full rebuild included).
/// Overridable with `AIDA_CACHE_LOCK_EXPECTED_SECS`. Diagnostic only.
const DEFAULT_EXPECTED_SECS: u64 = 120;

/// One lock-info record. The first five fields are the BUG-1595 format; every
/// later field is optional and skipped when absent, so old files parse and old
/// binaries can still read files written by this one (serde ignores unknown
/// fields). `started_at` is the acquired time (the name is kept for
/// compatibility).
// trace:TASK-1484 | ai:claude
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct CacheLockInfo {
    pub pid: u32,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub started_at: String,
    #[serde(default)]
    pub user: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Lock kind, e.g. `cache-write`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The locked resource (the cache database path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    /// Kernel process-start identity of `pid`, scheme-prefixed
    /// (`linux-starttime:<ticks>` on Linux, `sysinfo-start:<rfc3339>`
    /// elsewhere). Guards against PID reuse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_start_identity: Option<String>,
    /// How long the owner expected to hold the lock. Diagnostic only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_duration_secs: Option<u64>,
    /// `started_at + expected_duration_secs`, RFC 3339. Diagnostic only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_deadline: Option<String>,
    /// Last time the owner reported progress, RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat_at: Option<String>,
    /// What the owner is doing (the cache action, or a retry wait).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
}

impl CacheLockInfo {
    /// A record for THIS process holding the cache write lock on `cache_path`.
    // trace:TASK-1484 | ai:claude
    pub(crate) fn current(cache_path: &Path, phase: &str) -> Self {
        let expected = std::env::var("AIDA_CACHE_LOCK_EXPECTED_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_EXPECTED_SECS);
        Self::for_owner(
            std::process::id(),
            live_start_identity(std::process::id()),
            chrono::Utc::now(),
            cache_path,
            phase,
            expected,
        )
    }

    fn for_owner(
        pid: u32,
        pid_start_identity: Option<String>,
        now: chrono::DateTime<chrono::Utc>,
        cache_path: &Path,
        phase: &str,
        expected_secs: u64,
    ) -> Self {
        let now_s = now.to_rfc3339();
        Self {
            pid,
            command: std::env::args().collect::<Vec<_>>().join(" "),
            started_at: now_s.clone(),
            user: current_user(),
            session_id: std::env::var("AIDA_SESSION_ID")
                .ok()
                .filter(|s| !s.is_empty()),
            kind: Some(CACHE_WRITE_LOCK_KIND.to_string()),
            resource: Some(cache_path.display().to_string()),
            pid_start_identity,
            expected_duration_secs: Some(expected_secs),
            soft_deadline: Some(
                (now + chrono::Duration::seconds(expected_secs.min(i64::MAX as u64) as i64))
                    .to_rfc3339(),
            ),
            heartbeat_at: Some(now_s),
            phase: Some(phase.to_string()),
        }
    }

    pub fn started_at_utc(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        parse_utc(&self.started_at)
    }

    pub fn heartbeat_at_utc(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.heartbeat_at.as_deref().and_then(parse_utc)
    }

    /// The soft deadline: the explicit field, else `started_at + expected`.
    /// `None` for an old-format record, which carries neither.
    pub fn soft_deadline_utc(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        if let Some(deadline) = self.soft_deadline.as_deref().and_then(parse_utc) {
            return Some(deadline);
        }
        let expected = self.expected_duration_secs?;
        Some(
            self.started_at_utc()?
                + chrono::Duration::seconds(expected.min(i64::MAX as u64) as i64),
        )
    }

    /// How far past its soft deadline this lock is at `now`, if at all.
    /// Diagnostic evidence only: callers must never delete a live lock or
    /// signal its owner because of it.
    // trace:TASK-1484 | ai:claude
    pub fn overrun_at(&self, now: chrono::DateTime<chrono::Utc>) -> Option<chrono::Duration> {
        let deadline = self.soft_deadline_utc()?;
        (now > deadline).then(|| now.signed_duration_since(deadline))
    }

    fn command_label(&self) -> &str {
        if self.command.trim().is_empty() {
            "unknown command"
        } else {
            self.command.as_str()
        }
    }
}

fn parse_utc(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

/// Liveness of the process a lock-info record names.
// trace:TASK-1484 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockOwnerState {
    /// The record names this very process.
    Current,
    /// Another process, confirmed alive (identity matched, or a legacy record
    /// without an identity whose PID is alive).
    Alive,
    /// The PID is alive but its start identity could not be compared. Treated
    /// as alive (fail closed).
    Unknown,
    /// The recorded owner is gone. `pid_reused` is true when the PID is alive
    /// but belongs to a different process (start identity mismatch).
    Dead { pid_reused: bool },
}

impl LockOwnerState {
    /// Anything that is not provably dead is presumed alive (PRIN-5).
    pub fn presumed_alive(self) -> bool {
        !matches!(self, LockOwnerState::Dead { .. })
    }
}

/// Classify the recorded owner of `info`.
// trace:TASK-1484 | ai:claude
pub fn classify_lock_owner(info: &CacheLockInfo) -> LockOwnerState {
    classify_lock_owner_with(
        info,
        std::process::id(),
        crate::liveness::pid_is_alive,
        live_start_identity,
    )
}

/// Pure classifier with injected probes (the fixtures in the tests drive it).
fn classify_lock_owner_with(
    info: &CacheLockInfo,
    self_pid: u32,
    pid_alive: impl Fn(u32) -> bool,
    start_identity: impl Fn(u32) -> Option<String>,
) -> LockOwnerState {
    if !pid_alive(info.pid) {
        return LockOwnerState::Dead { pid_reused: false };
    }
    let alive_state = if info.pid == self_pid {
        LockOwnerState::Current
    } else {
        LockOwnerState::Alive
    };
    let Some(recorded) = info
        .pid_start_identity
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    else {
        // Legacy (BUG-1595) record: PID-only, as before.
        return alive_state;
    };
    match start_identity(info.pid) {
        // Cannot compare: fail closed.
        None => LockOwnerState::Unknown,
        // A different identity scheme (another platform/build wrote it):
        // not comparable, fail closed.
        Some(live) if identity_scheme(&live) != identity_scheme(recorded) => {
            LockOwnerState::Unknown
        }
        Some(live) if live == recorded => alive_state,
        Some(_) => LockOwnerState::Dead { pid_reused: true },
    }
}

fn identity_scheme(identity: &str) -> &str {
    identity.split_once(':').map(|(s, _)| s).unwrap_or("")
}

/// Scheme-prefixed kernel start identity of `pid`, or `None` when it cannot be
/// read. Linux reads the `starttime` field (clock ticks since boot) from the
/// process stat file, which is exact and never changes for a process's life.
// trace:TASK-1484 | ai:claude
#[cfg(target_os = "linux")]
pub fn live_start_identity(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_proc_stat_starttime(&stat).map(|ticks| format!("linux-starttime:{ticks}"))
}

/// Best effort elsewhere: sysinfo's start time (seconds resolution).
// trace:TASK-1484 | ai:claude
#[cfg(not(target_os = "linux"))]
pub fn live_start_identity(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    crate::liveness::process_start_identity(pid).map(|t| format!("sysinfo-start:{t}"))
}

/// Field 22 (`starttime`) of a Linux process stat line. The command name
/// (field 2) is parenthesised and may itself contain spaces or `)`, so parsing
/// resumes after the LAST `)`; the next token is field 3.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_proc_stat_starttime(stat: &str) -> Option<u64> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(22 - 3)?.parse().ok()
}

/// A read of the sidecar plus the owner verdict and any deadline overrun.
// trace:TASK-1484 | ai:claude
#[derive(Debug, Clone)]
pub struct CacheLockObservation {
    pub info: CacheLockInfo,
    pub owner: LockOwnerState,
    /// Past the soft deadline by this much. Diagnostic only.
    pub overrun: Option<chrono::Duration>,
}

impl CacheLockObservation {
    fn at(info: CacheLockInfo, owner: LockOwnerState, now: chrono::DateTime<chrono::Utc>) -> Self {
        let overrun = info.overrun_at(now);
        Self {
            info,
            owner,
            overrun,
        }
    }

    /// Age of the lock (since `started_at`), clamped at zero.
    pub fn age_secs(&self, now: chrono::DateTime<chrono::Utc>) -> Option<u64> {
        self.info
            .started_at_utc()
            .map(|t| now.signed_duration_since(t).num_seconds().max(0) as u64)
    }

    /// The overrun, but only when the owner is presumed alive: a dead owner's
    /// deadline is moot.
    pub fn live_overrun(&self) -> Option<chrono::Duration> {
        self.overrun.filter(|_| self.owner.presumed_alive())
    }

    /// One-line diagnostic for a live lock past its soft deadline.
    pub fn overrun_note(&self) -> Option<String> {
        let over = self.live_overrun()?;
        Some(format!(
            "it has run {} past its expected duration{} (diagnostic only; a live lock is never removed because an estimate expired)",
            humanize_secs(over.num_seconds().max(0) as u64),
            self.info
                .phase
                .as_deref()
                .map(|p| format!(", phase: {p}"))
                .unwrap_or_default()
        ))
    }
}

pub fn cache_lock_info_path(cache_path: &Path) -> PathBuf {
    cache_path.with_file_name(format!(
        "{}.lock-info",
        cache_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("cache.db")
    ))
}

pub fn read_cache_lock_info(cache_path: &Path) -> Result<Option<CacheLockInfo>> {
    read_lock_info_file(&cache_lock_info_path(cache_path))
}

fn read_lock_info_file(path: &Path) -> Result<Option<CacheLockInfo>> {
    if !path.exists() {
        return Ok(None);
    }
    let body = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read cache lock info at {}", path.display()))?;
    let info = serde_json::from_str(&body)
        .with_context(|| format!("Failed to parse cache lock info at {}", path.display()))?;
    Ok(Some(info))
}

/// Read the sidecar next to `cache_path` and classify its owner.
// trace:TASK-1484 | ai:claude
pub fn observe_cache_lock(cache_path: &Path) -> Result<Option<CacheLockObservation>> {
    observe_lock_info_file(&cache_lock_info_path(cache_path))
}

/// [`observe_cache_lock`] addressed by the sidecar path itself.
pub fn observe_lock_info_file(path: &Path) -> Result<Option<CacheLockObservation>> {
    Ok(read_lock_info_file(path)?.map(|info| {
        let owner = classify_lock_owner(&info);
        CacheLockObservation::at(info, owner, chrono::Utc::now())
    }))
}

/// True when the cache write-lock is held by a DIFFERENT process that is not
/// provably dead. Read paths consult this so they serve the last-good snapshot
/// instead of contending through the retry ladder (BUG-664). This process's
/// own record and a dead owner's record (crashed writer, or a reused PID)
/// return false, so a stale sidecar never wedges readers.
// trace:BUG-664 trace:TASK-1484 | ai:claude
pub fn foreign_writer_holds_lock(cache_path: &Path) -> bool {
    match read_cache_lock_info(cache_path) {
        Ok(Some(info)) => matches!(
            classify_lock_owner(&info),
            LockOwnerState::Alive | LockOwnerState::Unknown
        ),
        _ => false,
    }
}

/// Claim the sidecar for this process. Returns `true` when this call wrote it,
/// `false` when a record already exists (a live contender or stale metadata;
/// the SQLite retry ladder arbitrates and a dead owner's record is cleared
/// after a successful write).
// trace:TASK-1484 | ai:claude
pub(crate) fn write_cache_lock_info(cache_path: &Path, phase: &str) -> Result<bool> {
    use std::io::Write;

    let path = cache_lock_info_path(cache_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create cache lock-info parent {}",
                parent.display()
            )
        })?;
    }
    let body = serde_json::to_string_pretty(&CacheLockInfo::current(cache_path, phase))?;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => file
            .write_all(body.as_bytes())
            .map(|()| true)
            .with_context(|| format!("Failed to write cache lock info at {}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(err) => Err(err)
            .with_context(|| format!("Failed to write cache lock info at {}", path.display())),
    }
}

/// Remove the sidecar if THIS process wrote it (orderly release).
pub(crate) fn remove_cache_lock_info(cache_path: &Path) {
    let path = cache_lock_info_path(cache_path);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(info) = serde_json::from_str::<CacheLockInfo>(&body) else {
        return;
    };
    if info.pid == std::process::id() {
        let _ = std::fs::remove_file(path);
    }
}

/// Refresh this process's heartbeat and phase in its own sidecar. Best effort;
/// written to a temp file and renamed so readers never see a torn record.
// trace:TASK-1484 | ai:claude
pub(crate) fn touch_own_lock_info(cache_path: &Path, phase: &str) {
    let path = cache_lock_info_path(cache_path);
    let Ok(Some(mut info)) = read_lock_info_file(&path) else {
        return;
    };
    if classify_lock_owner(&info) != LockOwnerState::Current {
        return;
    }
    info.heartbeat_at = Some(chrono::Utc::now().to_rfc3339());
    info.phase = Some(phase.to_string());
    let Ok(body) = serde_json::to_string_pretty(&info) else {
        return;
    };
    let tmp = path.with_extension(format!("lock-info.tmp-{}", std::process::id()));
    if std::fs::write(&tmp, body).is_err() || std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Outcome of a dead-owner reclaim attempt.
// trace:TASK-1484 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockInfoReclaim {
    /// No sidecar present.
    Absent,
    /// The sidecar named a dead owner and was removed.
    Removed { pid: u32, pid_reused: bool },
    /// The owner is not provably dead (or the record changed underneath us,
    /// or it could not be parsed); left in place.
    Kept(Option<LockOwnerState>),
}

/// Remove the sidecar at `path` only if its recorded owner is provably dead.
/// Compare-and-delete: the file is re-read after classification and removed
/// only if unchanged, so a live writer that replaced it in between keeps its
/// record. Used after a successful cache write and by
/// `aida doctor heal stale-locks`.
// Known window with two concurrent cleaners: R1 re-reads a dead record and
// finds it unchanged; R2 then deletes it; a writer W creates its own record;
// R1's remove_file then deletes W's LIVE record. The cost is diagnostics only
// (W's contention message and heartbeat go missing until its next write):
// SQLite is the actual lock, so no write is ever unguarded by this race.
// trace:TASK-1484 | ai:claude
pub fn reclaim_dead_lock_info(path: &Path) -> Result<LockInfoReclaim> {
    reclaim_dead_lock_info_with(path, classify_lock_owner)
}

fn reclaim_dead_lock_info_with(
    path: &Path,
    classify: impl Fn(&CacheLockInfo) -> LockOwnerState,
) -> Result<LockInfoReclaim> {
    let body = match std::fs::read_to_string(path) {
        Ok(body) => body,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LockInfoReclaim::Absent)
        }
        Err(err) => {
            return Err(err)
                .with_context(|| format!("Failed to read cache lock info at {}", path.display()))
        }
    };
    let Ok(info) = serde_json::from_str::<CacheLockInfo>(&body) else {
        return Ok(LockInfoReclaim::Kept(None));
    };
    let owner = classify(&info);
    let LockOwnerState::Dead { pid_reused } = owner else {
        return Ok(LockInfoReclaim::Kept(Some(owner)));
    };
    match std::fs::read_to_string(path) {
        Ok(again) if again == body => {}
        Ok(_) => return Ok(LockInfoReclaim::Kept(None)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LockInfoReclaim::Absent)
        }
        Err(err) => {
            return Err(err)
                .with_context(|| format!("Failed to read cache lock info at {}", path.display()))
        }
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(LockInfoReclaim::Removed {
            pid: info.pid,
            pid_reused,
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(LockInfoReclaim::Absent),
        Err(err) => Err(err)
            .with_context(|| format!("Failed to remove stale lock info at {}", path.display())),
    }
}

/// After a successful write (this process held the SQLite write lock, so no
/// recorded owner can be mid-write), drop a dead owner's leftover record.
pub(crate) fn clear_dead_owner_lock_info(cache_path: &Path) {
    let _ = reclaim_dead_lock_info(&cache_lock_info_path(cache_path));
}

/// Build the terminal "database is locked" error, distinguishing live
/// contention from stale metadata left by a dead owner.
// trace:TASK-1484 | ai:claude
pub(crate) fn enrich_cache_lock_error(
    action: &str,
    observation: Option<&CacheLockObservation>,
    err: anyhow::Error,
) -> anyhow::Error {
    let now = chrono::Utc::now();
    match observation {
        Some(obs) if obs.owner == LockOwnerState::Current => anyhow::anyhow!(
            "database is locked while trying to {action}. \
             Try again. If this persists, run `aida doctor heal stale-locks`.\ncaused by: {}",
            err
        ),
        Some(obs) if obs.owner.presumed_alive() => {
            let mut msg = format!(
                "database is locked while trying to {action} by pid={} ({}) held since {} ({} ago).",
                obs.info.pid,
                obs.info.command_label(),
                obs.info.started_at,
                age_label(obs, now),
            );
            if let Some(note) = obs.overrun_note() {
                msg.push_str(&format!(" Note: {note}."));
            }
            anyhow::anyhow!(
                "{msg} Try again or check that process. If it's stuck, run `aida doctor heal stale-locks`.\ncaused by: {}",
                err
            )
        }
        Some(obs) => anyhow::anyhow!(
            "database is locked while trying to {action}. The lock-info names pid={} ({}) from {} ({} ago), \
             but that process {} (stale metadata), so another process holds the database without a record. \
             Try again; the stale record is cleared automatically after the next successful write, \
             or run `aida doctor heal stale-locks`.\ncaused by: {}",
            obs.info.pid,
            obs.info.command_label(),
            obs.info.started_at,
            age_label(obs, now),
            if matches!(obs.owner, LockOwnerState::Dead { pid_reused: true }) {
                "has exited and its PID was reused"
            } else {
                "is no longer running"
            },
            err
        ),
        None => anyhow::anyhow!(
            "database is locked while trying to {action}. \
             Try again. If this persists, run `aida doctor heal stale-locks`.\ncaused by: {}",
            err
        ),
    }
}

fn age_label(obs: &CacheLockObservation, now: chrono::DateTime<chrono::Utc>) -> String {
    obs.age_secs(now)
        .map(humanize_secs)
        .unwrap_or_else(|| "unknown age".to_string())
}

fn humanize_secs(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn current_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const DEAD: u32 = 0x7fff_fffe;
    const OTHER: u32 = 4242;

    fn fixture(pid: u32, identity: Option<&str>) -> CacheLockInfo {
        CacheLockInfo {
            pid,
            command: "aida schedule tick --hook".to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            user: "tester".to_string(),
            pid_start_identity: identity.map(str::to_string),
            kind: Some(CACHE_WRITE_LOCK_KIND.to_string()),
            ..Default::default()
        }
    }

    // Probes: OTHER and 1000 are alive; OTHER's live identity is `ticks:100`.
    fn alive(pid: u32) -> bool {
        pid == OTHER || pid == 1000
    }
    fn identity(pid: u32) -> Option<String> {
        (pid == OTHER || pid == 1000).then(|| "linux-starttime:100".to_string())
    }

    #[test]
    fn old_format_lock_info_parses_with_new_fields_absent() {
        let old = r#"{
  "pid": 12345,
  "command": "aida schedule tick --hook",
  "started_at": "2026-09-20T10:00:00+00:00",
  "user": "joe"
}"#;
        let info: CacheLockInfo = serde_json::from_str(old).unwrap();
        assert_eq!(info.pid, 12345);
        assert_eq!(info.command, "aida schedule tick --hook");
        assert!(info.pid_start_identity.is_none());
        assert!(info.soft_deadline_utc().is_none());
        assert!(info.overrun_at(chrono::Utc::now()).is_none());
        // Legacy PID-only liveness is preserved.
        assert_eq!(
            classify_lock_owner_with(
                &CacheLockInfo {
                    pid: OTHER,
                    ..info.clone()
                },
                1,
                alive,
                identity
            ),
            LockOwnerState::Alive
        );
    }

    #[test]
    fn new_record_round_trips_and_old_readers_ignore_new_fields() {
        let now = chrono::Utc::now();
        let info = CacheLockInfo::for_owner(
            OTHER,
            Some("linux-starttime:100".into()),
            now,
            Path::new("/x/.aida/cache.db"),
            "rebuild cache",
            30,
        );
        let body = serde_json::to_string(&info).unwrap();
        let back: CacheLockInfo = serde_json::from_str(&body).unwrap();
        assert_eq!(back, info);
        assert_eq!(back.kind.as_deref(), Some("cache-write"));
        assert_eq!(back.phase.as_deref(), Some("rebuild cache"));
        assert!(back.heartbeat_at_utc().is_some());
        assert_eq!(
            back.soft_deadline_utc().unwrap().timestamp(),
            (now + chrono::Duration::seconds(30)).timestamp()
        );

        // A BUG-1595-era reader (only the original fields) still parses it.
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct Legacy {
            pid: u32,
            command: String,
            started_at: String,
            user: String,
            session_id: Option<String>,
        }
        let legacy: Legacy = serde_json::from_str(&body).unwrap();
        assert_eq!(legacy.pid, OTHER);
    }

    #[test]
    fn dead_pid_is_dead_owner() {
        assert_eq!(
            classify_lock_owner_with(&fixture(DEAD, None), 1, alive, identity),
            LockOwnerState::Dead { pid_reused: false }
        );
    }

    #[test]
    fn pid_reuse_detected_by_start_identity_mismatch() {
        let info = fixture(OTHER, Some("linux-starttime:99"));
        assert_eq!(
            classify_lock_owner_with(&info, 1, alive, identity),
            LockOwnerState::Dead { pid_reused: true }
        );
        let same = fixture(OTHER, Some("linux-starttime:100"));
        assert_eq!(
            classify_lock_owner_with(&same, 1, alive, identity),
            LockOwnerState::Alive
        );
    }

    #[test]
    fn unknown_identity_fails_closed() {
        let info = fixture(OTHER, Some("linux-starttime:99"));
        // Live PID, identity unreadable.
        let state = classify_lock_owner_with(&info, 1, alive, |_| None);
        assert_eq!(state, LockOwnerState::Unknown);
        assert!(state.presumed_alive());
        // Live PID, identity from an incomparable scheme.
        let state = classify_lock_owner_with(&info, 1, alive, |_| {
            Some("sysinfo-start:2026-09-20T10:00:00+00:00".into())
        });
        assert_eq!(state, LockOwnerState::Unknown);

        // And reclaim refuses to delete it.
        let dir = tempdir().unwrap();
        let path = dir.path().join("cache.db.lock-info");
        std::fs::write(&path, serde_json::to_string(&info).unwrap()).unwrap();
        let out =
            reclaim_dead_lock_info_with(&path, |i| classify_lock_owner_with(i, 1, alive, |_| None))
                .unwrap();
        assert_eq!(out, LockInfoReclaim::Kept(Some(LockOwnerState::Unknown)));
        assert!(path.exists());
    }

    #[test]
    fn this_process_is_current() {
        let info = fixture(1000, Some("linux-starttime:100"));
        assert_eq!(
            classify_lock_owner_with(&info, 1000, alive, identity),
            LockOwnerState::Current
        );
    }

    #[test]
    fn live_owner_is_never_reclaimed_even_past_deadline() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cache.db.lock-info");
        let mut info = fixture(OTHER, Some("linux-starttime:100"));
        info.started_at = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        info.expected_duration_secs = Some(60);
        std::fs::write(&path, serde_json::to_string(&info).unwrap()).unwrap();

        let out =
            reclaim_dead_lock_info_with(&path, |i| classify_lock_owner_with(i, 1, alive, identity))
                .unwrap();
        assert_eq!(out, LockInfoReclaim::Kept(Some(LockOwnerState::Alive)));
        assert!(
            path.exists(),
            "a live owner's lock-info must never be deleted"
        );
    }

    #[test]
    fn overrun_produces_diagnostic_only() {
        let mut info = fixture(OTHER, Some("linux-starttime:100"));
        let now = chrono::Utc::now();
        info.started_at = (now - chrono::Duration::seconds(300)).to_rfc3339();
        info.expected_duration_secs = Some(60);
        info.phase = Some("rebuild cache".into());
        let obs = CacheLockObservation::at(info.clone(), LockOwnerState::Alive, now);
        let over = obs.live_overrun().expect("past deadline");
        assert_eq!(over.num_seconds(), 240);
        let note = obs.overrun_note().unwrap();
        assert!(note.contains("diagnostic only"), "{note}");
        assert!(note.contains("phase: rebuild cache"), "{note}");

        // The error message carries the evidence and still names the live holder.
        let err = enrich_cache_lock_error(
            "rebuild cache",
            Some(&obs),
            anyhow::anyhow!("database is locked"),
        )
        .to_string();
        assert!(err.contains("by pid=4242"), "{err}");
        assert!(err.contains("past its expected duration"), "{err}");

        // Within the deadline: no evidence. Dead owner: deadline is moot.
        let fresh = CacheLockObservation::at(
            CacheLockInfo {
                started_at: now.to_rfc3339(),
                ..info.clone()
            },
            LockOwnerState::Alive,
            now,
        );
        assert!(fresh.overrun_note().is_none());
        let dead = CacheLockObservation::at(info, LockOwnerState::Dead { pid_reused: false }, now);
        assert!(dead.overrun_note().is_none());
    }

    #[test]
    fn lock_error_distinguishes_dead_owner_metadata() {
        let obs = CacheLockObservation::at(
            fixture(DEAD, Some("linux-starttime:5")),
            LockOwnerState::Dead { pid_reused: true },
            chrono::Utc::now(),
        );
        let err =
            enrich_cache_lock_error("apply cache schema", Some(&obs), anyhow::anyhow!("busy"))
                .to_string();
        assert!(err.contains("stale metadata"), "{err}");
        assert!(err.contains("PID was reused"), "{err}");
        assert!(err.contains("aida doctor heal stale-locks"), "{err}");

        let none = enrich_cache_lock_error("open cache", None, anyhow::anyhow!("busy")).to_string();
        assert!(!none.contains("pid="), "{none}");
    }

    #[test]
    fn reclaim_removes_dead_owner_and_reports_pid_reuse() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cache.db.lock-info");
        let info = fixture(OTHER, Some("linux-starttime:1"));
        std::fs::write(&path, serde_json::to_string(&info).unwrap()).unwrap();
        let out =
            reclaim_dead_lock_info_with(&path, |i| classify_lock_owner_with(i, 1, alive, identity))
                .unwrap();
        assert_eq!(
            out,
            LockInfoReclaim::Removed {
                pid: OTHER,
                pid_reused: true
            }
        );
        assert!(!path.exists());
        assert_eq!(
            reclaim_dead_lock_info(&path).unwrap(),
            LockInfoReclaim::Absent
        );
    }

    #[test]
    fn proc_stat_starttime_parses_field_22() {
        // Command name with spaces and a closing paren; starttime = 987654.
        let stat = "4242 (aida (tick) x) S 1 4242 4242 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 50";
        assert_eq!(parse_proc_stat_starttime(stat), Some(987_654));
        assert_eq!(parse_proc_stat_starttime("garbage"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_identity_for_self_is_stable() {
        let a = live_start_identity(std::process::id()).expect("own identity");
        assert!(a.starts_with("linux-starttime:"), "{a}");
        assert_eq!(Some(a), live_start_identity(std::process::id()));
        assert_eq!(live_start_identity(0), None);
    }
}
