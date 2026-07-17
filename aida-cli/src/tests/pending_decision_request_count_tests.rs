use super::*;
use aida_core::{DecisionChoice, DecisionRequest, Requirement, RequirementsStore};

fn pending_dr() -> DecisionRequest {
    DecisionRequest {
        question: "Promote or ship?".to_string(),
        choices: vec![
            DecisionChoice {
                label: "Promote".to_string(),
                consequence: "decompose".to_string(),
                resolution: "tag:+epic".to_string(),
            },
            DecisionChoice {
                label: "Ship".to_string(),
                consequence: "implement".to_string(),
                resolution: "status:approved".to_string(),
            },
        ],
        recommended: Some(1),
        rationale: None,
        answered: None,
        note: None,
        asked_at: None,
        answered_at: None,
    }
}

fn answered_dr() -> DecisionRequest {
    let mut dr = pending_dr();
    dr.answered = Some(0);
    dr
}

/// Replicates the OLD path: list non-archived requirements, count those
/// with a pending DecisionRequest. The new helper must equal this.
fn old_pending_count(store: &RequirementsStore) -> usize {
    store
        .requirements
        .iter()
        .filter(|r| !r.archived)
        .filter(|r| {
            r.decision_request
                .as_ref()
                .map(|dr| dr.is_pending())
                .unwrap_or(false)
        })
        .count()
}

fn req(title: &str, dr: Option<DecisionRequest>, archived: bool) -> Requirement {
    let mut r = Requirement::new(title.to_string(), String::new());
    r.decision_request = dr;
    r.archived = archived;
    r
}

#[test]
fn matches_old_full_load_path_on_mixed_fixture() {
    let mut store = RequirementsStore::default();
    store.requirements = vec![
        req("pending-1", Some(pending_dr()), false),
        req("answered", Some(answered_dr()), false),
        req("no-decision", None, false),
        req("pending-2", Some(pending_dr()), false),
        // Archived pending: excluded by BOTH paths — `list_requirements(false)`
        // filtered archived, so the count must NOT include it.
        req("archived-pending", Some(pending_dr()), true),
    ];
    assert_eq!(pending_decision_request_count(&store), 2);
    assert_eq!(
        pending_decision_request_count(&store),
        old_pending_count(&store),
        "store-backed count must equal the old full-load count exactly"
    );
}

#[test]
fn empty_and_none_pending_yield_zero() {
    let mut store = RequirementsStore::default();
    assert_eq!(pending_decision_request_count(&store), 0);
    store.requirements = vec![
        req("none", None, false),
        req("answered", Some(answered_dr()), false),
    ];
    assert_eq!(pending_decision_request_count(&store), 0);
    assert_eq!(
        pending_decision_request_count(&store),
        old_pending_count(&store)
    );
}
