use super::{epic_descendant_uuid_set, filter_entries_by_descendant_set};
use aida_core::models::{Relationship, RelationshipType, RequirementType};
use aida_core::{QueueEntry, Requirement, RequirementsStore};
use std::collections::HashSet;
use uuid::Uuid;

fn mk_req(spec_id: &str, t: RequirementType) -> Requirement {
    let mut r = Requirement::new(spec_id.to_string(), format!("{spec_id} body"));
    r.spec_id = Some(spec_id.to_string());
    r.req_type = t;
    r
}

fn mk_entry(req: Uuid, role: Option<&str>) -> QueueEntry {
    QueueEntry {
        user_id: "alice".to_string(),
        requirement_id: req,
        position: 1000,
        added_by: "alice".to_string(),
        note: None,
        added_at: chrono::Utc::now(),
        for_role: role.map(str::to_string),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    }
}

// Build epic → story (child) → task (grandchild), plus an unrelated task.
// The hierarchy edge is recorded on BOTH endpoints (Parent on the child,
// Child on the parent), matching how the store carries it, so the OUTGOING
// Child+Parent union walk reaches every descendant.
fn store_epic_tree() -> (RequirementsStore, Uuid, Uuid, Uuid, Uuid) {
    let mut epic = mk_req("EPIC-54", RequirementType::Epic);
    let mut story = mk_req("STORY-100", RequirementType::Story);
    let mut grandchild = mk_req("TASK-200", RequirementType::Task);
    let other = mk_req("TASK-999", RequirementType::Task);

    // epic --Child--> story ; story --Parent--> epic
    epic.relationships.push(Relationship {
        rel_type: RelationshipType::Child,
        target_id: story.id,
        created_at: None,
        created_by: None,
    });
    story.relationships.push(Relationship {
        rel_type: RelationshipType::Parent,
        target_id: epic.id,
        created_at: None,
        created_by: None,
    });
    // story --Child--> grandchild ; grandchild --Parent--> story
    story.relationships.push(Relationship {
        rel_type: RelationshipType::Child,
        target_id: grandchild.id,
        created_at: None,
        created_by: None,
    });
    grandchild.relationships.push(Relationship {
        rel_type: RelationshipType::Parent,
        target_id: story.id,
        created_at: None,
        created_by: None,
    });

    let (eid, sid, gid, oid) = (epic.id, story.id, grandchild.id, other.id);
    let mut store = RequirementsStore::new();
    store.requirements = vec![epic, story, grandchild, other];
    (store, eid, sid, gid, oid)
}

#[test]
fn closure_is_transitive_and_includes_epic_itself() {
    let (store, epic, story, grandchild, other) = store_epic_tree();
    let set = epic_descendant_uuid_set(&store, epic);
    assert!(set.contains(&epic), "epic itself must be in the closure");
    assert!(set.contains(&story), "direct child must be in the closure");
    assert!(
        set.contains(&grandchild),
        "grandchild must be in the transitive closure"
    );
    assert!(
        !set.contains(&other),
        "unrelated spec must be excluded from the closure"
    );
}

#[test]
fn filter_keeps_direct_and_grandchild_excludes_unrelated() {
    let (store, epic, story, grandchild, other) = store_epic_tree();
    let set = epic_descendant_uuid_set(&store, epic);

    let e_epic = mk_entry(epic, None);
    let e_story = mk_entry(story, None);
    let e_grand = mk_entry(grandchild, None);
    let e_other = mk_entry(other, None);
    let entries: Vec<&QueueEntry> = vec![&e_epic, &e_story, &e_grand, &e_other];

    let kept = filter_entries_by_descendant_set(&entries, &set);
    let kept_ids: HashSet<Uuid> = kept.iter().map(|e| e.requirement_id).collect();
    assert_eq!(
        kept.len(),
        3,
        "epic + child + grandchild kept, other dropped"
    );
    assert!(kept_ids.contains(&epic));
    assert!(kept_ids.contains(&story));
    assert!(kept_ids.contains(&grandchild));
    assert!(!kept_ids.contains(&other));
}

#[test]
fn filter_composes_with_a_prior_role_filter() {
    let (store, epic, story, grandchild, _other) = store_epic_tree();
    let set = epic_descendant_uuid_set(&store, epic);

    // story routed to implementer (under epic), grandchild routed to
    // reviewer (under epic). A role pre-filter to "implementer" runs first,
    // exactly as the handler does, then the descendant filter ANDs over it.
    let e_story = mk_entry(story, Some("implementer"));
    let e_grand = mk_entry(grandchild, Some("reviewer"));
    let all: Vec<&QueueEntry> = vec![&e_story, &e_grand];

    let role_filtered: Vec<&QueueEntry> = all
        .iter()
        .copied()
        .filter(|e| e.for_role.as_deref() == Some("implementer"))
        .collect();
    let kept = filter_entries_by_descendant_set(&role_filtered, &set);
    assert_eq!(kept.len(), 1, "only the implementer item under the epic");
    assert_eq!(kept[0].requirement_id, story);
}

#[test]
fn empty_descendant_set_yields_empty_result() {
    let some = Uuid::new_v4();
    let e = mk_entry(some, None);
    let entries: Vec<&QueueEntry> = vec![&e];
    let empty: HashSet<Uuid> = HashSet::new();
    let kept = filter_entries_by_descendant_set(&entries, &empty);
    assert!(kept.is_empty(), "empty descendant set → empty result");
}

// TASK-1074: the EPIC-54 discrepancy, at the queue-filter surface. A story is
// a child of the epic AND has a SAME-RANK second parent (another story) that
// lives outside the epic. The shared rank-oriented closure
// (`epic_descendant_uuid_set` → `graph_walk::subtree_ids`) must NOT count that
// second parent as under the epic — the leak the old direction-agnostic walk
// let through (44 vs 43). trace:TASK-1074 | ai:claude
#[test]
fn closure_excludes_a_descendants_same_rank_second_parent() {
    let mut epic = mk_req("EPIC-54", RequirementType::Epic);
    let mut child = mk_req("STORY-699", RequirementType::Story);
    let mut second_parent = mk_req("STORY-698", RequirementType::Story);
    // epic --Parent--> child ; child --Child--> epic
    epic.relationships.push(Relationship {
        rel_type: RelationshipType::Parent,
        target_id: child.id,
        created_at: None,
        created_by: None,
    });
    child.relationships.push(Relationship {
        rel_type: RelationshipType::Child,
        target_id: epic.id,
        created_at: None,
        created_by: None,
    });
    // second_parent --Parent--> child ; child --Child--> second_parent
    second_parent.relationships.push(Relationship {
        rel_type: RelationshipType::Parent,
        target_id: child.id,
        created_at: None,
        created_by: None,
    });
    child.relationships.push(Relationship {
        rel_type: RelationshipType::Child,
        target_id: second_parent.id,
        created_at: None,
        created_by: None,
    });
    let (eid, cid, spid) = (epic.id, child.id, second_parent.id);
    let mut store = RequirementsStore::new();
    store.requirements = vec![epic, child, second_parent];

    let set = epic_descendant_uuid_set(&store, eid);
    assert!(set.contains(&eid), "epic itself");
    assert!(set.contains(&cid), "the real child");
    assert!(
        !set.contains(&spid),
        "the child's same-rank second parent is NOT under the epic"
    );
}
