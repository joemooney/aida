//! Stable terminal dates and the explicit, idempotent completion backfill.
// trace:BUG-2597 | ai:codex
use crate::models::{Requirement, RequirementStatus, RequirementsStore};
use chrono::{DateTime, Utc};

/// History is not necessarily ordered after a store merge; compare timestamps.
pub fn status_date(req: &Requirement, status: &str) -> Option<DateTime<Utc>> {
    req.history
        .iter()
        .filter(|entry| {
            entry.changes.iter().any(|change| {
                change.field_name == "status"
                    && change.new_value.trim().eq_ignore_ascii_case(status)
            })
        })
        .map(|entry| entry.timestamp)
        .max()
}

pub fn completed_at(req: &Requirement) -> Option<DateTime<Utc>> {
    req.implementation_info
        .as_ref()
        .and_then(|info| info.completed_at)
        .or_else(|| {
            (req.status == RequirementStatus::Completed)
                .then(|| status_date(req, "Completed"))
                .flatten()
        })
}

/// Closure dates for Rejected/Superseded are derived, never guessed from edits.
pub fn closed_at(req: &Requirement) -> Option<DateTime<Utc>> {
    match req.status {
        RequirementStatus::Rejected => status_date(req, "Rejected"),
        RequirementStatus::Superseded => status_date(req, "Superseded"),
        _ => None,
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct BackfillReport {
    pub updated: usize,
    pub unresolved: Vec<String>,
}

/// Run inside the caller's atomic store update, or on a read-only snapshot for
/// dry-run. Never change modified_at or invent status history for a data repair.
pub fn backfill(store: &mut RequirementsStore) -> BackfillReport {
    let mut report = BackfillReport::default();
    let derived: std::collections::HashSet<_> = store
        .requirements
        .iter()
        .filter(|req| req.req_type == crate::models::RequirementType::Epic)
        .filter(|req| {
            crate::rollup::derive_epic_status(store, req.id) == Some(RequirementStatus::Completed)
        })
        .map(|req| req.id)
        .collect();
    for req in &mut store.requirements {
        if req.status != RequirementStatus::Completed {
            if derived.contains(&req.id) && completed_at(req).is_none() {
                report.unresolved.push(format!(
                    "{}: derived Completed rollup has no persisted completion event",
                    req.spec_id.as_deref().unwrap_or(&req.title)
                ));
            }
            continue;
        }
        if req
            .implementation_info
            .as_ref()
            .and_then(|i| i.completed_at)
            .is_some()
        {
            continue;
        }
        if let Some(at) = status_date(req, "Completed") {
            req.implementation_info
                .get_or_insert_with(Default::default)
                .completed_at = Some(at);
            report.updated += 1;
        } else {
            report.unresolved.push(format!(
                "{}: no Completed status-history entry",
                req.spec_id.as_deref().unwrap_or(&req.title)
            ));
        }
    }
    report.unresolved.sort();
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::HistoryEntry;

    fn historical() -> Requirement {
        let mut req = Requirement::new("old completion".into(), "".into());
        req.status = RequirementStatus::Completed;
        for date in ["2026-02-01T00:00:00Z", "2026-01-01T00:00:00Z"] {
            let mut entry = HistoryEntry::new(
                "operator".into(),
                vec![Requirement::field_change(
                    "status",
                    "Done".into(),
                    "Completed".into(),
                )],
            );
            entry.timestamp = date.parse().unwrap();
            req.history.push(entry);
        }
        req
    }

    #[test]
    fn bug2597_reports_derived_completed_epics_without_inventing_dates() {
        use crate::models::{Relationship, RelationshipType, RequirementType};
        let mut epic = Requirement::new("rollup".into(), String::new());
        epic.req_type = RequirementType::Epic;
        epic.spec_id = Some("EPIC-2597".into());
        let mut child = historical();
        child.relationships.push(Relationship {
            rel_type: RelationshipType::Parent,
            target_id: epic.id,
            created_at: None,
            created_by: None,
        });
        let mut store = RequirementsStore::new();
        store.requirements = vec![epic, child];
        let report = backfill(&mut store);
        assert_eq!(report.updated, 1);
        assert_eq!(
            report.unresolved,
            vec!["EPIC-2597: derived Completed rollup has no persisted completion event"]
        );
        assert!(store.requirements[0].implementation_info.is_none());
        assert_eq!(backfill(&mut store).updated, 0);
    }

    #[test]
    fn bug2597_backfill_git_objects_is_idempotent() {
        use crate::db::DatabaseBackend;
        let dir = tempfile::tempdir().unwrap();
        let backend = crate::allocation_authority::home_fixture_backend(dir.path())
            .unwrap()
            .with_auto_commit(false);
        let mut store = RequirementsStore::new();
        let mut req = historical();
        req.spec_id = Some("TASK-2597".into());
        store.requirements.push(req);
        backend.save_imported(&store).unwrap();
        let path =
            crate::object_store::object_path(&dir.path().join("objects"), "TASK-2597").unwrap();
        let before = std::fs::read(&path).unwrap();
        assert_eq!(backfill(&mut backend.load().unwrap()).updated, 1);
        assert!(
            std::fs::read(&path).unwrap() == before,
            "dry-run changed a canonical object"
        );
        backend
            .update_atomically(|store| {
                assert_eq!(backfill(store).updated, 1);
            })
            .unwrap();
        let repaired = crate::object_store::read_object_from_path(&path).unwrap();
        assert_eq!(
            repaired.implementation_info.as_ref().unwrap().completed_at,
            status_date(&repaired, "Completed")
        );
        let once = std::fs::read(&path).unwrap();
        backend
            .update_atomically(|store| {
                assert_eq!(backfill(store).updated, 0);
            })
            .unwrap();
        assert!(
            std::fs::read(&path).unwrap() == once,
            "second backfill changed the object"
        );
    }

    #[test]
    fn bug2597_backfill_persists_fixture_store_and_dry_run_does_not_write() {
        use crate::db::{DatabaseBackend, YamlBackend};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("requirements.yaml");
        let backend = YamlBackend::new(&path);
        let mut store = RequirementsStore::new();
        store.requirements.push(historical());
        backend.save_imported(&store).unwrap();
        // Settle legacy load-time ID/type migrations before measuring dry-run.
        backend.load().unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        assert_eq!(backfill(&mut backend.load().unwrap()).updated, 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        backend
            .update_atomically(|store| {
                assert_eq!(backfill(store).updated, 1);
            })
            .unwrap();
        let persisted = backend.load().unwrap();
        assert_eq!(
            persisted.requirements[0]
                .implementation_info
                .as_ref()
                .unwrap()
                .completed_at,
            status_date(&persisted.requirements[0], "Completed")
        );
        backend
            .update_atomically(|store| {
                assert_eq!(backfill(store).updated, 0);
            })
            .unwrap();
    }

    #[test]
    fn bug2597_history_uses_latest_date_and_backfill_is_idempotent() {
        let req = historical();
        let expected = "2026-02-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(completed_at(&req), Some(expected));
        let mut store = RequirementsStore::new();
        store.requirements.push(req);
        let mut missing = Requirement::new("missing history".into(), "".into());
        missing.status = RequirementStatus::Completed;
        store.requirements.push(missing);
        let modified = store.requirements[0].modified_at;
        let report = backfill(&mut store);
        assert_eq!(report.updated, 1);
        assert_eq!(report.unresolved.len(), 1);
        assert_eq!(store.requirements[0].modified_at, modified);
        assert_eq!(backfill(&mut store).updated, 0);
        assert_eq!(completed_at(&store.requirements[0]), Some(expected));
    }

    #[test]
    fn bug2597_setter_and_direct_writes_stamp_new_completion() {
        let mut req = historical();
        req.set_status_from_str("in-progress");
        req.set_status_from_str("completed");
        let stamp = completed_at(&req).unwrap();
        assert!(stamp > status_date(&req, "Completed").unwrap());
        req.set_status_from_str("completed");
        assert_eq!(completed_at(&req), Some(stamp));
        let mut old = req.clone();
        old.status = RequirementStatus::Done;
        req.implementation_info = None;
        crate::provenance::stamp_write(&mut req, Some(&old), &Default::default()).unwrap();
        assert!(req.implementation_info.unwrap().completed_at.is_some());
    }

    #[test]
    fn bug2597_rejected_and_superseded_dates_use_matching_history() {
        let mut req = historical();
        for status in [RequirementStatus::Rejected, RequirementStatus::Superseded] {
            req.status = status.clone();
            let entry = HistoryEntry::new(
                "operator".into(),
                vec![Requirement::field_change(
                    "status",
                    "Draft".into(),
                    status.to_string(),
                )],
            );
            let at = entry.timestamp;
            req.history.push(entry);
            assert_eq!(closed_at(&req), Some(at));
        }
    }
}
