//! BUG-HOME-048: a PR-title trailer completes only a spec the PR covers.
// trace:BUG-B6B1278890774EF993D2EBC327862470-048 | ai:claude
use super::*;
use aida_core::{Relationship, RequirementStatus, RequirementType};

const NODE_138: &str = "TASK-B6B1278890774EF993D2EBC327862470-138";
const NODE_139: &str = "TASK-B6B1278890774EF993D2EBC327862470-139";
const NODE_140: &str = "TASK-B6B1278890774EF993D2EBC327862470-140";
const BRANCH: &str = "task-b6b1278890774ef993d2ebc327862470-138";

fn task(spec_id: &str, status: RequirementStatus) -> Requirement {
    let mut r = Requirement::new(format!("{spec_id} title"), String::new());
    r.spec_id = Some(spec_id.to_string());
    r.req_type = RequirementType::Task;
    r.status = status;
    r
}

/// The PR-3159 shape: the branch claims the node-local TASK-…-138, and the
/// legacy Draft TASK-1-114 answers to the bare agreed id TASK-139.
fn incident_store() -> RequirementsStore {
    let mut legacy = task("TASK-1-114", RequirementStatus::Draft);
    legacy.agreed_id = Some("TASK-139".to_string());
    legacy.title = "aida-doctor ≈ aida-recover consolidation candidate".to_string();
    RequirementsStore {
        requirements: vec![
            task(NODE_138, RequirementStatus::InProgress),
            task(NODE_139, RequirementStatus::Superseded),
            task(NODE_140, RequirementStatus::Approved),
            legacy,
        ],
        ..Default::default()
    }
}

fn rel(rel_type: RelationshipType, target_id: uuid::Uuid) -> Relationship {
    Relationship {
        rel_type,
        target_id,
        created_at: None,
        created_by: None,
    }
}

fn id_of(store: &RequirementsStore, spec_id: &str) -> uuid::Uuid {
    store
        .requirements
        .iter()
        .find(|r| r.spec_id.as_deref() == Some(spec_id))
        .unwrap()
        .id
}

#[test]
fn the_bare_legacy_trailer_resolves_to_the_legacy_spec() {
    // Guard on the fixture: the hazard only exists when the bare id resolves.
    let store = incident_store();
    let found = store
        .get_requirement_unambiguous("TASK-139")
        .unwrap()
        .unwrap();
    assert_eq!(found.spec_id.as_deref(), Some("TASK-1-114"));
}

/// Acceptance 1–3: a PR claimed for TASK-<node>-138 whose title says
/// `(TASK-139)` refuses the legacy spec, names both ids, and completes the
/// claimed spec instead of the Draft legacy one.
#[test]
fn a_bare_trailer_resolving_outside_the_claim_is_refused_naming_both_ids() {
    let store = incident_store();
    let coverage = TrailerCoverage::for_pr(&store, BRANCH, &[], Some(3159));
    assert!(coverage.is_some(), "the branch claims a spec");
    let trailer = crate::pr_cmd::ship_completion_trailer(
        &store,
        coverage.as_ref(),
        "[AI:claude] fix(ship-from-seat): quarantine cleanup (TASK-139)",
        BRANCH,
        "",
        false,
    );
    assert!(
        !trailer
            .ids
            .iter()
            .any(|id| id.eq_ignore_ascii_case("TASK-139")),
        "the legacy Draft must not be completed: {trailer:?}"
    );
    assert_eq!(trailer.ids, vec![NODE_138.to_string()], "{trailer:?}");
    let note = trailer
        .notes
        .iter()
        .find(|n| n.contains("TASK-139"))
        .unwrap_or_else(|| panic!("a refusal note: {trailer:?}"));
    assert!(note.contains("TASK-1-114"), "names the resolved id: {note}");
    assert!(note.contains("-138"), "names the claimed spec: {note}");
}

#[test]
fn every_bare_trailer_of_the_incident_title_is_refused() {
    let store = incident_store();
    let mut store = store;
    let mut legacy_138 = task("TASK-1-113", RequirementStatus::Done);
    legacy_138.agreed_id = Some("TASK-138".to_string());
    store.requirements.push(legacy_138);
    let coverage = TrailerCoverage::for_pr(&store, BRANCH, &[], None);
    let trailer = crate::pr_cmd::ship_completion_trailer(
        &store,
        coverage.as_ref(),
        "fix(ship-from-seat): x (TASK-138 TASK-139 TASK-140)",
        BRANCH,
        "",
        false,
    );
    assert_eq!(trailer.ids, vec![NODE_138.to_string()], "{trailer:?}");
    assert!(trailer.notes.iter().any(|n| n.contains("TASK-1-113")));
}

#[test]
fn a_spec_superseded_into_the_claim_is_covered() {
    let mut store = incident_store();
    let target = id_of(&store, NODE_138);
    let r139 = store
        .requirements
        .iter_mut()
        .find(|r| r.spec_id.as_deref() == Some(NODE_139))
        .unwrap();
    r139.relationships
        .push(rel(RelationshipType::SupersededBy, target));
    let coverage = TrailerCoverage::for_pr(&store, BRANCH, &[], None).unwrap();
    assert_eq!(coverage.refusal(&store, NODE_139), None);
    assert_eq!(coverage.refusal(&store, NODE_138), None);
    assert!(coverage.refusal(&store, NODE_140).is_some());
}

#[test]
fn the_review_story_covers_count() {
    let mut store = incident_store();
    let covered = id_of(&store, NODE_140);
    let mut review = Requirement::new(
        "Review PR-3159: fix(ship-from-seat): x".to_string(),
        String::new(),
    );
    review.spec_id = Some("STORY-B6B1278890774EF993D2EBC327862470-149".to_string());
    review.req_type = RequirementType::Story;
    review.relationships.push(rel(
        RelationshipType::Custom("implements".to_string()),
        covered,
    ));
    store.requirements.push(review);
    let with_pr = TrailerCoverage::for_pr(&store, BRANCH, &[], Some(3159)).unwrap();
    assert_eq!(with_pr.refusal(&store, NODE_140), None);
    let other_pr = TrailerCoverage::for_pr(&store, BRANCH, &[], Some(1)).unwrap();
    assert!(other_pr.refusal(&store, NODE_140).is_some());
}

#[test]
fn a_lease_scope_on_the_branch_is_part_of_the_claim() {
    let store = incident_store();
    let coverage =
        TrailerCoverage::for_pr(&store, "claude/ship-fixups", &[NODE_140.to_string()], None)
            .unwrap();
    assert_eq!(coverage.refusal(&store, NODE_140), None);
    assert!(coverage.refusal(&store, NODE_138).is_some());
}

#[test]
fn an_unclaimed_branch_keeps_the_title_trailer() {
    // No spec in the branch name and no lease: coverage is unknown, so an
    // ordinary hand-named PR still completes what its title names.
    let store = incident_store();
    let coverage = TrailerCoverage::for_pr(&store, "fix-thing", &[], None);
    assert!(coverage.is_none());
    let trailer = crate::pr_cmd::ship_completion_trailer(
        &store,
        coverage.as_ref(),
        &format!("fix: thing ({NODE_140})"),
        "fix-thing",
        "",
        false,
    );
    assert_eq!(trailer.ids, vec![NODE_140.to_string()]);
}

fn git(root: &std::path::Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Acceptance 3 end to end on the squash subject ship hands the forge: the
/// refused bare id never reaches main, so `aida pull` has nothing to
/// complete it with.
#[test]
fn the_squash_subject_drops_the_refused_trailer_and_names_the_claim() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "t@t.t"]);
    git(root, &["config", "user.name", "t"]);
    std::fs::write(root.join("a"), "1").unwrap();
    git(root, &["add", "a"]);
    git(root, &["commit", "-q", "-m", "chore: initial"]);
    let store_dir = root.join(".aida-store");
    let backend = crate::test_env::home_fixture_backend(&store_dir).unwrap();
    aida_core::DatabaseBackend::save(&backend, &incident_store()).unwrap();

    let title = "[AI:claude] fix(ship-from-seat): quarantine cleanup (TASK-139)";
    let pr = crate::pr_cmd::PrShipMetadata {
        title: title.to_string(),
        body: String::new(),
        number: Some(3159),
    };
    let (subject, trailer) =
        crate::pr_cmd::derive_pr_ship_squash_subject_from_message(root, &pr, BRANCH, title, false)
            .unwrap();
    let subject = subject.expect("the subject is rewritten when a trailer is withheld");
    assert!(!subject.contains("(TASK-139)"), "{subject}");
    assert!(
        subject.to_ascii_uppercase().contains("-138"),
        "the claimed spec carries completion: {subject}"
    );
    assert!(
        trailer.notes.iter().any(|n| n.contains("TASK-1-114")),
        "{trailer:?}"
    );
}

/// A git project whose root holds one commit and a store with the incident
/// specs (legacy YAML, which `load_store_for_lookup` reads at the root);
/// returns it and the head sha.
fn incident_project() -> (tempfile::TempDir, String) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".aida")).unwrap();
    crate::Storage::new(root.path().join("requirements.yaml"))
        .save(&incident_store())
        .unwrap();
    let head = crate::test_git::commit_fixture(root.path());
    (root, head)
}

/// `aida review record <spec> --verdict approved --pr 3159` from an
/// independent reviewer, the forge reporting PR-3159's head as [`BRANCH`].
fn record_approval(root: &std::path::Path, head: &str, spec: &str) -> anyhow::Result<()> {
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
            crate::handle_review_record_at(
                root.to_path_buf(),
                spec,
                "approved",
                Some(head),
                Some(BRANCH),
                Some("looks right"),
                &[],
                &[],
                Some(3159),
            )
        })
    })
}

/// Acceptance 3, the incident case: a PR claimed for TASK-<node>-138 whose
/// title says `(TASK-139)`, with legacy TASK-139 in the store. `review
/// record TASK-139 --pr 3159` refuses naming both ids, records nothing, and
/// the ship gate neither demands nor accepts an approval under TASK-139 — so
/// nothing completes the legacy spec.
#[test]
fn review_record_refuses_an_approval_under_a_foreign_bare_id() {
    let (root, head) = incident_project();
    let err = format!(
        "{:#}",
        record_approval(root.path(), &head, "TASK-139")
            .expect_err("an approval under an uncovered spec must be refused")
    );
    assert!(err.starts_with("refused:"), "{err}");
    assert!(err.contains("`TASK-139`"), "names the trailer id: {err}");
    assert!(err.contains("TASK-1-114"), "names the resolved spec: {err}");
    assert!(err.contains("-138"), "names the claimed spec: {err}");
    assert!(err.contains("nothing was recorded"), "{err}");
    assert!(
        crate::gate::review_verdict::read_recorded_verdict(root.path(), "TASK-139").is_none(),
        "no verdict may be written"
    );
    assert!(!crate::gate::review_verdict::verdict_path(root.path(), "TASK-139").exists());

    // The ship approval gate drops the foreign id from the ids it looks
    // verdicts up under, so no approval under TASK-139 can satisfy it.
    let ids = crate::pr_cmd::ship_gate_known_ids(
        root.path(),
        BRANCH,
        Some("[AI:claude] fix(ship-from-seat): quarantine cleanup (TASK-139)"),
        Some(3159),
    );
    assert!(
        !ids.iter().any(|id| id.eq_ignore_ascii_case("TASK-139")),
        "{ids:?}"
    );
    assert!(
        ids.iter().any(|id| id.ends_with("-138")),
        "the claimed spec stays: {ids:?}"
    );
}

/// The covered spec still records: the check refuses only what PR N does
/// not cover.
#[test]
fn review_record_accepts_the_claimed_spec() {
    let (root, head) = incident_project();
    record_approval(root.path(), &head, NODE_138).expect("the claimed spec records");
    assert!(crate::gate::review_verdict::read_recorded_verdict(root.path(), NODE_138).is_some());
}

/// Acceptance 2, rebase/merge: the branch commits land verbatim and the
/// pull-side auto-bump completes their trailers, so ship refuses a rebase
/// whose branch commit carries an uncovered trailer, naming both ids.
#[test]
fn a_rebase_with_an_uncovered_branch_commit_trailer_is_refused() {
    let store = incident_store();
    let root = tempfile::tempdir().unwrap();
    let refusal = crate::pr_cmd::rebase_trailer_refusal(
        root.path(),
        &store,
        BRANCH,
        3159,
        &[
            format!("fix(ship-from-seat): first slice ({NODE_138})"),
            "fix(ship-from-seat): quarantine cleanup (TASK-139)".to_string(),
        ],
    )
    .expect("an uncovered branch-commit trailer refuses the rebase");
    assert!(refusal.contains("`TASK-139`"), "{refusal}");
    assert!(refusal.contains("TASK-1-114"), "{refusal}");
    assert!(refusal.contains("-138"), "{refusal}");

    assert_eq!(
        crate::pr_cmd::rebase_trailer_refusal(
            root.path(),
            &store,
            BRANCH,
            3159,
            &[format!("fix: covered ({NODE_138})")],
        ),
        None
    );
}

/// Acceptance 2, `--final-slice`: the rebase completion ids come from the same
/// coverage-filtered trailer, so the final slice completes the claimed spec
/// and never the legacy one.
#[test]
fn a_final_slice_rebase_completes_only_the_covered_spec() {
    let store = incident_store();
    let coverage = TrailerCoverage::for_pr(&store, BRANCH, &[], Some(3159));
    let trailer = crate::pr_cmd::ship_completion_trailer_for_method(
        &store,
        coverage.as_ref(),
        crate::forge::MergeMethod::Rebase,
        true,
        "[3/3] fix(ship-from-seat): quarantine cleanup (TASK-139)",
        BRANCH,
        "",
        false,
    );
    assert!(
        !trailer
            .ids
            .iter()
            .any(|id| id.eq_ignore_ascii_case("TASK-139")),
        "{trailer:?}"
    );
    assert!(trailer.notes.iter().any(|n| n.contains("TASK-1-114")));
}
