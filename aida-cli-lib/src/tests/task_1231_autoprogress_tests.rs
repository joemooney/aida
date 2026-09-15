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
fn fences_supervised_execution_mode_even_without_a_keystone_tag() {
    // The load-bearing fence: a spec fenced to a SUPERVISED execution_mode
    // (drive/guided/operator/decide) must never be selected by an unattended
    // pass — even when it carries NO keystone tag. This is the exact shape the
    // tag-only fence missed: TASK-1230 (drive, tags=["batch:morning"]) and
    // TASK-1231 (drive, tags=["autonomy cron ..."]) both slipped through.
    // Unset + drain stay drainable. trace:TASK-1231 | ai:claude
    let json = serde_json::json!([
        { "spec_id": "TASK-DRIVE", "req_type": "Task", "tags": ["batch:morning"], "execution_mode": "drive" },
        { "spec_id": "TASK-GUIDED", "req_type": "Task", "tags": [], "execution_mode": "guided" },
        { "spec_id": "TASK-OPERATOR", "req_type": "Task", "tags": [], "execution_mode": "operator" },
        { "spec_id": "TASK-DECIDE", "req_type": "Task", "tags": [], "execution_mode": "decide" },
        { "spec_id": "TASK-DRAIN", "req_type": "Task", "tags": [], "execution_mode": "drain" },
        { "spec_id": "TASK-UNSET", "req_type": "Task", "tags": [] }, // no mode = unset
    ])
    .to_string();
    // Only the explicit `drain` and the unset (drainable-by-default) survive;
    // all four supervised modes are fenced.
    assert_eq!(
        select_ready_from_json(&json, 10),
        vec!["TASK-DRAIN".to_string(), "TASK-UNSET".to_string()]
    );
}

#[test]
fn explicit_drain_execution_mode_overrides_keystone_tag_heuristic() {
    // BUG-1157: `/aida-derisk` can deliberately set security/architecture
    // tagged work to `drain`; the explicit mode is authoritative, while the
    // keystone tag net remains only for ungroomed/unset specs.
    // trace:BUG-1157 | ai:codex
    let json = serde_json::json!([
        { "spec_id": "STORY-DRAIN-SECURITY", "req_type": "Story", "tags": ["security"], "execution_mode": "drain" },
        { "spec_id": "TASK-DRAIN-ARCH", "req_type": "Task", "tags": ["architecture"], "execution_mode": "drain" },
        { "spec_id": "TASK-UNSET-SECURITY", "req_type": "Task", "tags": ["security"] },
        { "spec_id": "TASK-GUIDED-PLAIN", "req_type": "Task", "tags": ["cleanup"], "execution_mode": "guided" },
    ])
    .to_string();

    assert_eq!(
        select_ready_from_json(&json, 10),
        vec![
            "STORY-DRAIN-SECURITY".to_string(),
            "TASK-DRAIN-ARCH".to_string()
        ]
    );
}

#[test]
fn fences_release_meta_tasks_even_when_drain_mode() {
    // Release prep is queueable and tracked, but it is an operator-guided
    // publishing boundary; autoprogress must not feed it into a headless drain.
    // trace:STORY-1125 | ai:codex
    let json = serde_json::json!([
        { "spec_id": "STORY-1125", "req_type": "Story", "tags": ["release workflow meta-task aida:release"], "execution_mode": "drain" },
        { "spec_id": "TASK-OK", "req_type": "Task", "tags": ["cleanup"], "execution_mode": "drain" },
    ])
    .to_string();

    assert_eq!(
        select_ready_from_json(&json, 10),
        vec!["TASK-OK".to_string()]
    );
}

#[test]
fn empty_or_malformed_json_is_empty_not_a_panic() {
    assert!(select_ready_from_json("", 5).is_empty());
    assert!(select_ready_from_json("not json", 5).is_empty());
    assert!(select_ready_from_json("{}", 5).is_empty());
    assert!(select_ready_from_json("[]", 5).is_empty());
}
