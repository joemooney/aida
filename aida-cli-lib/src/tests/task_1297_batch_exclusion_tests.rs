//! TASK-1297: unit tests for the pure "M other approved specs routed to this
//! role are not in this batch" count derivation — the fix for a `--batch`
//! drain that reports only what it drained and stays silent about approved,
//! role-routed work its filter excluded. The 2026-09-19 incident: a batch of
//! 3 open members was re-driven every wave for hours while 16 approved specs
//! routed to the same role sat outside the filter, invisible.
//!
//! trace:TASK-1297 | ai:claude

use super::{batch_exclusion_clause, count_routed_excluded_from_batch, RoutedSpecSnapshot};
use aida_core::RequirementStatus;

fn member(tag: &str) -> RoutedSpecSnapshot {
    RoutedSpecSnapshot {
        tags: vec![tag.to_string()],
        status: RequirementStatus::Approved,
    }
}

fn routed_approved() -> RoutedSpecSnapshot {
    RoutedSpecSnapshot {
        tags: vec![],
        status: RequirementStatus::Approved,
    }
}

fn routed_with_status(status: RequirementStatus) -> RoutedSpecSnapshot {
    RoutedSpecSnapshot {
        tags: vec![],
        status,
    }
}

// AC: a batch of 3 members with 16 other approved specs routed to the same
// role outside the batch reports exactly 16 — the exact shape of the
// 2026-09-19 incident (`batch:followups-0918h` had 3 open members while 16
// approved followups-0918i + BUG-1283 specs sat invisible).
// trace:TASK-1297 | ai:claude
#[test]
fn reports_sixteen_when_three_members_and_sixteen_other_approved_routed_specs() {
    let want_tag = "batch:followups-0918h";
    let mut snapshots: Vec<RoutedSpecSnapshot> = (0..3).map(|_| member(want_tag)).collect();
    snapshots.extend((0..16).map(|_| routed_approved()));
    assert_eq!(count_routed_excluded_from_batch(&snapshots, want_tag), 16);
}

/// AC: `M == 0` when every routed-and-approved spec is a batch member — the
/// filter excluded nothing, so the line renders no extra noise.
#[test]
fn zero_when_nothing_is_excluded() {
    let want_tag = "batch:solo";
    let snapshots = vec![member(want_tag), member(want_tag)];
    assert_eq!(count_routed_excluded_from_batch(&snapshots, want_tag), 0);
}

/// A routed-but-not-approved spec (Draft, InProgress, Done, …) outside the
/// batch must NOT inflate the count — only `Approved` counts, per the
/// acceptance wording ("M other APPROVED specs").
#[test]
fn non_approved_routed_specs_outside_the_batch_are_not_counted() {
    let want_tag = "batch:x";
    let snapshots = vec![
        member(want_tag),
        routed_with_status(RequirementStatus::Draft),
        routed_with_status(RequirementStatus::InProgress),
        routed_with_status(RequirementStatus::Done),
        routed_approved(),
    ];
    assert_eq!(count_routed_excluded_from_batch(&snapshots, want_tag), 1);
}

/// Batch-tag matching is case-insensitive, mirroring the member-resolution
/// tag check in `resolve_batch_members_with_context` — a differently-cased
/// `batch:NAME` tag on a routed spec must still count it as IN the batch.
#[test]
fn batch_tag_match_is_case_insensitive() {
    let snapshots = vec![RoutedSpecSnapshot {
        tags: vec!["Batch:Followups-0918H".to_string()],
        status: RequirementStatus::Approved,
    }];
    assert_eq!(
        count_routed_excluded_from_batch(&snapshots, "batch:followups-0918h"),
        0
    );
}

/// A spec carrying several tags, one of which is the batch tag, is still a
/// member — the exclusion count must not double-count it just because it
/// also carries unrelated tags.
#[test]
fn member_with_multiple_tags_is_still_recognized_as_in_batch() {
    let want_tag = "batch:multi";
    let snapshots = vec![RoutedSpecSnapshot {
        tags: vec!["priority:high".to_string(), want_tag.to_string()],
        status: RequirementStatus::Approved,
    }];
    assert_eq!(count_routed_excluded_from_batch(&snapshots, want_tag), 0);
}

// --- batch_exclusion_clause: the operator-facing line ---

#[test]
fn clause_is_none_when_excluded_is_zero() {
    assert_eq!(batch_exclusion_clause(0, Some("implementer")), None);
}

#[test]
fn clause_names_the_role_and_pluralizes() {
    let line = batch_exclusion_clause(16, Some("implementer")).unwrap();
    assert_eq!(
        line,
        "16 other approved specs routed to role:implementer are not in this batch"
    );
}

#[test]
fn clause_singular_for_exactly_one() {
    let line = batch_exclusion_clause(1, Some("implementer")).unwrap();
    assert_eq!(
        line,
        "1 other approved spec routed to role:implementer is not in this batch"
    );
}

#[test]
fn clause_falls_back_when_role_unresolved() {
    let line = batch_exclusion_clause(2, None).unwrap();
    assert_eq!(
        line,
        "2 other approved specs routed to this role are not in this batch"
    );
}
