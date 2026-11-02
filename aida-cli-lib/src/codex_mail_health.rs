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
    judge(
        &snapshot,
        &project,
        &registration.thread,
        registration.max_age_seconds,
        registration.snapshot_max_age_seconds,
        now,
    )
}

/// Whether one thread's snapshot is failing, and its bounded summary.
fn judge(
    snapshot: &Snapshot,
    project: &Path,
    thread: &str,
    max_age_seconds: u64,
    snapshot_max_age_seconds: u64,
    now: f64,
) -> Result<Option<String>> {
    let known = snapshot
        .accepted_unacknowledged_count
        .checked_sub(snapshot.accepted_unknown_age_count)
        .context("inconsistent health counts")?;
    if snapshot.version != 1
        || snapshot.project != project
        || snapshot.thread != thread
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
        || oldest.is_some_and(|age| age >= max_age_seconds)
        || snapshot_age > snapshot_max_age_seconds;
    Ok(failing.then(|| format!(
        "accepted_unacknowledged_count={} accepted_unknown_age_count={} oldest_known_notification_age_seconds={} recovery_required={} snapshot_age_seconds={}",
        snapshot.accepted_unacknowledged_count, snapshot.accepted_unknown_age_count,
        oldest.map(|n| n.to_string()).unwrap_or_else(|| "unknown".into()),
        snapshot.recovery_required, snapshot_age)))
}
/// Accepted mail an armed session has not read for this long fails.
const ARMED_MAX_AGE_SECONDS: u64 = 3600;
/// An armed session's snapshot older than this means its watcher stopped.
const ARMED_SNAPSHOT_MAX_AGE_SECONDS: u64 = 900;

pub(crate) fn scan(project: &Path) -> Vec<DoctorFinding> {
    let live: Vec<_> = crate::agent_registry::load_active_entries(project)
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    let armed: Vec<(String, String)> = live
        .iter()
        .filter(|e| e.mail_wake == Some(crate::agent_registry::MailWake::Armed))
        .filter_map(|e| {
            let thread = crate::mailbox_wake_run::row_thread(e)?;
            Some((e.name.clone().unwrap_or_else(|| e.id.clone()), thread))
        })
        .collect();
    // The watcher keys its state by the canonical project path.
    let canonical = project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf());
    let mut findings = scan_with(
        project,
        &crate::mailbox_wake_run::state_root(&canonical),
        &armed,
        chrono::Utc::now().timestamp() as f64,
    );
    findings.extend(unarmed_findings(project, &live, chrono::Utc::now()));
    findings.extend(service_findings(project));
    findings
}

/// The default `aida doctor` check: live Codex sessions that should be woken
/// for mail while the project's wake service is not installed. Reads the
/// registry and one unit file only.
// trace:STORY-2256 | ai:claude
pub(crate) fn service_findings(project: &Path) -> Vec<DoctorFinding> {
    let live = crate::agent_registry::load_live_entries(project);
    service_findings_in(
        project,
        &live,
        crate::mailbox_wake_run::service_installed(project),
    )
}

/// [`service_findings`] over the live rows `live`.
// trace:STORY-2256 | ai:claude
pub(crate) fn service_findings_in(
    project: &Path,
    live: &[crate::agent_registry::AgentRegistryEntry],
    installed: bool,
) -> Vec<DoctorFinding> {
    use crate::agent_registry::MailWake;
    use crate::mail_wake::WakeAdapter;
    if installed {
        return Vec::new();
    }
    let waiting: Vec<String> = live
        .iter()
        .filter(|e| e.ended_at.is_none())
        .filter(|e| WakeAdapter::for_vendor(&e.agent_type) == WakeAdapter::Codex)
        .filter(|e| {
            !matches!(
                e.mail_wake,
                Some(MailWake::Off | MailWake::Unsupported | MailWake::Retired)
            )
        })
        .map(|e| e.name.clone().unwrap_or_else(|| e.id.clone()))
        .collect();
    if waiting.is_empty() {
        return Vec::new();
    }
    vec![DoctorFinding {
        category: CATEGORY.into(),
        id: format!("{CATEGORY}:service-not-installed"),
        summary: format!(
            "{} live Codex agent(s) ({}) but the mail-wake service is not installed; their \
             mail arrives only on their next turn",
            waiting.len(),
            waiting.join(", ")
        ),
        action: format!(
            "Install it: (cd {} && aida mailbox wake-service install), then arm each agent \
             with `aida agent wake arm <agent>`",
            project.display()
        ),
        safe_heal: false,
    }]
}

/// A launch's wake left `pending` this long never got its thread armed.
const PENDING_MAX_AGE_SECONDS: i64 = 600;

/// Live Codex sessions whose launch asked for a mail wake that never armed:
/// `failed`, or still `pending` long after start. The SessionStart hook's
/// warning may never reach the operator, so the doctor names the exact
/// manual commands.
// trace:STORY-HOME-096 | trace:STORY-HOME-096.aca8bab2 | ai:claude
fn unarmed_findings(
    project: &Path,
    entries: &[crate::agent_registry::AgentRegistryEntry],
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<DoctorFinding> {
    use crate::agent_registry::MailWake;
    entries
        .iter()
        .filter(|e| e.ended_at.is_none())
        .filter_map(|e| {
            let why = match e.mail_wake? {
                MailWake::Failed => "mail wake failed to arm".to_string(),
                MailWake::Pending
                    if (now - e.started_at).num_seconds() >= PENDING_MAX_AGE_SECONDS =>
                {
                    format!(
                        "mail wake still pending {} min after start",
                        (now - e.started_at).num_minutes()
                    )
                }
                _ => return None,
            };
            let target = e.name.clone().unwrap_or_else(|| e.id.clone());
            Some(DoctorFinding {
                category: CATEGORY.into(),
                id: format!("{CATEGORY}:{target}:unarmed"),
                summary: format!("{target}: {why}; its mail arrives only on its next turn"),
                action: format!(
                    "To arm it by hand, run: {}",
                    crate::mail_wake::manual_arm_commands(project, &target)
                ),
                safe_heal: false,
            })
        })
        .collect()
}

/// The advisor-thread registration (when there is one, or when nothing is
/// armed) plus every armed session's own thread health.
// trace:STORY-HOME-096 | ai:claude
fn scan_with(
    project: &Path,
    state_root: &Path,
    armed: &[(String, String)],
    now: f64,
) -> Vec<DoctorFinding> {
    let registered =
        std::fs::symlink_metadata(project.join(".aida/codex-mail-health.json")).is_ok();
    let mut findings = if registered || armed.is_empty() {
        scan_at(project, now)
    } else {
        Vec::new()
    };
    for (handle, thread) in armed {
        let summary = match assess_armed(project, state_root, thread, now) {
            Ok(None) => continue,
            Ok(Some(summary)) => summary,
            Err(_) => "health snapshot missing, invalid or unavailable (is the project's \
                       mail-wake service running?)"
                .into(),
        };
        findings.push(DoctorFinding {
            category: CATEGORY.into(),
            id: format!("{CATEGORY}:{handle}"),
            summary: format!("{handle}: {summary}"),
            action: "Check `aida mailbox wake-service status`; recover uncertainty with \
                     `aida mailbox wake-run --recover <thread> --accepted|--retry`. Never \
                     automatically requeue accepted mail."
                .into(),
            safe_heal: false,
        });
    }
    findings
}

fn assess_armed(
    project: &Path,
    state_root: &Path,
    thread: &str,
    now: f64,
) -> Result<Option<String>> {
    let project = project.canonicalize()?;
    let state = directory(&state_root.join(thread), true)?;
    let snapshot: Snapshot = read(&state, "health.json")?;
    judge(
        &snapshot,
        &project,
        thread,
        ARMED_MAX_AGE_SECONDS,
        ARMED_SNAPSHOT_MAX_AGE_SECONDS,
        now,
    )
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
    // trace:STORY-HOME-096 | ai:claude
    const ARMED: &str = "01a185ba-0000-7000-8000-000000000004";
    fn armed_snapshot(root: &Path, state_root: &Path, observed_at: f64, recovery: bool) {
        let dir = state_root.join(ARMED);
        std::fs::create_dir_all(&dir).unwrap();
        for d in [state_root, dir.as_path()] {
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        write(
            &dir.join("health.json"),
            serde_json::json!({"version":1,"project":root,"thread":ARMED,
            "observed_at":observed_at,"accepted_unacknowledged_count":0,
            "accepted_unknown_age_count":0,"oldest_known_notification_at":null,
            "recovery_required":recovery}),
        );
    }
    // The doctor reads every armed session's own thread health; with no
    // advisor registration, armed rows alone decide the check.
    // trace:STORY-HOME-096.ac316a6a | ai:claude
    #[test]
    fn story_home_096_doctor_iterates_armed_rows() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let state_root = root.join("wake-state");
        let armed = vec![("codex-agent-04".to_string(), ARMED.to_string())];
        let missing = scan_with(&root, &state_root, &armed, 10000.0);
        assert_eq!(missing.len(), 1, "no advisor finding, one missing snapshot");
        assert!(missing[0].summary.starts_with("codex-agent-04: "));
        assert_eq!(missing[0].id, "codex-mail-wake:codex-agent-04");
        armed_snapshot(&root, &state_root, 9990.0, false);
        assert!(scan_with(&root, &state_root, &armed, 10000.0).is_empty());
        assert_eq!(
            scan_with(&root, &state_root, &armed, 11000.0).len(),
            1,
            "stale"
        );
        armed_snapshot(&root, &state_root, 9990.0, true);
        assert!(scan_with(&root, &state_root, &armed, 10000.0)[0]
            .summary
            .contains("recovery_required=true"));
        // Nothing armed and no registration: the advisor check still reports.
        assert_eq!(scan_with(&root, &state_root, &[], 10000.0).len(), 1);
    }
    // A launch whose wake failed, or stayed pending long after start, is
    // reported with the exact manual commands the hook's stderr carried.
    // trace:STORY-HOME-096.aca8bab2 | trace:STORY-HOME-096.ac316a6a | ai:claude
    #[test]
    fn story_home_096_doctor_reports_unarmed_launches_with_manual_commands() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let row = |pid: u32, name: Option<&str>, wake: &str, started: &str| {
            let name = name
                .map(|n| format!("name = \"{n}\"\n"))
                .unwrap_or_default();
            toml::from_str::<crate::agent_registry::AgentRegistryEntry>(&format!(
                "id = \"codex-{pid}\"\nagent_type = \"codex\"\npid = {pid}\n{name}\
                 started_at = \"{started}\"\nlast_active_at = \"{started}\"\n\
                 worktree_path = \"/w\"\nsource = \"agent-launcher\"\nmail_wake = \"{wake}\"\n"
            ))
            .unwrap()
        };
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-30T01:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let rows = [
            row(1, Some("codex-agent-01"), "failed", "2026-10-30T00:59:00Z"),
            row(2, None, "pending", "2026-10-30T00:30:00Z"),
            row(3, Some("codex-agent-03"), "pending", "2026-10-30T00:58:00Z"),
            row(4, Some("codex-agent-04"), "armed", "2026-10-30T00:00:00Z"),
            row(5, Some("codex-agent-05"), "off", "2026-10-30T00:00:00Z"),
        ];
        let findings = unarmed_findings(root, &rows, now);
        assert_eq!(findings.len(), 2, "{findings:?}");
        assert!(findings[0]
            .summary
            .starts_with("codex-agent-01: mail wake failed"));
        assert!(findings[0]
            .action
            .contains("aida mailbox wake-service install"));
        assert!(findings[0]
            .action
            .contains("aida agent wake arm codex-agent-01"));
        assert!(findings[1].summary.contains("pending 30 min"));
        assert!(findings[1].action.contains("aida agent wake arm codex-2"));
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
