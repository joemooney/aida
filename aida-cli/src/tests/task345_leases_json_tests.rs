use super::*;

// A minimal fan-out lease at `scope` for shaping the JSON rows.
fn lease(scope: &str) -> SessionLease {
    SessionLease {
        id: "abc123def456".into(),
        scope: scope.into(),
        slug: scope.to_ascii_lowercase(),
        owner: "tester".into(),
        worktree_path: std::path::PathBuf::from("/tmp/wt").join(scope),
        branch: format!("{scope}-branch"),
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

// TASK-345: the drain xref is present + fully populated only for the lease
// whose scope is the drain's current member; every other lease is null.
#[test]
fn drain_xref_only_on_current_member() {
    let mut drain = drain_state::DrainState::new_single("STORY-1", "run-uuid-xyz", false);
    drain.current_phase = Some("1 (implementer)".to_string());

    let shown = vec![
        (lease("STORY-1"), LeaseState::Live, Some(4242u32)),
        (lease("TASK-9"), LeaseState::Dormant, None),
    ];
    let rows = leases_json_rows(&shown, Some(&drain));
    assert_eq!(rows.len(), 2);

    // Row 0 is the drain's current member → drain xref populated.
    let r0 = &rows[0];
    assert_eq!(r0["scope"], "STORY-1");
    assert_eq!(r0["state"], "live");
    assert_eq!(r0["pid"], 4242);
    assert_eq!(r0["role"], "implementer");
    let x = &r0["drain"];
    assert!(x.is_object(), "current member must carry a drain xref");
    assert_eq!(x["run_uuid"], "run-uuid-xyz");
    assert_eq!(x["member"], "STORY-1");
    assert_eq!(x["phase"], "1 (implementer)");
    assert_eq!(x["mode"], "single");
    assert_eq!(x["orchestrator_pid"], drain.orchestrator_pid);
    assert!(x["started_at"].is_string());
    // Single-spec drain → no batch.
    assert!(x["batch"].is_null());

    // Row 1 is not the drain member → drain xref is null.
    assert_eq!(rows[1]["scope"], "TASK-9");
    assert!(
        rows[1]["drain"].is_null(),
        "non-member lease omits the xref"
    );
    assert_eq!(rows[1]["pid"], serde_json::Value::Null);
}

// TASK-345: with no live drain every lease's xref is null, but the row
// shape (all documented keys) is still stable + backward-compatible.
#[test]
fn no_drain_yields_null_xref_stable_shape() {
    let shown = vec![(lease("TASK-3"), LeaseState::Stale, None)];
    let rows = leases_json_rows(&shown, None);
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    for key in [
        "id", "scope", "branch", "role", "worktree", "state", "pid", "drain",
    ] {
        assert!(r.get(key).is_some(), "row must always carry `{key}`");
    }
    assert_eq!(r["state"], "stale");
    assert!(r["drain"].is_null());
}

// TASK-345: an empty batch (`run_uuid` unset between members) still renders
// a valid, null-run_uuid xref for the current member.
#[test]
fn batch_member_xref_carries_batch_name() {
    let mut drain =
        drain_state::DrainState::new_batch("cleanup-batch", &["BUG-1".into(), "BUG-2".into()]);
    drain.current = Some("BUG-1".to_string());
    drain.current_phase = Some("3 (reviewer)".to_string());

    let shown = vec![(lease("BUG-1"), LeaseState::Live, Some(7u32))];
    let rows = leases_json_rows(&shown, Some(&drain));
    let x = &rows[0]["drain"];
    assert_eq!(x["mode"], "batch");
    assert_eq!(x["batch"], "cleanup-batch");
    assert_eq!(x["phase"], "3 (reviewer)");
    // run_uuid is empty between members → serialized as null, not "".
    assert!(x["run_uuid"].is_null());
}
