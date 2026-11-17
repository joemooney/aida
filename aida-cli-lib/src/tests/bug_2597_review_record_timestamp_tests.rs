//! BUG-2597 AC1, review-record path: `aida review record` writes only the
//! verdict artefact. Recording a verdict on a Completed or Rejected spec must
//! leave its terminal dates exactly as they were: the stored completion date,
//! and the closure date derived from the terminal status-history entry.
// trace:BUG-2597 | ai:claude

use aida_core::models::RequirementsStore;
use aida_core::terminal_timestamps::{closed_at, completed_at};
use aida_core::{Requirement, RequirementStatus, RequirementType};
use chrono::{DateTime, TimeZone, Utc};

const COMPLETED: &str = "TASK-25972";
const REJECTED: &str = "TASK-25973";
const BRANCH: &str = "task-25972-work";

/// A fixed past instant, so an accidental re-stamp to "now" is detectable.
fn past() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap()
}

/// A spec moved into `to` through the shared history helper, its history
/// back-dated to [`past`].
fn terminal(spec: &str, to: RequirementStatus) -> Requirement {
    let mut req = Requirement::new(format!("{spec} terminal"), String::new());
    req.spec_id = Some(spec.into());
    req.req_type = RequirementType::Task;
    req.status = RequirementStatus::InProgress;
    aida_core::conflict::set_status_recorded(&mut req, to, "test");
    for entry in &mut req.history {
        entry.timestamp = past();
    }
    if req.status == RequirementStatus::Completed {
        req.implementation_info
            .get_or_insert_with(Default::default)
            .completed_at = Some(past());
    }
    req
}

/// A git project with one commit and a legacy root store holding one
/// Completed and one Rejected spec; returns it and the head sha.
fn project() -> (tempfile::TempDir, String) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".aida")).unwrap();
    let mut store = RequirementsStore::new();
    store.requirements = vec![
        terminal(COMPLETED, RequirementStatus::Completed),
        terminal(REJECTED, RequirementStatus::Rejected),
    ];
    crate::Storage::new(root.path().join("requirements.yaml"))
        .save(&store)
        .unwrap();
    let head = crate::test_git::commit_fixture(root.path());
    (root, head)
}

fn load(root: &std::path::Path, spec: &str) -> Requirement {
    crate::Storage::new(root.join("requirements.yaml"))
        .load()
        .unwrap()
        .get_requirement_by_spec_id(spec)
        .unwrap()
        .clone()
}

/// `aida review record <spec> --verdict <verdict>` from an independent
/// reviewer, every ambient root pinned to `root`.
fn record(root: &std::path::Path, head: &str, spec: &str, verdict: &str) -> anyhow::Result<()> {
    let root_str = root.to_str().unwrap();
    let _pinned = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_PROJECT_ROOT", Some(root_str)),
        ("AIDA_DRIVE_ROOT", Some(root_str)),
        ("AIDA_REVIEW_VERDICT_FILE", None),
        ("AIDA_FROM_PR_HEAD_SHA", None),
        (crate::gate::seat_authority::GRANT_ENV, None),
    ]);
    let subject = crate::ReviewPrSubject {
        head_ref: BRANCH.into(),
        commit_subjects: vec![],
    };
    let reviewer = crate::gate::review_authority::IdentitySource {
        session_ids: vec!["independent-reviewer".into()],
        ..Default::default()
    };
    crate::with_review_pr_subject(subject, || {
        crate::gate::review_authority::with_identity_source(reviewer, || {
            crate::handle_review_record_fixture_at(
                root.to_path_buf(),
                spec,
                verdict,
                Some(head),
                Some(BRANCH),
                Some("terminal-date regression"),
                &[],
                &[],
                None,
            )
        })
    })
}

#[test]
fn bug2597_review_record_preserves_terminal_status_timestamps() {
    let (root, head) = project();
    let store_bytes = || std::fs::read(root.path().join("requirements.yaml")).unwrap();
    let completed_before = load(root.path(), COMPLETED);
    let rejected_before = load(root.path(), REJECTED);
    assert_eq!(completed_at(&completed_before), Some(past()));
    assert_eq!(closed_at(&rejected_before), Some(past()));
    let bytes_before = store_bytes();

    record(root.path(), &head, COMPLETED, "approved").expect("records on a Completed spec");
    record(root.path(), &head, REJECTED, "approved").expect("records on a Rejected spec");
    assert!(
        crate::gate::review_verdict::read_recorded_verdict(root.path(), COMPLETED).is_some(),
        "the verdict itself is written"
    );

    let completed_after = load(root.path(), COMPLETED);
    let rejected_after = load(root.path(), REJECTED);
    assert_eq!(completed_after.status, RequirementStatus::Completed);
    assert_eq!(rejected_after.status, RequirementStatus::Rejected);
    assert_eq!(
        completed_after
            .implementation_info
            .as_ref()
            .and_then(|i| i.completed_at),
        Some(past()),
        "the stored completion date is preserved"
    );
    assert_eq!(completed_at(&completed_after), Some(past()));
    assert_eq!(closed_at(&rejected_after), Some(past()));
    assert_eq!(
        store_bytes(),
        bytes_before,
        "review record must not rewrite the spec store"
    );
}
