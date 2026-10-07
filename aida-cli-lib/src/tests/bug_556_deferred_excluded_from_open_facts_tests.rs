use super::*;

fn open_spec(spec_id: &str, deferred: bool) -> Requirement {
    let mut r = Requirement::new(spec_id.to_string(), String::new());
    r.spec_id = Some(spec_id.into());
    r.req_type = RequirementType::Story;
    r.status = RequirementStatus::Draft;
    r.deferred = deferred;
    r
}

/// Regression for BUG-556: `collect_open_facts` (the source for `aida human`
/// and `burndown explain`) must drop structurally-deferred specs (the
/// `aida defer` view-flag), exactly as it drops archived ones — otherwise a
/// parked spec leaks back onto the human worklist as ungroomed/umbrella.
// trace:BUG-556
#[test]
fn deferred_specs_are_excluded_from_open_facts() {
    let store = aida_core::RequirementsStore {
        requirements: vec![
            open_spec("STORY-1", false),
            open_spec("STORY-2", true),
            open_spec("STORY-3", true), // reason
        ],
        ..Default::default()
    };
    let in_flight = std::collections::HashMap::new();
    let ids: Vec<String> = collect_open_facts(&store, &in_flight)
        .into_iter()
        .map(|f| f.id)
        .collect();
    assert!(ids.contains(&"STORY-1".to_string()));
    assert!(
        !ids.contains(&"STORY-2".to_string()),
        "deferred spec must be excluded from open facts (BUG-556)"
    );
}

// trace:BUG-1793 | ai:antigravity
#[test]
fn defer_single_empty_reason_validation() {
    let dir = tempfile::tempdir().unwrap();
    let store_root = dir.path().join(".aida-store");
    std::fs::create_dir_all(&store_root).unwrap();
    let cache_path = dir.path().join("cache.db");
    let backend = aida_core::CachedGitBackend::open(&store_root, &cache_path).unwrap();

    // ADR-66 (STORY-1473): the hold gate takes a validated, session-bound
    // `product` seat — an env role is only a hint — and the ambient context
    // is pinned so a live orchestrator or TTY on the developer's machine
    // cannot satisfy the gate. The seat is real, so the refusal under test
    // is the ARGUMENT validation, not the authority gate.
    // trace:STORY-1473 | ai:claude
    let _seat = crate::test_env::AmbientGuard::hermetic_with_seat(dir.path(), "product", &[]);

    let err = crate::defer_cmd::defer_single("SPEC-1", None, Some("   "), &backend, &store_root)
        .unwrap_err();
    assert!(err
        .to_string()
        .contains("--reason must contain a non-empty reason"));
}
