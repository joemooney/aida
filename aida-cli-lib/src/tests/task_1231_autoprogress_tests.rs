//! TASK-1231 — `aida autoprogress` ready-set selection.
//!
//! Covers the pure seam: given `aida list --format json`, keep only
//! implementable types and bound the count. The orchestration (project
//! resolution, lock-skip, single-spec drain) is I/O and integration-verified.
//! Field shapes mirror the real surface: `spec_id` + `req_type` with the
//! capitalized cache form (`Task`, `Epic`, …).
// trace:TASK-1231 | ai:claude

use super::*;

fn item(spec_id: &str, req_type: &str) -> serde_json::Value {
    serde_json::json!({ "spec_id": spec_id, "req_type": req_type, "status": "Approved" })
}

#[test]
fn keeps_implementable_types_drops_knowledge_and_structural() {
    let json = serde_json::json!([
        item("TASK-1", "Task"),
        item("BUG-2", "Bug"),
        item("STORY-3", "Story"),
        item("EPIC-4", "Epic"),    // structural — never drained
        item("ADR-5", "Decision"), // knowledge — never drained
        item("DOC-6", "Doc"),      // knowledge — never drained
    ])
    .to_string();
    let got = select_ready_from_json(&json, 10);
    assert_eq!(
        got,
        vec![
            "TASK-1".to_string(),
            "BUG-2".to_string(),
            "STORY-3".to_string()
        ]
    );
}

#[test]
fn bounds_to_max() {
    let json = serde_json::json!([
        item("TASK-1", "Task"),
        item("TASK-2", "Task"),
        item("TASK-3", "Task"),
    ])
    .to_string();
    assert_eq!(select_ready_from_json(&json, 2), vec!["TASK-1", "TASK-2"]);
}

#[test]
fn accepts_specs_wrapper_shape() {
    // The list surface may emit `{ "specs": [...] }`.
    let json = serde_json::json!({
        "specs": [
            item("BUG-9", "Bug"),
            item("EPIC-9", "Epic"),
        ]
    })
    .to_string();
    assert_eq!(select_ready_from_json(&json, 10), vec!["BUG-9".to_string()]);
}

#[test]
fn fences_keystone_tagged_work_specs() {
    // A story/task is a drainable TYPE, but a keystone-marker tag must fence it
    // — autoprogress may never headless-drive a keystone (BUG-1120 class). Tags
    // arrive space-joined in one array element (the real list-surface shape).
    let json = serde_json::json!([
        { "spec_id": "STORY-1", "req_type": "Story", "tags": ["frontend keystone parent:EPIC-9"] },
        { "spec_id": "TASK-2", "req_type": "Task", "tags": ["security"] },
        { "spec_id": "TASK-3", "req_type": "Task", "tags": ["docs papercut"] },
    ])
    .to_string();
    // Only the un-keystoned TASK-3 survives.
    assert_eq!(
        select_ready_from_json(&json, 10),
        vec!["TASK-3".to_string()]
    );
}

#[test]
fn empty_or_malformed_json_is_empty_not_a_panic() {
    assert!(select_ready_from_json("", 5).is_empty());
    assert!(select_ready_from_json("not json", 5).is_empty());
    assert!(select_ready_from_json("{}", 5).is_empty());
    assert!(select_ready_from_json("[]", 5).is_empty());
}
