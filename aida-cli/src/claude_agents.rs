//! Query `claude agents --json` for the live Claude Code session view.
//!
//! SPIKE-30 (2026-05-29): the strategic-recompose synthesis points at
//! `claude agents --json` as the highest-leverage compose surface from
//! Claude Code 2.1.154. This module is the read-side: shell out, parse,
//! and graceful-degrade when the binary isn't present or returns garbage.
//!
//! AIDA's `aida status` uses it to surface the cross-substrate picture:
//! AIDA leases ↔ live Claude Code sessions, with explicit drift detection
//! (lease without process, process without lease).
//!
//! Schema confirmed live on 2026-05-29 against claude 2.1.156:
//!   `pid` (u32), `cwd` (string), `kind` (string), `startedAt` (millis),
//!   `sessionId` (uuid), and optional `name`, `status`.
//!
//! trace:SPIKE-30 | ai:claude

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // pid/kind/started_at_ms/name are read via the json! macro
                    // in print_status_json's claude_code projection; static
                    // dead-code analysis misses that indirection.
pub struct ClaudeAgentEntry {
    pub pid: u32,
    pub cwd: PathBuf,
    pub kind: String,
    #[serde(rename = "startedAt")]
    pub started_at_ms: i64,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

impl ClaudeAgentEntry {
    /// Short prefix of the sessionId for compact display columns.
    pub fn short_session_id(&self) -> String {
        self.session_id.chars().take(8).collect()
    }
}

/// Query `claude agents --json`. Returns `None` if the binary isn't on PATH,
/// the command fails, or stdout isn't valid JSON — `aida status` MUST NOT
/// fail when Claude Code isn't installed.
pub fn list_agents() -> Option<Vec<ClaudeAgentEntry>> {
    // TASK-1081: route the vendor binary through the single headless-spawn
    // resolver so an `AIDA_AGENT_CMD` mock also serves the liveness query; unset
    // yields the native `claude` (byte-identical). trace:TASK-1081
    let output = Command::new(crate::session::resolve_agent_program("claude"))
        .args(["agents", "--json"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice::<Vec<ClaudeAgentEntry>>(&output.stdout).ok()
}

/// Partition entries into (in_scope, elsewhere) by cwd. "In scope" = cwd is
/// `project_root` (or under it), OR cwd matches a known AIDA worktree path
/// passed in via `worktree_paths` (the active leases' `worktree_path`
/// values). The lease-set is the authoritative source for sibling-worktree
/// in-scopeness — a heuristic on the `<project>-` filename prefix
/// false-positives unrelated sibling projects like `aida-tutor` / `aida-chat`.
pub fn partition_by_project(
    entries: &[ClaudeAgentEntry],
    project_root: &Path,
    worktree_paths: &[PathBuf],
) -> (Vec<ClaudeAgentEntry>, Vec<ClaudeAgentEntry>) {
    let mut in_scope = Vec::new();
    let mut elsewhere = Vec::new();
    for entry in entries {
        let cwd = entry.cwd.as_path();
        let under_root = cwd == project_root || cwd.starts_with(project_root);
        let matches_worktree = worktree_paths
            .iter()
            .any(|w| cwd == w || cwd.starts_with(w));
        if under_root || matches_worktree {
            in_scope.push(entry.clone());
        } else {
            elsewhere.push(entry.clone());
        }
    }
    (in_scope, elsewhere)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(cwd: &str, session_id: &str) -> ClaudeAgentEntry {
        ClaudeAgentEntry {
            pid: 1,
            cwd: PathBuf::from(cwd),
            kind: "interactive".into(),
            started_at_ms: 0,
            session_id: session_id.into(),
            name: None,
            status: None,
        }
    }

    #[test]
    fn partition_treats_lease_worktrees_as_in_scope() {
        let entries = vec![
            entry("/home/joe/ai/aida", "a"),
            entry("/home/joe/ai/aida-task-574", "b"),
            entry("/home/joe/ai/aida-chat", "c"),
            entry("/home/joe/other/repo", "d"),
        ];
        let project = PathBuf::from("/home/joe/ai/aida");
        let worktrees = vec![PathBuf::from("/home/joe/ai/aida-task-574")];
        let (in_scope, elsewhere) = partition_by_project(&entries, &project, &worktrees);
        let in_ids: Vec<_> = in_scope.iter().map(|e| e.session_id.clone()).collect();
        let out_ids: Vec<_> = elsewhere.iter().map(|e| e.session_id.clone()).collect();
        assert_eq!(in_ids, vec!["a", "b"]);
        assert_eq!(out_ids, vec!["c", "d"]);
    }

    #[test]
    fn partition_rejects_unrelated_siblings_without_lease_match() {
        // aida-tutor / aida-chat share the project's `aida-` prefix but
        // aren't AIDA worktrees — they're separate projects. Without a
        // lease covering them, they belong in `elsewhere`.
        let entries = vec![
            entry("/home/joe/ai/aida-tutor", "tut"),
            entry("/home/joe/ai/aida-chat", "chat"),
        ];
        let project = PathBuf::from("/home/joe/ai/aida");
        let (in_scope, elsewhere) = partition_by_project(&entries, &project, &[]);
        assert!(in_scope.is_empty());
        assert_eq!(elsewhere.len(), 2);
    }

    #[test]
    fn short_session_id_truncates_to_eight() {
        let e = entry("/x", "019e71f4-c08d-7a31-a29d-03a34ae899a9");
        assert_eq!(e.short_session_id(), "019e71f4");
    }

    #[test]
    fn empty_input_partitions_to_empty() {
        let project = PathBuf::from("/home/joe/ai/aida");
        let (a, b) = partition_by_project(&[], &project, &[]);
        assert!(a.is_empty());
        assert!(b.is_empty());
    }

    #[test]
    fn parses_real_claude_agents_json() {
        let sample = r#"[
          {"pid":792651,"cwd":"/home/joe/ai/aida","kind":"interactive","startedAt":1780027101320,"sessionId":"1bf450af-f9e9-49d0-a098-79696b14cefc","status":"busy"},
          {"pid":807372,"cwd":"/home/joe/ai/aida-task-574","kind":"interactive","startedAt":1780028332682,"sessionId":"019e71f4-c08d-7a31-a29d-03a34ae899a9"}
        ]"#;
        let parsed: Vec<ClaudeAgentEntry> = serde_json::from_str(sample).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].status.as_deref(), Some("busy"));
        assert!(parsed[1].status.is_none());
    }
}
