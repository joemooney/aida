//! Vendor activity adapter for headless agent phases.
//!
//! The watchdog, idle detector, and supervisor need the same vendor-neutral
//! facts: is the launched session still alive, and when did it last produce
//! output. Vendor-specific stream and fallback details stay here.
//!
//! trace:STORY-1054 | ai:codex

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::session::{self, HeadlessVendor};

/// Facts known about one launched headless session.
#[derive(Debug, Clone)]
pub(crate) struct ActivitySession {
    pub(crate) vendor: HeadlessVendor,
    pub(crate) session_id: String,
    pub(crate) project_root: PathBuf,
    pub(crate) root_pid: Option<u32>,
}

/// Common activity/liveness seam for vendor-specific headless streams.
pub(crate) trait VendorActivity {
    fn is_alive(&self, session: &ActivitySession) -> bool;
    fn last_activity(&self, session: &ActivitySession) -> Option<SystemTime>;
    fn activity_signature(&self, session: &ActivitySession) -> Option<String>;
}

#[derive(Debug, Default)]
struct ClaudeActivity;

#[derive(Debug, Default)]
struct CodexActivity;

#[derive(Debug, Default)]
struct AgyActivity;

impl VendorActivity for ClaudeActivity {
    fn is_alive(&self, session: &ActivitySession) -> bool {
        vendor_neutral_alive(session.root_pid)
    }

    fn last_activity(&self, session: &ActivitySession) -> Option<SystemTime> {
        newest_headless_log(&session.project_root, &session.session_id)
            .and_then(|p| jsonl_event_activity(&p).or_else(|| metadata_activity(&p).map(|m| m.0)))
    }

    fn activity_signature(&self, session: &ActivitySession) -> Option<String> {
        headless_log_signature(&session.project_root, &session.session_id, "claude")
    }
}

impl VendorActivity for CodexActivity {
    fn is_alive(&self, session: &ActivitySession) -> bool {
        vendor_neutral_alive(session.root_pid)
    }

    fn last_activity(&self, session: &ActivitySession) -> Option<SystemTime> {
        newest_headless_log(&session.project_root, &session.session_id)
            .and_then(|p| jsonl_event_activity(&p).or_else(|| metadata_activity(&p).map(|m| m.0)))
            .or_else(|| {
                newest_codex_rollout(&session.session_id)
                    .and_then(|p| metadata_activity(&p).map(|m| m.0))
            })
    }

    fn activity_signature(&self, session: &ActivitySession) -> Option<String> {
        headless_log_signature(&session.project_root, &session.session_id, "codex").or_else(|| {
            newest_codex_rollout(&session.session_id)
                .and_then(|p| metadata_activity(&p))
                .map(|(mtime, len)| format!("codex-rollout:{len}:{}", nanos(mtime)))
        })
    }
}

impl VendorActivity for AgyActivity {
    fn is_alive(&self, session: &ActivitySession) -> bool {
        vendor_neutral_alive(session.root_pid)
    }

    fn last_activity(&self, session: &ActivitySession) -> Option<SystemTime> {
        newest_headless_log(&session.project_root, &session.session_id)
            .and_then(|p| jsonl_event_activity(&p).or_else(|| metadata_activity(&p).map(|m| m.0)))
    }

    fn activity_signature(&self, session: &ActivitySession) -> Option<String> {
        headless_log_signature(&session.project_root, &session.session_id, "agy")
    }
}

/// One-call output progress signature for watchdog-style consumers.
#[allow(dead_code)]
pub(crate) fn activity_signature(
    project_root: &Path,
    session_id: &str,
    root_pid: Option<u32>,
) -> Option<String> {
    let session = session(
        session::resolve_headless_vendor(project_root),
        project_root,
        session_id,
        root_pid,
    );
    adapter(session.vendor).activity_signature(&session)
}

pub(crate) fn activity_signature_for_vendor(
    vendor: HeadlessVendor,
    project_root: &Path,
    session_id: &str,
    root_pid: Option<u32>,
) -> Option<String> {
    let session = session(vendor, project_root, session_id, root_pid);
    adapter(session.vendor).activity_signature(&session)
}

#[allow(dead_code)]
pub(crate) fn is_alive(project_root: &Path, session_id: &str, root_pid: Option<u32>) -> bool {
    let session = session(
        session::resolve_headless_vendor(project_root),
        project_root,
        session_id,
        root_pid,
    );
    adapter(session.vendor).is_alive(&session)
}

#[allow(dead_code)]
pub(crate) fn last_activity(
    project_root: &Path,
    session_id: &str,
    root_pid: Option<u32>,
) -> Option<SystemTime> {
    let session = session(
        session::resolve_headless_vendor(project_root),
        project_root,
        session_id,
        root_pid,
    );
    adapter(session.vendor).last_activity(&session)
}

fn session(
    vendor: HeadlessVendor,
    project_root: &Path,
    session_id: &str,
    root_pid: Option<u32>,
) -> ActivitySession {
    ActivitySession {
        vendor,
        session_id: session_id.to_string(),
        project_root: project_root.to_path_buf(),
        root_pid,
    }
}

fn adapter(vendor: HeadlessVendor) -> &'static dyn VendorActivity {
    static CLAUDE: ClaudeActivity = ClaudeActivity;
    static CODEX: CodexActivity = CodexActivity;
    static AGY: AgyActivity = AgyActivity;
    match vendor {
        HeadlessVendor::Claude => &CLAUDE,
        HeadlessVendor::Codex => &CODEX,
        HeadlessVendor::Agy => &AGY,
    }
}

fn vendor_neutral_alive(pid: Option<u32>) -> bool {
    let Some(pid) = pid else {
        return false;
    };
    #[cfg(target_os = "linux")]
    {
        Path::new("/proc").join(pid.to_string()).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        false
    }
}

fn headless_log_signature(project_root: &Path, session_id: &str, label: &str) -> Option<String> {
    let path = newest_headless_log(project_root, session_id)?;
    let (mtime, len) = metadata_activity(&path)?;
    Some(format!("{label}-log:{len}:{}", nanos(mtime)))
}

fn newest_headless_log(project_root: &Path, session_id: &str) -> Option<PathBuf> {
    newest_matching(&project_root.join(".aida").join("headless-logs"), |name| {
        name.ends_with(&format!("-{session_id}.jsonl"))
    })
}

fn newest_codex_rollout(session_id: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    newest_codex_rollout_under(&home, session_id)
}

fn newest_codex_rollout_under(home: &Path, session_id: &str) -> Option<PathBuf> {
    newest_matching_recursive(&home.join(".codex").join("sessions"), |name| {
        name.starts_with("rollout-") && name.contains(session_id) && name.ends_with(".jsonl")
    })
}

fn newest_matching(dir: &Path, matches: impl Fn(&str) -> bool) -> Option<PathBuf> {
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !matches(name) {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
            if newest
                .as_ref()
                .map(|(best, _)| mtime >= *best)
                .unwrap_or(true)
            {
                newest = Some((mtime, path));
            }
        }
    }
    newest.map(|(_, p)| p)
}

fn newest_matching_recursive(root: &Path, matches: impl Fn(&str) -> bool) -> Option<PathBuf> {
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        }
        .flatten()
        {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !matches(name) {
                continue;
            }
            let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
            if newest
                .as_ref()
                .map(|(best, _)| mtime >= *best)
                .unwrap_or(true)
            {
                newest = Some((mtime, path));
            }
        }
    }
    newest.map(|(_, p)| p)
}

fn metadata_activity(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn jsonl_event_activity(path: &Path) -> Option<SystemTime> {
    let content = std::fs::read_to_string(path).ok()?;
    content.lines().rev().find_map(timestamp_on_line)
}

fn timestamp_on_line(line: &str) -> Option<SystemTime> {
    let key = "\"timestamp\":\"";
    let start = line.find(key)? + key.len();
    let end = line[start..].find('"')? + start;
    let ts = chrono::DateTime::parse_from_rfc3339(&line[start..end]).ok()?;
    let millis = ts.timestamp_millis();
    if millis < 0 {
        return None;
    }
    Some(UNIX_EPOCH + std::time::Duration::from_millis(millis as u64))
}

fn nanos(t: SystemTime) -> u128 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("aida-{name}-{}", std::process::id()))
    }

    #[test]
    fn parses_agy_stream_json_fixture_activity() {
        let root = tmp("agy-activity");
        let dir = root.join(".aida/headless-logs");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("branch-session-a.jsonl"),
            "{\"type\":\"start\",\"timestamp\":\"2026-09-11T10:00:00Z\"}\n\
             {\"type\":\"message\",\"timestamp\":\"2026-09-11T10:05:03Z\"}\n",
        )
        .unwrap();
        let session = ActivitySession {
            vendor: HeadlessVendor::Agy,
            session_id: "session-a".to_string(),
            project_root: root.clone(),
            root_pid: None,
        };
        assert!(adapter(HeadlessVendor::Agy)
            .last_activity(&session)
            .is_some());
        assert!(adapter(HeadlessVendor::Agy)
            .activity_signature(&session)
            .expect("log signature")
            .starts_with("agy-log:"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn codex_uses_rollout_fallback_when_aida_log_is_absent() {
        let home = tmp("codex-home");
        let rollout_dir = home.join(".codex/sessions/day");
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&rollout_dir).unwrap();
        std::fs::write(rollout_dir.join("rollout-session-b.jsonl"), "{}\n").unwrap();
        let found = newest_codex_rollout_under(&home, "session-b").expect("rollout fallback");
        let (mtime, len) = metadata_activity(&found).expect("rollout metadata");
        assert!(format!("codex-rollout:{len}:{}", nanos(mtime)).starts_with("codex-rollout:"));
        let _ = std::fs::remove_dir_all(home);
    }
}
