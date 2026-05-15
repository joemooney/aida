//! Crash recovery — `.aida/tui-state.json`.
//!
//! A TUI crash kills its PTY children, but each hosted Claude
//! conversation is a durable `.jsonl` file (TASK-112) that
//! `aida queue work --resume` can continue. This module records the live
//! tab set to `.aida/tui-state.json` so the next `aida tui` launch
//! re-attaches the orphaned sessions instead of losing them.
//!
//! Lifecycle: the file is rewritten on every tab spawn / close, so it
//! always reflects the current set. A clean `prefix q` quit clears it
//! (nothing to recover); `prefix d` detach and a hard crash both leave
//! it in place for the next launch to pick up.
//!
//! trace:STORY-135 | ai:claude

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One hosted session, enough to re-attach it. Only `scope` +
/// `session_id` are recorded: `aida queue work <scope> --resume <id>`
/// re-derives the role and worktree itself, and the TUI — which shells
/// out rather than owning that logic — never learns them to persist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabRecord {
    /// The Claude conversation id (resumable via `--resume`).
    pub session_id: String,
    /// The EPIC / STORY / … scope the session is working.
    pub scope: String,
}

/// The recoverable tab set written to `.aida/tui-state.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TuiState {
    pub tabs: Vec<TabRecord>,
}

/// `.aida/tui-state.json` under `project_root`. Covered by the
/// deny-by-default `.aida/*` gitignore rule (BUG-73) — never committed.
fn state_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("tui-state.json")
}

/// Write the live tab set. Best-effort — a write failure (e.g. `.aida`
/// is missing) is swallowed: crash recovery is a safety net, not
/// something worth aborting the TUI for.
pub fn save(project_root: &Path, state: &TuiState) {
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let _ = std::fs::write(state_path(project_root), json);
    }
}

/// Load a previously-recorded tab set. `None` when the file is absent,
/// unreadable, or malformed — each just means "nothing to recover."
pub fn load(project_root: &Path) -> Option<TuiState> {
    let raw = std::fs::read_to_string(state_path(project_root)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Remove the state file — a clean exit has nothing to recover, so a
/// later launch must not re-attach a session the user already closed.
pub fn clear(project_root: &Path) {
    let _ = std::fs::remove_file(state_path(project_root));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique temp dir with a `.aida/` subdir, mimicking a project root.
    fn temp_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "aida-tui-state-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        root
    }

    #[test]
    fn tui_state_roundtrips() {
        let root = temp_root();
        let state = TuiState {
            tabs: vec![
                TabRecord {
                    session_id: "019e2d4f-aaaa".to_string(),
                    scope: "EPIC-26".to_string(),
                },
                TabRecord {
                    session_id: "019e2d50-bbbb".to_string(),
                    scope: "BUG-9".to_string(),
                },
            ],
        };
        save(&root, &state);
        let loaded = load(&root).expect("state round-trips");
        assert_eq!(loaded.tabs.len(), 2);
        assert_eq!(loaded.tabs[0].scope, "EPIC-26");
        assert_eq!(loaded.tabs[1].session_id, "019e2d50-bbbb");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn load_is_none_when_absent_or_malformed() {
        let root = temp_root();
        // Absent file → nothing to recover.
        assert!(load(&root).is_none());

        // Malformed JSON → still None, never a panic.
        std::fs::write(state_path(&root), "{ not json").unwrap();
        assert!(load(&root).is_none());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn clear_removes_the_state_file() {
        let root = temp_root();
        save(&root, &TuiState::default());
        assert!(state_path(&root).exists());
        clear(&root);
        assert!(!state_path(&root).exists());
        // Clearing an already-absent file is a harmless no-op.
        clear(&root);

        std::fs::remove_dir_all(&root).ok();
    }
}
