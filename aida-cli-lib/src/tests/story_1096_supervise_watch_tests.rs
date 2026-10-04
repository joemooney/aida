//! STORY-1096 slice 1 — `aida supervise watch` unit tests.
//!
//! Covers the pure seams of the oversight watch pass: objective-drift
//! detection, identity parsing of queue JSON, and objective resolution
//! (flag → `[oversight] objective` config → error).
// trace:STORY-1096 | ai:claude

use super::*;
use aida_core::models::{RequirementStatus, RequirementType};
use aida_core::Requirement;

fn child(spec_id: &str, status: RequirementStatus) -> Requirement {
    let mut r = Requirement::new(spec_id.to_string(), String::new());
    r.spec_id = Some(spec_id.into());
    r.req_type = RequirementType::Story;
    r.status = status;
    r
}

fn empty_queue() -> std::collections::HashSet<String> {
    std::collections::HashSet::new()
}

fn typed_child(spec_id: &str, status: RequirementStatus, req_type: RequirementType) -> Requirement {
    let mut r = child(spec_id, status);
    r.req_type = req_type;
    r
}

#[test]
fn drift_detects_approved_unqueued_children() {
    let reqs = vec![
        child("STORY-1", RequirementStatus::Approved),
        child("STORY-2", RequirementStatus::Approved),
    ];
    let children: Vec<&Requirement> = reqs.iter().collect();
    let drift = compute_drift(&children, &empty_queue());
    assert_eq!(drift, vec!["STORY-1".to_string(), "STORY-2".to_string()]);
}

// BUG-1130: an accepted ADR carries status Approved but is NOT implementable
// work — realign must never queue it (nor any knowledge-class / structural
// type). Only the real Approved story child is drift.
// trace:BUG-1130 | ai:claude
#[test]
fn drift_excludes_non_implementable_child_types() {
    let reqs = vec![
        typed_child(
            "ADR-26",
            RequirementStatus::Approved,
            RequirementType::Decision,
        ),
        typed_child(
            "CON-1",
            RequirementStatus::Approved,
            RequirementType::Constraint,
        ),
        typed_child("EPIC-9", RequirementStatus::Approved, RequirementType::Epic),
        typed_child("DOC-1", RequirementStatus::Approved, RequirementType::Doc),
        typed_child(
            "STORY-40",
            RequirementStatus::Approved,
            RequirementType::Story,
        ),
    ];
    let children: Vec<&Requirement> = reqs.iter().collect();
    let drift = compute_drift(&children, &empty_queue());
    assert_eq!(drift, vec!["STORY-40".to_string()]);
    assert!(!drift.contains(&"ADR-26".to_string()));
}

#[test]
fn drift_skips_nonapproved_archived_deferred_and_queued() {
    let mut approved_but_queued = child("STORY-10", RequirementStatus::Approved);
    approved_but_queued.spec_id = Some("STORY-10".into());
    let mut archived = child("STORY-11", RequirementStatus::Approved);
    archived.archived = true;
    let mut deferred = child("STORY-12", RequirementStatus::Approved);
    deferred.deferred = true;

    let reqs = vec![
        child("STORY-9", RequirementStatus::InProgress), // not approved
        approved_but_queued,                             // approved but queued
        archived,                                        // approved but archived
        deferred,                                        // approved but deferred
        child("STORY-13", RequirementStatus::Approved),  // the only real drift
    ];
    let children: Vec<&Requirement> = reqs.iter().collect();

    let mut queued = std::collections::HashSet::new();
    queued.insert("STORY-10".to_string());

    let drift = compute_drift(&children, &queued);
    assert_eq!(drift, vec!["STORY-13".to_string()]);
}

#[test]
fn drift_is_idempotent_once_queued() {
    let reqs = vec![child("STORY-20", RequirementStatus::Approved)];
    let children: Vec<&Requirement> = reqs.iter().collect();

    // First pass: unqueued → drift.
    assert_eq!(
        compute_drift(&children, &empty_queue()),
        vec!["STORY-20".to_string()]
    );

    // After realign (now queued) → no drift on the next pass.
    let mut queued = std::collections::HashSet::new();
    queued.insert("STORY-20".to_string());
    assert!(compute_drift(&children, &queued).is_empty());
}

#[test]
fn is_spec_id_recognizes_shapes() {
    assert!(is_spec_id("STORY-1094"));
    assert!(is_spec_id("BUG-12"));
    assert!(is_spec_id("EPIC-1-001"));
    assert!(!is_spec_id("hello"));
    assert!(!is_spec_id("foo-bar"));
    assert!(!is_spec_id("123-45"));
    assert!(!is_spec_id(""));
}

#[test]
fn collect_queue_identity_spec_ids_reads_only_entry_identity_fields() {
    let value = serde_json::json!({
        "items": [
            {"spec_id": "STORY-1", "role": "implementer"},
            {"agreed_id": "bug-2", "note": "lowercase still matches, uppercased"},
        ],
        "other": "not-an-id",
    });
    let mut set = std::collections::HashSet::new();
    collect_queue_identity_spec_ids(&value["items"], &mut set);
    assert!(set.contains("STORY-1"));
    assert!(set.contains("BUG-2"));
    assert!(!set.contains("NOT-AN-ID"));
}

#[test]
fn collect_queue_identity_spec_ids_ignores_note_and_scope_spec_ids() {
    // trace:TASK-1219 | ai:codex
    let value = serde_json::json!([
        {
            "spec_id": "STORY-30",
            "title": "real queued child",
            "for_scope": "STORY-31",
            "note": "mentions TASK-999 but that task is not queued"
        },
        {
            "title": "malformed row has no identity field",
            "note": "STORY-32"
        }
    ]);
    let mut set = std::collections::HashSet::new();
    collect_queue_identity_spec_ids(&value, &mut set);
    assert!(set.contains("STORY-30"));
    assert!(!set.contains("STORY-31"));
    assert!(!set.contains("TASK-999"));
    assert!(!set.contains("STORY-32"));
}

#[test]
fn resolve_objective_prefers_flag() {
    let dir = tempfile::tempdir().unwrap();
    let got = resolve_objective(Some("EPIC-99"), dir.path()).unwrap();
    assert_eq!(got, "EPIC-99");
}

#[test]
fn resolve_objective_falls_back_to_config() {
    let dir = tempfile::tempdir().unwrap();
    let aida = dir.path().join(".aida");
    std::fs::create_dir_all(&aida).unwrap();
    std::fs::write(
        aida.join("config.toml"),
        "[oversight]\nobjective = \"EPIC-63\"\n",
    )
    .unwrap();
    let got = resolve_objective(None, dir.path()).unwrap();
    assert_eq!(got, "EPIC-63");
}

#[test]
fn resolve_objective_errors_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    assert!(resolve_objective(None, dir.path()).is_err());
}

// BUG-1623 n3: with --json --execute, stdout carries exactly one JSON
// document and the nudge line goes to stderr; a dry run never runs the
// nudge.
// trace:BUG-1623 | ai:claude
#[test]
fn watch_json_execute_keeps_stdout_pure_json_and_routes_nudge_to_stderr() {
    let report = WatchReport {
        objective: "EPIC-1".to_string(),
        total_children: 2,
        done_children: 1,
        drift: vec!["STORY-2".to_string()],
        realigned: vec!["STORY-2".to_string()],
        redrive: "off (ADR-26 default)".to_string(),
        redriven: Vec::new(),
        reclassified: Vec::new(),
        redrive_plan: Vec::new(),
    };
    let line = "nudged advisor about 1 transiently parked spec(s)";
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut calls = 0;
    emit_pass_output(
        &report,
        true,
        true,
        &mut || {
            calls += 1;
            Some(line.to_string())
        },
        &mut out,
        &mut err,
    )
    .unwrap();
    assert_eq!(calls, 1);
    let out = String::from_utf8(out).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&out).expect("stdout is one JSON document");
    assert_eq!(doc["objective"], "EPIC-1");
    assert!(!out.contains(line), "{out}");
    assert_eq!(String::from_utf8(err).unwrap().trim(), line);

    // A dry run does not nudge at all.
    let (mut out, mut err) = (Vec::new(), Vec::new());
    emit_pass_output(
        &report,
        false,
        true,
        &mut || panic!("a dry run must not nudge"),
        &mut out,
        &mut err,
    )
    .unwrap();
    serde_json::from_slice::<serde_json::Value>(&out).unwrap();
    assert!(err.is_empty());
}
