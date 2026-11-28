//! BUG-2738: a refusing review verdict must not shelve a spec that already
//! landed (Completed, or its PR merged); a live spec still shelves. Driven
//! end to end through `aida review record`.
//! trace:BUG-2738 | ai:claude
use super::landed_refusal;
use aida_core::{DatabaseBackend, Requirement, RequirementStatus};

const SPEC: &str = "BUG-9738";
const PR: u64 = 9738;

/// A project at a temp root holding [`SPEC`] at `status`, with a GitHub
/// origin and a fake `gh` answering every PR read with `pr_state`.
struct Fixture {
    root: tempfile::TempDir,
    head: String,
    _env: crate::test_env::EnvVarsGuard,
}

impl Fixture {
    fn new(status: RequirementStatus, pr_state: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join(".aida-store");
        std::fs::create_dir_all(root.path().join(".aida")).unwrap();
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(
            root.path().join(".aida/config.toml"),
            "mode = \"distributed\"\nstore_path = \".aida-store\"\n",
        )
        .unwrap();
        let mut req = Requirement::new("landed spec".into(), "desc".into());
        req.spec_id = Some(SPEC.into());
        req.status = status;
        backend(root.path()).add_requirement(req).unwrap();
        let head = crate::test_git::commit_fixture(root.path());
        let ok = crate::test_git::git_command()
            .arg("-C")
            .arg(root.path())
            .args([
                "remote",
                "add",
                "origin",
                "https://github.com/test/repo.git",
            ])
            .status()
            .unwrap()
            .success();
        assert!(ok, "git remote add");
        let gh = root.path().join("gh");
        crate::test_exec::write_executable(
            &gh,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' '{{\"state\":\"{pr_state}\",\"headRefOid\":\"{head}\"}}'\n"
            ),
        );
        let root_str = root.path().to_str().unwrap().to_string();
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_PROJECT_ROOT", Some(&root_str)),
            ("AIDA_DRIVE_ROOT", Some(&root_str)),
            ("AIDA_REVIEW_VERDICT_FILE", None),
            ("AIDA_TEST_GH_BINARY", Some(gh.to_str().unwrap())),
        ]);
        Fixture { root, head, _env }
    }

    fn request_changes(&self) {
        crate::handle_review_record_fixture_at(
            self.root.path().to_path_buf(),
            SPEC,
            "request-changes",
            Some(&self.head),
            Some("bug-9738-work"),
            Some("the regression is still open"),
            &["fix the regression".to_string()],
            &[],
            Some(PR),
        )
        .expect("recording a refusing verdict succeeds");
        let recorded =
            crate::gate::review_verdict::read_recorded_verdict(self.root.path(), "PR-9738")
                .expect("the verdict is in the gate ledger");
        assert!(recorded.kind.blocks_done());
        assert_eq!(recorded.reviewed_sha.as_deref(), Some(self.head.as_str()));
    }

    fn spec(&self) -> Requirement {
        backend(self.root.path())
            .get_requirement_by_spec_id(SPEC)
            .unwrap()
            .expect("fixture spec")
    }
}

fn backend(root: &std::path::Path) -> aida_core::CachedGitBackend {
    let store = root.join(".aida-store");
    crate::test_env::home_fixture_cached_backend(
        &store,
        &aida_core::CachedGitBackend::default_cache_path(&store),
    )
    .unwrap()
}

fn assert_landed_note(req: &Requirement, status: RequirementStatus) {
    assert_eq!(req.status, status);
    assert!(req.failure_reason.is_none(), "{:?}", req.failure_reason);
    assert!(
        req.comments.iter().any(
            |c| c.content.contains("CHANGES REQUESTED verdict recorded by")
                && c.content.contains(&format!("recorded by {}", c.author))
                && c.content.contains("not shelved")
        ),
        "{:?}",
        req.comments
    );
}

#[test]
fn landed_refusal_covers_completed_and_merged_prs_only() {
    use RequirementStatus::*;
    assert!(landed_refusal(&Completed, false).is_some());
    assert!(landed_refusal(&Done, true).is_some());
    assert!(landed_refusal(&InProgress, true).is_some());
    for live in [Done, InProgress, NeedsAttention] {
        assert!(landed_refusal(&live, false).is_none());
    }
}

// trace:BUG-2738.AC1 | ai:claude
#[test]
fn bug_2738_ac1_request_changes_on_completed_spec_is_noted_not_shelved() {
    let f = Fixture::new(RequirementStatus::Completed, "OPEN");
    f.request_changes();
    assert_landed_note(&f.spec(), RequirementStatus::Completed);
}

// trace:BUG-2738.AC1 | ai:claude
#[test]
fn bug_2738_ac1_request_changes_on_done_spec_with_merged_pr_is_noted_not_shelved() {
    let f = Fixture::new(RequirementStatus::Done, "MERGED");
    f.request_changes();
    assert_landed_note(&f.spec(), RequirementStatus::Done);
}

// trace:BUG-2738.AC2 | ai:claude
#[test]
fn bug_2738_ac2_request_changes_on_live_spec_still_shelves() {
    for (status, state) in [
        (RequirementStatus::InProgress, "OPEN"),
        // A closed, unmerged PR has not landed.
        (RequirementStatus::InProgress, "CLOSED"),
        // An unrecognised forge state is not "merged".
        (RequirementStatus::Done, "garbage"),
    ] {
        let f = Fixture::new(status.clone(), state);
        f.request_changes();
        let req = f.spec();
        assert_eq!(
            req.status,
            RequirementStatus::NeedsAttention,
            "{status} {state}"
        );
        assert!(req.failure_reason.is_some());
        assert!(!req
            .comments
            .iter()
            .any(|c| c.content.contains("not shelved")));
    }
}

/// Publish (Done, PR open) then a CR at the same head: the CR parks the spec,
/// a repeated publish-done write cannot move it back, and it is reworkable.
// trace:BUG-2738.AC2 | ai:claude
#[test]
fn bug_2738_publish_then_request_changes_stays_parked_and_reworkable() {
    let f = Fixture::new(RequirementStatus::InProgress, "OPEN");
    let storage = aida_core::Storage::new(f.root.path().join(".aida-store"));
    let subject = "fix: the thing (BUG-9738)";
    assert!(crate::publication_completion::mark_done_after_push_in(
        &storage, SPEC, subject, &f.head
    )
    .unwrap());
    assert_eq!(f.spec().status, RequirementStatus::Done);
    f.request_changes();
    assert_eq!(f.spec().status, RequirementStatus::NeedsAttention);
    assert!(
        !crate::publication_completion::mark_done_after_push_in(&storage, SPEC, subject, &f.head)
            .unwrap(),
        "a publish-done write never overrides the CR's park"
    );
    let req = f.spec();
    assert_eq!(req.status, RequirementStatus::NeedsAttention);
    let ctx = crate::requeue::ReturnCtx {
        via: "`aida queue rework`".into(),
        via_slug: "queue-rework",
        author: "advisor".into(),
        clear_escalation: false,
        reason: None,
        automated: false,
    };
    let (outcome, _) = crate::requeue::return_to_flight_in_storage(
        &storage,
        req.id,
        &RequirementStatus::NeedsAttention,
        &RequirementStatus::InProgress,
        &ctx,
    )
    .unwrap();
    assert!(
        matches!(outcome, crate::requeue::ReturnOutcome::Returned { .. }),
        "{outcome:?}"
    );
    assert_eq!(f.spec().status, RequirementStatus::InProgress);
}
