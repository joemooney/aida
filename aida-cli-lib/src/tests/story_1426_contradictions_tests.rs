//! Tests for STORY-1426: Store-wide semantic contradiction sweep.
//
// trace:STORY-1426 | ai:antigravity

use chrono::Utc;
use std::collections::HashMap;

use crate::contradictions::{
    find_mechanical_candidates, find_mechanical_candidates_at, paginate_findings,
    sweep_contradictions, ContradictionFinding,
};
use crate::evaluator::{EvaluatorError, MockEvaluator};
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

#[test]
fn test_evaluator_error_preserves_mechanical_candidate() {
    let mut store = RequirementsStore::default();
    let vis = make_req(
        "VIS-1",
        "Vision",
        "live",
        RequirementType::Vision,
        RequirementStatus::Approved,
        10,
    );
    let vis_id = vis.id;
    store.requirements.push(vis);
    let mut change = make_req(
        "CR-6",
        "Retire vision",
        "Retires VIS-1",
        RequirementType::ChangeRequest,
        RequirementStatus::Completed,
        1,
    );
    change.relationships.push(Relationship {
        rel_type: RelationshipType::References,
        target_id: vis_id,
        created_at: None,
        created_by: None,
    });
    store.requirements.push(change);
    let mock = MockEvaluator::new().with_choice_error(EvaluatorError::Timeout("offline".into()));
    let findings = sweep_contradictions(&store, Some(&mock)).unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].verdict, "evaluation-unavailable");
}

#[test]
fn test_terminal_plan_missing_followup_child_is_reported() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("docs/plans")).unwrap();
    std::fs::write(
        root.path().join("docs/plans/entry.md"),
        "# Plan\n\n## Followups\n\n- Codegraph requirements graph auto population\n\n## Related\n",
    )
    .unwrap();
    let mut store = RequirementsStore::default();
    let mut epic = make_req(
        "EPIC-63",
        "Entry lane",
        "Plan: docs/plans/entry.md",
        RequirementType::Epic,
        // Exact canonical object currently presents Approved while `aida show`
        // and all six linked children carry the completed roll-up.
        RequirementStatus::Approved,
        10,
    );
    for number in 0..6 {
        let child = make_req(
            &format!("TASK-63-{number}"),
            &format!("Completed memory-lane child {number}"),
            "A completed implementation child for the memory lane.",
            RequirementType::Task,
            RequirementStatus::Completed,
            5,
        );
        epic.relationships.push(Relationship {
            rel_type: RelationshipType::Parent,
            target_id: child.id,
            created_at: None,
            created_by: None,
        });
        store.requirements.push(child);
    }
    store.requirements.push(epic);
    let candidates = find_mechanical_candidates_at(&store, root.path());
    assert!(candidates
        .iter()
        .any(|c| c.spec_a_id == "EPIC-63" && c.spec_b_id == "docs/plans/entry.md#Followups"));
}

#[test]
fn test_exact_epic_63_store_shape_survives_compatible_evaluator() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let mut store = RequirementsStore::default();
    store.requirements.push(make_req(
        "EPIC-63",
        "Entry 'memory lane': AIDA under the hood as an invisible requirements store / memory / notepad for a normal Claude or Codex chat (no automation machinery)",
        "The entry adoption lane for AIDA: used transparently as a requirements store + memory + notepad that the coding agent consults and updates on its own, with the user knowing nothing about AIDA (like codegraph under the hood). Store + query CLI + trace + trailer + MCP + --footprint minimal already exist; this epic packages them into a coherent no-machinery lane. Excludes queue/drain/orchestrator/roles/multi-agent and codegraph auto-population (deferred). Plan: docs/plans/2026-09-12-entry-memory-lane.md. Operator-approved shape 2026-09-12.",
        RequirementType::Epic,
        RequirementStatus::Completed,
        10,
    ));

    let candidates = find_mechanical_candidates_at(&store, repo_root);
    let epic_candidate = candidates
        .iter()
        .find(|candidate| {
            candidate.spec_a_id == "EPIC-63" && candidate.spec_b_id.ends_with("#Followups")
        })
        .expect("the exact EPIC-63 plan Followups shape must be discovered");
    assert!(epic_candidate.spec_b_title.contains("Codegraph"));

    let mut probabilities = HashMap::new();
    probabilities.insert("compatible".to_string(), 0.99);
    let mock = MockEvaluator::new().with_choice("compatible", 0.99, probabilities);
    let findings =
        crate::contradictions::sweep_contradictions_at(&store, Some(&mock), repo_root).unwrap();
    let finding = findings
        .iter()
        .find(|finding| finding.spec_a_id == "EPIC-63")
        .expect("a compatible heuristic must not erase a mechanical candidate");
    assert_eq!(finding.verdict, "candidate");
    assert!(finding.mechanical_reason.contains("Followups"));
}

fn finding(a: &str, b: &str, reason: &str) -> ContradictionFinding {
    ContradictionFinding {
        spec_a_id: a.into(),
        spec_a_title: a.into(),
        spec_a_status: "Completed".into(),
        spec_a_type: "Epic".into(),
        spec_b_id: b.into(),
        spec_b_title: b.into(),
        spec_b_status: "MissingChild".into(),
        spec_b_type: "PlanFollowup".into(),
        verdict: "candidate".into(),
        confidence: 0.0,
        probability: 0.0,
        heuristic: true,
        model: "fixture".into(),
        mechanical_reason: reason.into(),
        summary: "fixture".into(),
    }
}

#[test]
fn test_bounded_pages_are_deterministic_and_lossless() {
    let input = vec![
        finding(
            "EPIC-3",
            "plan#3",
            "Terminal EPIC-3 plan promises Followups bullet with no corresponding child spec",
        ),
        finding(
            "ADR-2",
            "ADR-9",
            "Accepted ADR ADR-2 overlaps later accepted ADR ADR-9 by tag",
        ),
        finding(
            "EPIC-1",
            "plan#1",
            "Terminal EPIC-1 plan promises Followups bullet with no corresponding child spec",
        ),
        finding(
            "VIS-1",
            "CR-6",
            "Approved Vision (VIS-1) is older than completed Change Request (CR-6) referencing it",
        ),
        finding(
            "EPIC-2",
            "plan#2",
            "Terminal EPIC-2 plan promises Followups bullet with no corresponding child spec",
        ),
    ];
    let first = paginate_findings(input.clone(), 2, 0, false);
    let second = paginate_findings(input.clone(), 2, first.next_offset.unwrap(), false);
    let third = paginate_findings(input.clone(), 2, second.next_offset.unwrap(), false);
    assert_eq!((first.total, first.returned, first.remaining), (5, 2, 3));
    assert_eq!(first.category_counts.values().sum::<usize>(), 5);
    assert_eq!(first.category_counts["plan-followup"], 3);

    let walked = first
        .findings
        .iter()
        .chain(&second.findings)
        .chain(&third.findings)
        .map(|f| (&f.spec_a_id, &f.spec_b_id))
        .collect::<Vec<_>>();
    let all = paginate_findings(input, 99, 99, true);
    let complete = all
        .findings
        .iter()
        .map(|f| (&f.spec_a_id, &f.spec_b_id))
        .collect::<Vec<_>>();
    assert_eq!(
        walked, complete,
        "pagination must neither lose nor duplicate findings"
    );
    assert_eq!(all.remaining, 0);
    assert_eq!(all.next_offset, None);
}

#[test]
fn test_sweep_contradictions_without_evaluator_reports_mechanical_candidates() {
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

    // Explicitly pass None: zero Jev access / zero evaluator configured
    let findings = sweep_contradictions(&store, None).unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].spec_a_id, "VIS-1");
    assert_eq!(findings[0].spec_b_id, "CR-6");
    assert_eq!(findings[0].verdict, "candidate");
    assert_eq!(findings[0].model, "mechanical-join");
    assert!(findings[0]
        .mechanical_reason
        .contains("older than completed"));
}
