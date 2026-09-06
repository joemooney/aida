//! Live drain projection for the default EPIC-54 cockpit.
//!
//! Reads the same `.aida/drain-state.json` payload that `aida drain status`
//! renders and corroborates the recorded orchestrator PID through
//! `aida_core::liveness::pid_is_alive`, the shared process probe used by the
//! other live-work surfaces. Stale or missing files return `None`, so the
//! cockpit stays uncluttered when no drain is live.
//!
//! trace:STORY-833 | ai:codex

use std::path::{Path, PathBuf};

use serde::Deserialize;

const STATE_QUEUED: &str = "queued";
const STATE_COMPLETED: &str = "completed";
const STATE_FAILED: &str = "failed";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrainPanel {
    pub scope: String,
    pub command: String,
    pub orchestrator_pid: u32,
    pub started_at: String,
    pub current: Option<String>,
    pub current_phase: Option<String>,
    pub members: Vec<DrainPanelMember>,
    pub on_exit: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrainPanelMember {
    pub spec: String,
    pub state: String,
    pub pr: Option<u32>,
}

impl DrainPanel {
    pub fn current_position(&self) -> Option<usize> {
        let current = self.current.as_ref()?;
        self.members
            .iter()
            .position(|m| &m.spec == current)
            .map(|i| i + 1)
    }

    pub fn started_local(&self) -> String {
        match chrono::DateTime::parse_from_rfc3339(&self.started_at) {
            Ok(dt) => dt
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string(),
            Err(_) => self.started_at.clone(),
        }
    }
}

impl DrainPanelMember {
    pub fn label(&self) -> String {
        match self.state.as_str() {
            STATE_COMPLETED => "completed".to_string(),
            STATE_FAILED => "failed".to_string(),
            STATE_QUEUED => "queued".to_string(),
            other => other
                .strip_prefix("in-phase-")
                .map(|n| format!("phase {n}"))
                .unwrap_or_else(|| other.to_string()),
        }
    }

    pub fn is_running(&self) -> bool {
        self.state.starts_with("in-phase-")
    }

    pub fn is_failed(&self) -> bool {
        self.state == STATE_FAILED
    }
}

#[derive(Debug, Deserialize)]
struct DrainStateFile {
    command: String,
    mode: String,
    #[serde(default)]
    batch: Option<String>,
    members: Vec<DrainMemberFile>,
    #[serde(default)]
    current: Option<String>,
    #[serde(default)]
    current_phase: Option<String>,
    orchestrator_pid: u32,
    started_at: String,
    on_drain_complete: String,
}

#[derive(Debug, Deserialize)]
struct DrainMemberFile {
    spec: String,
    state: String,
    #[serde(default)]
    pr: Option<u32>,
}

pub fn probe(project_root: &Path) -> Option<DrainPanel> {
    let path = drain_state_path(project_root);
    let body = std::fs::read_to_string(path).ok()?;
    let raw: DrainStateFile = serde_json::from_str(&body).ok()?;
    if !aida_core::liveness::pid_is_alive(raw.orchestrator_pid) {
        return None;
    }
    Some(panel_from(raw))
}

fn drain_state_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("drain-state.json")
}

fn panel_from(raw: DrainStateFile) -> DrainPanel {
    let scope = match raw.batch {
        Some(name) => format!("batch:{name}"),
        None if raw.mode == "next-n" => "next-N queue drain".to_string(),
        None => raw
            .members
            .first()
            .map(|m| m.spec.clone())
            .unwrap_or_else(|| "single spec".to_string()),
    };
    DrainPanel {
        scope,
        command: raw.command,
        orchestrator_pid: raw.orchestrator_pid,
        started_at: raw.started_at,
        current: raw.current,
        current_phase: raw.current_phase,
        members: raw
            .members
            .into_iter()
            .map(|m| DrainPanelMember {
                spec: m.spec,
                state: m.state,
                pr: m.pr,
            })
            .collect(),
        on_exit: raw.on_drain_complete,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_from_batch_names_scope_and_position() {
        let panel = panel_from(DrainStateFile {
            command: "aida queue work --batch demo --auto-complete".into(),
            mode: "batch".into(),
            batch: Some("demo".into()),
            members: vec![
                DrainMemberFile {
                    spec: "TASK-1".into(),
                    state: STATE_COMPLETED.into(),
                    pr: Some(10),
                },
                DrainMemberFile {
                    spec: "TASK-2".into(),
                    state: "in-phase-2".into(),
                    pr: None,
                },
            ],
            current: Some("TASK-2".into()),
            current_phase: Some("2 (ci)".into()),
            orchestrator_pid: 123,
            started_at: "2026-09-05T12:00:00Z".into(),
            on_drain_complete: "next queued item remains".into(),
        });

        assert_eq!(panel.scope, "batch:demo");
        assert_eq!(panel.current_position(), Some(2));
        assert_eq!(panel.members[0].label(), "completed");
        assert_eq!(panel.members[1].label(), "phase 2");
        assert!(panel.members[1].is_running());
    }

    #[test]
    fn panel_from_next_n_uses_queue_scope_label() {
        let panel = panel_from(DrainStateFile {
            command: "aida queue work next3 --auto-complete".into(),
            mode: "next-n".into(),
            batch: None,
            members: vec![DrainMemberFile {
                spec: "TASK-1".into(),
                state: STATE_QUEUED.into(),
                pr: None,
            }],
            current: None,
            current_phase: None,
            orchestrator_pid: 123,
            started_at: "bad-time".into(),
            on_drain_complete: "done".into(),
        });

        assert_eq!(panel.scope, "next-N queue drain");
        assert_eq!(panel.current_position(), None);
        assert_eq!(panel.started_local(), "bad-time");
    }
}
