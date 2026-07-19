//! Global, role-scoped work queue spanning projects.
//!
//! Stored at `~/.aida/queue/<role>.yaml`. Entries are role-scoped because the
//! use case is "one persona, multiple projects" — e.g., the implementer hat
//! looks at AIDA in the morning and paradox in the afternoon and wants one
//! merged inbox of work routed to it.
//!
//! Local queues (per-project, at `<project>/.aida-store/registry/queues/<user>.yaml`)
//! and global queues are merged when listing without `--global`. Local takes
//! precedence on collisions; global entries are tagged `[origin:<project>]`.
//!
//! trace:FR-1-012 | ai:claude

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One entry in the global, role-scoped queue. Carries a project pointer so
/// callers can resolve the underlying requirement back to its store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalQueueEntry {
    pub requirement_id: uuid::Uuid,
    /// Absolute path to the project root (the directory containing `.aida/config.toml`).
    pub project_root: PathBuf,
    /// Human-readable project name (basename or `[deployment].name`-style label).
    pub project_name: String,
    /// Cached spec id for display when the foreign project is offline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_id: Option<String>,
    /// Cached agreed_id (short, merge-gated id) for display when the foreign
    /// project is offline. Mirrors the spec_id cache; renderers prefer
    /// `agreed_id.or(spec_id)` so global queue surfaces stop diverging
    /// from `aida list` / local queue after `aida db merge-gate`.
    /// trace:BUG-83 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agreed_id: Option<String>,
    /// Cached title for display when the foreign project is offline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub position: i64,
    pub added_by: String,
    pub added_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The role this entry is routed to (the same value as the queue file's name).
    pub for_role: String,
}

/// Path to the global queue file for the given role. Creates `~/.aida/queue/`
/// if it doesn't exist.
pub fn queue_path(role: &str) -> Result<PathBuf> {
    let home = dirs::home_dir().context("Cannot determine home directory for global queue")?;
    let dir = home.join(".aida").join("queue");
    std::fs::create_dir_all(&dir).with_context(|| {
        format!(
            "Failed to create global queue directory at {}",
            dir.display()
        )
    })?;
    Ok(dir.join(format!("{}.yaml", role)))
}

pub fn load(role: &str) -> Result<Vec<GlobalQueueEntry>> {
    let path = queue_path(role)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&path)?;
    Ok(serde_yaml::from_str(&content).unwrap_or_default())
}

pub fn save(role: &str, entries: &[GlobalQueueEntry]) -> Result<()> {
    let path = queue_path(role)?;
    let yaml = serde_yaml::to_string(entries)?;
    std::fs::write(&path, yaml)?;
    Ok(())
}

/// Upsert an entry. Two entries match when their (requirement_id, project_root)
/// pair matches — same requirement in different projects is allowed (and would
/// be unusual but valid).
pub fn add(role: &str, entry: GlobalQueueEntry) -> Result<()> {
    let mut entries = load(role)?;
    entries.retain(|e| {
        !(e.requirement_id == entry.requirement_id && e.project_root == entry.project_root)
    });
    entries.push(entry);
    entries.sort_by_key(|e| e.position);
    save(role, &entries)
}

/// Remove an entry by requirement id (optionally scoped to a specific project).
/// Returns true if at least one entry was removed.
pub fn remove(
    role: &str,
    requirement_id: &uuid::Uuid,
    project_root: Option<&Path>,
) -> Result<bool> {
    let mut entries = load(role)?;
    let before = entries.len();
    entries.retain(|e| {
        if e.requirement_id != *requirement_id {
            return true;
        }
        match project_root {
            Some(root) => e.project_root != root,
            None => false,
        }
    });
    let removed = entries.len() != before;
    if removed {
        save(role, &entries)?;
    }
    Ok(removed)
}

/// Best-effort label for the current project: prefer the basename of the
/// project root (visible to humans), fall back to a stringified path.
pub fn project_name_for(root: &Path) -> String {
    root.file_name()
        .and_then(|os| os.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| root.display().to_string())
}
