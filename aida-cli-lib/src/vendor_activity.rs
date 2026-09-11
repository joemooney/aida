//! Vendor activity adapter for headless agent phases.
//!
//! The watchdog, drain-status pacing, and liveness surfaces need the same
//! answer: is this launched session still alive, and when did it last emit real
//! output? Vendor-specific log formats live here so callers do not grow
//! `claude` / `codex` / `agy` branches of their own.
//!
//! trace:STORY-1054 | ai:codex

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::session::HeadlessVendor;

#[derive(Debug, Clone)]
pub(crate) struct VendorActivityContext {
    pub(crate) project_root: PathBuf,
    pub(crate) session_id: String,
}

impl VendorActivityContext {
    pub(crate) fn new(project_root: &Path, session_id: &str) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
            session_id: session_id.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActivitySnapshot {
    pub(crate) alive: bool,
    pub(crate) last_activity: Option<SystemTime>,
    pub(crate) source: Option<&'static str>,
}

impl ActivitySnapshot {
    fn from_parts(
        alive: bool,
        last_activity: Option<SystemTime>,
        source: Option<&'static str>,
    ) -> Self {
        Self {
            alive,
            last_activity,
            source,
        }
    }

    pub(crate) fn signature(&self) -> Option<String> {
        let source = self.source?;
        let nanos = self
            .last_activity?
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        Some(format!("{source}:{nanos}:{}", self.alive))
    }
}

pub(crate) trait VendorActivity {
    fn snapshot(&self, ctx: &VendorActivityContext) -> ActivitySnapshot;
}

pub(crate) fn snapshot(vendor: HeadlessVendor, ctx: &VendorActivityContext) -> ActivitySnapshot {
    match vendor {
        HeadlessVendor::Claude => ClaudeActivity.snapshot(ctx),
        HeadlessVendor::Codex => CodexActivity.snapshot(ctx),
        HeadlessVendor::Agy => AgyActivity.snapshot(ctx),
    }
}

fn log_path_for_session(project_root: &Path, session_id: &str) -> Option<PathBuf> {
    let dir = project_root.join(".aida").join("headless-logs");
    let suffix = format!("-{session_id}.jsonl");
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(&dir).ok()?.flatten() {
        let path = entry.path();
        let is_match = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with(&suffix))
            .unwrap_or(false);
        if !is_match {
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
    newest.map(|(_, path)| path)
}

fn parsed_jsonl_activity(path: &Path) -> Option<SystemTime> {
    let body = fs::read_to_string(path).ok()?;
    let saw_event = body.lines().any(|line| {
        !line.trim().is_empty() && serde_json::from_str::<serde_json::Value>(line).is_ok()
    });
    if !saw_event {
        return None;
    }
    fs::metadata(path).ok()?.modified().ok()
}

fn shared_log_activity(ctx: &VendorActivityContext) -> Option<SystemTime> {
    let path = log_path_for_session(&ctx.project_root, &ctx.session_id)?;
    parsed_jsonl_activity(&path)
}

fn lease_pid_alive(ctx: &VendorActivityContext) -> bool {
    let Some((_, _, _, creator_pid)) =
        crate::find_orchestrated_lease(&ctx.project_root, &ctx.session_id)
    else {
        return false;
    };
    creator_pid.is_some_and(pid_alive)
}

fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
        rc == 0
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

struct ClaudeActivity;
struct CodexActivity;
struct AgyActivity;

impl VendorActivity for ClaudeActivity {
    fn snapshot(&self, ctx: &VendorActivityContext) -> ActivitySnapshot {
        ActivitySnapshot::from_parts(
            lease_pid_alive(ctx),
            shared_log_activity(ctx),
            Some("headless_log"),
        )
    }
}

impl VendorActivity for CodexActivity {
    fn snapshot(&self, ctx: &VendorActivityContext) -> ActivitySnapshot {
        if let Some(at) = shared_log_activity(ctx) {
            return ActivitySnapshot::from_parts(
                lease_pid_alive(ctx),
                Some(at),
                Some("headless_log"),
            );
        }
        ActivitySnapshot::from_parts(
            lease_pid_alive(ctx),
            codex_rollout_activity(&ctx.session_id),
            Some("codex_rollout"),
        )
    }
}

impl VendorActivity for AgyActivity {
    fn snapshot(&self, ctx: &VendorActivityContext) -> ActivitySnapshot {
        ActivitySnapshot::from_parts(
            lease_pid_alive(ctx),
            shared_log_activity(ctx),
            Some("headless_log"),
        )
    }
}

fn codex_sessions_root() -> Option<PathBuf> {
    if let Ok(root) = std::env::var("AIDA_CODEX_SESSIONS_DIR") {
        let trimmed = root.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    dirs::home_dir().map(|home| home.join(".codex").join("sessions"))
}

fn codex_rollout_activity(session_id: &str) -> Option<SystemTime> {
    let root = codex_sessions_root()?;
    let mut newest = None;
    visit_rollouts(&root, session_id, &mut newest);
    newest
}

fn visit_rollouts(dir: &Path, session_id: &str, newest: &mut Option<SystemTime>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            visit_rollouts(&path, session_id, newest);
            continue;
        }
        let is_rollout = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
            .unwrap_or(false);
        if !is_rollout {
            continue;
        }
        let matches_session = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|name| name.contains(session_id))
            .unwrap_or(false)
            || fs::read_to_string(&path)
                .map(|body| body.contains(session_id))
                .unwrap_or(false);
        if !matches_session {
            continue;
        }
        let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
        if newest.map(|best| mtime > best).unwrap_or(true) {
            *newest = Some(mtime);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn claude_activity_parses_stream_json_log() {
        let tmp = tempfile::tempdir().unwrap();
        let session = "019e9999-0000-7000-8000-000000000000";
        let dir = tmp.path().join(".aida").join("headless-logs");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("story-1054-{session}.jsonl")),
            "{\"type\":\"assistant\",\"message\":{\"content\":[]}}\n",
        )
        .unwrap();
        let ctx = VendorActivityContext::new(tmp.path(), session);

        let snap = snapshot(HeadlessVendor::Claude, &ctx);

        assert_eq!(snap.source, Some("headless_log"));
        assert!(snap.last_activity.is_some());
    }

    #[test]
    fn codex_activity_falls_back_to_matching_rollout_file() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("codex").join("sessions").join("2026");
        fs::create_dir_all(&sessions).unwrap();
        let session = "codex-session-1054";
        fs::write(
            sessions.join("rollout-abc.jsonl"),
            format!("{{\"session_id\":\"{session}\",\"event\":\"step\"}}\n"),
        )
        .unwrap();
        std::env::set_var(
            "AIDA_CODEX_SESSIONS_DIR",
            tmp.path().join("codex").join("sessions"),
        );
        let ctx = VendorActivityContext::new(tmp.path(), session);

        let snap = snapshot(HeadlessVendor::Codex, &ctx);

        assert_eq!(snap.source, Some("codex_rollout"));
        assert!(snap.last_activity.is_some());
        std::env::remove_var("AIDA_CODEX_SESSIONS_DIR");
    }

    #[test]
    fn agy_activity_parses_stream_json_fixture() {
        let tmp = tempfile::tempdir().unwrap();
        let session = "agy-session-1054";
        let dir = tmp.path().join(".aida").join("headless-logs");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("story-1054-{session}.jsonl")),
            "{\"type\":\"assistant_delta\",\"delta\":\"ok\"}\n",
        )
        .unwrap();
        let ctx = VendorActivityContext::new(tmp.path(), session);

        let snap = snapshot(HeadlessVendor::Agy, &ctx);

        assert_eq!(snap.source, Some("headless_log"));
        assert!(snap.last_activity.is_some());
    }

    #[test]
    fn activity_signature_changes_when_timestamp_changes() {
        let a = ActivitySnapshot::from_parts(false, Some(UNIX_EPOCH), Some("headless_log"));
        let b = ActivitySnapshot::from_parts(
            false,
            Some(UNIX_EPOCH + Duration::from_secs(1)),
            Some("headless_log"),
        );

        assert_ne!(a.signature(), b.signature());
    }
}
