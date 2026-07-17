use super::*;

/// Build a TASK-957 claim lease for `scope`, keyed to `pid` (the advisory
/// liveness signal). No worktree — exactly what `aida claim` writes.
fn claim(id: &str, scope: &str, pid: Option<u32>) -> SessionLease {
    SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: "tester".into(),
        worktree_path: std::path::PathBuf::new(),
        branch: scope.to_ascii_lowercase(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("advisor".into()),
        creator_pid: pid,
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
        claim_verb: true,
    }
}

/// A claim with a LIVE creator pid classifies Live even with NO worktree —
/// the advisory-lock path shared with BUG-511 review leases. This is what
/// makes the claim block the gates.
#[test]
fn claim_with_live_pid_is_live() {
    let now = chrono::Utc::now();
    let alive = claim("c1", "TASK-957", Some(std::process::id()));
    assert!(
        matches!(lease_state_for(&alive, &[], now), LeaseState::Live),
        "a claim held by a live process must read Live (no worktree needed)"
    );
}

/// A claim whose creator pid is dead/absent classifies Stale — a dead
/// claimer's claim is ignored by every gate (no crash-deadlock).
#[test]
fn claim_with_dead_pid_is_stale() {
    let now = chrono::Utc::now();
    let dead = claim("c2", "TASK-957", None);
    assert!(
        matches!(lease_state_for(&dead, &[], now), LeaseState::Stale),
        "a claim with no live process must read Stale"
    );
}

/// ACCEPTANCE: after `aida claim SPEC-N`, a foreign `aida queue work SPEC-N`
/// refuses — the claim is caught by the pre-pickup gate exactly like an
/// AIDA-launched lease, with liveness driven by the claim's pid.
#[test]
fn live_claim_makes_foreign_pickup_refuse() {
    let now = chrono::Utc::now();
    let leases = vec![claim("c-live", "BUG-631", Some(std::process::id()))];
    let blocked = live_spec_claim_by_other(&leases, None, &["BUG-631"], lease_is_live(&[], now));
    assert!(
        blocked.is_some(),
        "a live claim must make a foreign pickup refuse"
    );
    assert_eq!(blocked.unwrap().id, "c-live");
}

/// ACCEPTANCE: a STALE-claimer's claim is ignored — a dead advisor session
/// must not lock the spec, so the foreign pickup proceeds.
#[test]
fn stale_claim_does_not_block_pickup() {
    let now = chrono::Utc::now();
    // pid None → dead → lease_is_live returns false for it.
    let leases = vec![claim("c-dead", "BUG-631", None)];
    let blocked = live_spec_claim_by_other(&leases, None, &["BUG-631"], lease_is_live(&[], now));
    assert!(
        blocked.is_none(),
        "a stale claimer's claim must be ignored (no crash-deadlock)"
    );
}

/// `is_own_claim` is the idempotency / unclaim key: same scope + same
/// creator pid + claim kind. A self-claim is a no-op refresh (matches); a
/// different pid, a different scope, or a non-claim lease does not.
#[test]
fn is_own_claim_matches_only_same_pid_scope_and_kind() {
    let mine = claim("c-mine", "TASK-957", Some(4242));
    assert!(
        is_own_claim(&mine, "TASK-957", Some(4242)),
        "same scope + pid + claim kind ⇒ own claim (idempotent refresh)"
    );
    // case-insensitive scope.
    assert!(is_own_claim(&mine, "task-957", Some(4242)));
    // different pid ⇒ a DIFFERENT session's claim, not mine.
    assert!(!is_own_claim(&mine, "TASK-957", Some(9999)));
    // different scope ⇒ not this spec.
    assert!(!is_own_claim(&mine, "BUG-631", Some(4242)));
    // a non-claim lease (e.g. a real session lease) is never an own-claim.
    let mut session = mine.clone();
    session.claim_verb = false;
    assert!(!is_own_claim(&session, "TASK-957", Some(4242)));
}

/// End-to-end at the lease-dir level: write a claim lease, confirm the gate
/// catches it via `list_leases`, then remove the caller's claim (the
/// `unclaim` core) and confirm the gate no longer fires.
#[test]
fn write_then_unclaim_round_trips_through_the_gate() {
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let now = chrono::Utc::now();
    let my_pid = Some(std::process::id());

    // `aida claim BUG-631` core: write a live claim into the lease dir.
    let lease = claim("c-rt", "BUG-631", my_pid);
    std::fs::create_dir_all(leases_dir(root)).unwrap();
    std::fs::write(
        lease_path(root, &lease.id),
        toml::to_string_pretty(&lease).unwrap(),
    )
    .unwrap();

    let leases = list_leases(root);
    assert!(
        live_spec_claim_by_other(&leases, None, &["BUG-631"], lease_is_live(&[], now)).is_some(),
        "the written claim must be seen by the pre-pickup gate"
    );

    // `aida unclaim BUG-631` core: remove the caller's own claim.
    for l in leases.iter().filter(|l| is_own_claim(l, "BUG-631", my_pid)) {
        std::fs::remove_file(lease_path(root, &l.id)).unwrap();
    }

    let leases = list_leases(root);
    assert!(
        live_spec_claim_by_other(&leases, None, &["BUG-631"], lease_is_live(&[], now)).is_none(),
        "after unclaim the gate must no longer fire"
    );
}
