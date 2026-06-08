// trace:ARCH-distributed-conflict | ai:claude
//! Conflict detection and resolution for distributed AIDA.
//!
//! When two nodes edit the same requirement concurrently, we need to:
//! 1. Detect the conflict (two versions with divergent histories)
//! 2. Surface it to the user (don't silently overwrite)
//! 3. Provide resolution options (accept-mine, accept-theirs, merge)
//!
//! This module implements field-level conflict detection by comparing
//! two versions of a requirement and identifying which fields diverged.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::Requirement;

/// A detected conflict between two versions of a requirement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequirementConflict {
    /// The requirement's UUID
    pub id: Uuid,
    /// The spec_id (for display)
    pub spec_id: String,
    /// Fields that have conflicting values
    pub fields: Vec<FieldConflict>,
    /// Timestamp of the local version
    pub local_modified: DateTime<Utc>,
    /// Timestamp of the remote version
    pub remote_modified: DateTime<Utc>,
}

/// A conflict on a single field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldConflict {
    /// Name of the conflicting field
    pub field: String,
    /// Local value
    pub local_value: String,
    /// Remote value
    pub remote_value: String,
}

/// How to resolve a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Keep the local version for all conflicting fields
    AcceptLocal,
    /// Keep the remote version for all conflicting fields
    AcceptRemote,
    /// Keep the version with the later timestamp (LWW)
    LastWriteWins,
}

/// Detect conflicts between a local and remote version of a requirement.
/// Returns None if there are no conflicts (versions are identical or one is strictly newer).
pub fn detect_conflict(local: &Requirement, remote: &Requirement) -> Option<RequirementConflict> {
    if local.id != remote.id {
        return None; // Different requirements, not a conflict
    }

    // If timestamps are identical, no conflict
    if local.modified_at == remote.modified_at {
        return None;
    }

    let mut fields = Vec::new();

    // Compare fields that matter for conflict detection
    if local.title != remote.title {
        fields.push(FieldConflict {
            field: "title".to_string(),
            local_value: local.title.clone(),
            remote_value: remote.title.clone(),
        });
    }

    if local.description != remote.description {
        fields.push(FieldConflict {
            field: "description".to_string(),
            local_value: truncate(&local.description, 100),
            remote_value: truncate(&remote.description, 100),
        });
    }

    if local.effective_status() != remote.effective_status() {
        fields.push(FieldConflict {
            field: "status".to_string(),
            local_value: local.effective_status(),
            remote_value: remote.effective_status(),
        });
    }

    if local.effective_priority() != remote.effective_priority() {
        fields.push(FieldConflict {
            field: "priority".to_string(),
            local_value: local.effective_priority(),
            remote_value: remote.effective_priority(),
        });
    }

    if local.owner != remote.owner {
        fields.push(FieldConflict {
            field: "owner".to_string(),
            local_value: local.owner.clone(),
            remote_value: remote.owner.clone(),
        });
    }

    if local.tags != remote.tags {
        fields.push(FieldConflict {
            field: "tags".to_string(),
            local_value: format!("{:?}", local.tags),
            remote_value: format!("{:?}", remote.tags),
        });
    }

    if fields.is_empty() {
        return None; // Same content despite different timestamps
    }

    Some(RequirementConflict {
        id: local.id,
        spec_id: local
            .spec_id
            .clone()
            .unwrap_or_else(|| local.id.to_string()),
        fields,
        local_modified: local.modified_at,
        remote_modified: remote.modified_at,
    })
}

/// Apply a resolution strategy to a conflict.
/// Returns the resolved requirement.
pub fn resolve_conflict(
    local: &Requirement,
    remote: &Requirement,
    resolution: Resolution,
) -> Requirement {
    match resolution {
        Resolution::AcceptLocal => local.clone(),
        Resolution::AcceptRemote => remote.clone(),
        Resolution::LastWriteWins => {
            if local.modified_at >= remote.modified_at {
                local.clone()
            } else {
                remote.clone()
            }
        }
    }
}

/// Detect conflicts between a local store and a set of remote requirements.
/// Returns all detected conflicts.
pub fn detect_store_conflicts(
    local_reqs: &[Requirement],
    remote_reqs: &[Requirement],
) -> Vec<RequirementConflict> {
    let mut conflicts = Vec::new();

    for local in local_reqs {
        for remote in remote_reqs {
            if local.id == remote.id {
                if let Some(conflict) = detect_conflict(local, remote) {
                    conflicts.push(conflict);
                }
                break;
            }
        }
    }

    conflicts
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

/// Format a conflict for display.
impl std::fmt::Display for RequirementConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Conflict on {}", self.spec_id)?;
        writeln!(
            f,
            "  Local modified:  {}",
            self.local_modified.format("%Y-%m-%d %H:%M:%S")
        )?;
        writeln!(
            f,
            "  Remote modified: {}",
            self.remote_modified.format("%Y-%m-%d %H:%M:%S")
        )?;
        for field in &self.fields {
            writeln!(f, "  Field: {}", field.field)?;
            writeln!(f, "    Local:  {}", field.local_value)?;
            writeln!(f, "    Remote: {}", field.remote_value)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_req(title: &str, status: &str) -> Requirement {
        let mut req = Requirement::new(title.to_string(), "description".to_string());
        req.set_status_from_str(status);
        req
    }

    #[test]
    fn test_no_conflict_identical() {
        let req = make_req("Title", "Draft");
        assert!(detect_conflict(&req, &req).is_none());
    }

    #[test]
    fn test_no_conflict_same_content_different_time() {
        let mut local = make_req("Title", "Draft");
        let mut remote = local.clone();
        // Same content, different timestamps — no real conflict
        local.modified_at = Utc::now();
        remote.modified_at = Utc::now();
        // modified_at will differ by nanoseconds but content is the same
        assert!(detect_conflict(&local, &remote).is_none());
    }

    #[test]
    fn test_conflict_on_title() {
        let local = make_req("Local Title", "Draft");
        let mut remote = local.clone();
        remote.title = "Remote Title".to_string();
        remote.modified_at = Utc::now();

        let conflict = detect_conflict(&local, &remote);
        assert!(conflict.is_some());

        let c = conflict.unwrap();
        assert_eq!(c.fields.len(), 1);
        assert_eq!(c.fields[0].field, "title");
        assert_eq!(c.fields[0].local_value, "Local Title");
        assert_eq!(c.fields[0].remote_value, "Remote Title");
    }

    #[test]
    fn test_conflict_multiple_fields() {
        let mut local = make_req("Title", "Draft");
        local.owner = "joe".to_string();

        let mut remote = local.clone();
        remote.title = "Changed Title".to_string();
        remote.owner = "alice".to_string();
        remote.set_status_from_str("Approved");
        remote.modified_at = Utc::now();

        let conflict = detect_conflict(&local, &remote).unwrap();
        assert_eq!(conflict.fields.len(), 3); // title, status, owner
    }

    #[test]
    fn test_resolve_accept_local() {
        let local = make_req("Local", "Draft");
        let mut remote = local.clone();
        remote.title = "Remote".to_string();
        remote.modified_at = Utc::now();

        let resolved = resolve_conflict(&local, &remote, Resolution::AcceptLocal);
        assert_eq!(resolved.title, "Local");
    }

    #[test]
    fn test_resolve_accept_remote() {
        let local = make_req("Local", "Draft");
        let mut remote = local.clone();
        remote.title = "Remote".to_string();
        remote.modified_at = Utc::now();

        let resolved = resolve_conflict(&local, &remote, Resolution::AcceptRemote);
        assert_eq!(resolved.title, "Remote");
    }

    #[test]
    fn test_resolve_lww() {
        let mut local = make_req("Local", "Draft");
        let mut remote = local.clone();
        remote.title = "Remote".to_string();

        // Make remote newer
        local.modified_at = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        remote.modified_at = chrono::DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let resolved = resolve_conflict(&local, &remote, Resolution::LastWriteWins);
        assert_eq!(resolved.title, "Remote"); // remote is newer
    }

    #[test]
    fn test_store_conflicts() {
        let req1_local = make_req("Req 1 Local", "Draft");
        let mut req1_remote = req1_local.clone();
        req1_remote.title = "Req 1 Remote".to_string();
        req1_remote.modified_at = Utc::now();

        let req2 = make_req("Req 2", "Draft"); // no conflict

        let conflicts = detect_store_conflicts(&[req1_local, req2.clone()], &[req1_remote, req2]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].fields[0].field, "title");
    }

    #[test]
    fn test_conflict_display() {
        let mut local = make_req("Local", "Draft");
        local.spec_id = Some("FR-1-001".to_string());
        let mut remote = local.clone();
        remote.title = "Remote".to_string();
        remote.modified_at = Utc::now();

        let conflict = detect_conflict(&local, &remote).unwrap();
        let display = format!("{}", conflict);
        assert!(display.contains("FR-1-001"));
        assert!(display.contains("title"));
        assert!(display.contains("Local"));
        assert!(display.contains("Remote"));
    }
}
