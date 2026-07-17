use super::*;
use aida_core::{Relationship, Requirement, RequirementType, RequirementsStore};
use uuid::Uuid;

/// Build a Requirement via the canonical `::new()` constructor and
/// patch the fields the helper inspects: spec_id, agreed_id, type.
fn req(spec_id: &str, agreed: Option<&str>, t: RequirementType) -> Requirement {
    let mut r = Requirement::new(spec_id.to_string(), String::new());
    r.spec_id = Some(spec_id.into());
    r.agreed_id = agreed.map(String::from);
    r.req_type = t;
    r
}

fn rel(target: Uuid, t: RelationshipType) -> Relationship {
    Relationship {
        rel_type: t,
        target_id: target,
        created_at: Some(chrono::Utc::now()),
        created_by: Some("t".into()),
    }
}

/// Child edge into an Epic → derives that Epic's spec_id.
// trace:TASK-44 | ai:claude
#[test]
fn child_into_epic_derives_label() {
    let epic = req("EPIC-20", None, RequirementType::Epic);
    let mut task = req("TASK-44", None, RequirementType::Task);
    task.relationships
        .push(rel(epic.id, RelationshipType::Child));
    let store = RequirementsStore {
        requirements: vec![epic, task.clone()],
        ..Default::default()
    };
    assert_eq!(
        derive_parent_epic_label(&task, &store),
        Some("EPIC-20".to_string())
    );
}

/// Child edge into a non-Epic (Story) → None.
#[test]
fn child_into_non_epic_returns_none() {
    let story = req("STORY-9", None, RequirementType::Story);
    let mut task = req("TASK-44", None, RequirementType::Task);
    task.relationships
        .push(rel(story.id, RelationshipType::Child));
    let store = RequirementsStore {
        requirements: vec![story, task.clone()],
        ..Default::default()
    };
    assert_eq!(derive_parent_epic_label(&task, &store), None);
}

/// No relationships → None (the orphan-TASK case).
#[test]
fn no_relationships_returns_none() {
    let task = req("TASK-42", None, RequirementType::Task);
    let store = RequirementsStore {
        requirements: vec![task.clone()],
        ..Default::default()
    };
    assert_eq!(derive_parent_epic_label(&task, &store), None);
}

/// Parent edge (the inverse direction) is NOT what we want — a Parent
/// rel on `req` means `req` is the parent of something else. We
// only derive from Child edges. trace:TASK-44 | ai:claude
#[test]
fn parent_edge_does_not_derive() {
    let epic = req("EPIC-20", None, RequirementType::Epic);
    let mut task = req("TASK-44", None, RequirementType::Task);
    // Wrong direction: TASK has a Parent edge pointing at EPIC
    // (semantically: TASK is parent of EPIC — nonsense, but the
    // helper shouldn't be fooled by it).
    task.relationships
        .push(rel(epic.id, RelationshipType::Parent));
    let store = RequirementsStore {
        requirements: vec![epic, task.clone()],
        ..Default::default()
    };
    assert_eq!(derive_parent_epic_label(&task, &store), None);
}

/// Dangling target (parent UUID not in store) → None, no panic.
#[test]
fn dangling_target_returns_none() {
    let mut task = req("TASK-44", None, RequirementType::Task);
    task.relationships
        .push(rel(Uuid::now_v7(), RelationshipType::Child));
    let store = RequirementsStore {
        requirements: vec![task.clone()],
        ..Default::default()
    };
    assert_eq!(derive_parent_epic_label(&task, &store), None);
}

/// agreed_id wins over spec_id when both are present — the
/// merge-gate-canonical short form is what users recognize.
#[test]
fn prefers_agreed_id_over_spec_id() {
    let epic = req("EPIC-7-001", Some("EPIC-20"), RequirementType::Epic);
    let mut task = req("TASK-44", None, RequirementType::Task);
    task.relationships
        .push(rel(epic.id, RelationshipType::Child));
    let store = RequirementsStore {
        requirements: vec![epic, task.clone()],
        ..Default::default()
    };
    assert_eq!(
        derive_parent_epic_label(&task, &store),
        Some("EPIC-20".to_string())
    );
}

/// Multiple Child edges, only one of which points to an Epic → that
/// one wins. Order is "first match" so a stable ordering isn't
/// guaranteed for multi-Epic cases (uncommon).
#[test]
fn mixed_child_edges_picks_epic() {
    let epic = req("EPIC-20", None, RequirementType::Epic);
    let story = req("STORY-9", None, RequirementType::Story);
    let mut task = req("TASK-44", None, RequirementType::Task);
    task.relationships
        .push(rel(story.id, RelationshipType::Child));
    task.relationships
        .push(rel(epic.id, RelationshipType::Child));
    let store = RequirementsStore {
        requirements: vec![epic, story, task.clone()],
        ..Default::default()
    };
    assert_eq!(
        derive_parent_epic_label(&task, &store),
        Some("EPIC-20".to_string())
    );
}

/// A `--tree` parent-backed group header must surface the parent
/// requirement's own title + status — never a count-only header that
/// reads like a synthetic bucket. Guards against the TASK-0439 regression.
#[test]
fn tree_header_for_real_parent_shows_title_and_status() {
    colored::control::set_override(false);
    let mut epic = req("EPIC-56", None, RequirementType::Epic);
    epic.title = "Apply AXI lessons".to_string();
    epic.status = aida_core::RequirementStatus::Draft;
    // One In Progress child → the BUG-626 rollup derives InProgress, which the
    // header must show even though the epic's stored status is Draft.
    let mut child = req("TASK-1", None, RequirementType::Task);
    child.status = aida_core::RequirementStatus::InProgress;
    epic.relationships
        .push(rel(child.id, RelationshipType::Child));
    let store = RequirementsStore {
        requirements: vec![epic.clone(), child],
        ..Default::default()
    };
    let header = tree_group_header("EPIC-56", "~unscoped", 5, Some(&epic), &store);
    assert!(header.contains("EPIC-56"), "header: {header}");
    assert!(
        header.contains("Apply AXI lessons"),
        "parent title missing: {header}"
    );
    assert!(header.contains("InProgress"), "status missing: {header}");
    assert!(
        header.contains("5 items"),
        "count metadata missing: {header}"
    );
    // Must NOT be the bare count-only form.
    assert_ne!(header, "EPIC-56 (5 items)");
}

/// BUG-658: the `--tree` EPIC group header must show the children-derived
/// rollup status (BUG-626), not the raw stored YAML status. An epic stored
/// `Draft` whose only child is In Progress must render `InProgress` in the
/// header — agreeing with `aida show` / `aida why` / the row render.
// trace:BUG-658 | ai:claude
#[test]
fn tree_header_epic_status_is_children_rollup_not_stored() {
    colored::control::set_override(false);
    // Stored status deliberately disagrees with the children rollup.
    let mut epic = req("EPIC-77", None, RequirementType::Epic);
    epic.title = "Drifted epic".to_string();
    epic.status = aida_core::RequirementStatus::Draft; // stale stored value
    let mut child = req("STORY-3", None, RequirementType::Story);
    child.status = aida_core::RequirementStatus::InProgress;
    epic.relationships
        .push(rel(child.id, RelationshipType::Child));
    let store = RequirementsStore {
        requirements: vec![epic.clone(), child],
        ..Default::default()
    };
    let header = tree_group_header("EPIC-77", "~unscoped", 2, Some(&epic), &store);
    // Rollup-derived status (a child In Progress → InProgress).
    assert!(
        header.contains("InProgress"),
        "header should show the children rollup status, got: {header}"
    );
    // The raw stored Draft must NOT leak through.
    assert!(
        !header.contains("Draft"),
        "header leaked the raw stored status: {header}"
    );
}

/// The synthetic Unscoped bucket stays a plain count-only header — it has
/// no backing requirement, so it must remain distinguishable from a real
/// parent row (TASK-0439).
#[test]
fn tree_header_for_unscoped_is_count_only() {
    colored::control::set_override(false);
    let store = RequirementsStore::default();
    let header = tree_group_header("~unscoped", "~unscoped", 3, None, &store);
    assert_eq!(header, "Unscoped (3 items)");
}

/// Singular count uses "item", not "items" (TASK-0439).
#[test]
fn tree_header_singular_count() {
    colored::control::set_override(false);
    let mut epic = req("EPIC-7", None, RequirementType::Epic);
    epic.title = "Solo".to_string();
    epic.status = aida_core::RequirementStatus::Draft;
    // All children Completed → rollup derives Completed.
    let mut child = req("TASK-9", None, RequirementType::Task);
    child.status = aida_core::RequirementStatus::Completed;
    epic.relationships
        .push(rel(child.id, RelationshipType::Child));
    let store = RequirementsStore {
        requirements: vec![epic.clone(), child],
        ..Default::default()
    };
    let header = tree_group_header("EPIC-7", "~unscoped", 1, Some(&epic), &store);
    assert!(header.contains("(1 item)"), "header: {header}");
    assert!(!header.contains("items"), "should be singular: {header}");
}

/// A label that doesn't resolve to a requirement (shouldn't normally
/// happen) falls back to the plain id + count header rather than panicking
/// or dropping the count (TASK-0439).
#[test]
fn tree_header_unresolved_parent_falls_back_to_count() {
    colored::control::set_override(false);
    let store = RequirementsStore::default();
    let header = tree_group_header("EPIC-99", "~unscoped", 2, None, &store);
    assert_eq!(header, "EPIC-99 (2 items)");
}
