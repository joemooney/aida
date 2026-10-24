// trace:BUG-2215 TASK-2326 | ai:codex
use super::*;

fn fixture(status: RequirementStatus) -> (tempfile::TempDir, Storage, Requirement) {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().join("requirements.yaml"));
    let mut req = Requirement::new("claim rollback".into(), "fixture".into());
    req.spec_id = Some("BUG-2215".into());
    req.status = status;
    req.owner = "prior-owner".into();
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req.clone());
    storage.save(&store).unwrap();
    (dir, storage, req)
}

#[test]
fn bug_2215_failed_session_restores_both_status_and_owner() {
    let (dir, storage, req) = fixture(RequirementStatus::Approved);
    let change =
        crate::claim::persist_claim_status_with_receipt(dir.path(), &req, "new-owner").unwrap();
    assert!(change.bumped);
    let mut cleanup = SessionStartCleanup::new(dir.path(), "BUG-2215");
    cleanup.claim_write = Some(change);
    drop(cleanup);
    let actual = storage.load().unwrap().requirements.remove(0);
    assert_eq!(actual.status, RequirementStatus::Approved);
    assert_eq!(actual.owner, "prior-owner");
}

#[test]
fn bug_2215_failed_session_restores_owner_even_without_a_status_bump() {
    // Initial preflight saw Approved; by the atomic write another writer has
    // moved it InProgress. The owner-only write must still get a receipt.
    let (dir, storage, req) = fixture(RequirementStatus::Approved);
    storage
        .update_spec_atomically(&req, |r| r.status = RequirementStatus::InProgress)
        .unwrap();
    let change =
        crate::claim::persist_claim_status_with_receipt(dir.path(), &req, "new-owner").unwrap();
    assert!(!change.bumped);
    assert!(change.changed());
    let mut cleanup = SessionStartCleanup::new(dir.path(), "BUG-2215");
    cleanup.claim_write = Some(change);
    drop(cleanup);
    let actual = storage.load().unwrap().requirements.remove(0);
    assert_eq!(actual.status, RequirementStatus::InProgress);
    assert_eq!(actual.owner, "prior-owner");
}

#[test]
fn bug_2215_failed_session_keeps_a_later_owner_or_status_edit() {
    for change_owner in [false, true] {
        let (dir, storage, req) = fixture(RequirementStatus::Approved);
        let change =
            crate::claim::persist_claim_status_with_receipt(dir.path(), &req, "new-owner").unwrap();
        storage
            .update_spec_atomically(&req, |r| {
                if change_owner {
                    r.owner = "later-owner".into();
                } else {
                    r.status = RequirementStatus::Completed;
                }
                r.modified_at = chrono::Utc::now();
            })
            .unwrap();
        let expected = storage.load().unwrap().requirements.remove(0);
        let mut cleanup = SessionStartCleanup::new(dir.path(), "BUG-2215");
        cleanup.claim_write = Some(change);
        drop(cleanup);
        let actual = storage.load().unwrap().requirements.remove(0);
        assert_eq!(actual.status, expected.status);
        assert_eq!(actual.owner, expected.owner);
        assert_eq!(actual.modified_at, expected.modified_at);
    }
}
