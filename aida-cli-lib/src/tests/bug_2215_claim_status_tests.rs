// trace:BUG-2215 | ai:codex
use super::*;

fn requirement(status: RequirementStatus) -> Requirement {
    let mut req = Requirement::new("Claim status".into(), "Acceptance fixture".into());
    req.status = status;
    req
}

#[test]
fn bug_2215_claim_approved_records_claimant_and_progress() {
    let mut req = requirement(RequirementStatus::Approved);
    assert!(claim_status(&mut req, "claimant@example.com"));
    assert_eq!(req.status, RequirementStatus::InProgress);
    assert_eq!(req.owner, "claimant@example.com");
    assert!(!claim_status(&mut req, "claimant@example.com"));
}

#[test]
fn bug_2215_claim_never_promotes_draft_or_terminal_work() {
    for status in [
        RequirementStatus::Draft,
        RequirementStatus::Done,
        RequirementStatus::Completed,
        RequirementStatus::Rejected,
    ] {
        let mut req = requirement(status.clone());
        assert!(!claim_status(&mut req, "claimant"));
        assert_eq!(req.status, status);
        assert!(req.owner.is_empty());
    }
}

#[test]
fn bug_2215_unclaim_without_pr_returns_approved() {
    let mut req = requirement(RequirementStatus::InProgress);
    unclaim_status(&mut req, &PrLookup::NoOpenPr);
    assert_eq!(req.status, RequirementStatus::Approved);
}

#[test]
fn bug_2215_open_pr_or_unknown_preserves_progress() {
    for lookup in [
        PrLookup::Found(OpenPrInfo {
            number: 42,
            title: "work".into(),
            url: "url".into(),
            head_branch: None,
        }),
        PrLookup::GhMissing,
        PrLookup::GhFailed("auth".into()),
        PrLookup::GhUnreachable("offline".into()),
    ] {
        let mut req = requirement(RequirementStatus::InProgress);
        unclaim_status(&mut req, &lookup);
        assert_eq!(req.status, RequirementStatus::InProgress);
    }
}

#[test]
fn bug_2215_unclaim_cannot_change_merge_completion_or_draft() {
    for status in [
        RequirementStatus::Draft,
        RequirementStatus::Done,
        RequirementStatus::Completed,
        RequirementStatus::Rejected,
    ] {
        let mut req = requirement(status.clone());
        unclaim_status(&mut req, &PrLookup::NoOpenPr);
        assert_eq!(req.status, status);
    }
}

#[test]
fn bug_2215_atomic_claim_rechecks_live_status_and_preserves_other_specs() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path().join("requirements.yaml"));
    let target = requirement(RequirementStatus::Approved);
    let other = requirement(RequirementStatus::Draft);
    let mut store = aida_core::RequirementsStore::default();
    store.requirements = vec![target.clone(), other.clone()];
    storage.save(&store).unwrap();
    storage
        .update_spec_atomically(&target, |r| {
            claim_status(r, "claimant");
        })
        .unwrap();
    // Exercise the project-scoped legacy resolver, not only Storage directly.
    let legacy = claim_storage(dir.path()).unwrap();
    assert_eq!(legacy.load().unwrap().requirements.len(), 2);
    let loaded = storage.load().unwrap();
    assert_eq!(loaded.requirements[0].status, RequirementStatus::InProgress);
    assert_eq!(loaded.requirements[0].owner, "claimant");
    assert_eq!(loaded.requirements[1].id, other.id);
    assert_eq!(loaded.requirements[1].status, RequirementStatus::Draft);
    storage
        .update_spec_atomically(&target, |r| r.status = RequirementStatus::Completed)
        .unwrap();
    storage
        .update_spec_atomically(&target, |r| {
            claim_status(r, "late claimant");
        })
        .unwrap();
    assert_eq!(
        storage.load().unwrap().requirements[0].status,
        RequirementStatus::Completed
    );
}

#[test]
fn bug_2215_status_failure_rolls_back_only_our_new_claim() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("claim.toml");
    for (refreshed, actual, retained) in [
        (false, b"ours".as_slice(), false),
        (true, b"ours".as_slice(), true),
        (false, b"changed".as_slice(), true),
    ] {
        std::fs::write(&path, actual).unwrap();
        let result = finish_claim_write(
            &path,
            b"ours",
            refreshed,
            Err(anyhow::anyhow!("store write refused")),
        );
        assert!(result.is_err());
        assert_eq!(path.exists(), retained);
    }
}

#[test]
fn bug_2215_real_claim_and_unclaim_legacy_project_round_trip() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let git = crate::test_git::git_command()
        .args(["init", "-q", "-b", "main"])
        .arg(dir.path())
        .status()
        .unwrap();
    assert!(git.success());
    let mut req = requirement(RequirementStatus::Approved);
    req.spec_id = Some("BUG-1".into());
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req.clone());
    let storage = Storage::new(dir.path().join("requirements.yaml"));
    storage.save(&store).unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    handle_claim("BUG-1", None).unwrap();
    let claimed = storage.load().unwrap();
    assert_eq!(
        claimed.requirements[0].status,
        RequirementStatus::InProgress
    );
    assert!(!claimed.requirements[0].owner.is_empty());
    assert_eq!(list_leases(dir.path()).len(), 1);
    handle_unclaim("BUG-1").unwrap();
    assert_eq!(
        storage.load().unwrap().requirements[0].status,
        RequirementStatus::Approved
    );
    assert!(list_leases(dir.path()).is_empty());
    storage
        .update_spec_atomically(&req, |r| r.status = RequirementStatus::Draft)
        .unwrap();
    handle_claim("BUG-1", None).unwrap();
    assert_eq!(
        storage.load().unwrap().requirements[0].status,
        RequirementStatus::Draft
    );
    handle_unclaim("BUG-1").unwrap();
    std::env::set_current_dir(original).unwrap();
}
