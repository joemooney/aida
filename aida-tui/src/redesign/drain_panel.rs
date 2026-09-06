//! Live drain projection for the default EPIC-54 cockpit.
//!
//! Reads the same `.aida/drain-state.json` payload that `aida drain status`
//! renders, resolving the shared main worktree before probing so sibling
//! worktrees see the orchestrator-owned state file. It corroborates the
//! recorded orchestrator PID through `aida_core::liveness::pid_is_alive`, the
//! shared process probe used by the other live-work surfaces. Stale or missing
//! files return `None`, so the cockpit stays uncluttered when no drain is live.
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
    let drain_root = main_worktree_root_from(project_root);
    let path = drain_state_path(&drain_root);
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

/// Resolve the shared main worktree root the same way `aida drain status`
/// does, so a cockpit launched from a sibling worktree reads the canonical
/// drain-state file next to the orchestrator.
// trace:STORY-833 | ai:codex
fn main_worktree_root_from(start: &Path) -> PathBuf {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(start)
        .args(["worktree", "list", "--porcelain"])
        .output();
    if let Ok(o) = out {
        if o.status.success() {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    return PathBuf::from(p);
                }
            }
        }
    }
    start.to_path_buf()
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
    use std::process::Command;

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

    #[test]
    fn probe_reads_main_worktree_drain_state_from_sibling_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        let sibling = tmp.path().join("sibling");
        std::fs::create_dir(&main).unwrap();
        run_git(&main, &["init"]);
        run_git(&main, &["config", "user.email", "codex@example.invalid"]);
        run_git(&main, &["config", "user.name", "Codex"]);
        std::fs::write(main.join("README.md"), "fixture\n").unwrap();
        run_git(&main, &["add", "README.md"]);
        run_git(&main, &["commit", "-m", "fixture"]);
        run_git(
            &main,
            &["worktree", "add", "--detach", sibling.to_str().unwrap()],
        );

        let aida_dir = main.join(".aida");
        std::fs::create_dir(&aida_dir).unwrap();
        std::fs::write(
            aida_dir.join("drain-state.json"),
            format!(
                r#"{{
  "command": "aida queue work --batch demo --auto-complete",
  "mode": "batch",
  "batch": "demo",
  "members": [
    {{"spec": "TASK-1", "state": "completed", "pr": 10}},
    {{"spec": "TASK-2", "state": "in-phase-2", "pr": null}}
  ],
  "current": "TASK-2",
  "current_phase": "2 (ci)",
  "orchestrator_pid": {},
  "started_at": "2026-09-05T12:00:00Z",
  "on_drain_complete": "next queued item remains"
}}"#,
                std::process::id()
            ),
        )
        .unwrap();

        let panel = probe(&sibling).expect("sibling worktree should see main drain state");

        assert_eq!(panel.scope, "batch:demo");
        assert_eq!(panel.current.as_deref(), Some("TASK-2"));
        assert_eq!(panel.current_position(), Some(2));
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
