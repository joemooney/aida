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
        creator_pid_start_time: None,
        active_pid: None,
        active_pid_start_time: None,
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
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
        driver: None,
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

/// BUG-2143: run git in `dir`, panicking on failure.
fn git_in(dir: &std::path::Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs")
        .status
        .success();
    assert!(ok, "git {args:?} failed in {}", dir.display());
}

/// BUG-2143: a claim with no worktree records NO branch — never the primary
/// checkout's `main`, which reap would then protect forever. A claim naming
/// the primary checkout itself also records none; a claim naming a real
/// worktree records that worktree's branch.
// trace:BUG-2143 | ai:claude
#[test]
fn claim_lease_branch_is_the_worktree_branch_or_none() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("main");
    std::fs::create_dir_all(&root).unwrap();
    git_in(&root, &["init", "-q", "-b", "main"]);
    git_in(
        &root,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let wt = tmp.path().join("wt");
    git_in(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "fix/bug-2143",
            wt.to_str().unwrap(),
        ],
    );

    assert_eq!(
        crate::claim::claim_lease_branch(std::path::Path::new(""), &root),
        "",
        "a worktree-less claim must not inherit the primary checkout's branch"
    );
    assert_eq!(crate::claim::claim_lease_branch(&root, &root), "");
    assert_eq!(crate::claim::claim_lease_branch(&wt, &root), "fix/bug-2143");
}

/// BUG-2143: a worktree-less lease that recorded `main` (legacy claims) is
/// branchless to the reap — so a dead claim on a merged spec is reapable and
/// the reap never touches `main` — while a lease that HOLDS a worktree keeps
/// its branch (and the protected-ref floor) unchanged.
// trace:BUG-2143 | ai:claude
#[test]
fn reap_treats_worktreeless_main_claim_as_branchless() {
    let mut legacy = claim("c1", "BUG-2143", None);
    legacy.branch = "main".into();
    assert_eq!(
        crate::session_reap::reap_branch(&legacy, Some("origin/main")),
        ""
    );

    let mut feature = claim("c2", "BUG-2143", None);
    feature.branch = "fix/bug-2143".into();
    assert_eq!(
        crate::session_reap::reap_branch(&feature, Some("origin/main")),
        "fix/bug-2143"
    );

    let mut in_place = claim("c3", "BUG-2143", None);
    in_place.branch = "main".into();
    in_place.worktree_path = std::path::PathBuf::from("/some/checkout");
    assert_eq!(
        crate::session_reap::reap_branch(&in_place, Some("origin/main")),
        "main",
        "a lease holding a worktree keeps its protected branch"
    );
}

/// BUG-2143: a claim lease holds no worktree, so `aida worktree add` must not
/// re-enter it (that printed a blank path as success).
// trace:BUG-2143 | ai:claude
#[test]
fn claim_lease_holds_no_worktree() {
    let c = claim("c1", "BUG-2143", None);
    assert!(!crate::worktree_gc::lease_holds_worktree(&c));
    let mut held = c.clone();
    held.worktree_path = std::path::PathBuf::from("/home/u/ai/aida-bug-2143");
    assert!(crate::worktree_gc::lease_holds_worktree(&held));
}
