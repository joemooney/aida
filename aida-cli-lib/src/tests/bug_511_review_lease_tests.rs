use super::*;

fn review_lease(id: &str, scope: &str, creator_pid: Option<u32>) -> SessionLease {
    SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_lowercase(),
        owner: "tester".into(),
        worktree_path: std::path::PathBuf::new(),
        branch: "feature-x".into(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("reviewer".into()),
        creator_pid,
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
        review_verb: true,
        claim_verb: false,
    }
}

fn write_lease(project_root: &std::path::Path, lease: &SessionLease) -> std::path::PathBuf {
    let dir = leases_dir(project_root);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{}.toml", lease.id));
    std::fs::write(&path, toml::to_string_pretty(lease).unwrap()).unwrap();
    path
}

/// A review lease classifies by creator PID alone — no worktree exists
/// (the path is empty), so the standard matrix would always say Stale.
#[test]
fn lease_state_for_review_lease_tracks_creator_pid() {
    let now = chrono::Utc::now();
    let alive = review_lease("rev001", "BUG-511", Some(std::process::id()));
    assert!(matches!(
        lease_state_for(&alive, &[], now),
        LeaseState::Live
    ));

    // No recorded PID (or a dead one) → stale, never dormant: a review
    // lease's lifetime is exactly its process's lifetime.
    let dead = review_lease("rev002", "BUG-511", None);
    assert!(matches!(
        lease_state_for(&dead, &[], now),
        LeaseState::Stale
    ));
}

/// A live review lease lands in the in-flight scope map with its role,
/// so `explain_open` can say "being reviewed" (and the queue footer
/// stops suggesting `aida review` for a spec mid-review).
#[test]
fn in_flight_role_map_includes_live_review_lease() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_lease(
        root,
        &review_lease("rev003", "BUG-511", Some(std::process::id())),
    );

    let map = in_flight_lease_role_map(root);
    assert_eq!(
        map.get("bug-511"),
        Some(&Some("reviewer".to_string())),
        "live review lease must appear with its role: {map:?}"
    );

    // And a dead one must NOT appear.
    let tmp2 = tempfile::tempdir().unwrap();
    write_lease(tmp2.path(), &review_lease("rev004", "BUG-511", None));
    assert!(in_flight_lease_role_map(tmp2.path()).is_empty());
}

/// AC-4: a dead review lease is always safe to release — decided before
/// the config gate and the mtime freshness clock (both of which protect
/// worktree sessions, which a review lease is not).
#[test]
fn auto_release_review_lease_decides_by_pid_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let cfg = orchestrator::OrchestratorConfig {
        auto_release_dormant_leases: false, // gate must not matter
        stale_lease_threshold_minutes: 10,
    };

    let alive = review_lease("rev005", "BUG-511", Some(std::process::id()));
    write_lease(root, &alive);
    assert_eq!(
        auto_release_decision_for_lease(root, &alive, &cfg),
        orchestrator::AutoReleaseDecision::Live
    );

    // Dead PID + freshly-written lease file: a session lease would be
    // pinned Live by the mtime gate; the review lease releases.
    let dead = review_lease("rev006", "BUG-511", None);
    write_lease(root, &dead);
    assert!(matches!(
        auto_release_decision_for_lease(root, &dead, &cfg),
        orchestrator::AutoReleaseDecision::SafelyDormant {
            process_dead: true,
            ..
        }
    ));
}

/// The advisory-lease arm of force_cleanup_lease removes the lease file
/// and never touches worktree machinery (an empty worktree_path would
/// resolve the symlink-strip legs relative to CWD).
#[test]
fn force_cleanup_advisory_lease_removes_file_only() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let lease = review_lease("rev007", "BUG-511", None);
    let path = write_lease(root, &lease);

    assert!(force_cleanup_lease(root, &lease));
    assert!(!path.exists(), "lease file must be removed");
}

/// The acquire → conflict → release round trip: a held lease refuses a
/// second acquire; dropping the guard releases the file and frees the
/// scope.
#[test]
fn acquire_review_lease_refuses_second_then_releases_on_drop() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let guard = acquire_review_lease(root, "BUG-511", "feature-x").unwrap();
    let err = acquire_review_lease(root, "BUG-511", "feature-x")
        .expect_err("second acquire must refuse while the first guard is held");
    assert!(
        err.to_string().contains("already in flight"),
        "unexpected refusal text: {err}"
    );

    drop(guard);
    assert!(
        list_leases(root).is_empty(),
        "dropping the guard must release the lease"
    );
    // Scope is free again.
    let _ = acquire_review_lease(root, "BUG-511", "feature-x").unwrap();
}
