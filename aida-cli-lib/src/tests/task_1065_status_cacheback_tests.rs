use super::*;
use aida_core::{DecisionChoice, DecisionRequest, Requirement, RequirementStatus, RequirementType};

fn backend_in(dir: &std::path::Path) -> aida_core::CachedGitBackend {
    let store_root = dir.join("store");
    let cache_path = dir.join(".aida").join("cache.db");
    std::fs::create_dir_all(&store_root).unwrap();
    aida_core::CachedGitBackend::open(&store_root, &cache_path).unwrap()
}

fn pending_dr() -> DecisionRequest {
    DecisionRequest {
        question: "Ship or promote?".into(),
        choices: vec![DecisionChoice {
            label: "Ship".into(),
            consequence: "implement".into(),
            resolution: "status:approved".into(),
        }],
        recommended: Some(0),
        rationale: None,
        answered: None,
        note: None,
        asked_at: None,
        answered_at: None,
    }
}

// Acceptance: `aida status --full` SHALL NOT trigger a full-store load. Build a
// backend, populate it, then DELETE the object YAMLs so a real `backend.load()`
// yields nothing (HEAD is unchanged, so the cache stays fresh and no rebuild
// fires). The cache-projected status store must STILL carry the row — proving
// it is sourced from the cache, not a full load.
#[test]
fn status_store_comes_from_cache_not_full_load() {
    use aida_core::DatabaseBackend;
    let dir = tempfile::tempdir().unwrap();
    let backend = backend_in(dir.path());

    let mut r = Requirement::new("cache-backed row".into(), String::new());
    r.spec_id = Some("TASK-1".into());
    r.status = RequirementStatus::InProgress;
    r.req_type = RequirementType::Task;
    backend.add_requirement(r).unwrap();

    // Nuke the object YAMLs — a real full load now sees an empty object store.
    let objects = dir.path().join("store").join("objects");
    if objects.exists() {
        std::fs::remove_dir_all(&objects).unwrap();
    }
    assert_eq!(
        backend.load().map(|s| s.requirements.len()).unwrap_or(0),
        0,
        "precondition: a full backend.load() must NOT see the row after the objects are removed"
    );

    // The cache-projected status store still carries the row.
    let store = build_status_store_from_cache(&backend).unwrap();
    assert_eq!(store.requirements.len(), 1);
    let req = &store.requirements[0];
    assert_eq!(req.spec_id.as_deref(), Some("TASK-1"));
    assert_eq!(req.status, RequirementStatus::InProgress);
    assert_eq!(req.req_type, RequirementType::Task);
}

// The decision-inbox count reads the `has_pending_decision` cache column and
// matches the store-based reference oracle exactly (non-archived + pending).
#[test]
fn pending_decision_count_reads_cache_column() {
    use aida_core::DatabaseBackend;
    let dir = tempfile::tempdir().unwrap();
    let backend = backend_in(dir.path());
    assert_eq!(backend.pending_decision_count().unwrap(), 0);

    let mut r = Requirement::new("needs a call".into(), String::new());
    r.spec_id = Some("TASK-2".into());
    r.decision_request = Some(pending_dr());
    backend.add_requirement(r).unwrap();

    // Answered decisions do not count.
    let mut answered = Requirement::new("already decided".into(), String::new());
    answered.spec_id = Some("TASK-4".into());
    let mut dr = pending_dr();
    dr.answered = Some(0);
    answered.decision_request = Some(dr);
    backend.add_requirement(answered).unwrap();

    // Archived pending is excluded (matches the old store predicate).
    let mut a = Requirement::new("archived pending".into(), String::new());
    a.spec_id = Some("TASK-3".into());
    a.decision_request = Some(pending_dr());
    a.archived = true;
    backend.add_requirement(a).unwrap();

    assert_eq!(backend.pending_decision_count().unwrap(), 1);
    // Cache-column count equals the store-based reference oracle.
    let loaded = backend.load().unwrap();
    assert_eq!(
        backend.pending_decision_count().unwrap(),
        pending_decision_request_count(&loaded),
    );
}
