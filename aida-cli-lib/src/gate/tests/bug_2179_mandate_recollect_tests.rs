//! BUG-2179: the mandate cover re-collects its store evidence when a
//! concurrent writer changes the store mid-collection, a bounded number of
//! times, then refuses (fail closed).
//!
//! TASK-2019 (folded into BUG-2195, TASK-2067): a dirty-store snapshot is
//! retried within the same bound instead of refusing at once.
// trace:BUG-2179 | ai:claude
// trace:TASK-2067 | ai:antigravity

use super::{collect_on_stable_store, MANDATE_COLLECT_ATTEMPTS};
use crate::gate::mandate::MandateRefusal;
use std::cell::Cell;
use std::time::Duration;

/// A store snapshot source that returns `versions` in order, then repeats the
/// last one. `None` is a dirty store.
fn snapshots(versions: Vec<Option<u32>>) -> impl FnMut() -> Result<Option<u32>, MandateRefusal> {
    let mut next = 0;
    move || {
        let v = versions[next.min(versions.len() - 1)];
        next += 1;
        Ok(v)
    }
}

fn equal(a: &u32, b: &u32) -> bool {
    a == b
}

#[test]
fn a_store_that_changes_once_is_recollected_and_the_ship_proceeds() {
    let collections = Cell::new(0u32);
    // Attempt 1 sees 1 -> 2 (a write landed mid-collection); attempt 2 sees 2 -> 2.
    let out = collect_on_stable_store(
        snapshots(vec![Some(1), Some(2), Some(2), Some(2)]),
        equal,
        || {
            collections.set(collections.get() + 1);
            collections.get()
        },
        Duration::ZERO,
    );
    assert_eq!(out.ok(), Some(2), "the second, stable collection is used");
    assert_eq!(collections.get(), 2);
}

#[test]
fn a_store_that_changes_on_every_attempt_is_refused_after_the_bound() {
    let collections = Cell::new(0u32);
    let mut version = 0u32;
    let out = collect_on_stable_store(
        || {
            version += 1;
            Ok(Some(version))
        },
        equal,
        || collections.set(collections.get() + 1),
        Duration::ZERO,
    );
    assert_eq!(collections.get(), MANDATE_COLLECT_ATTEMPTS);
    match out {
        Err(MandateRefusal::TraceEvidenceUnreadable(why)) => {
            assert!(why.contains("the store changed"), "{why}");
            assert!(
                why.contains(&format!("{MANDATE_COLLECT_ATTEMPTS} attempts")),
                "{why}"
            );
        }
        other => panic!("expected a TraceEvidenceUnreadable refusal, got {other:?}"),
    }
}

#[test]
fn an_unreadable_snapshot_refuses_without_collecting() {
    let collections = Cell::new(0u32);
    let out: Result<(), _> = collect_on_stable_store(
        || -> Result<Option<u32>, MandateRefusal> {
            Err(MandateRefusal::TraceEvidenceUnreadable("no store".into()))
        },
        equal,
        || collections.set(collections.get() + 1),
        Duration::ZERO,
    );
    assert!(matches!(
        out,
        Err(MandateRefusal::TraceEvidenceUnreadable(_))
    ));
    assert_eq!(collections.get(), 0);
}

/// BUG-2195: the stability rule, not byte equality, decides whether an
/// attempt is kept. Two different snapshots the rule calls stable are used.
// trace:TASK-2067 | ai:antigravity
#[test]
fn snapshots_the_rule_calls_stable_keep_the_first_collection() {
    let collections = Cell::new(0u32);
    let out = collect_on_stable_store(
        snapshots(vec![Some(1), Some(2)]),
        |_, _| true,
        || {
            collections.set(collections.get() + 1);
            collections.get()
        },
        Duration::ZERO,
    );
    assert_eq!(out.ok(), Some(1));
    assert_eq!(collections.get(), 1);
}

/// TASK-2019: a store dirty at the first snapshot (a concurrent write in
/// flight) is retried, and the next clean, stable attempt is used.
// trace:TASK-2067 | ai:antigravity
#[test]
fn a_dirty_store_is_retried_within_the_bound() {
    let collections = Cell::new(0u32);
    let out = collect_on_stable_store(
        snapshots(vec![None, Some(7), Some(7)]),
        equal,
        || {
            collections.set(collections.get() + 1);
            collections.get()
        },
        Duration::ZERO,
    );
    assert_eq!(out.ok(), Some(1), "the clean attempt is used");
    assert_eq!(collections.get(), 1, "the dirty attempt collects nothing");
}

/// TASK-2019: a store that turns dirty mid-collection discards that attempt
/// and retries.
// trace:TASK-2067 | ai:antigravity
#[test]
fn a_store_dirtied_mid_collection_is_recollected() {
    let collections = Cell::new(0u32);
    let out = collect_on_stable_store(
        snapshots(vec![Some(1), None, Some(2), Some(2)]),
        equal,
        || {
            collections.set(collections.get() + 1);
            collections.get()
        },
        Duration::ZERO,
    );
    assert_eq!(out.ok(), Some(2));
    assert_eq!(collections.get(), 2);
}

/// TASK-2019: a store dirty on every attempt still refuses after the bound
/// (fail closed), naming the uncommitted evidence.
// trace:TASK-2067 | ai:antigravity
#[test]
fn a_store_dirty_on_every_attempt_refuses_after_the_bound() {
    let snapshots_taken = Cell::new(0u32);
    let collections = Cell::new(0u32);
    let out = collect_on_stable_store(
        || -> Result<Option<u32>, MandateRefusal> {
            snapshots_taken.set(snapshots_taken.get() + 1);
            Ok(None)
        },
        equal,
        || collections.set(collections.get() + 1),
        Duration::ZERO,
    );
    assert_eq!(snapshots_taken.get(), MANDATE_COLLECT_ATTEMPTS);
    assert_eq!(collections.get(), 0);
    match out {
        Err(MandateRefusal::TraceEvidenceUnreadable(why)) => {
            assert!(why.contains("uncommitted evidence"), "{why}");
            assert!(
                why.contains(&format!("{MANDATE_COLLECT_ATTEMPTS} attempts")),
                "{why}"
            );
        }
        other => panic!("expected a TraceEvidenceUnreadable refusal, got {other:?}"),
    }
}
