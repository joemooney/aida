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

/// Three-way structured merge of one spec YAML during a store-leg rebase.
///
/// MU-204 / STORY-641: when two clones concurrently edit the SAME spec, the
/// store-leg rebase hits a textual conflict in `objects/TYPE/000/SPEC.yaml`
/// even though the spec is structurally reconcilable: the `history:`,
/// `comments:`, and `processing_record:` arrays are append-only (each entry
/// carries an immutable `id`), and the scalar fields can be resolved with the
/// same last-write-wins policy `resolve_conflict` already uses.
///
/// This is a PURE function so it is unit-testable in isolation; the git
/// plumbing (reading the three stages, writing the result, continuing the
/// rebase) lives in the pull path.
///
/// Policy, matching `conflict.rs::resolve_conflict` (LastWriteWins by
/// `modified_at`) and never silently dropping an edit:
/// - **Scalar fields** (title, description, status, priority, owner, …):
///   take the whole `ours`/`theirs` requirement that has the later
///   `modified_at` as the scalar base (LWW). Ties → `ours`.
/// - **`history:`** — union by `HistoryEntry.id` (dedupe), ordered by
///   `timestamp` then `id` for determinism. Both clones' entries survive.
/// - **`comments:`** — union by `Comment.id`, ordered by `created_at` then
///   `id`. (`base` may contain comments removed on neither side; the union of
///   ours+theirs preserves everything either side has.)
/// - **`processing_record:`** — union by `ProcessingRecord.id`, ordered by
///   `timestamp` then `id`.
/// - **`tags:`** — union of both sides (a `HashSet`, so a set union).
///
/// `base` is the merge-base version; it is currently used only to anchor the
/// id-keyed unions implicitly (ours+theirs is a superset of base for
/// append-only arrays). It is accepted so the signature is a true three-way
/// merge and so future field-level policies can diff against the base.
pub fn merge_spec_three_way(
    base: &Requirement,
    ours: &Requirement,
    theirs: &Requirement,
) -> Requirement {
    // Scalar base: last-write-wins by modified_at (ties → ours). This carries
    // title / description / status / priority / owner / feature / weight /
    // relationships / dependencies / archived / etc. from whichever side wrote
    // most recently — the same rule resolve_conflict applies.
    let mut merged = if ours.modified_at >= theirs.modified_at {
        ours.clone()
    } else {
        theirs.clone()
    };

    // History: union by entry id, deterministic order. base contributes
    // nothing new (append-only ⇒ ours+theirs ⊇ base) but we fold it in too so
    // an entry present only in base (e.g. one side rewrote the array) is never
    // dropped.
    merged.history = union_history(&[&base.history, &ours.history, &theirs.history]);

    // Comments: union by comment id, deterministic order.
    merged.comments = union_comments(&[&base.comments, &ours.comments, &theirs.comments]);

    // Processing records: union by record id, deterministic order.
    merged.processing_record = union_processing_records(&[
        &base.processing_record,
        &ours.processing_record,
        &theirs.processing_record,
    ]);

    // Tags: set union across both edited sides.
    let mut tags = ours.tags.clone();
    tags.extend(theirs.tags.iter().cloned());
    merged.tags = tags;

    merged
}

/// Union HistoryEntry arrays by `id`, ordered by `(timestamp, id)`.
fn union_history(
    sources: &[&Vec<crate::models::HistoryEntry>],
) -> Vec<crate::models::HistoryEntry> {
    use std::collections::HashMap;
    let mut by_id: HashMap<Uuid, crate::models::HistoryEntry> = HashMap::new();
    for src in sources {
        for entry in src.iter() {
            by_id.entry(entry.id).or_insert_with(|| entry.clone());
        }
    }
    let mut out: Vec<_> = by_id.into_values().collect();
    out.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then(a.id.cmp(&b.id)));
    out
}

/// Union Comment arrays by `id`, ordered by `(created_at, id)`.
fn union_comments(sources: &[&Vec<crate::models::Comment>]) -> Vec<crate::models::Comment> {
    use std::collections::HashMap;
    let mut by_id: HashMap<Uuid, crate::models::Comment> = HashMap::new();
    for src in sources {
        for c in src.iter() {
            by_id.entry(c.id).or_insert_with(|| c.clone());
        }
    }
    let mut out: Vec<_> = by_id.into_values().collect();
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    out
}

/// Union ProcessingRecord arrays by `id`, ordered by `(timestamp, id)`.
fn union_processing_records(
    sources: &[&Vec<crate::models::ProcessingRecord>],
) -> Vec<crate::models::ProcessingRecord> {
    use std::collections::HashMap;
    let mut by_id: HashMap<Uuid, crate::models::ProcessingRecord> = HashMap::new();
    for src in sources {
        for r in src.iter() {
            by_id.entry(r.id).or_insert_with(|| r.clone());
        }
    }
    let mut out: Vec<_> = by_id.into_values().collect();
    out.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then(a.id.cmp(&b.id)));
    out
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

// trace:BUG-475
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}...")
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

    // trace:BUG-475
    #[test]
    fn test_truncate_ascii_truncates() {
        let s = "a".repeat(150);
        let out = truncate(&s, 100);
        assert_eq!(out, format!("{}...", "a".repeat(100)));
    }

    // trace:BUG-475
    #[test]
    fn test_truncate_short_unchanged() {
        assert_eq!(truncate("short", 100), "short");
    }

    // trace:BUG-475 — multi-byte char straddling the byte cutoff must not panic.
    #[test]
    fn test_truncate_multibyte_near_boundary_no_panic() {
        // 99 ASCII bytes + a 2-byte 'é' => char 100 straddles byte index 100.
        let s = "a".repeat(99) + "é";
        // 100 chars total, so it is not truncated, and must not panic on the byte slice.
        let out = truncate(&s, 100);
        assert_eq!(out, s);

        // Now force truncation right at the multi-byte char (101 chars, max 100).
        let s2 = "a".repeat(100) + "é";
        let out2 = truncate(&s2, 100);
        assert_eq!(out2, format!("{}...", "a".repeat(100)));
    }

    // ----- STORY-641: three-way structured merge (MU-204) -----

    use crate::models::{Comment, FieldChange, HistoryEntry};

    fn hist(author: &str, ts: &str) -> HistoryEntry {
        HistoryEntry {
            id: Uuid::now_v7(),
            author: author.to_string(),
            timestamp: DateTime::parse_from_rfc3339(ts)
                .unwrap()
                .with_timezone(&Utc),
            changes: vec![FieldChange {
                field_name: "status".to_string(),
                old_value: "draft".to_string(),
                new_value: "approved".to_string(),
            }],
        }
    }

    // trace:STORY-641 — concurrent same-spec history appends union by id.
    #[test]
    fn test_merge_unions_history_by_id_dedupe() {
        let mut base = make_req("Title", "Draft");
        let shared = hist("base", "2026-01-01T00:00:00Z");
        base.history = vec![shared.clone()];

        let mut ours = base.clone();
        let ours_entry = hist("a", "2026-01-02T00:00:00Z");
        ours.history = vec![shared.clone(), ours_entry.clone()];
        ours.set_status_from_str("Approved");
        ours.modified_at = DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let mut theirs = base.clone();
        let theirs_entry = hist("b", "2026-01-03T00:00:00Z");
        theirs.history = vec![shared.clone(), theirs_entry.clone()];
        theirs.set_status_from_str("In Progress");
        theirs.modified_at = DateTime::parse_from_rfc3339("2026-01-03T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let merged = merge_spec_three_way(&base, &ours, &theirs);

        // All three unique entries present, deduped (shared appears once).
        assert_eq!(merged.history.len(), 3);
        let ids: std::collections::HashSet<_> = merged.history.iter().map(|h| h.id).collect();
        assert!(ids.contains(&shared.id));
        assert!(ids.contains(&ours_entry.id));
        assert!(ids.contains(&theirs_entry.id));
        // Ordered by timestamp.
        assert_eq!(merged.history[0].id, shared.id);
        assert_eq!(merged.history[1].id, ours_entry.id);
        assert_eq!(merged.history[2].id, theirs_entry.id);
    }

    // trace:STORY-641 — scalar fields resolve last-write-wins by modified_at.
    #[test]
    fn test_merge_scalar_lww_by_modified_at() {
        let base = make_req("Title", "Draft");

        let mut ours = base.clone();
        ours.set_status_from_str("Approved");
        ours.modified_at = DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let mut theirs = base.clone();
        theirs.set_status_from_str("In Progress");
        theirs.modified_at = DateTime::parse_from_rfc3339("2026-01-03T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        // theirs is newer → its status wins.
        let merged = merge_spec_three_way(&base, &ours, &theirs);
        assert_eq!(merged.effective_status(), "In Progress");

        // Flip: ours newer → ours status wins.
        let mut ours2 = ours.clone();
        ours2.modified_at = DateTime::parse_from_rfc3339("2026-01-04T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let merged2 = merge_spec_three_way(&base, &ours2, &theirs);
        assert_eq!(merged2.effective_status(), "Approved");
    }

    // trace:STORY-641 — tags union across both sides.
    #[test]
    fn test_merge_tags_union() {
        let mut base = make_req("Title", "Draft");
        base.tags.insert("shared".to_string());

        let mut ours = base.clone();
        ours.tags.insert("ours-only".to_string());
        ours.modified_at = Utc::now();

        let mut theirs = base.clone();
        theirs.tags.insert("theirs-only".to_string());
        theirs.modified_at = Utc::now();

        let merged = merge_spec_three_way(&base, &ours, &theirs);
        assert!(merged.tags.contains("shared"));
        assert!(merged.tags.contains("ours-only"));
        assert!(merged.tags.contains("theirs-only"));
        assert_eq!(merged.tags.len(), 3);
    }

    // trace:STORY-641 — identical-on-both-sides merge is a no-op.
    #[test]
    fn test_merge_identical_is_noop() {
        let mut base = make_req("Title", "Draft");
        base.history = vec![hist("base", "2026-01-01T00:00:00Z")];
        base.tags.insert("t".to_string());
        let ours = base.clone();
        let theirs = base.clone();

        let merged = merge_spec_three_way(&base, &ours, &theirs);
        assert_eq!(merged.title, base.title);
        assert_eq!(merged.effective_status(), base.effective_status());
        assert_eq!(merged.history.len(), 1);
        assert_eq!(merged.history[0].id, base.history[0].id);
        assert_eq!(merged.tags, base.tags);
    }

    // trace:STORY-641 — comments union by id (append-only thread).
    #[test]
    fn test_merge_unions_comments_by_id() {
        let base = make_req("Title", "Draft");

        let mut ours = base.clone();
        let c_ours = Comment::new("a".to_string(), "ours comment".to_string());
        ours.comments = vec![c_ours.clone()];
        ours.modified_at = Utc::now();

        let mut theirs = base.clone();
        let c_theirs = Comment::new("b".to_string(), "theirs comment".to_string());
        theirs.comments = vec![c_theirs.clone()];
        theirs.modified_at = Utc::now();

        let merged = merge_spec_three_way(&base, &ours, &theirs);
        assert_eq!(merged.comments.len(), 2);
        let ids: std::collections::HashSet<_> = merged.comments.iter().map(|c| c.id).collect();
        assert!(ids.contains(&c_ours.id));
        assert!(ids.contains(&c_theirs.id));
    }

    // trace:BUG-475 — emoji (4-byte) at the cutoff truncates on a char boundary.
    #[test]
    fn test_truncate_emoji_at_boundary() {
        let s = "a".repeat(99) + "😀" + &"b".repeat(10);
        let out = truncate(&s, 100);
        let expected: String = s.chars().take(100).collect();
        assert_eq!(out, format!("{expected}..."));
        // Sanity: the emoji survived intact (no mid-char slice).
        assert!(out.contains('😀'));
    }
}
