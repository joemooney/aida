use super::*;

fn spec_lease(id: &str, scope: &str, worktree: std::path::PathBuf) -> SessionLease {
    SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: "tester".into(),
        worktree_path: worktree,
        branch: scope.to_ascii_lowercase(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: None,
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
    }
}

/// A live lease (its holder process is alive) → the In-Progress flag is
/// liveness-backed: verdict `Live`.
#[test]
fn live_lease_yields_live_verdict() {
    assert_eq!(
        classify_spec_liveness(Some(LeaseState::Live), true),
        SpecLiveness::Live
    );
}

/// A dead-pid lease (worktree gone / no live claude) classifies Stale, and
/// a Dormant lease (worktree present but no live process) also reads Stale —
/// the operator asked specifically "is a LIVE process working it?".
#[test]
fn dead_or_dormant_lease_yields_stale_verdict() {
    assert_eq!(
        classify_spec_liveness(Some(LeaseState::Stale), true),
        SpecLiveness::Stale
    );
    assert_eq!(
        classify_spec_liveness(Some(LeaseState::Dormant), true),
        SpecLiveness::Stale
    );
}

/// In-Progress with NO spec-scoped lease → flag-only: the status flag is
/// not liveness-backed (the advisor Agent-tool fan-out case, correctly).
#[test]
fn in_progress_without_lease_yields_flag_only() {
    assert_eq!(classify_spec_liveness(None, true), SpecLiveness::FlagOnly);
}

/// Not In-Progress with no lease → no active session expected.
#[test]
fn not_in_progress_without_lease_yields_no_session() {
    assert_eq!(classify_spec_liveness(None, false), SpecLiveness::NoSession);
}

/// The spec→session link: a lease whose scope IS the spec id is found
/// (case-insensitively, matching agreed OR spec id); a generic
/// `harness-worktree` fan-out lease is NOT — that absence is the honest
/// flag-only signal.
#[test]
fn spec_scoped_lease_matches_spec_id_not_harness_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let owned = spec_lease("l-task894", "TASK-894", tmp.path().to_path_buf());
    let harness = spec_lease("l-harness", "harness-worktree", tmp.path().to_path_buf());
    let leases = vec![harness, owned];

    let found = spec_scoped_lease(&leases, &["TASK-894", "task-894"]);
    assert!(
        found.is_some(),
        "spec-scoped lease must be found by spec id"
    );
    assert_eq!(found.unwrap().scope, "TASK-894");

    // A spec with only a harness fan-out lease present has no spec link.
    assert!(
        spec_scoped_lease(&leases, &["TASK-700"]).is_none(),
        "a spec with no scope-matching lease reads flag-only"
    );
}

// The `aida why` STALLED decision (BUG-623) — a spec-scoped lease whose
// worktree is GONE classifies as Stale (not Live), which drives the
// "in-flight but STALLED — no live process" branch instead of the
// "being worked now" line. (A live lease would classify Live and keep the
// genuine in-flight message.)
#[test]
fn why_stale_path_triggers_when_lease_not_live() {
    let now = chrono::Utc::now();
    // Worktree path that does not exist → classify_lease_state → Stale.
    let gone = spec_lease(
        "l-gone",
        "TASK-894",
        std::path::PathBuf::from("/nonexistent/aida-task-894"),
    );
    let state = lease_state_for(&gone, &[], now);
    assert!(
        !matches!(state, LeaseState::Live),
        "a lease with a missing worktree must not be Live: {state:?}"
    );
    // … which is exactly the condition the `why` STALLED branch keys on.
    assert_eq!(
        classify_spec_liveness(Some(state), true),
        SpecLiveness::Stale
    );
}
