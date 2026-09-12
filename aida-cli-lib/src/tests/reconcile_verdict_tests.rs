use super::{latest_reopen_transition_at, merged_at_satisfies_latest_reopen, reconcile_verdict};
use crate::auto_complete::PhaseReconcile;
use aida_core::{FieldChange, HistoryEntry, Requirement};
use chrono::{DateTime, Utc};
use uuid::Uuid;

fn dt(ts: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(ts)
        .unwrap()
        .with_timezone(&Utc)
}

fn hist_status(ts: &str, old_value: &str, new_value: &str) -> HistoryEntry {
    HistoryEntry {
        id: Uuid::now_v7(),
        author: "test".to_string(),
        timestamp: dt(ts),
        changes: vec![FieldChange {
            field_name: "status".to_string(),
            old_value: old_value.to_string(),
            new_value: new_value.to_string(),
        }],
    }
}

#[test]
fn merged_pr_and_completed_spec_is_shipped() {
    match reconcile_verdict(Some(94), true, "BUG-233") {
        PhaseReconcile::ShippedOutOfBand { reason } => {
            assert!(reason.contains("PR-94"), "{reason}");
            assert!(reason.contains("BUG-233"), "{reason}");
        }
        other => panic!("expected ShippedOutOfBand, got {other:?}"),
    }
}

#[test]
fn verified_merged_pr_alone_is_shipped() {
    // Instance A: the human merged out-of-band; the status auto-bump may
    // lag, so a merged PR that credits the dispatched spec is proof
    // enough.
    assert!(matches!(
        reconcile_verdict(Some(94), false, "BUG-233"),
        PhaseReconcile::ShippedOutOfBand { .. }
    ));
}

#[test]
fn completed_spec_with_no_pr_is_shipped() {
    // Instance B: resolved-by-supersession — no PR was ever needed.
    assert!(matches!(
        reconcile_verdict(None, true, "BUG-230"),
        PhaseReconcile::ShippedOutOfBand { .. }
    ));
}

#[test]
fn no_pr_and_open_spec_is_a_genuine_failure() {
    // The regression guard: reality confirms nothing shipped.
    assert_eq!(
        reconcile_verdict(None, false, "BUG-241"),
        PhaseReconcile::GenuineFailure
    );
}

#[test]
fn latest_reopen_transition_uses_terminal_to_open_status_change() {
    let mut req = Requirement::new("reopened".to_string(), String::new());
    req.history = vec![
        hist_status("2026-09-10T12:00:00Z", "In Progress", "Completed"),
        hist_status("2026-09-11T12:00:00Z", "Completed", "Approved"),
        hist_status("2026-09-12T12:00:00Z", "Rejected", "In Progress"),
    ];

    let reopened = latest_reopen_transition_at(&req).expect("reopen detected");
    assert_eq!(reopened.to_rfc3339(), "2026-09-12T12:00:00+00:00");
}

#[test]
fn merged_pr_must_be_newer_than_latest_reopen() {
    let reopen = dt("2026-09-12T12:00:00Z");
    let before = dt("2026-09-12T11:59:59Z");
    let after = dt("2026-09-12T12:00:01Z");

    assert!(!merged_at_satisfies_latest_reopen(
        Some(before),
        Some(reopen)
    ));
    assert!(!merged_at_satisfies_latest_reopen(None, Some(reopen)));
    assert!(merged_at_satisfies_latest_reopen(Some(after), Some(reopen)));
    assert!(merged_at_satisfies_latest_reopen(None, None));
}
