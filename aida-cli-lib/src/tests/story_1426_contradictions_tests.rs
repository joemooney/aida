//! Tests for STORY-1426: Store-wide semantic contradiction sweep.
//
// trace:STORY-1426 | ai:antigravity

use chrono::Utc;
use std::collections::HashMap;

use crate::contradictions::{find_mechanical_candidates, sweep_contradictions};
use crate::evaluator::MockEvaluator;
use aida_core::{
    Relationship, RelationshipType, Requirement, RequirementStatus, RequirementType,
    RequirementsStore,
};

fn make_req(
    spec_id: &str,
    title: &str,
    description: &str,
    req_type: RequirementType,
    status: RequirementStatus,
    days_ago: i64,
) -> Requirement {
    let now = Utc::now();
    let created = now - chrono::Duration::days(days_ago);
    let mut req = Requirement::new(title.to_string(), description.to_string());
    req.spec_id = Some(spec_id.to_string());
    req.agreed_id = Some(spec_id.to_string());
    req.req_type = req_type;
    req.status = status;
    req.created_at = created;
    req.modified_at = created;
    req
}

#[test]
fn test_mechanical_join_vis1_cr6_instance() {
    let mut store = RequirementsStore::default();

    // Spec A: VIS-1 Vision in Approved status, 100 days old
    let vis1 = make_req(
        "VIS-1",
        "AIDA is your project's missing index — of intent, not just code",
        "Every project needs an index.",
        RequirementType::Vision,
        RequirementStatus::Approved,
        100,
    );
    let vis1_id = vis1.id;
    store.requirements.push(vis1);

    // Spec B: CR-6 ChangeRequest in Completed status, 50 days old, referencing VIS-1
    let mut cr6 = make_req(
        "CR-6",
        "Reposition the 'missing index' headline (intent/lifecycle)",
        "Kill-shot scan found tagline collides with auto-code-graph tools. Retiring VIS-1.",
        RequirementType::ChangeRequest,
        RequirementStatus::Completed,
        50,
    );
    cr6.relationships.push(Relationship {
        rel_type: RelationshipType::References,
        target_id: vis1_id,
        created_at: None,
        created_by: None,
    });
    store.requirements.push(cr6);

    let candidates = find_mechanical_candidates(&store);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].spec_a_id, "VIS-1");
    assert_eq!(candidates[0].spec_b_id, "CR-6");
}

#[test]
fn test_sweep_contradictions_with_mock_evaluator() {
    let mut store = RequirementsStore::default();

    let vis1 = make_req(
        "VIS-1",
        "AIDA is your project's missing index",
        "Missing index vision.",
        RequirementType::Vision,
        RequirementStatus::Approved,
        100,
    );
    let vis1_id = vis1.id;
    store.requirements.push(vis1);

    let mut cr6 = make_req(
        "CR-6",
        "Reposition index headline",
        "Retire the index headline from VIS-1.",
        RequirementType::ChangeRequest,
        RequirementStatus::Completed,
        50,
    );
    cr6.relationships.push(Relationship {
        rel_type: RelationshipType::References,
        target_id: vis1_id,
        created_at: None,
        created_by: None,
    });
    store.requirements.push(cr6);

    // Mock returns contradicts
    let mut probs = HashMap::new();
    probs.insert("contradicts".to_string(), 0.92);
    let mock = MockEvaluator::new().with_choice("contradicts", 0.92, probs);

    let findings = sweep_contradictions(&store, Some(&mock)).unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].spec_a_id, "VIS-1");
    assert_eq!(findings[0].spec_b_id, "CR-6");
    assert_eq!(findings[0].verdict, "contradicts");
    assert_eq!(findings[0].probability, 0.92);
    assert!(findings[0].heuristic);
}

#[test]
fn test_sweep_contradictions_clean_store() {
    let mut store = RequirementsStore::default();

    let story1 = make_req(
        "STORY-1",
        "First Story",
        "Does something normal.",
        RequirementType::Story,
        RequirementStatus::Completed,
        50,
    );
    let story2 = make_req(
        "STORY-2",
        "Second Story",
        "Does another thing.",
        RequirementType::Story,
        RequirementStatus::Completed,
        10,
    );
    store.requirements.push(story1);
    store.requirements.push(story2);

    let mock = MockEvaluator::new();
    let findings = sweep_contradictions(&store, Some(&mock)).unwrap();
    assert!(findings.is_empty());
}
