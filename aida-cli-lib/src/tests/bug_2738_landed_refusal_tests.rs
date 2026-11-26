//! BUG-2738: a refusing review verdict must not shelve a spec that already
//! landed (Completed, or its PR merged or closed); a live spec still shelves.
//! trace:BUG-2738 | ai:claude
use super::{landed_refusal, record_refusal_on_landed_spec, shelve_spec_on_failure};
use aida_core::{DatabaseBackend, Requirement, RequirementStatus, RequirementsStore};

/// A git-canonical project at a temp root holding one spec at `status`.
fn project_with(status: RequirementStatus) -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("work");
    std::fs::create_dir_all(work.join(".aida")).unwrap();
    let ok = std::process::Command::new("git")
        .current_dir(&work)
        .args(["init", "-q", "-b", "main"])
        .status()
        .unwrap()
        .success();
    assert!(ok, "git init");
    std::fs::write(
        work.join(".aida").join("config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();
    let mut req = Requirement::new("landed spec".to_string(), String::new());
    req.spec_id = Some("BUG-9738".to_string());
    req.status = status;
    let mut store = RequirementsStore::default();
    store.requirements.push(req);
    crate::test_env::home_fixture_backend(&work.join(".aida-store"))
        .unwrap()
        .save(&store)
        .unwrap();
    (tmp, work)
}

fn reload(work: &std::path::Path) -> Requirement {
    crate::test_env::home_fixture_backend(&work.join(".aida-store"))
        .unwrap()
        .get_requirement_unambiguous("BUG-9738")
        .unwrap()
        .expect("fixture spec")
}

#[test]
fn landed_refusal_covers_completed_and_merged_or_closed_prs_only() {
    use RequirementStatus::*;
    assert!(landed_refusal(&Completed, None).is_some());
    assert!(landed_refusal(&Completed, Some("OPEN")).is_some());
    assert!(landed_refusal(&Done, Some("MERGED")).is_some());
    assert!(landed_refusal(&Done, Some(" merged\n")).is_some());
    assert!(landed_refusal(&InProgress, Some("CLOSED")).is_some());
    for live in [Done, InProgress] {
        assert!(landed_refusal(&live, Some("OPEN")).is_none());
        assert!(landed_refusal(&live, None).is_none());
        assert!(landed_refusal(&live, Some("")).is_none());
    }
}

/// AC1: a CR on a Completed spec leaves it Completed, sets no failure reason
/// and adds a comment naming the writer and the verdict.
#[test]
fn request_changes_on_completed_spec_records_comment_without_shelving() {
    let (_tmp, work) = project_with(RequirementStatus::Completed);
    let reason =
        record_refusal_on_landed_spec(&work, "BUG-9738", None, "reviewer-7", "CHANGES REQUESTED")
            .unwrap();
    assert_eq!(reason, Some("the spec is already Completed"));
    let req = reload(&work);
    assert_eq!(req.status, RequirementStatus::Completed);
    assert!(req.failure_reason.is_none());
    assert!(
        req.comments.iter().any(|c| c.author == "reviewer-7"
            && c.content
                .contains("CHANGES REQUESTED verdict recorded by reviewer-7")
            && c.content.contains("not shelved")),
        "{:?}",
        req.comments
    );
}

/// AC1, the BUG-2728 shape: a Done spec whose PR merged is not reopened.
#[test]
fn request_changes_on_done_spec_with_merged_pr_is_not_shelved() {
    let (_tmp, work) = project_with(RequirementStatus::Done);
    let reason = record_refusal_on_landed_spec(
        &work,
        "BUG-9738",
        Some("MERGED"),
        "rev",
        "CHANGES REQUESTED",
    )
    .unwrap();
    assert_eq!(reason, Some("its PR is already merged"));
    let req = reload(&work);
    assert_eq!(req.status, RequirementStatus::Done);
    assert!(req.failure_reason.is_none());
    assert_eq!(req.comments.len(), 1);
}

/// AC2: a live In Progress spec with an open PR still shelves as before.
#[test]
fn request_changes_on_live_in_progress_spec_still_shelves() {
    let (_tmp, work) = project_with(RequirementStatus::InProgress);
    let skipped =
        record_refusal_on_landed_spec(&work, "BUG-9738", Some("OPEN"), "rev", "CHANGES REQUESTED")
            .unwrap();
    assert!(skipped.is_none());
    assert!(
        reload(&work).comments.is_empty(),
        "no landed note on a live spec"
    );
    let fr = shelve_spec_on_failure(
        &work,
        "BUG-9738",
        "reviewer",
        3,
        "verdict:request-changes",
        "fix it",
        "address the findings",
    )
    .unwrap()
    .expect("a live spec shelves");
    assert_eq!(fr.kind, "verdict:request-changes");
    let req = reload(&work);
    assert_eq!(req.status, RequirementStatus::NeedsAttention);
    assert!(req.comments.iter().any(|c| c.content
        == "Shelved by leases::shelve_spec_on_failure during reviewer: verdict:request-changes"));
}
