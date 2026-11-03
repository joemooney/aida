// A notified stale overlapping head whose author is gone is no longer
// parked forever: the watcher updates the branch, or routes it to the
// advisor, and `aida awaiting` lists every parked change with its reason.
// trace:BUG-2426 | ai:claude
use super::*;
use std::cell::Cell;

fn fixture() -> (tempfile::TempDir, Store) {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path(), "github:github.com/owner/repo").unwrap();
    (temp, store)
}

fn change(head: u64, overlap: bool) -> forge::Change {
    forge::Change {
        id: 7,
        head: format!("{head:040x}"),
        base: "main".into(),
        author: "writer".into(),
        gitlab: false,
        overlap: Some(overlap),
        behind: Some(u64::from(overlap)),
        head_branch: Some("lane-branch".into()),
    }
}

/// The watcher's Notify arm: deliver the stale-head notice and record it sent.
fn notify(root: &Path, store: &Store, c: &forge::Change) {
    let Action::Notify(notice) = select(store, c, None, 2).unwrap() else {
        panic!("a stale overlapping head is notified first")
    };
    deliver(root, &notice).unwrap();
    store
        .transaction(&c.key(), |state| {
            state
                .entries
                .get_mut(&c.key())
                .unwrap()
                .notices
                .get_mut(&c.head)
                .unwrap()
                .sent = true;
            Ok(())
        })
        .unwrap();
}

/// One watcher pass over a change whose base probe is already known: the
/// follow-up, then admission, recording the skip as `watch_change` does.
fn pass(
    root: &Path,
    store: &Store,
    c: &forge::Change,
    live: update::Liveness<'_>,
    updater: update::Updater<'_>,
) -> Option<Action> {
    if let Some(reason) = update::follow_up(root, store, c, live, updater).unwrap() {
        record_skip(store, c, Some(&reason)).unwrap();
        return Some(Action::Skip(reason));
    }
    let action = select(store, c, None, 2).unwrap();
    record_skip(
        store,
        c,
        match &action {
            Action::Skip(r) => Some(r.as_str()),
            _ => None,
        },
    )
    .unwrap();
    Some(action)
}

// AC1 + AC4: a dead author's stale overlapping head gets exactly one
// update-branch, and the merged head is reviewed on the next tick.
#[test]
fn bug2426_dead_author_stale_head_is_updated_then_reviewed_next_tick() {
    let (temp, store) = fixture();
    let stale = change(1, true);
    notify(temp.path(), &store, &stale);
    let updates = Cell::new(0);
    let dead = |_: &forge::Change| Some(false);
    let forge_ok = |c: &forge::Change| {
        assert_eq!(c.id, 7);
        updates.set(updates.get() + 1);
        Ok(())
    };
    let Some(Action::Skip(reason)) = pass(temp.path(), &store, &stale, &dead, &forge_ok) else {
        panic!("the stale head itself is not reviewed")
    };
    assert_eq!(
        updates.get(),
        1,
        "update-branch is invoked for a dead author"
    );
    assert!(reason.starts_with(update::STALE_UPDATED), "{reason}");
    // The forge has not reported the new head yet: no second update.
    pass(temp.path(), &store, &stale, &dead, &forge_ok);
    assert_eq!(updates.get(), 1, "one update per head");
    // Next tick: the forge reports the merged head, current with its base.
    let merged = change(2, false);
    let action = pass(temp.path(), &store, &merged, &dead, &forge_ok).unwrap();
    let attempt = match action {
        Action::Dispatch(attempt) => attempt,
        other => panic!("the merged head must be reviewed: {other:?}"),
    };
    assert_eq!(attempt.head, merged.head);
    assert_eq!(updates.get(), 1);
    let entry = &store.load().unwrap().entries[&merged.key()];
    assert!(entry.update.is_none(), "the old head's update is moot");
    assert!(entry.skipped.is_none(), "a dispatched head is not parked");
}

// A live author owns the merge: the watcher does not touch the branch.
#[test]
fn bug2426_live_or_unknown_author_branch_is_not_updated() {
    let (temp, store) = fixture();
    let stale = change(1, true);
    notify(temp.path(), &store, &stale);
    let never = |_: &forge::Change| -> std::result::Result<(), String> {
        panic!("a live author's branch must not be updated")
    };
    let Some(Action::Skip(reason)) = pass(temp.path(), &store, &stale, &|_| Some(true), &never)
    else {
        panic!("skip expected")
    };
    assert!(reason.contains("is live"), "{reason}");
    let Some(Action::Skip(reason)) = pass(temp.path(), &store, &stale, &|_| None, &never) else {
        panic!("skip expected")
    };
    assert!(reason.contains("branch is unknown"), "{reason}");
}

// AC2 + AC3: a refused update routes the change to the advisor once, and
// the parked change is listed with that reason.
#[test]
fn bug2426_conflicting_update_routes_to_the_advisor_and_is_listed() {
    let (temp, store) = fixture();
    let stale = change(1, true);
    notify(temp.path(), &store, &stale);
    let conflicts = |_: &forge::Change| -> std::result::Result<(), String> {
        Err("update-branch conflicts: merge conflict between base and head".into())
    };
    let dead = |_: &forge::Change| Some(false);
    for _ in 0..2 {
        let Some(Action::Skip(reason)) = pass(temp.path(), &store, &stale, &dead, &conflicts)
        else {
            panic!("skip expected")
        };
        assert!(reason.contains("update-branch conflicts"), "{reason}");
        assert!(reason.contains("routed to the advisor"), "{reason}");
    }
    let messages = aida_core::mailbox::read_local_messages(temp.path()).unwrap();
    let routed: Vec<_> = messages
        .iter()
        .filter(|m| m.to == aida_core::mailbox::Recipient::Agent("advisor".into()))
        .filter(|m| {
            m.subject
                .as_deref()
                .is_some_and(|s| s.contains("update-branch"))
        })
        .collect();
    assert_eq!(routed.len(), 1, "routed once: {messages:?}");
    assert_eq!(
        routed[0].subject.as_deref(),
        Some("PR-7 behind `main` with overlap; update-branch conflicts")
    );
    let skipped = skipped_changes(temp.path());
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].0, "PR-7");
    assert_eq!(skipped[0].1, stale.head);
    assert!(skipped[0].2.contains("update-branch conflicts"));
    // The awaiting channel reads the same record.
    let items = crate::awaiting_you::WatchSkippedItem::from_watcher(temp.path());
    assert_eq!(items.len(), 1);
    let report = crate::awaiting_you::AwaitingReport {
        review_watch_skipped: items,
        ..Default::default()
    };
    let mut out = Vec::new();
    report.render(false, &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("PR-7"), "{text}");
    assert!(text.contains("update-branch conflicts"), "{text}");
    assert_eq!(
        report.to_json()["review_watch_skipped"][0]["change"],
        "PR-7"
    );
}

// AC3: other parking skips are listed too; a reviewed or in-flight head is not.
#[test]
fn bug2426_parking_skips_are_recorded_and_cleared() {
    assert!(parks(
        "two-round review cap reached; route further work to the advisor"
    ));
    assert!(parks("review of this head exited without completing (x)"));
    assert!(parks(update::STALE_NOTIFIED));
    assert!(!parks("head already has a recorded review"));
    assert!(!parks("head already reserved (Running)"));
    assert!(!parks("global reviewer capacity (2) is held by PR-1"));
    let (temp, store) = fixture();
    let c = change(3, false);
    record_skip(&store, &c, Some("two-round review cap reached")).unwrap();
    let first = skipped_changes(temp.path());
    assert_eq!(first.len(), 1);
    // A steady skip keeps its first-seen time.
    record_skip(&store, &c, Some("two-round review cap reached")).unwrap();
    assert_eq!(skipped_changes(temp.path())[0].3, first[0].3);
    record_skip(&store, &c, Some("head already has a recorded review")).unwrap();
    assert!(skipped_changes(temp.path()).is_empty());
}
