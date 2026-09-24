//! TASK-1454: the pending-approval marker — local runtime state recording
//! that a LIVE Claude Code seat is blocked on an unanswered permission
//! prompt (a `Notification` hook fired with the `permission_prompt`
//! matcher). This is the ground-truth signal `aida ps` / `aida awaiting`
//! were missing per BUG-1553: a purely time/process-based heuristic cannot
//! distinguish a long-running tool call from a genuine unanswered approval
//! gate, but Claude Code's own Notification hook can, because it only fires
//! when Claude Code itself is showing that prompt.
//!
//! One JSON file per Claude Code session id under
//! `.aida/pending-approval/<session>.json` — local per-clone runtime state,
//! covered by the deny-by-default `.aida/*` gitignore rule (no new
//! allow-list entry needed, the same convention `.aida/session-notices/`
//! and `.aida/sessions/` (leases) already use).
//!
//! Lifecycle: the `Notification(permission_prompt)` hook writes the marker
//! (`aida session pending-approval-set`); the next `UserPromptSubmit` or
//! `PostToolUse` for that session clears it (`aida session
//! pending-approval-clear`) — either means the block resolved (a human
//! answered, or sent a new prompt). A marker that outlives
//! [`PENDING_APPROVAL_STALE_SECS`] with neither event firing (a crashed or
//! killed session never gets a next turn) is treated as stale and ignored —
//! and opportunistically swept off disk the next time anything lists the
//! directory, so a dead session's marker doesn't sit forever.
//!
//! trace:TASK-1454 | ai:claude

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Directory name under `.aida/` holding one marker file per blocked session.
const PENDING_APPROVAL_DIRNAME: &str = "pending-approval";

/// A marker older than this is stale and no longer counts as "blocked" —
/// the backstop for a session that crashes before either clearing event
/// fires. The BUG-1553 incident this feature closes ran ~20 minutes with no
/// resolving signal at all; this floor sits comfortably above any
/// legitimate wait for a human to notice and answer a prompt.
// trace:TASK-1454 | ai:claude
pub(crate) const PENDING_APPROVAL_STALE_SECS: i64 = 30 * 60;

/// One recorded pending-approval marker.
// trace:TASK-1454 | ai:claude
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PendingApprovalMarker {
    /// The Claude Code session id the Notification hook fired for — joined
    /// against a lease's own resolved `claude_session_id` (the same
    /// STORY-153 join BUG-1553's activity probe already uses) to attribute
    /// the marker to an `aida ps` row.
    pub session_id: String,
    /// The tool name Claude Code is asking permission for, when the
    /// notification message could be parsed for one. `None` is still a
    /// useful marker — the seat is confirmed blocked even when the tool
    /// name couldn't be extracted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The raw notification message (truncated), kept for `aida ps
    /// --verbose`-style debugging. Never required for the render/classify
    /// logic itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// When the marker was written — the "blocked since" timestamp the
    /// elapsed/staleness math is computed from.
    pub since: DateTime<Utc>,
}

/// `.aida/pending-approval/` under the given project root.
// trace:TASK-1454 | ai:claude
pub(crate) fn pending_approval_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join(PENDING_APPROVAL_DIRNAME)
}

/// Sanitize a Claude Code session id into a safe filename — the same
/// defensive neutralize-path-separators pattern `session_notice_path`
/// (`session_reap.rs`, FR-284) uses for lease ids, applied here so a
/// crafted/odd session id can never escape the marker directory.
// trace:TASK-1454 | ai:claude
fn safe_marker_filename(session_id: &str) -> String {
    let safe: String = session_id
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == '.' {
                '_'
            } else {
                c
            }
        })
        .collect();
    format!("{safe}.json")
}

/// The on-disk path for `session_id`'s marker (whether or not it exists).
// trace:TASK-1454 | ai:claude
pub(crate) fn pending_approval_path(project_root: &Path, session_id: &str) -> PathBuf {
    pending_approval_dir(project_root).join(safe_marker_filename(session_id))
}

/// Write (or overwrite) the marker for `session_id` — the
/// `Notification(permission_prompt)` hook's side effect.
// trace:TASK-1454 | ai:claude
pub(crate) fn write_marker(
    project_root: &Path,
    session_id: &str,
    tool: Option<&str>,
    message: Option<&str>,
    now: DateTime<Utc>,
) -> std::io::Result<()> {
    let dir = pending_approval_dir(project_root);
    std::fs::create_dir_all(&dir)?;
    let marker = PendingApprovalMarker {
        session_id: session_id.to_string(),
        tool: tool
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        // A generous cap — this is a debugging aid, never load-bearing for
        // the classify/render logic.
        message: message
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(200).collect()),
        since: now,
    };
    let body = serde_json::to_string_pretty(&marker)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(pending_approval_path(project_root, session_id), body)
}

/// Clear `session_id`'s marker. Idempotent — a missing file is success, not
/// an error, since the common case is "nothing to clear" (most turns have
/// no marker at all).
// trace:TASK-1454 | ai:claude
pub(crate) fn clear_marker(project_root: &Path, session_id: &str) -> std::io::Result<()> {
    match std::fs::remove_file(pending_approval_path(project_root, session_id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn read_marker_file(path: &Path) -> Option<PendingApprovalMarker> {
    let body = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&body).ok()
}

/// Is `marker` past [`PENDING_APPROVAL_STALE_SECS`] as of `now`?
// trace:TASK-1454 | ai:claude
pub(crate) fn is_stale(marker: &PendingApprovalMarker, now: DateTime<Utc>) -> bool {
    now.signed_duration_since(marker.since).num_seconds() >= PENDING_APPROVAL_STALE_SECS
}

/// Every marker on disk that is NOT stale as of `now`. A stale marker is
/// opportunistically deleted here rather than merely filtered — the normal
/// clearing hooks only fire on THAT session's own next turn, so a crashed
/// session's marker would otherwise sit in the directory forever; sweeping
/// it here keeps every other reader (both `aida ps` and `aida awaiting`,
/// which both call this) self-healing with no separate GC command.
///
/// Cheap by construction for the common case: when the directory is empty
/// (no seat is blocked — true on the overwhelming majority of turns) this
/// is a single `read_dir` that returns nothing, no lease/process probing at
/// all. Callers that need to resolve a marker to a spec/lease (`aida ps`,
/// `aida awaiting`) only pay that heavier cost when this list is non-empty.
// trace:TASK-1454 | ai:claude
pub(crate) fn list_active(project_root: &Path, now: DateTime<Utc>) -> Vec<PendingApprovalMarker> {
    let dir = pending_approval_dir(project_root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(marker) = read_marker_file(&path) else {
            continue;
        };
        if is_stale(&marker, now) {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        out.push(marker);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        chrono::Utc::now()
    }

    // trace:TASK-1454 | ai:claude
    #[test]
    fn write_then_read_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let t = now();
        write_marker(
            tmp.path(),
            "sess-1",
            Some("Bash"),
            Some("needs your permission"),
            t,
        )
        .unwrap();
        let markers = list_active(tmp.path(), t);
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].session_id, "sess-1");
        assert_eq!(markers[0].tool.as_deref(), Some("Bash"));
    }

    // trace:TASK-1454 | ai:claude
    #[test]
    fn empty_dir_is_cheap_and_returns_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        // No marker directory at all — must degrade cleanly, not error.
        assert!(list_active(tmp.path(), now()).is_empty());
    }

    // trace:TASK-1454 | ai:claude
    #[test]
    fn clear_removes_the_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let t = now();
        write_marker(tmp.path(), "sess-2", None, None, t).unwrap();
        assert_eq!(list_active(tmp.path(), t).len(), 1);
        clear_marker(tmp.path(), "sess-2").unwrap();
        assert!(list_active(tmp.path(), t).is_empty());
    }

    // trace:TASK-1454 | ai:claude
    #[test]
    fn clear_of_missing_marker_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        clear_marker(tmp.path(), "never-written").unwrap();
    }

    // trace:TASK-1454 | ai:claude
    #[test]
    fn stale_marker_is_excluded_and_swept_from_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let old = now() - chrono::Duration::seconds(PENDING_APPROVAL_STALE_SECS + 60);
        write_marker(tmp.path(), "sess-3", Some("Write"), None, old).unwrap();
        let path = pending_approval_path(tmp.path(), "sess-3");
        assert!(path.exists());
        assert!(list_active(tmp.path(), now()).is_empty());
        // Self-healing: the stale file is removed, not just filtered.
        assert!(!path.exists());
    }

    // trace:TASK-1454 | ai:claude
    #[test]
    fn marker_just_under_the_stale_floor_still_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let t = now();
        let almost_stale = t - chrono::Duration::seconds(PENDING_APPROVAL_STALE_SECS - 5);
        write_marker(tmp.path(), "sess-4", None, None, almost_stale).unwrap();
        assert_eq!(list_active(tmp.path(), t).len(), 1);
    }

    // trace:TASK-1454 | ai:claude
    #[test]
    fn session_id_with_path_separators_cannot_escape_the_marker_dir() {
        let tmp = tempfile::tempdir().unwrap();
        write_marker(tmp.path(), "../../etc/passwd", Some("Bash"), None, now()).unwrap();
        let dir = pending_approval_dir(tmp.path());
        // The written file must land INSIDE the marker dir, never above it.
        let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(entries.len(), 1);
    }

    // trace:TASK-1454 | ai:claude
    #[test]
    fn malformed_marker_file_is_skipped_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = pending_approval_dir(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("garbage.json"), "not json").unwrap();
        assert!(list_active(tmp.path(), now()).is_empty());
    }
}
