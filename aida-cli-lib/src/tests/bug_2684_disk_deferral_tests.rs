//! BUG-2684: a lane launch refused below the disk floor is deferred, not
//! failed. Both launch shapes (the standalone runner, which marks the agent
//! only at its spawn, and the drain's headless exec, which marks it before
//! its preflight) roll their pickup setup back; the spec's queue rows stay
//! with the reason `deferred: disk below floor at <ts>`; and once space is
//! free an ordinary pickup admits it. Nothing relaunches anything.
// trace:BUG-2684 | ai:claude

use super::tests::fixture;
use crate::queue_cmd::{run_pickup_launch, AgentLaunchMark};
use std::path::{Path, PathBuf};

fn launching_setup(scope: &str) -> crate::session_start::SessionStartReceipt {
    let receipt = crate::session_start::session_start_with_receipt(
        scope,
        None,
        None,
        false,
        None,
        None,
        "auto",
        false,
        None,
        false,
        None,
        None,
        false,
        None,
        /* force_claim */ false,
        /* use_pool */ Some(false),
        /* self_claim */ true,
        /* review_coexistence */ None,
        /* contained_seat */ false,
        /* rollback_failed_setup */ true,
        crate::session_start::PathReentry::None,
        None,
        /* done_rework_admitted */ false,
        /* no_human */ false,
    )
    .unwrap();
    assert!(receipt.owns_setup(), "a launching pickup owns its setup");
    receipt
}

fn lease_tree(main: &Path, scope: &str) -> Option<(String, PathBuf)> {
    crate::list_leases(main)
        .into_iter()
        .find(|l| l.scope == scope)
        .map(|l| (l.id, l.worktree_path))
}

/// Set the lane disk floor the real preflight reads.
fn set_floor(main: &Path, gib: u64) {
    std::fs::create_dir_all(main.join(".aida")).unwrap();
    std::fs::write(
        main.join(".aida/config.toml"),
        format!("[doctor.disk_headroom]\nmin_free_gib = {gib}\n"),
    )
    .unwrap();
}

/// No host has this much free space.
const UNREACHABLE_FLOOR_GIB: u64 = 1 << 40;

#[test]
fn bug_2684_both_launch_shapes_roll_back_below_the_floor_and_admit_after_recovery() {
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let main = fx.main.clone();
    set_floor(&main, UNREACHABLE_FLOOR_GIB);

    // Standalone shape: the agent is marked only at its spawn, after the
    // preflight.
    let scope = "TASK-926841";
    let receipt = launching_setup(scope);
    let (_, tree) = lease_tree(&main, scope).expect("setup minted a lease");
    let (result, rolled_back) = run_pickup_launch(&main, receipt, |_agent| {
        crate::machine_readiness::lane_disk_preflight(&main)?;
        unreachable!("the preflight refuses below the floor");
    });
    let error = result.unwrap_err();
    assert!(
        crate::lane_disk_deferral::refusal(&error).is_some(),
        "{error:#}"
    );
    assert!(rolled_back, "a refusal before any agent rolls back");
    assert!(lease_tree(&main, scope).is_none() && !tree.exists());

    // Drain shape: the exec path marks the agent before the preflight runs,
    // yet the refusal still launched nobody and rolls back.
    let scope = "TASK-926842";
    let receipt = launching_setup(scope);
    let (_, tree) = lease_tree(&main, scope).expect("setup minted a lease");
    let (result, rolled_back) = run_pickup_launch(&main, receipt, |agent: &AgentLaunchMark| {
        agent.starting();
        crate::machine_readiness::lane_disk_preflight(&main)?;
        unreachable!("the preflight refuses below the floor");
    });
    assert!(result.is_err());
    assert!(rolled_back, "the drain shape's disk refusal rolls back too");
    assert!(lease_tree(&main, scope).is_none() && !tree.exists());

    // Space is freed: the next ordinary pickup of the same spec sets up
    // again (nothing left behind blocks it) and its preflight admits it.
    set_floor(&main, 1);
    let receipt = launching_setup(scope);
    let (lease_id, tree) = lease_tree(&main, scope).expect("setup minted a lease");
    let (result, rolled_back) = run_pickup_launch(&main, receipt, |agent: &AgentLaunchMark| {
        agent.starting();
        crate::machine_readiness::lane_disk_preflight(&main)
    });
    result.expect("the preflight admits the launch above the floor");
    assert!(!rolled_back);
    assert_eq!(
        lease_tree(&main, scope).map(|(id, _)| id).as_deref(),
        Some(lease_id.as_str())
    );
    assert!(tree.exists());
}

const USER: &str = "alice";

fn store() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let store_root = tmp.path().join(".aida-store");
    std::fs::create_dir_all(&store_root).unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    aida_core::git_ops::init(&store_root).unwrap();
    aida_core::git_ops::configure_user(&store_root, "Test", "test@example.com").unwrap();
    (tmp, store_root)
}

fn add(store_root: &Path, spec_id: &str) -> aida_core::Requirement {
    use aida_core::DatabaseBackend;
    let cache = aida_core::CachedGitBackend::default_cache_path(store_root);
    let mut req = aida_core::Requirement::new(format!("spec {spec_id}"), "body".into());
    req.spec_id = Some(spec_id.to_string());
    req.status = aida_core::RequirementStatus::Approved;
    crate::test_env::home_fixture_cached_backend(store_root, &cache)
        .unwrap()
        .add_requirement(req)
        .unwrap()
}

fn enqueue(storage: &aida_core::Storage, req: &aida_core::Requirement, note: Option<&str>) {
    storage
        .queue_add(aida_core::QueueEntry {
            user_id: USER.to_string(),
            requirement_id: req.id,
            position: 2000,
            added_by: USER.to_string(),
            note: note.map(str::to_string),
            added_at: chrono::Utc::now(),
            for_role: Some("implementer".to_string()),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        })
        .unwrap();
}

fn refusal() -> anyhow::Error {
    crate::machine_readiness::require_lane_space(Path::new("/lanes"), 1, 60)
        .unwrap_err()
        .context("queue work BUG-926843")
}

#[test]
fn bug_2684_disk_refusal_keeps_the_spec_queued_with_its_reason() {
    let (_tmp, store_root) = store();
    let storage = aida_core::Storage::new(&store_root);
    let queued = add(&store_root, "BUG-926843");
    let unqueued = add(&store_root, "BUG-926844");
    enqueue(&storage, &queued, Some("operator note"));

    // A failure that is not the disk floor passes through and records nothing.
    let other = crate::lane_disk_deferral::defer(
        &storage,
        "BUG-926843",
        &[queued.id],
        anyhow::anyhow!("cannot measure lane disk headroom"),
    );
    assert_eq!(crate::exit_code_for_error(&other), 1);
    let row = &storage.queue_list(USER, true).unwrap()[0];
    assert_eq!(row.note.as_deref(), Some("operator note"));

    // The disk refusal: the row stays, in place, routed as before, with the
    // reason first and the operator's note kept.
    let deferred =
        crate::lane_disk_deferral::defer(&storage, "BUG-926843", &[queued.id], refusal());
    assert_eq!(
        crate::exit_code_for_error(&deferred),
        crate::lane_disk_deferral::EXIT_CODE
    );
    let message = format!("{deferred:#}");
    assert!(message.contains("lane launch refused"), "{message}");
    assert!(message.contains("BUG-926843 stays queued"), "{message}");
    let rows = storage.queue_list(USER, true).unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.requirement_id, queued.id);
    assert_eq!(row.position, 2000);
    assert_eq!(row.for_role.as_deref(), Some("implementer"));
    let note = row.note.as_deref().unwrap();
    assert!(
        note.starts_with(crate::lane_disk_deferral::NOTE_PREFIX),
        "{note}"
    );
    assert!(note.ends_with(" · operator note"), "{note}");
    assert!(
        message.contains(note.split(" · ").next().unwrap()),
        "{message}"
    );

    // A second refusal replaces the reason rather than stacking it.
    crate::lane_disk_deferral::defer(&storage, "BUG-926843", &[queued.id], refusal());
    let note = storage.queue_list(USER, true).unwrap()[0]
        .note
        .clone()
        .unwrap();
    assert_eq!(
        note.matches(crate::lane_disk_deferral::NOTE_PREFIX).count(),
        1
    );

    // The deferred spec is still a fresh, pickable queue item: not deferred,
    // failed, punted or parked.
    let store = storage.load().unwrap();
    let req = store.get_requirement_by_spec_id("BUG-926843").unwrap();
    assert_eq!(req.status, aida_core::RequirementStatus::Approved);
    assert!(!req.deferred);
    assert_eq!(
        crate::queue_cmd::queue_fresh_pickup_policy(req, &store, false, None),
        crate::queue_cmd::QueueFreshPickup::Pickable
    );

    // A spec with no queue row gets none; the message says how to retry.
    let deferred =
        crate::lane_disk_deferral::defer(&storage, "BUG-926844", &[unqueued.id], refusal());
    assert!(
        format!("{deferred:#}").contains("no queue row"),
        "{deferred:#}"
    );
    assert_eq!(storage.queue_list(USER, true).unwrap().len(), 1);
}
