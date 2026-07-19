use super::{effective_display_status, is_terminal_status, rel_should_write_inverse};
use aida_core::models::{
    RelationshipType, Requirement, RequirementStatus, RequirementType, RequirementsStore,
};

fn store_with_two() -> (RequirementsStore, uuid::Uuid, uuid::Uuid) {
    let mut store = RequirementsStore::new();
    let parent = Requirement::new("Parent epic".into(), "desc".into());
    let child = Requirement::new("Child task".into(), "desc".into());
    let (pid, cid) = (parent.id, child.id);
    store.add_requirement_with_spec_id(parent);
    store.add_requirement_with_spec_id(child);
    (store, pid, cid)
}

#[test]
fn parent_child_edges_force_a_reciprocal() {
    // `rel add --type parent` and `--type child` both write a reciprocal
    // even without `--bidirectional`; other types stay opt-in.
    assert!(rel_should_write_inverse(&RelationshipType::Parent, false));
    assert!(rel_should_write_inverse(&RelationshipType::Child, false));
    assert!(!rel_should_write_inverse(
        &RelationshipType::References,
        false
    ));
    assert!(rel_should_write_inverse(
        &RelationshipType::References,
        true
    ));
}

#[test]
fn rel_add_type_parent_matches_add_parent_shape() {
    // `aida rel add --type parent PARENT CHILD` must store the SAME edge
    // pair as `aida add --parent`: `parent --Parent--> child` on the parent
    // and the reciprocal `child --Child--> parent` on the child.
    let (mut store, pid, cid) = store_with_two();
    let write_inverse = rel_should_write_inverse(&RelationshipType::Parent, false);
    store
        .add_relationship(&pid, RelationshipType::Parent, &cid, write_inverse)
        .unwrap();

    let parent = store.get_requirement_by_id(&pid).unwrap();
    assert!(
        parent
            .relationships
            .iter()
            .any(|r| r.rel_type == RelationshipType::Parent && r.target_id == cid),
        "parent should carry `Parent --> child`"
    );
    let child = store.get_requirement_by_id(&cid).unwrap();
    assert!(
        child
            .relationships
            .iter()
            .any(|r| r.rel_type == RelationshipType::Child && r.target_id == pid),
        "child should carry the reciprocal `Child --> parent`"
    );
}

#[test]
fn rel_add_dedups_repeated_edges() {
    // A repeated identical edge must not accumulate. The model layer rejects
    // the duplicate, and the CLI surfaces it as a friendly no-op (tested via
    // the model's reject here).
    let (mut store, pid, cid) = store_with_two();
    store
        .add_relationship(&pid, RelationshipType::Parent, &cid, false)
        .unwrap();
    let second = store.add_relationship(&pid, RelationshipType::Parent, &cid, false);
    assert!(
        second.is_err(),
        "duplicate edge must be rejected, not stored"
    );
    let parent = store.get_requirement_by_id(&pid).unwrap();
    let count = parent
        .relationships
        .iter()
        .filter(|r| r.rel_type == RelationshipType::Parent && r.target_id == cid)
        .count();
    assert_eq!(count, 1, "only one Parent edge after a repeat");
}

// BUG-628: the rel-add completed-parent guard must read the EFFECTIVE
// (derived-for-epic) status, the same value `aida show` and the cache
// display — not the epic's stale STORED status. Previously the guard read
// `parent.status`, so an epic carrying a stale stored `Completed` (while its
// children derive to Draft/InProgress) was wrongly blocked as "is
// Completed", contradicting the display. This asserts the exact guard
// composition `is_terminal_status(effective_display_status(...))` tracks the
// children, not the stored field. trace:BUG-628 | ai:claude
#[test]
fn rel_add_guard_reads_derived_epic_status_not_stored() {
    let mut store = RequirementsStore::new();

    // Epic with a stale stored `Completed` but an InProgress child → the
    // rollup derives InProgress (non-terminal), so the guard must NOT block.
    let mut epic = Requirement::new("Epic".into(), "desc".into());
    epic.req_type = RequirementType::Epic;
    epic.status = RequirementStatus::Completed; // stale stored value
    let mut child = Requirement::new("Child".into(), "desc".into());
    child.status = RequirementStatus::InProgress;
    epic.relationships.push(aida_core::models::Relationship {
        rel_type: RelationshipType::Parent,
        target_id: child.id,
        created_at: None,
        created_by: None,
    });
    child.relationships.push(aida_core::models::Relationship {
        rel_type: RelationshipType::Child,
        target_id: epic.id,
        created_at: None,
        created_by: None,
    });
    let epic_ref_id = epic.id;
    store.requirements.push(epic);
    store.requirements.push(child);

    let epic = store.get_requirement_by_id(&epic_ref_id).unwrap();
    let effective = effective_display_status(&store, epic);
    assert_eq!(
        effective,
        RequirementStatus::InProgress,
        "guard reads the DERIVED rollup (InProgress), not stored Completed"
    );
    assert!(
        !is_terminal_status(&effective),
        "a derived-InProgress epic is NOT a terminal parent — guard allows the edge"
    );
    // Sanity: the STORED status WOULD have wrongly tripped the guard.
    assert!(
        is_terminal_status(&epic.status),
        "stale stored Completed would have wrongly blocked under the old guard"
    );
}

// BUG-628: the inverse — a genuinely-finished epic (all children Completed)
// derives Completed and the guard still fires, so unifying onto the derived
// value doesn't weaken the real protection. trace:BUG-628 | ai:claude
#[test]
fn rel_add_guard_still_blocks_a_truly_completed_epic() {
    let mut store = RequirementsStore::new();
    let mut epic = Requirement::new("Epic".into(), "desc".into());
    epic.req_type = RequirementType::Epic;
    epic.status = RequirementStatus::Draft; // stored Draft, but children all done
    let mut c1 = Requirement::new("Child 1".into(), "desc".into());
    c1.status = RequirementStatus::Completed;
    let mut c2 = Requirement::new("Child 2".into(), "desc".into());
    c2.status = RequirementStatus::Completed;
    for c in [&mut c1, &mut c2] {
        epic.relationships.push(aida_core::models::Relationship {
            rel_type: RelationshipType::Parent,
            target_id: c.id,
            created_at: None,
            created_by: None,
        });
    }
    let epic_ref_id = epic.id;
    store.requirements.push(epic);
    store.requirements.push(c1);
    store.requirements.push(c2);

    let epic = store.get_requirement_by_id(&epic_ref_id).unwrap();
    let effective = effective_display_status(&store, epic);
    assert_eq!(effective, RequirementStatus::Completed);
    assert!(
        is_terminal_status(&effective),
        "an epic whose children are all Completed derives Completed — guard fires"
    );
}

#[test]
fn tree_walk_resolves_children_from_both_orientations() {
    // `--tree` walks OUTGOING [Child, Parent] from the epic. The canonical
    // orientation (`epic --Parent--> child`, post-TASK-679) and the legacy
    // back-compat orientation (`epic --Child--> child`) both resolve, deduped.
    use aida_core::graph_walk::{walk_union, Direction};

    let mut store = RequirementsStore::new();
    let mut epic = Requirement::new("Epic".into(), "desc".into());
    epic.status = RequirementStatus::InProgress;
    let canonical_child = Requirement::new("Canonical child".into(), "desc".into());
    let legacy_child = Requirement::new("Legacy child".into(), "desc".into());
    let (eid, can_id, leg_id) = (epic.id, canonical_child.id, legacy_child.id);

    // Canonical: epic --Parent--> child.
    epic.relationships.push(aida_core::models::Relationship {
        rel_type: RelationshipType::Parent,
        target_id: can_id,
        created_at: None,
        created_by: None,
    });
    // Legacy back-compat: epic --Child--> child (other stored orientation).
    epic.relationships.push(aida_core::models::Relationship {
        rel_type: RelationshipType::Child,
        target_id: leg_id,
        created_at: None,
        created_by: None,
    });
    store.add_requirement_with_spec_id(canonical_child);
    store.add_requirement_with_spec_id(legacy_child);
    store.add_requirement_with_spec_id(epic);

    let res = walk_union(
        &store,
        eid,
        &[(
            vec![RelationshipType::Child, RelationshipType::Parent],
            Direction::Outgoing,
        )],
        None,
    );
    let nodes: std::collections::HashSet<uuid::Uuid> = res.nodes.iter().copied().collect();
    assert_eq!(
        nodes,
        std::collections::HashSet::from([can_id, leg_id]),
        "tree must find children from both the canonical and legacy edge orientations"
    );
}
