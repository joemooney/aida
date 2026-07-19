//! BUG-754: regression tests for the fanout-pickup lease-take status bump.
//!
//! During a harness-fanout burndown, an implementer subagent's lease is
//! registered via `aida session harness-worktree-register` (the SubagentStart
//! hook). When that lease is spec-scoped, the spec must flip Approved → In
//! Progress at lease-take — otherwise `aida queue list` shows the same spec as
//! both a pickable Approved row and in-flight/leased (the observed BUG-754
//! contradiction). trace:BUG-754 | ai:claude

use super::{
    bump_spec_in_progress_at_lease_take, harness_lease_spec_scope, Requirement, RequirementStatus,
    Storage,
};

/// Build a temp project root with a distributed-style `.aida-store` directory
/// containing one spec with the given id + status. Returns the tempdir (keep
/// it alive) — the store is at `<root>/.aida-store`.
fn project_with_spec(spec_id: &str, status: RequirementStatus) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::create_dir_all(root.join(".aida-store")).unwrap();
    let storage = Storage::new(root.join(".aida-store"));
    let mut req = Requirement::new("fanout pickup spec".into(), String::new());
    req.spec_id = Some(spec_id.to_string());
    req.status = status;
    let mut store = aida_core::models::RequirementsStore::new();
    store.requirements = vec![req];
    storage.save(&store).unwrap();
    dir
}

fn spec_status(root: &std::path::Path, spec_id: &str) -> RequirementStatus {
    let storage = Storage::new(root.join(".aida-store"));
    let store = storage.load().unwrap();
    store
        .requirements
        .iter()
        .find(|r| r.spec_id.as_deref() == Some(spec_id))
        .unwrap()
        .status
        .clone()
}

// ── Pure scope gate ─────────────────────────────────────────────────────────

#[test]
fn generic_harness_scope_is_not_a_spec() {
    assert_eq!(harness_lease_spec_scope("harness-worktree"), None);
    assert_eq!(harness_lease_spec_scope("HARNESS-WORKTREE"), None);
}

#[test]
fn spec_scopes_resolve_to_canonical_ids() {
    assert_eq!(
        harness_lease_spec_scope("TASK-1117").as_deref(),
        Some("TASK-1117")
    );
    assert_eq!(
        harness_lease_spec_scope("bug-754").as_deref(),
        Some("BUG-754")
    );
    // A spec-named branch also resolves (scope may carry the branch form).
    assert_eq!(
        harness_lease_spec_scope("task-1117-edit-editor").as_deref(),
        Some("TASK-1117")
    );
}

#[test]
fn non_spec_scopes_do_not_resolve() {
    assert_eq!(harness_lease_spec_scope("main"), None);
    assert_eq!(harness_lease_spec_scope("worktree-agent-a0f3696d"), None);
    assert_eq!(harness_lease_spec_scope(""), None);
}

// ── Effectful bump on the fanout pickup path ────────────────────────────────

/// The regression: a spec-scoped lease-take on an Approved spec flips it to
/// In Progress, so queue/list views agree with the lease about in-flight work.
#[test]
fn lease_take_bumps_approved_spec_to_in_progress() {
    let dir = project_with_spec("TASK-1117", RequirementStatus::Approved);
    assert!(bump_spec_in_progress_at_lease_take(dir.path(), "TASK-1117"));
    assert_eq!(
        spec_status(dir.path(), "TASK-1117"),
        RequirementStatus::InProgress
    );
}

/// Idempotent: re-taking a lease on an already-In-Progress spec is a no-op.
#[test]
fn lease_take_is_idempotent_for_in_progress_spec() {
    let dir = project_with_spec("TASK-1117", RequirementStatus::Approved);
    assert!(bump_spec_in_progress_at_lease_take(dir.path(), "TASK-1117"));
    assert!(!bump_spec_in_progress_at_lease_take(
        dir.path(),
        "TASK-1117"
    ));
    assert_eq!(
        spec_status(dir.path(), "TASK-1117"),
        RequirementStatus::InProgress
    );
}

/// The lifecycle owns every non-Approved status — lease-take never touches
/// them (Done stays Done, NeedsAttention stays parked, Draft stays Draft).
#[test]
fn lease_take_leaves_non_approved_statuses_alone() {
    // Representative non-Approved statuses (all share the same no-bump branch;
    // the store fixture is I/O-heavy, so don't enumerate the full enum).
    for status in [
        RequirementStatus::Draft,
        RequirementStatus::Planned,
        RequirementStatus::Done,
        RequirementStatus::NeedsAttention,
    ] {
        let dir = project_with_spec("BUG-9", status.clone());
        assert!(
            !bump_spec_in_progress_at_lease_take(dir.path(), "BUG-9"),
            "status {status:?} must not bump"
        );
        assert_eq!(spec_status(dir.path(), "BUG-9"), status);
    }
}

/// A generic harness-worktree lease (no spec derivable) never touches the store.
#[test]
fn generic_scope_never_bumps() {
    let dir = project_with_spec("TASK-1117", RequirementStatus::Approved);
    assert!(!bump_spec_in_progress_at_lease_take(
        dir.path(),
        "harness-worktree"
    ));
    assert_eq!(
        spec_status(dir.path(), "TASK-1117"),
        RequirementStatus::Approved
    );
}

/// A scope naming a spec that doesn't exist in the store is a quiet no-op.
#[test]
fn unknown_spec_scope_is_a_noop() {
    let dir = project_with_spec("TASK-1117", RequirementStatus::Approved);
    assert!(!bump_spec_in_progress_at_lease_take(
        dir.path(),
        "TASK-9999"
    ));
}
