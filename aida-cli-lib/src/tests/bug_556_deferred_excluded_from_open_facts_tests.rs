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
            open_spec("STORY-2", true), // deferred → must not appear
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
