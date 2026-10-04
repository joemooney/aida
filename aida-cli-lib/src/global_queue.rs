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

use aida_core::{read_atomic, write_atomic};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
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
    let home = crate::home_dir().context("Cannot determine home directory for global queue")?;
    let dir = home.join(".aida").join("queue");
    std::fs::create_dir_all(&dir).with_context(|| {
        format!(
            "Failed to create global queue directory at {}",
            dir.display()
        )
    })?;
    Ok(dir.join(format!("{}.yaml", role)))
}

// trace:BUG-1682 | ai:codex
pub fn load(role: &str) -> Result<Vec<GlobalQueueEntry>> {
    read_queue(&queue_path(role)?, role)
}

// Only a final NotFound initializes a queue. In particular, an empty/null
// YAML document must not silently turn a damaged queue into an empty one.
// trace:BUG-1682 | ai:codex
fn read_queue(path: &Path, role: &str) -> Result<Vec<GlobalQueueEntry>> {
    let context = || {
        format!(
            "Failed to read global queue for role {role} at {}",
            path.display()
        )
    };
    let content = match read_atomic(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(context),
    };
    let document: serde_yaml::Value = serde_yaml::from_str(&content).with_context(context)?;
    anyhow::ensure!(
        document.is_sequence(),
        "{}: expected a YAML sequence",
        context()
    );
    serde_yaml::from_value(document).with_context(context)
}

// A permanent sidecar protects the complete read/modify/publish interval.
// Closing the descriptor releases the lock on every return or unwind. Never
// unlink it: a contender could otherwise lock a different inode. This is
// blocking, advisory coordination of cooperating writers, not fsync durability
// or transactional placement of positions computed by callers.
// trace:BUG-1682 | ai:codex
struct QueueMutation {
    path: PathBuf,
    role: String,
    _lock: std::fs::File,
}

// trace:BUG-1682 | ai:codex
impl QueueMutation {
    fn acquire(role: &str) -> Result<Self> {
        let path = queue_path(role)?;
        let sidecar = path.with_extension("lock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&sidecar)
            .with_context(|| {
                format!(
                    "Failed to open global queue lock for role {role} at {}",
                    sidecar.display()
                )
            })?;
        #[cfg(test)]
        tests::observe("before_lock", &path);
        lock.lock_exclusive().with_context(|| {
            format!(
                "Failed to lock global queue for role {role} at {}",
                sidecar.display()
            )
        })?;
        #[cfg(test)]
        tests::observe("after_lock", &path);
        Ok(Self {
            path,
            role: role.to_owned(),
            _lock: lock,
        })
    }

    fn load(&self) -> Result<Vec<GlobalQueueEntry>> {
        read_queue(&self.path, &self.role)
    }

    // No public snapshot-save escape hatch: publishing requires this guard.
    fn save(&self, entries: &[GlobalQueueEntry]) -> Result<()> {
        let yaml = serde_yaml::to_string(entries).with_context(|| {
            format!(
                "Failed to serialize global queue for role {} at {}",
                self.role,
                self.path.display()
            )
        })?;
        #[cfg(test)]
        tests::observe("before_publish", &self.path);
        write_atomic(&self.path, yaml).with_context(|| {
            format!(
                "Failed to write global queue for role {} at {}",
                self.role,
                self.path.display()
            )
        })?;
        #[cfg(test)]
        tests::observe("after_publish", &self.path);
        Ok(())
    }
}

/// Upsert an entry. Two entries match when their (requirement_id, project_root)
/// pair matches — same requirement in different projects is allowed (and would
/// be unusual but valid).
// trace:BUG-1682 | ai:codex
pub fn add(role: &str, entry: GlobalQueueEntry) -> Result<()> {
    let mutation = QueueMutation::acquire(role)?;
    let mut entries = mutation.load()?;
    entries.retain(|e| {
        !(e.requirement_id == entry.requirement_id && e.project_root == entry.project_root)
    });
    entries.push(entry);
    entries.sort_by_key(|e| e.position);
    mutation.save(&entries)
}

/// Remove an entry by requirement id (optionally scoped to a specific project).
/// Returns true if at least one entry was removed.
// trace:BUG-1682 | ai:codex
pub fn remove(
    role: &str,
    requirement_id: &uuid::Uuid,
    project_root: Option<&Path>,
) -> Result<bool> {
    let mutation = QueueMutation::acquire(role)?;
    let mut entries = mutation.load()?;
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
        mutation.save(&entries)?;
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

#[cfg(test)]
#[path = "tests/bug_1682_global_queue_tests.rs"]
mod tests;
