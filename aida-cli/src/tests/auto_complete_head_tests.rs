use super::*;
use aida_core::RequirementStatus::*;

/// Only pre-implementation statuses are drivable by the orchestrator from
/// scratch — In Progress / Done are mid-flight, Completed / Rejected are
// terminal. trace:TASK-292 | ai:claude
#[test]
fn drivable_statuses_are_pre_implementation_only() {
    assert!(auto_complete_head_drivable(&Draft));
    assert!(auto_complete_head_drivable(&Approved));
    assert!(auto_complete_head_drivable(&Planned));
    assert!(!auto_complete_head_drivable(&InProgress));
    assert!(!auto_complete_head_drivable(&Done));
    assert!(!auto_complete_head_drivable(&Completed));
    assert!(!auto_complete_head_drivable(&Rejected));
}

/// The common case: the head is drivable, so it is picked with no skips.
#[test]
fn picks_drivable_head_with_no_skips() {
    let candidates = vec![
        ("TASK-1".to_string(), Approved),
        ("TASK-2".to_string(), Draft),
    ];
    let (spec, skipped) = pick_auto_complete_head(&candidates).expect("a drivable head");
    assert_eq!(spec, "TASK-1");
    assert!(skipped.is_empty());
}

/// Acceptance criterion: an in-flight head is skipped to the next eligible
/// item, and every skipped item is reported back to the caller so it can
// surface a note. trace:TASK-292 | ai:claude
#[test]
fn skips_in_flight_head_to_next_eligible() {
    let candidates = vec![
        ("TASK-1".to_string(), InProgress),
        ("TASK-2".to_string(), Done),
        ("TASK-3".to_string(), Approved),
    ];
    let (spec, skipped) = pick_auto_complete_head(&candidates).expect("a drivable item");
    assert_eq!(spec, "TASK-3");
    assert_eq!(
        skipped,
        vec![
            ("TASK-1".to_string(), InProgress),
            ("TASK-2".to_string(), Done),
        ]
    );
}

/// Acceptance criterion: a genuinely empty queue yields an empty skip
/// list — the caller renders "queue is empty … nothing to drive".
#[test]
fn empty_queue_yields_empty_skip_list() {
    let candidates: Vec<(String, RequirementStatus)> = Vec::new();
    let skipped = pick_auto_complete_head(&candidates).expect_err("no drivable item");
    assert!(skipped.is_empty());
}

/// A queue holding only in-flight / terminal items is not drivable: the
/// error carries every skipped item so the caller can name them — a
// distinct case from the empty queue. trace:TASK-292 | ai:claude
#[test]
fn all_in_flight_queue_is_not_drivable() {
    let candidates = vec![
        ("TASK-1".to_string(), InProgress),
        ("TASK-2".to_string(), Completed),
    ];
    let skipped = pick_auto_complete_head(&candidates).expect_err("nothing drivable");
    assert_eq!(
        skipped,
        vec![
            ("TASK-1".to_string(), InProgress),
            ("TASK-2".to_string(), Completed),
        ]
    );
}
