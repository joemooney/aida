//! BUG-2208: a Refs-only slice PR of a multi-PR spec (commit subjects end in
//! `(Refs SPEC-ID)`, never `(SPEC-ID)`) must still get a reviewer: the
//! auto-queue files a review story that covers the spec without completing
//! it, `queue work PR-N --role reviewer` resolves the spec ahead of a stray
//! headline mention or a superseded member, and a fallback that still cannot
//! resolve says why.
// trace:TASK-1955 | ai:claude (BUG-2208)

use super::*;
use aida_core::{Relationship, RelationshipType, Requirement, RequirementsStore};

fn spec(id: &str) -> Requirement {
    let mut req = Requirement::new(format!("{id} title"), String::new());
    req.spec_id = Some(id.to_string());
    req
}

fn rel(rel_type: RelationshipType, target: &Requirement) -> Relationship {
    Relationship {
        rel_type,
        target_id: target.id,
        created_at: None,
        created_by: None,
    }
}

/// The PR-2826 shape: every subject is `(Refs TASK-1955)` and one headline
/// mentions BUG-2130 in prose.
fn slice_messages() -> Vec<String> {
    vec![
        "[AI:claude] fix(review): slice 7 sweep items, part 1 (Refs TASK-1955)\n\nbody".into(),
        "[AI:claude] style(test): rustfmt the BUG-2130 identity tests (Refs TASK-1955)".into(),
        "[AI:claude] fix(test): ratchet (Refs TASK-1955)\n\n* trace line (TASK-2017)".into(),
    ]
}

#[test]
fn refs_only_commits_are_a_slice_of_their_spec_not_zero_coverage() {
    let cov = review_coverage_from_commits(&slice_messages());
    assert!(cov.delivered.is_empty(), "{cov:?}");
    assert_eq!(cov.slice_refs, vec!["TASK-1955".to_string()]);
    // The body trailer stays informational; the prose mention is not a spec.
    assert_eq!(cov.referenced, vec!["TASK-2017".to_string()]);
    // The BUG-776 gate counts the slice, so the story is filed.
    assert_eq!(
        auto_queue_skip_reason(
            true,
            Some(2826),
            Some(cov.delivered.len() + cov.slice_refs.len())
        ),
        None
    );
}

#[test]
fn a_delivered_trailer_wins_over_a_refs_trailer_for_the_same_spec() {
    let msgs = vec![
        "[AI:claude] fix: part 1 (Refs TASK-7)".to_string(),
        "[AI:claude] fix: last part (TASK-7)".to_string(),
        "[AI:claude] fix: side work (Refs TASK-8)".to_string(),
    ];
    let cov = review_coverage_from_commits(&msgs);
    assert_eq!(cov.delivered, vec!["TASK-7".to_string()]);
    assert_eq!(cov.slice_refs, vec!["TASK-8".to_string()]);
    assert!(cov.referenced.is_empty());
}

#[test]
fn a_spec_superseded_into_another_candidate_is_dropped() {
    let mut store = RequirementsStore::new();
    let mut sweep = spec("TASK-1955");
    let mut folded = spec("BUG-2130");
    let unrelated = spec("BUG-9");
    sweep
        .relationships
        .push(rel(RelationshipType::Supersedes, &folded));
    store.requirements.push(sweep.clone());
    store.requirements.push(folded.clone());
    store.requirements.push(unrelated);
    assert_eq!(
        crate::pr_lookup::reduce_to_most_specific_specs(
            &store,
            &["BUG-2130".into(), "TASK-1955".into()]
        ),
        vec!["TASK-1955".to_string()]
    );
    // The reciprocal edge alone is enough too.
    sweep.relationships.clear();
    folded
        .relationships
        .push(rel(RelationshipType::SupersededBy, &sweep));
    store.requirements[0] = sweep;
    store.requirements[1] = folded;
    assert_eq!(
        crate::pr_lookup::reduce_to_most_specific_specs(
            &store,
            &["TASK-1955".into(), "BUG-2130".into()]
        ),
        vec!["TASK-1955".to_string()]
    );
    // Unrelated specs are still both kept (real ambiguity).
    assert_eq!(
        crate::pr_lookup::reduce_to_most_specific_specs(
            &store,
            &["TASK-1955".into(), "BUG-9".into()]
        )
        .len(),
        2
    );
}

/// A fake `gh` whose `pr view --json title,commits` answer is `tsv`.
fn fake_gh(dir: &std::path::Path, tsv: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("gh");
    std::fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' '{tsv}'\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn a_refs_only_pr_resolves_to_its_spec_over_a_headline_mention() {
    let dir = tempfile::tempdir().unwrap();
    let gh = fake_gh(
        dir.path(),
        "[AI:claude] fix(review): slice 7 (Refs TASK-1955)\t\
         [AI:claude] style(test): rustfmt the BUG-2130 identity tests (Refs TASK-1955)",
    );
    let _gh = crate::test_env::EnvVarGuard::set("AIDA_TEST_GH_BINARY", &gh);
    let store = RequirementsStore::new();
    assert_eq!(
        crate::pr_lookup::resolve_pr_to_spec(dir.path(), 2826, &store).unwrap(),
        "TASK-1955"
    );
}

#[test]
fn reviewer_fallback_that_cannot_resolve_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let gh = fake_gh(
        dir.path(),
        "[AI:claude] fix: two things\t[AI:claude] fix: a (TASK-1)\t[AI:claude] fix: b (TASK-2)",
    );
    let root = dir.path().join("aida-store");
    let storage = Storage::new(&root);
    aida_core::GitBackend::new(&root)
        .unwrap()
        .save(&RequirementsStore::default())
        .unwrap();
    let _gh = crate::test_env::EnvVarGuard::set("AIDA_TEST_GH_BINARY", &gh);
    let err = crate::queue_cmd::resolve_queue_work_plan(
        &storage,
        "u",
        Some("PR-77"),
        None,
        false,
        true,
        false,
        Some("reviewer"),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("no queued review story for PR-77"), "{err}");
    assert!(
        err.contains("reviewing without a story failed too")
            && err.contains("multiple backing specs: TASK-1, TASK-2"),
        "{err}"
    );
}
