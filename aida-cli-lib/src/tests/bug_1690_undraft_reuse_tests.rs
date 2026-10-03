//! BUG-1690: nothing ever un-drafted a PR, so a reused draft PR passed CI and
//! review and then stalled at the phase-4 merge, which every forge refuses
//! for a draft. Phase 1's adoption point now marks a reused draft ready
//! through the forge seam — except for `review:draft-only` specs, whose draft
//! IS the STORY-529 human-review hold.
//!
//! Hermetic like the TASK-1421 seam tests: an injected RecordingForge, no
//! `gh`/`glab`, no network.
// trace:BUG-1690 | ai:claude

use super::{Phase1PrResolve, RealPhaseDriver, Storage};
use crate::forge::fake::RecordingForge;
use crate::forge::{ChangeLookup, ChangeRef};

fn driver_with(root: &std::path::Path, spec: &str, forge: &RecordingForge) -> RealPhaseDriver {
    let mut driver = RealPhaseDriver::new(
        root.to_path_buf(),
        spec.into(),
        "test".into(),
        None,
        true,
        None,
        crate::AutonomyMode::Default,
        "test-token".into(),
        false,
        false,
        false,
        false,
        crate::auto_complete::LifecycleSkip::none(),
        crate::auto_complete::AutoCompleteVariant::ThroughCi,
    );
    driver.forge_factory = Some(forge.factory());
    driver
}

fn change(id: u64, branch: &str) -> ChangeRef {
    ChangeRef {
        id,
        url: format!("https://example.invalid/pull/{id}"),
        branch: branch.into(),
        base: String::new(),
        title: Some("feat: reused publication".into()),
    }
}

/// BUG-1690 AC2: a retry that reuses a draft PR reaches a mergeable state
/// instead of stalling — phase 1's adoption point marks the draft ready
/// through the forge, before CI/review/merge ever see it. AC1's chosen branch
/// (un-draft the reused PR, per the decision recorded on the spec) is exactly
/// what this witnesses.
#[test]
fn reused_draft_pr_is_marked_ready_at_adoption() {
    // trace:BUG-1690.ac3bca82 | ai:claude
    // trace:BUG-1690.acd399c2 | ai:claude
    let tmp = tempfile::tempdir().unwrap();
    let mut forge = RecordingForge::new();
    forge.open_for_branch = ChangeLookup::Found(change(47, "claude/bug-1690"));
    forge.is_draft = true;
    let mut driver = driver_with(tmp.path(), "BUG-1690", &forge);

    let Phase1PrResolve::Found(pr) = driver.detect_phase1_pr("claude/bug-1690") else {
        panic!("the scripted open change must resolve as Found")
    };
    driver.undraft_reused_publication(&pr);

    assert_eq!(
        forge.readied(),
        vec![47],
        "the reused draft PR must be marked ready exactly once"
    );
}

/// BUG-1690: a reused PR that is already ready is left alone — the un-draft
/// call is keyed on the forge-reported draft state, not fired unconditionally.
// trace:BUG-1690 | ai:claude
#[test]
fn reused_ready_pr_is_not_touched() {
    let tmp = tempfile::tempdir().unwrap();
    let mut forge = RecordingForge::new();
    forge.open_for_branch = ChangeLookup::Found(change(48, "claude/bug-1690"));
    let mut driver = driver_with(tmp.path(), "BUG-1690", &forge);

    let Phase1PrResolve::Found(pr) = driver.detect_phase1_pr("claude/bug-1690") else {
        panic!("the scripted open change must resolve as Found")
    };
    driver.undraft_reused_publication(&pr);

    assert!(
        forge.readied().is_empty(),
        "a non-draft PR must not be touched: {:?}",
        forge.readied()
    );
}

/// BUG-1690: the STORY-529 `review:draft-only` draft is a deliberate
/// human-review hold — the drain's merge path never reads that tag, so the
/// draft state is the only thing keeping the PR unmerged. Un-drafting it
/// would let the drain auto-merge straight past the gate. Leave it.
// trace:BUG-1690 | ai:claude
#[test]
fn draft_only_tagged_spec_keeps_its_draft() {
    let tmp = tempfile::tempdir().unwrap();
    // A legacy store in the project root: `load_store_for_lookup` resolves
    // `requirements.yaml` relative to the project root, which keeps this
    // fixture inside the tempdir (the distributed-store walk refuses temp
    // roots by design — BUG-1598).
    let mut req = aida_core::Requirement::new("draft-only reuse".to_string(), String::new());
    req.spec_id = Some("BUG-1690".to_string());
    req.tags.insert(crate::pr_ship::DRAFT_ONLY_TAG.to_string());
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    Storage::new(tmp.path().join("requirements.yaml"))
        .save(&store)
        .unwrap();

    let mut forge = RecordingForge::new();
    forge.open_for_branch = ChangeLookup::Found(change(49, "claude/bug-1690"));
    forge.is_draft = true;
    let mut driver = driver_with(tmp.path(), "BUG-1690", &forge);

    let Phase1PrResolve::Found(pr) = driver.detect_phase1_pr("claude/bug-1690") else {
        panic!("the scripted open change must resolve as Found")
    };
    assert!(
        driver.spec_is_draft_only_tagged(),
        "the fixture store must resolve the spec as draft-only tagged"
    );
    driver.undraft_reused_publication(&pr);

    assert!(
        forge.readied().is_empty(),
        "a review:draft-only draft is the human-review hold and must stay a draft: {:?}",
        forge.readied()
    );
}
