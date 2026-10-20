//! BUG-2179: the mandate cover re-collects its store evidence when a
//! concurrent writer changes the store mid-collection, a bounded number of
//! times, then refuses (fail closed).
// trace:BUG-2179 | ai:claude

use super::{collect_on_stable_store, MANDATE_COLLECT_ATTEMPTS};
use crate::mandate::MandateRefusal;
use std::cell::Cell;

/// A store snapshot source that returns `versions` in order, then repeats the
/// last one.
fn snapshots(versions: Vec<u32>) -> impl FnMut() -> Result<u32, MandateRefusal> {
    let mut next = 0;
    move || {
        let v = versions[next.min(versions.len() - 1)];
        next += 1;
        Ok(v)
    }
}

#[test]
fn a_store_that_changes_once_is_recollected_and_the_ship_proceeds() {
    let collections = Cell::new(0u32);
    // Attempt 1 sees 1 -> 2 (a write landed mid-collection); attempt 2 sees 2 -> 2.
    let out = collect_on_stable_store(snapshots(vec![1, 2, 2, 2]), || {
        collections.set(collections.get() + 1);
        collections.get()
    });
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
            Ok(version)
        },
        || collections.set(collections.get() + 1),
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
        || -> Result<u32, MandateRefusal> {
            Err(MandateRefusal::TraceEvidenceUnreadable("no store".into()))
        },
        || collections.set(collections.get() + 1),
    );
    assert!(matches!(
        out,
        Err(MandateRefusal::TraceEvidenceUnreadable(_))
    ));
    assert_eq!(collections.get(), 0);
}
