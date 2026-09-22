//! Provenance and partition tests for the advisor's draft grooming lens.
//! trace:BUG-1498 | ai:codex

use super::*;

fn row(id: &str, description: &str, tags: &[&str]) -> aida_core::RequirementSummary {
    aida_core::RequirementSummary {
        id: uuid::Uuid::new_v4(),
        spec_id: Some(id.into()),
        agreed_id: Some(id.into()),
        title: format!("{id} title"),
        description: description.into(),
        status: "Draft".into(),
        priority: "medium".into(),
        owner: String::new(),
        assignee: None,
        feature: "Uncategorized".into(),
        req_type: "Bug".into(),
        tags: tags.iter().map(|s| (*s).to_string()).collect(),
        created_at: String::new(),
        modified_at: String::new(),
        archived: false,
        archived_at: None,
        deferred: false,
        deferred_at: None,
        deferred_until: None,
        in_degree: 0,
        out_degree: 0,
        heft: 0,
        blocked: false,
        has_pending_decision: false,
        execution_mode: None,
        weight: None,
        origin: None,
        yaml_path: String::new(),
    }
}

fn ids(rows: &[aida_core::RequirementSummary]) -> Vec<&str> {
    rows.iter().filter_map(|r| r.spec_id.as_deref()).collect()
}

fn mixed() -> Vec<aida_core::RequirementSummary> {
    vec![
        row("BUG-1", "Filed by a person", &[]),
        row("BUG-2", "Current machine row", &["auto-drafted"]),
        row(
            "BUG-3",
            "Auto-drafted by `aida queue work TASK-1 --auto-complete` after phase 3 failed.",
            &[],
        ),
    ]
}

#[test]
fn default_draft_view_keeps_human_and_hides_machine_provenance() {
    let mut rows = mixed();
    assert_eq!(apply_machine_draft_lens(&mut rows, true, false, false), 2);
    assert_eq!(ids(&rows), vec!["BUG-1"]);
}

#[test]
fn machine_view_includes_tagged_and_legacy_rows_only() {
    let mut rows = mixed();
    assert_eq!(apply_machine_draft_lens(&mut rows, true, true, false), 0);
    assert_eq!(ids(&rows), vec!["BUG-2", "BUG-3"]);
}

#[test]
fn mixed_status_queries_are_not_silently_partitioned() {
    let mut rows = mixed();
    assert_eq!(apply_machine_draft_lens(&mut rows, false, false, false), 0);
    assert_eq!(ids(&rows), vec!["BUG-1", "BUG-2", "BUG-3"]);
}

#[test]
fn explicit_machine_tag_remains_a_batchable_escape_hatch() {
    let mut rows = mixed();
    assert_eq!(apply_machine_draft_lens(&mut rows, true, false, true), 0);
    assert_eq!(ids(&rows), vec!["BUG-1", "BUG-2", "BUG-3"]);
}

#[test]
fn exact_draft_detector_rejects_mixed_status_specs() {
    assert!(status_spec_is_exact_draft(" Draft "));
    assert!(!status_spec_is_exact_draft("draft,approved"));
    assert!(!status_spec_is_exact_draft("open"));
}
