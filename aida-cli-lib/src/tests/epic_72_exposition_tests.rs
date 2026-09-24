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

// ---------------------------------------------------------------------------
// TASK-1470 integration fixes: env-var rename, fail-closed advisory audit,
// loopback-only wiki server, vendored (local) mermaid.
// trace:TASK-1470 | ai:claude
// ---------------------------------------------------------------------------

mod task_1470 {
    use super::make_sample_requirement;
    use crate::evaluator::{EvaluatorError, MockEvaluator};
    use crate::exposition::{
        audit_exposition, build_bounded_closure, extract_offline_exposition, jev_key_unset_notice,
        resolve_jev_api_key, ExpositionAudience, JEV_API_KEY_ENV,
    };
    use crate::wiki::{
        bind_wiki_listener, generate_architecture_map, generate_index_html, generate_spec_html,
        serve_connections, wiki_bind_addr, DEFAULT_WIKI_PORT, MERMAID_ASSET_NAME, MERMAID_JS,
    };
    use std::io::{Read, Write};
    use std::net::TcpStream;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name: &str| {
            owned
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        }
    }

    #[test]
    fn jev_key_is_read_from_aida_prefixed_var_only() {
        assert_eq!(JEV_API_KEY_ENV, "AIDA_JEV_API_KEY");
        // The legacy unprefixed name must NOT enable egress.
        assert_eq!(resolve_jev_api_key(env_of(&[("JEV_API_KEY", "k1")])), None);
        assert_eq!(
            resolve_jev_api_key(env_of(&[("TYPESAFE_API_KEY", "k1")])),
            None
        );
        assert_eq!(
            resolve_jev_api_key(env_of(&[("AIDA_JEV_API_KEY", "  k2  ")])),
            Some("k2".to_string())
        );
        // Blank means unset: fail closed to offline.
        assert_eq!(
            resolve_jev_api_key(env_of(&[("AIDA_JEV_API_KEY", "   ")])),
            None
        );
        assert_eq!(resolve_jev_api_key(env_of(&[])), None);
    }

    #[test]
    fn unset_key_notice_names_the_var_and_promises_no_egress() {
        let notice = jev_key_unset_notice();
        assert!(notice.contains("AIDA_JEV_API_KEY"));
        assert!(notice.contains("nothing was sent over the network"));
    }

    #[test]
    fn advisory_audit_uses_mocked_jev_score() {
        let req = make_sample_requirement("TASK-1", "Spec", "A plain spec body.\n\n- rule one");
        let closure = build_bounded_closure(&req, std::slice::from_ref(&req));
        let sidecar = extract_offline_exposition(&req, ExpositionAudience::Operator, &closure);
        let mock = MockEvaluator::new().with_noul(0.5);
        let audit = audit_exposition(&sidecar, &req, Some(&mock));
        assert_eq!(audit.evaluator, "jev-system-one");
        assert!((audit.constraint_preservation_score - 0.5).abs() < f64::EPSILON);
        assert!(audit.needs_revision);
    }

    #[test]
    fn failed_remote_audit_fails_closed_without_leaking_detail() {
        let req = make_sample_requirement("TASK-1", "Spec", "A plain spec body.\n\n- rule one");
        let closure = build_bounded_closure(&req, std::slice::from_ref(&req));
        let sidecar = extract_offline_exposition(&req, ExpositionAudience::Operator, &closure);
        let mock = MockEvaluator::new().with_noul_error(EvaluatorError::ApiError {
            status: 401,
            message: "bad token sk-secret-value".to_string(),
        });
        let audit = audit_exposition(&sidecar, &req, Some(&mock));
        assert_eq!(audit.evaluator, "mechanical-audit");
        assert!(audit
            .findings
            .iter()
            .any(|f| f.contains("Jev advisory unavailable (HTTP 401)")));
        assert!(audit.findings.iter().all(|f| !f.contains("sk-secret")));
    }

    #[test]
    fn wiki_binds_loopback_only() {
        assert_eq!(DEFAULT_WIKI_PORT, 8420);
        assert!(wiki_bind_addr(DEFAULT_WIKI_PORT).ip().is_loopback());
        assert_eq!(wiki_bind_addr(9).to_string(), "127.0.0.1:9");
        let listener = bind_wiki_listener(0).expect("bind loopback");
        assert!(listener.local_addr().unwrap().ip().is_loopback());
    }

    #[test]
    fn wiki_pages_load_mermaid_locally_not_from_a_cdn() {
        let req = make_sample_requirement("TASK-9", "Spec", "Body.");
        let closure = build_bounded_closure(&req, std::slice::from_ref(&req));
        let expo = extract_offline_exposition(&req, ExpositionAudience::Operator, &closure);
        let index = generate_index_html(&[(req.clone(), expo.clone(), false)]);
        let page = generate_spec_html(&req, &expo, false, &closure);
        for html in [&index, &page] {
            assert!(html.contains(r#"<script src="mermaid.min.js"></script>"#));
            assert!(!html.contains("cdn.jsdelivr"));
            assert!(!html.contains("https://"), "no remote URLs in wiki pages");
        }
        // Vendored bundle is the real mermaid build, not a stub.
        assert!(MERMAID_JS.len() > 1_000_000);
        assert!(String::from_utf8_lossy(&MERMAID_JS[..200]).contains("__esbuild_esm_mermaid"));
    }

    #[test]
    fn architecture_map_is_derived_from_the_store_not_hardcoded() {
        let mut epic = make_sample_requirement("EPIC-1", "Epic", "Body.");
        epic.req_type = aida_core::RequirementType::Epic;
        let mut child = make_sample_requirement("TASK-2", "Child", "Body.");
        child.relationships.push(aida_core::Relationship {
            rel_type: aida_core::RelationshipType::Parent,
            target_id: epic.id,
            created_at: None,
            created_by: None,
        });
        let closure = build_bounded_closure(&child, &[epic.clone(), child.clone()]);
        let expo = extract_offline_exposition(&child, ExpositionAudience::Operator, &closure);
        let rows = vec![(epic.clone(), expo.clone(), false), (child, expo, false)];
        let map = generate_architecture_map(&rows);
        assert!(map.contains(r#"EPIC_1["EPIC-1"] --> TASK_2["TASK-2"]"#));
        assert!(!map.contains("EPIC_72"));
        let empty = generate_architecture_map(&rows[..1]);
        assert!(empty.contains("No epics with child specs yet"));
    }

    fn http_get(addr: std::net::SocketAddr, path: &str) -> (String, Vec<u8>) {
        let mut stream = TcpStream::connect(addr).expect("connect");
        let request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n");
        stream.write_all(request.as_bytes()).unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("header terminator");
        (
            String::from_utf8_lossy(&raw[..split]).to_string(),
            raw[split + 4..].to_vec(),
        )
    }

    #[test]
    fn wiki_server_serves_vendored_mermaid_over_loopback() {
        // Case 1: the asset exists in the wiki dir (fresh build).
        let with_asset = tempfile::tempdir().unwrap();
        std::fs::write(with_asset.path().join(MERMAID_ASSET_NAME), MERMAID_JS).unwrap();
        // Case 2: an older build without the asset falls back to the embedded copy.
        let without_asset = tempfile::tempdir().unwrap();

        for dir in [with_asset.path(), without_asset.path()] {
            let listener = bind_wiki_listener(0).unwrap();
            let addr = listener.local_addr().unwrap();
            assert!(addr.ip().is_loopback());
            let dir_owned = dir.to_path_buf();
            let server =
                std::thread::spawn(move || serve_connections(&listener, &dir_owned, Some(2)));

            let (head, body) = http_get(addr, "/mermaid.min.js");
            assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
            assert!(head.contains("Content-Type: application/javascript"));
            assert_eq!(body, MERMAID_JS);

            let (head, _) = http_get(addr, "/../etc/passwd");
            assert!(head.starts_with("HTTP/1.1 403"), "{head}");
            server.join().unwrap();
        }
    }
}
