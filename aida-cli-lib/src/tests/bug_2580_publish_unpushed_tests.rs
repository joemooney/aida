//! BUG-2580: a self-publishing implementer lane that exits 0 with a clean
//! seat and commits it never pushed is published by its launcher, instead of
//! ending `failed-after-start: no-new-published-head` with the work stranded.
// trace:BUG-2580 | ai:claude

use super::*;
use crate::headless_background::OriginHead;
use std::cell::Cell;

const LAUNCH: &str = "1111111111111111111111111111111111111111";
const WORK: &str = "2222222222222222222222222222222222222222";

fn seat(head: &str, origin: OriginHead, uncommitted: usize) -> SeatState {
    SeatState {
        head: Some(head.to_string()),
        origin,
        uncommitted: Some(uncommitted),
        head_has_unpushed: Some(true),
    }
}

fn published() -> Outcome {
    Outcome::PrOpened {
        pr: 9,
        head: Some(WORK.to_string()),
    }
}

/// Run the decision the way `run_recorded` does: settle the seat, then
/// publish. Returns the outcome and whether the publish route was taken.
fn run(outcome: Outcome, success: bool, state: &SeatState, publish_ok: bool) -> (Outcome, bool) {
    let launch = LaunchSeat {
        pr: None,
        head: Some(LAUNCH),
    };
    let (_, unshipped) = settle_outcome(outcome.clone(), &launch, state);
    let called = Cell::new(false);
    let out = publish_unpushed(
        outcome,
        success,
        unshipped.as_ref(),
        || {
            called.set(true);
            anyhow::ensure!(publish_ok, "push refused");
            Ok(())
        },
        published,
    );
    (out, called.get())
}

#[test]
fn committed_but_unpushed_work_is_published_and_reported_as_a_new_head() {
    let state = seat(WORK, OriginHead::Absent, 0);
    assert_eq!(run(Outcome::NoPr, true, &state, true), (published(), true));

    // The published outcome is what the attempt records as pushed.
    let tmp = tempfile::tempdir().unwrap();
    let attempt = crate::lane_result::Attempt::begin(tmp.path(), "BUG-test", Some("work")).unwrap();
    attempt
        .bind("BUG-test", "work", None, Some(LAUNCH))
        .unwrap();
    attempt.started(123);
    let kind = attempt
        .finish_outcome(&published(), true, false, false)
        .unwrap();
    assert_eq!(kind, crate::lane_result::Kind::Pushed);
}

#[test]
fn a_rework_round_that_committed_on_its_old_pr_head_is_published() {
    let launch_pr = (9, Some(LAUNCH.to_string()));
    let launch = LaunchSeat {
        pr: Some(&launch_pr),
        head: Some(LAUNCH),
    };
    let stale = Outcome::PrOpened {
        pr: 9,
        head: Some(LAUNCH.to_string()),
    };
    let state = seat(WORK, OriginHead::At(LAUNCH.to_string()), 0);
    let (_, unshipped) = settle_outcome(stale.clone(), &launch, &state);
    let out = publish_unpushed(stale, true, unshipped.as_ref(), || Ok(()), published);
    assert_eq!(out, published());
}

#[test]
fn a_dirty_seat_a_failed_child_a_punt_and_a_hold_are_left_alone() {
    let unpushed = seat(WORK, OriginHead::Absent, 0);
    let dirty = seat(WORK, OriginHead::Absent, 3);
    let unmoved = seat(LAUNCH, OriginHead::Absent, 0);
    let punted = Outcome::Punted {
        reason: "blocked".into(),
    };
    let held = Outcome::Held { branch: "b".into() };
    for (why, outcome, success, state) in [
        ("dirty seat", Outcome::NoPr, true, &dirty),
        ("child failed", Outcome::NoPr, false, &unpushed),
        ("no new commits", Outcome::NoPr, true, &unmoved),
        ("punt", punted, true, &unpushed),
        ("hold", held, true, &unpushed),
    ] {
        assert_eq!(
            run(outcome.clone(), success, state, true),
            (outcome, false),
            "{why}"
        );
    }
    // The dirty seat keeps the report it has today.
    let launch = LaunchSeat {
        pr: None,
        head: Some(LAUNCH),
    };
    let (settled, work) = settle_outcome(Outcome::NoPr, &launch, &dirty);
    assert_eq!(
        report(&settled, Some(0), work.as_ref()).0,
        "implementer: no-pr reason=uncommitted-work uncommitted=3"
    );
}

#[test]
fn a_failed_publish_keeps_the_failure_and_the_retry_command_names_the_seat() {
    let state = seat(WORK, OriginHead::Absent, 0);
    assert_eq!(
        run(Outcome::NoPr, true, &state, false),
        (Outcome::NoPr, true)
    );
    assert_eq!(
        seat_publish_command("BUG-7", Path::new("/lanes/aida-seat-x")),
        "aida ship BUG-7 --from-seat /lanes/aida-seat-x"
    );
}

#[test]
fn a_seat_with_no_new_commits_ends_no_change_not_unpushed() {
    let out = classify_no_change(Outcome::NoPr, true, || Ok(false)).unwrap();
    assert_eq!(out, Outcome::NoChange);
    assert_eq!(
        report(&out, Some(0), None),
        ("implementer: no-change".into(), 0)
    );
}

/// Review finding 2 (AC4): a launcher publish that pushed but could not open
/// the PR leaves nothing "unpushed", and the error must still name the seat
/// and the command that finishes the publish.
#[test]
fn the_error_names_the_retry_command_after_any_failed_seat_publish() {
    let seat = Path::new("/lanes/aida-seat-x");
    let retry = "`aida ship BUG-7 --from-seat /lanes/aida-seat-x`";
    let pushed_no_pr = unpublished_note(None, true, true, "BUG-7", seat);
    assert!(pushed_no_pr.contains(retry), "{pushed_no_pr}");
    assert!(
        pushed_no_pr.contains("/lanes/aida-seat-x;"),
        "{pushed_no_pr}"
    );
    let unpushed = unpublished_note(Some(&Unshipped::UnpushedCommits), true, true, "BUG-7", seat);
    assert!(unpushed.contains(retry), "{unpushed}");
    // Nothing to retry: no failed publish, a dirty seat, a plain worktree.
    assert_eq!(unpublished_note(None, true, false, "BUG-7", seat), "");
    assert_eq!(unpublished_note(None, false, true, "BUG-7", seat), "");
    assert_eq!(
        unpublished_note(
            Some(&Unshipped::UncommittedWork(2)),
            true,
            false,
            "BUG-7",
            seat
        ),
        ", leaving 2 uncommitted path(s) in its worktree"
    );
    assert_eq!(
        unpublished_note(
            Some(&Unshipped::UnpushedCommits),
            false,
            false,
            "BUG-7",
            seat
        ),
        ", leaving commits it did not push in its worktree"
    );
}

/// Review finding 1 (AC2): a dirty seat with no commits is still a stall
/// (`no-pr reason=uncommitted-work`), never `no-change`.
#[test]
fn a_dirty_seat_with_no_commits_is_not_no_change() {
    let no_commits = |o| classify_no_change(o, true, || Ok(false));
    let launch = LaunchSeat {
        pr: None,
        head: Some(LAUNCH),
    };
    let dirty = seat(LAUNCH, OriginHead::Absent, 2);
    let out = no_change_only_when_clean(Outcome::NoPr, &dirty, no_commits).unwrap();
    assert_eq!(out, Outcome::NoPr);
    let (settled, work) = settle_outcome(out, &launch, &dirty);
    assert_eq!(
        report(&settled, Some(0), work.as_ref()),
        (
            "implementer: no-pr reason=uncommitted-work uncommitted=2".to_string(),
            NO_PR_EXIT_CODE
        )
    );
    // Unreadable is not clean either.
    let mut unreadable = seat(LAUNCH, OriginHead::Absent, 0);
    unreadable.uncommitted = None;
    assert_eq!(
        no_change_only_when_clean(Outcome::NoPr, &unreadable, no_commits).unwrap(),
        Outcome::NoPr
    );
    let clean = seat(LAUNCH, OriginHead::Absent, 0);
    assert_eq!(
        no_change_only_when_clean(Outcome::NoPr, &clean, no_commits).unwrap(),
        Outcome::NoChange
    );
}
