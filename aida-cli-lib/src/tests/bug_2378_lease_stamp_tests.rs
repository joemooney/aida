//! BUG-2378 AC2: after the agreed-ID backfill retires a clone's legacy node,
//! a new lease stamps the clone's bound node (the root), never the retired
//! node its shared `.aida/node.toml` still names. The lease commit message
//! (`claim lease <scope> (node <id>)`) is built from the same `node_id`.
// trace:BUG-2378 | ai:claude
use super::*;

#[test]
fn bug_2378_a_new_lease_stamps_the_bound_node_not_the_retired_legacy_node() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join(".aida-store");
    std::fs::create_dir_all(store.join(".aida")).unwrap();
    aida_core::git_ops::init(&store).unwrap();
    // The retired legacy node the shared store worktree still carries, as
    // the complete node/provider pair: `bootstrap` refuses a legacy node
    // without its provider ("incomplete legacy node/provider pair").
    std::fs::write(
        store.join(".aida/node.toml"),
        "node_id='7'\nuser_id=0\nhostname=''\nregistered_at='2026-01-01T00:00:00Z'\n",
    )
    .unwrap();
    std::fs::write(
        store.join(".aida/dispenser.toml"),
        "[mode.Distributed]\nnode_id='7'\n[sequences]\n",
    )
    .unwrap();
    let binding = aida_core::work_binding::bootstrap(&store)
        .unwrap()
        .expect("a WORK clone binds its own node");
    assert_ne!(binding.node_id, "7");

    let claim = build_claim(&store, "FR-1", dir.path(), "claude", false);
    assert_eq!(claim.node_id, binding.node_id);
    assert_eq!(
        aida_core::allocation_authority::clone_node_id(&store)
            .unwrap()
            .as_deref(),
        Some(binding.node_id.as_str()),
        "`aida node show` and the lease stamp name the same node"
    );
}
