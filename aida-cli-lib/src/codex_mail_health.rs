//! Data-only host health for the one registered Codex mail watcher.
// trace:TASK-2347 | ai:codex
use crate::DoctorFinding;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub(crate) const CATEGORY: &str = "codex-mail-wake";
const LIMIT: u64 = 16 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Registration {
    version: u32,
    project: PathBuf,
    thread: String,
    state_dir: PathBuf,
    job: String,
    max_age_seconds: u64,
    snapshot_max_age_seconds: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u32,
    project: PathBuf,
    thread: String,
    observed_at: f64,
    accepted_unacknowledged_count: u32,
    accepted_unknown_age_count: u32,
    oldest_known_notification_at: Option<f64>,
    recovery_required: bool,
}
fn directory(path: &Path, private: bool) -> Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    let forbidden = if private { 0o077 } else { 0o022 };
    if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & forbidden != 0 {
        bail!("unsafe health directory")
    }
    Ok(file)
}
fn read<T: serde::de::DeserializeOwned>(anchor: &std::fs::File, leaf: &str) -> Result<T> {
    let path = PathBuf::from(format!("/proc/self/fd/{}", anchor.as_raw_fd())).join(leaf);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
        || meta.len() > LIMIT
    {
        bail!("unsafe health record")
    }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        bail!("oversized health record")
    }
    Ok(serde_json::from_slice(&bytes)?)
}
fn assess(project: &Path, now: f64) -> Result<Option<String>> {
    let local = directory(&project.join(".aida"), false)?;
    let registration: Registration = read(&local, "codex-mail-health.json")?;
    let project = project.canonicalize()?;
    if registration.version != 1
        || registration.project != project
        || uuid::Uuid::parse_str(&registration.thread).is_err()
        || !registration.state_dir.is_absolute()
        || !(60..=604800).contains(&registration.max_age_seconds)
        || !(900..=10800).contains(&registration.snapshot_max_age_seconds)
        || !registration.job.starts_with("aida-codex-mail-")
        || !registration.job.ends_with("-health")
    {
        bail!("invalid health registration")
    }
    let state = directory(&registration.state_dir, true)?;
    let snapshot: Snapshot = read(&state, "health.json")?;
    let known = snapshot
        .accepted_unacknowledged_count
        .checked_sub(snapshot.accepted_unknown_age_count)
        .context("inconsistent health counts")?;
    if snapshot.version != 1
        || snapshot.project != project
        || snapshot.thread != registration.thread
        || snapshot.accepted_unacknowledged_count > 100_000
        || !snapshot.observed_at.is_finite()
        || snapshot.observed_at < 0.0
        || snapshot.observed_at > now + 5.0
        || (known > 0) != snapshot.oldest_known_notification_at.is_some()
        || snapshot
            .oldest_known_notification_at
            .is_some_and(|t| !t.is_finite() || t < 0.0 || t > snapshot.observed_at)
    {
        bail!("invalid health snapshot")
    }
    let snapshot_age = (now - snapshot.observed_at).max(0.0) as u64;
    let oldest = snapshot
        .oldest_known_notification_at
        .map(|t| (now - t).max(0.0) as u64);
    let failing = snapshot.recovery_required
        || snapshot.accepted_unknown_age_count > 0
        || oldest.is_some_and(|age| age >= registration.max_age_seconds)
        || snapshot_age > registration.snapshot_max_age_seconds;
    Ok(failing.then(|| format!(
        "accepted_unacknowledged_count={} accepted_unknown_age_count={} oldest_known_notification_age_seconds={} recovery_required={} snapshot_age_seconds={}",
        snapshot.accepted_unacknowledged_count, snapshot.accepted_unknown_age_count,
        oldest.map(|n| n.to_string()).unwrap_or_else(|| "unknown".into()),
        snapshot.recovery_required, snapshot_age)))
}
pub(crate) fn scan(project: &Path) -> Vec<DoctorFinding> {
    scan_at(project, chrono::Utc::now().timestamp() as f64)
}
fn scan_at(project: &Path, now: f64) -> Vec<DoctorFinding> {
    let summary = match assess(project, now) {
        Ok(None) => return Vec::new(),
        Ok(Some(summary)) => summary,
        // Never echo untrusted record contents, mailbox data or local paths.
        Err(_) => "health registration/snapshot missing, invalid or unavailable".into(),
    };
    vec![DoctorFinding { category: CATEGORY.into(), id: CATEGORY.into(), summary,
        action: "Inspect watcher status and target availability; recover uncertainty explicitly. Never automatically requeue accepted mail.".into(),
        safe_heal: false }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let state = root.join("state");
        std::fs::create_dir(root.join(".aida")).unwrap();
        std::fs::create_dir(&state).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        write(
            &root.join(".aida/codex-mail-health.json"),
            serde_json::json!({
            "version":1,"project":root,"thread":"01a1700b-717d-71f2-b7e0-0569b05f3dee",
            "state_dir":state,"job":"aida-codex-mail-fixture-health","max_age_seconds":3600,"snapshot_max_age_seconds":900}),
        );
        (dir, root)
    }
    fn write(path: &Path, value: serde_json::Value) {
        std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn snapshot(root: &Path, known: Option<f64>, unknown: u32, recovery: bool) {
        write(
            &root.join("state/health.json"),
            serde_json::json!({"version":1,"project":root,
            "thread":"01a1700b-717d-71f2-b7e0-0569b05f3dee","observed_at":10000,
            "accepted_unacknowledged_count":u32::from(known.is_some())+unknown,
            "accepted_unknown_age_count":unknown,"oldest_known_notification_at":known,"recovery_required":recovery}),
        );
    }
    #[test]
    fn task_2347_health_healthy_stale_unknown_uncertain_and_recovered() {
        let (_dir, root) = fixture();
        snapshot(&root, None, 0, false);
        assert!(scan_at(&root, 10000.0).is_empty());
        snapshot(&root, Some(100.0), 0, false);
        assert!(scan_at(&root, 10000.0)[0].summary.contains("9900"));
        snapshot(&root, None, 1, false);
        assert!(scan_at(&root, 10000.0)[0].summary.contains("unknown"));
        snapshot(&root, None, 0, true);
        assert!(scan_at(&root, 10000.0)[0]
            .summary
            .contains("recovery_required=true"));
        snapshot(&root, None, 0, false);
        assert!(scan_at(&root, 10000.0).is_empty());
        assert!(!scan_at(&root, 11000.0).is_empty());
    }
    #[test]
    fn task_2347_health_missing_links_and_wrong_identity_fail_without_leaking() {
        let (_dir, root) = fixture();
        assert!(!scan_at(&root, 10000.0).is_empty());
        snapshot(&root, None, 0, false);
        let path = root.join("state/health.json");
        let target = root.join("private-data");
        std::fs::rename(&path, &target).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let summary = &scan_at(&root, 10000.0)[0].summary;
        assert!(!summary.contains("private-data"));
        assert!(!summary.contains(root.to_str().unwrap()));
    }
}
