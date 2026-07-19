use super::*;

fn claim_lease(id: &str, scope: &str) -> SessionLease {
    SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: "tester".into(),
        // A real path is irrelevant here — liveness is injected, not probed.
        worktree_path: std::path::PathBuf::from(format!("/tmp/aida-{}", scope.to_lowercase())),
        branch: scope.to_ascii_lowercase(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: None,
        active_pid: None,
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

fn req_for(spec_id: &str) -> Requirement {
    let mut r = Requirement::new(format!("Title for {spec_id}"), "".into());
    r.spec_id = Some(spec_id.to_string());
    r
}

// ── pre-pickup gate (live_spec_claim_by_other) ──

/// A different session holding a LIVE claim on the spec blocks a fresh pickup
/// — the BUG-634 duplicate-fanout case.
#[test]
fn live_spec_claim_by_other_blocks_on_live() {
    let leases = vec![claim_lease("sess-a", "BUG-634")];
    let claim = live_spec_claim_by_other(&leases, None, &["BUG-634"], |_| true);
    assert!(claim.is_some(), "a live foreign claim must block pickup");
    assert_eq!(claim.unwrap().id, "sess-a");
}

/// A DEAD/stale claim is ignored — a crashed agent must not permanently lock
/// its spec (no crash-deadlock).
#[test]
fn live_spec_claim_by_other_ignores_stale() {
    let leases = vec![claim_lease("sess-dead", "BUG-634")];
    // is_live → false for every lease simulates a dead holder.
    let claim = live_spec_claim_by_other(&leases, None, &["BUG-634"], |_| false);
    assert!(claim.is_none(), "a dead/stale claim must not block pickup");
}

/// The caller's OWN lease never blocks — resuming your own work is fine.
#[test]
fn live_spec_claim_by_other_ignores_self() {
    let mine = claim_lease("sess-mine", "BUG-634");
    let leases = vec![mine.clone()];
    let claim = live_spec_claim_by_other(&leases, Some(&mine), &["BUG-634"], |_| true);
    assert!(claim.is_none(), "must not block on the caller's own lease");
}

/// A generic `harness-worktree` fan-out lease does NOT match a spec scope, so
/// it never gates a pickup (the documented follow-on, not this slice).
#[test]
fn live_spec_claim_by_other_ignores_non_spec_scope() {
    let leases = vec![claim_lease("sess-fanout", "harness-worktree")];
    let claim = live_spec_claim_by_other(&leases, None, &["BUG-634"], |_| true);
    assert!(
        claim.is_none(),
        "a non-spec-scoped lease must not block pickup"
    );
}

// ── pre-edit gate (lease_owning_spec liveness filter) ──

/// A LIVE foreign claim on the spec is returned to the edit gate (warn/block)
/// — the EPIC-54-reject case.
#[test]
fn lease_owning_spec_returns_live_foreign() {
    let target = req_for("EPIC-54");
    let mut store = RequirementsStore::new();
    store.requirements.push(target.clone());
    let leases = vec![claim_lease("sess-b", "EPIC-54")];
    let owner = lease_owning_spec(
        &leases,
        None,
        target.id,
        target.spec_id.as_deref(),
        &store,
        |_| true,
    );
    assert!(owner.is_some(), "a live foreign claim must be enforced");
    assert_eq!(owner.unwrap().id, "sess-b");
}

/// A DEAD/stale foreign claim is skipped by the edit gate — a crashed agent
/// can't permanently block edits/rejects of its spec.
#[test]
fn lease_owning_spec_skips_dead_holder() {
    let target = req_for("EPIC-54");
    let mut store = RequirementsStore::new();
    store.requirements.push(target.clone());
    let leases = vec![claim_lease("sess-dead", "EPIC-54")];
    let owner = lease_owning_spec(
        &leases,
        None,
        target.id,
        target.spec_id.as_deref(),
        &store,
        |_| false,
    );
    assert!(
        owner.is_none(),
        "a dead/stale claim must not block an edit (no crash-deadlock)"
    );
}

/// The caller's own live lease is skipped — a session edits specs in its own
/// scope freely.
#[test]
fn lease_owning_spec_skips_own_live_lease() {
    let target = req_for("EPIC-54");
    let mut store = RequirementsStore::new();
    store.requirements.push(target.clone());
    let mine = claim_lease("sess-mine", "EPIC-54");
    let leases = vec![mine.clone()];
    let owner = lease_owning_spec(
        &leases,
        Some(&mine),
        target.id,
        target.spec_id.as_deref(),
        &store,
        |_| true,
    );
    assert!(owner.is_none(), "must not flag the caller's own live lease");
}
