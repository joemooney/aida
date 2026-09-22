//! Unit and integration tests for EPIC-72: Multi-Audience Technical Exposition Engine & Interactive Architecture Wiki
//! (TASK-1435, TASK-1436, TASK-1438, TASK-1439).
//
// trace:EPIC-72 trace:TASK-1435 trace:TASK-1436 trace:TASK-1438 trace:TASK-1439 | ai:antigravity

use crate::exposition::{
    audit_exposition, build_bounded_closure, extract_offline_exposition, load_exposition,
    save_exposition, CriterionCitation, ExpositionAudience, ExpositionAudit, ExpositionProvenance,
    ExpositionSidecar, HumanEdits, EXPOSITION_SCHEMA_VERSION,
};
use crate::wiki::{generate_index_html, generate_spec_html};
use aida_core::{Requirement, RequirementStatus, RequirementType};
use tempfile::tempdir;

fn make_sample_requirement(id: &str, title: &str, desc: &str) -> Requirement {
    let mut req = Requirement::new(title.to_string(), desc.to_string());
    req.spec_id = Some(id.to_string());
    req.req_type = RequirementType::Task;
    req.status = RequirementStatus::Approved;
    req
}

#[test]
// trace:TASK-1435 | ai:antigravity
fn test_sidecar_schema_serialization_and_deserialization() {
    let sidecar = ExpositionSidecar {
        schema_version: EXPOSITION_SCHEMA_VERSION,
        spec_id: "TASK-1435".to_string(),
        audience: ExpositionAudience::Operator,
        title: "Architecture spike: exposition sidecar schema".to_string(),
        summary: "Architecture spike for exposition sidecar schema and bounded drift hashing."
            .to_string(),
        rationale: "Decouples human-facing architectural narratives from internal YAML stores."
            .to_string(),
        key_constraints: vec![
            "Must store sidecars at .aida-store/expositions/<SPEC-ID>/<audience>.yaml".to_string(),
            "Must compute SHA-256 hash over bounded graph closure".to_string(),
            "Fail-closed invariant enforcement on canonical requirements".to_string(),
        ],
        tradeoffs: vec!["Static pre-rendering vs live generation".to_string()],
        open_questions: vec!["Cache invalidation on deep transitive closures".to_string()],
        acceptance_criteria_citations: vec![CriterionCitation {
            criterion_id: "CRIT-1".to_string(),
            plain_summary: "Bounded depth-1 closure hashing".to_string(),
        }],
        provenance: ExpositionProvenance {
            generator: "aida-offline-v1".to_string(),
            generated_at: "2026-09-22T10:00:00Z".to_string(),
            source_hash: "a1b2c3d4e5f67890".to_string(),
            closure_spec_ids: vec!["TASK-1435".to_string()],
        },
        human_edits: None,
        audit: Some(ExpositionAudit {
            evaluator: "mechanical-audit".to_string(),
            evaluated_at: "2026-09-22T10:00:01Z".to_string(),
            readability_score: 0.92,
            jargon_saturation_score: 0.15,
            constraint_preservation_score: 0.98,
            deterministic_checks_passed: true,
            needs_revision: false,
            findings: vec![],
        }),
    };

    let yaml_str = serde_yaml::to_string(&sidecar).expect("Failed to serialize sidecar to YAML");
    assert!(yaml_str.contains("spec_id: TASK-1435"));
    assert!(yaml_str.contains("audience: operator"));
    assert!(yaml_str.contains("schema_version: 1"));
    assert!(yaml_str.contains("source_hash: a1b2c3d4e5f67890"));

    let deserialized: ExpositionSidecar =
        serde_yaml::from_str(&yaml_str).expect("Failed to deserialize sidecar from YAML");
    assert_eq!(deserialized.spec_id, sidecar.spec_id);
    assert_eq!(deserialized.audience, sidecar.audience);
    assert_eq!(deserialized.summary, sidecar.summary);
    assert_eq!(deserialized.key_constraints.len(), 3);
    assert!(deserialized.audit.is_some());
    let audit = deserialized.audit.unwrap();
    assert!(audit.deterministic_checks_passed);
    assert!(!audit.needs_revision);
}

#[test]
// trace:TASK-1435 | ai:antigravity
fn test_bounded_closure_determinism_and_sensitivity() {
    let mut req = make_sample_requirement(
        "TASK-1435",
        "Architecture spike: exposition sidecar schema",
        "Define the sidecar YAML schema at .aida-store/expositions/<SPEC-ID>/<audience>.yaml. Fail closed on missing invariants.",
    );

    let parent = make_sample_requirement(
        "EPIC-72",
        "Multi-Audience Technical Exposition Engine",
        "Parent epic for technical expositions and architecture wiki.",
    );

    let blocker = make_sample_requirement(
        "ADR-55",
        "Evaluator Substrate Architecture",
        "Evaluation engine design for validation gates.",
    );

    req.relationships.push(aida_core::Relationship {
        rel_type: aida_core::RelationshipType::Parent,
        target_id: parent.id.clone(),
        created_at: None,
        created_by: None,
    });
    req.relationships.push(aida_core::Relationship {
        rel_type: aida_core::RelationshipType::BlockedBy,
        target_id: blocker.id.clone(),
        created_at: None,
        created_by: None,
    });

    let all = vec![req.clone(), parent.clone(), blocker.clone()];

    let closure_1 = build_bounded_closure(&req, &all);
    let hash_1 = closure_1.compute_sha256();

    // Determinism: identical store produces identical hash
    let closure_1_repeat = build_bounded_closure(&req, &all);
    let hash_1_repeat = closure_1_repeat.compute_sha256();
    assert_eq!(hash_1, hash_1_repeat, "Hash must be deterministic");

    // Sensitivity: changing parent status alters hash
    let mut modified_parent = parent.clone();
    modified_parent.status = RequirementStatus::Completed;
    let all_modified_parent = vec![req.clone(), modified_parent, blocker.clone()];
    let closure_2 = build_bounded_closure(&req, &all_modified_parent);
    let hash_2 = closure_2.compute_sha256();
    assert_ne!(
        hash_1, hash_2,
        "Hash must change when parent status changes"
    );

    // Sensitivity: changing blocker status alters hash
    let mut modified_blocker = blocker.clone();
    modified_blocker.status = RequirementStatus::Completed;
    let all_modified_blocker = vec![req.clone(), parent.clone(), modified_blocker];
    let closure_3 = build_bounded_closure(&req, &all_modified_blocker);
    let hash_3 = closure_3.compute_sha256();
    assert_ne!(
        hash_1, hash_3,
        "Hash must change when blocker status changes"
    );

    // Sensitivity: changing the requirement description alters hash
    let mut modified_req = req.clone();
    modified_req.description = "Altered requirement body.".to_string();
    let all_modified_req = vec![modified_req.clone(), parent.clone(), blocker.clone()];
    let closure_4 = build_bounded_closure(&modified_req, &all_modified_req);
    let hash_4 = closure_4.compute_sha256();
    assert_ne!(hash_1, hash_4, "Hash must change when requirement changes");
}

#[test]
// trace:TASK-1436 | ai:antigravity
fn test_offline_extraction_all_personas() {
    let req = make_sample_requirement(
        "TASK-1436",
        "Implement aida explain CLI command",
        "Implement CLI command aida explain <SPEC> [--audience <ROLE>] [--refresh] [--json].\n\n## Acceptance Criteria\n- Must fail closed if invariants fail\n- Must report stale status when inputs change\n- Preserves human edits under human_reviewed: true",
    );

    let closure = build_bounded_closure(&req, &[req.clone()]);
    let hash = closure.compute_sha256();

    // Operator persona
    let op_expo = extract_offline_exposition(&req, ExpositionAudience::Operator, &closure);
    assert_eq!(op_expo.audience, ExpositionAudience::Operator);
    assert!(op_expo.summary.contains("Implement CLI command"));
    assert!(!op_expo.key_constraints.is_empty());
    assert_eq!(op_expo.provenance.source_hash, hash);

    // Executive persona
    let exec_expo = extract_offline_exposition(&req, ExpositionAudience::Executive, &closure);
    assert_eq!(exec_expo.audience, ExpositionAudience::Executive);
    assert!(exec_expo.summary.contains("Implement CLI command"));

    // Implementer persona
    let impl_expo = extract_offline_exposition(&req, ExpositionAudience::Implementer, &closure);
    assert_eq!(impl_expo.audience, ExpositionAudience::Implementer);
    assert!(impl_expo.summary.contains("Implement CLI command"));

    // Contributor persona
    let contrib_expo = extract_offline_exposition(&req, ExpositionAudience::Contributor, &closure);
    assert_eq!(contrib_expo.audience, ExpositionAudience::Contributor);
    assert!(contrib_expo.summary.contains("Implement CLI command"));
}

#[test]
// trace:TASK-1438 | ai:antigravity
fn test_deterministic_audit_invariant_detection() {
    let req = make_sample_requirement(
        "TASK-1438",
        "Integrate advisory quality audit",
        "Must fail closed when security invariants are violated. CI must verify all acceptance criteria.",
    );

    let closure = build_bounded_closure(&req, &[req.clone()]);

    // Valid exposition that includes the critical fail-closed invariant
    let mut sidecar = extract_offline_exposition(&req, ExpositionAudience::Operator, &closure);
    let audit_pass = audit_exposition(&sidecar, &req, None);
    assert!(
        audit_pass.deterministic_checks_passed,
        "Audit should pass when invariants are preserved"
    );
    assert!(!audit_pass.needs_revision);

    // Deliberately strip fail-closed invariant from exposition
    sidecar.summary = "A casual overview with no mention of safety rules.".to_string();
    sidecar.rationale = "General rationale.".to_string();
    sidecar.key_constraints = vec!["Random constraint".to_string()];

    let audit_fail = audit_exposition(&sidecar, &req, None);
    assert!(
        !audit_fail.deterministic_checks_passed,
        "Audit must fail when canonical spec specifies fail-closed but exposition omits it"
    );
    assert!(audit_fail.needs_revision);
    assert!(
        audit_fail
            .findings
            .iter()
            .any(|f| f.contains("fail-closed")),
        "Findings should cite the dropped fail-closed invariant"
    );
}

#[test]
// trace:TASK-1436 | ai:antigravity
fn test_human_reviewed_protection_and_staleness() {
    let sidecar = ExpositionSidecar {
        schema_version: EXPOSITION_SCHEMA_VERSION,
        spec_id: "TASK-1436".to_string(),
        audience: ExpositionAudience::Operator,
        title: "Implement aida explain CLI command".to_string(),
        summary: "Automated summary.".to_string(),
        rationale: "Human authored rationale.".to_string(),
        key_constraints: vec!["Custom constraint".to_string()],
        tradeoffs: vec![],
        open_questions: vec![],
        acceptance_criteria_citations: vec![],
        provenance: ExpositionProvenance {
            generator: "manual-override".to_string(),
            generated_at: "2026-09-22T10:00:00Z".to_string(),
            source_hash: "initial_sha256_hash".to_string(),
            closure_spec_ids: vec!["TASK-1436".to_string()],
        },
        human_edits: Some(HumanEdits {
            reviewed_by: "architect".to_string(),
            reviewed_at: "2026-09-22T10:05:00Z".to_string(),
            notes: "Approved after team review.".to_string(),
            override_summary: Some("Human authored custom summary.".to_string()),
            human_reviewed: true,
        }),
        audit: None,
    };

    // If current hash matches, not stale
    assert!(!sidecar.is_stale("initial_sha256_hash"));

    // If canonical inputs changed, flagged as stale
    assert!(sidecar.is_stale("new_upstream_sha256_hash"));

    // Human reviewed flag remains true
    assert!(sidecar.is_human_reviewed());
    assert_eq!(
        sidecar.effective_summary(),
        "Human authored custom summary."
    );
}

#[test]
// trace:TASK-1435 trace:TASK-1436 | ai:antigravity
fn test_sidecar_filesystem_roundtrip() {
    let temp = tempdir().expect("Failed to create tempdir");
    let root = temp.path();

    let sidecar = ExpositionSidecar {
        schema_version: EXPOSITION_SCHEMA_VERSION,
        spec_id: "SPEC-999".to_string(),
        audience: ExpositionAudience::Executive,
        title: "Test Spec".to_string(),
        summary: "High-level overview for leadership.".to_string(),
        rationale: "Delivers business value through transparency.".to_string(),
        key_constraints: vec!["Deliver by Q4".to_string()],
        tradeoffs: vec![],
        open_questions: vec![],
        acceptance_criteria_citations: vec![],
        provenance: ExpositionProvenance {
            generator: "aida-offline-v1".to_string(),
            generated_at: "2026-09-22T12:00:00Z".to_string(),
            source_hash: "testhash123".to_string(),
            closure_spec_ids: vec!["SPEC-999".to_string()],
        },
        human_edits: None,
        audit: None,
    };

    save_exposition(root, &sidecar).expect("Failed to save exposition");

    let expected_file = root
        .join(".aida-store")
        .join("expositions")
        .join("SPEC-999")
        .join("executive.yaml");
    assert!(
        expected_file.exists(),
        "Sidecar YAML file must exist at expected path"
    );

    let loaded = load_exposition(root, "SPEC-999", ExpositionAudience::Executive)
        .expect("Failed to load saved exposition");
    assert_eq!(loaded.spec_id, "SPEC-999");
    assert_eq!(loaded.audience, ExpositionAudience::Executive);
    assert_eq!(loaded.summary, "High-level overview for leadership.");
}

#[test]
// trace:TASK-1439 | ai:antigravity
fn test_wiki_html_generation_structure() {
    let req1 = make_sample_requirement(
        "EPIC-72",
        "Living Project Wiki",
        "Interactive architecture visualization and multi-audience exposition wiki.",
    );
    let mut req2 = make_sample_requirement(
        "TASK-1439",
        "Implement aida wiki",
        "Generates static browsable HTML projection with Mermaid graph diagrams.",
    );

    req2.relationships.push(aida_core::Relationship {
        rel_type: aida_core::RelationshipType::Parent,
        target_id: req1.id.clone(),
        created_at: None,
        created_by: None,
    });

    let all = vec![req1.clone(), req2.clone()];

    let closure1 = build_bounded_closure(&req1, &all);
    let expo1 = extract_offline_exposition(&req1, ExpositionAudience::Operator, &closure1);

    let closure2 = build_bounded_closure(&req2, &all);
    let expo2 = extract_offline_exposition(&req2, ExpositionAudience::Operator, &closure2);

    let rows = vec![
        (req1.clone(), expo1.clone(), false),
        (req2.clone(), expo2.clone(), true),
    ];

    let index_html = generate_index_html(&rows);
    assert!(index_html.contains("<!DOCTYPE html>"));
    assert!(index_html.contains("Living Project Wiki"));
    assert!(index_html.contains("EPIC-72"));
    assert!(index_html.contains("TASK-1439"));
    assert!(index_html.contains("badge-fresh"));
    assert!(index_html.contains("badge-stale"));

    let spec_html = generate_spec_html(&req2, &expo2, true, &closure2);
    assert!(spec_html.contains("<!DOCTYPE html>"));
    assert!(spec_html.contains("TASK-1439"));
    assert!(spec_html.contains("mermaid"));
    assert!(spec_html.contains("graph TD"));
    assert!(spec_html.contains("Stale (Graph Closure Drifted)"));
}
