//! BUG-2705: a self-publishing (Claude) lane in a seat clone that exits
//! cleanly with its edits uncommitted gets the host post-exit commit before
//! its seat is published and before the fast gate requires a clean committed
//! seat. When the host does not commit, the lane says exactly why.
// trace:BUG-2705 | ai:claude

use super::seat_publish_tests::{
    git_ok, no_commit, opts, seat_with_publication_pending, test_bundle, test_commit,
};
use super::tests::fixture;
use crate::headless_background::{OriginHead, SeatState};
use crate::ship_from_seat::PostExitCommit;
use crate::standalone_implementer::{
    commit_leftovers, self_published_seat_exit, LaunchSeat, LeftoverCommit, Outcome, SeatExit,
};
use std::cell::{Cell, RefCell};
use std::path::Path;

/// The seat as host git sees it. The production reader probes a recorded
/// clone through bwrap, which a unit test cannot rely on.
fn host_seat(origin: &Path, clone: &Path, branch: &str) -> SeatState {
    let head = git_ok(clone, &["rev-parse", "HEAD"]);
    let remote = crate::test_git::git_command()
        .arg("-C")
        .arg(origin)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .output()
        .unwrap();
    let origin = if remote.status.success() {
        OriginHead::At(String::from_utf8_lossy(&remote.stdout).trim().to_string())
    } else {
        OriginHead::Absent
    };
    let pushed = matches!(&origin, OriginHead::At(at) if *at == head);
    let status = git_ok(clone, &["status", "--porcelain"]);
    SeatState {
        head: Some(head),
        origin,
        uncommitted: Some(status.lines().filter(|l| !l.trim().is_empty()).count()),
        head_has_unpushed: Some(!pushed),
    }
}

/// The exit sequence a self-publishing seat-clone lane runs: its uncommitted
/// edits are committed by the host as a `[AI:claude]` commit, that commit is
/// published, and the fast gate's head check runs against the committed,
/// published head.
#[test]
fn bug_2705_claude_seat_clone_leftovers_are_committed_published_and_gated() {
    let fx = fixture();
    let branch = "claude-rework";
    let clone = seat_with_publication_pending(&fx, "leftover", "c0ffee270501", branch);
    let launch = git_ok(&clone, &["rev-parse", "HEAD"]);
    assert_ne!(git_ok(&clone, &["status", "--porcelain"]), "");
    let launch_pr = (9, Some(launch.clone()));
    let launch_seat = LaunchSeat {
        pr: Some(&launch_pr),
        head: Some(&launch),
    };
    let forge = crate::forge::fake::RecordingForge {
        opens_as: Some(77),
        ..crate::forge::fake::RecordingForge::new()
    };
    let published = Cell::new(false);
    let (outcome, note) = self_published_seat_exit(
        // A rework round: the PR is still at the launch head.
        Outcome::PrOpened {
            pr: 9,
            head: Some(launch.clone()),
        },
        true,
        &launch_seat,
        &SeatExit {
            read_seat: &|| host_seat(&fx.origin, &clone, branch),
            post_exit: &|| {
                crate::ship_from_seat::post_exit_commit_as(
                    &fx.main,
                    &clone,
                    "BUG-1",
                    "lane-2705",
                    true,
                    Some("claude"),
                    &test_commit,
                )
            },
            publish: &|| {
                published.set(true);
                crate::ship_from_seat::run_from(
                    &fx.main,
                    opts(&clone, false, false),
                    &crate::ship_from_seat::Deps {
                        bundle: &test_bundle,
                        commit: &no_commit,
                        forge: &|_| Box::new(forge.clone()) as Box<dyn crate::forge::Forge>,
                        after_open: &|_, _| {},
                    },
                )
            },
            republished: &|| Outcome::PrOpened {
                pr: 77,
                head: Some(git_ok(
                    &fx.origin,
                    &["rev-parse", &format!("refs/heads/{branch}")],
                )),
            },
            no_change: &|outcome| Ok(outcome),
        },
    )
    .unwrap();

    assert_eq!(note, None, "a committed seat leaves no unresolved reason");
    let tip = git_ok(&clone, &["rev-parse", "HEAD"]);
    assert_ne!(tip, launch, "the host made no commit");
    assert_eq!(git_ok(&clone, &["status", "--porcelain"]), "");
    let subject = git_ok(&clone, &["log", "-1", "--format=%s"]);
    assert!(subject.starts_with("[AI:claude] "), "{subject}");
    assert!(subject.ends_with("(BUG-1)"), "{subject}");
    assert!(published.get(), "the committed seat was not published");
    assert_eq!(
        git_ok(&fx.origin, &["rev-parse", &format!("refs/heads/{branch}")]),
        tip
    );
    assert_eq!(
        outcome,
        Outcome::PrOpened {
            pr: 77,
            head: Some(tip.clone())
        }
    );
    assert_eq!(forge.opened.lock().unwrap().len(), 1);

    // The pushed fast gate now binds to the committed, published head and
    // runs, instead of refusing a dirty seat.
    let gated = RefCell::new(None);
    let (head, marker, _) = crate::lane_fast_gate::pushed_head_attempt(
        &host_seat(&fx.origin, &clone, branch).head.unwrap(),
        &tip,
        || {
            *gated.borrow_mut() = Some(git_ok(&clone, &["rev-parse", "HEAD"]));
            Ok((tip.clone(), "pass".into(), String::new()))
        },
    )
    .unwrap();
    assert_eq!(gated.into_inner().as_deref(), Some(tip.as_str()));
    assert_eq!((head.as_str(), marker.as_str()), (tip.as_str(), "pass"));
}

/// A failed exit's salvage commit is tagged with the lane's own vendor and
/// keeps its `(Refs SPEC)` trailer.
#[test]
fn bug_2705_post_exit_salvage_is_tagged_with_the_lanes_vendor() {
    let fx = fixture();
    let clone = seat_with_publication_pending(&fx, "salvage2705", "c0ffee270502", "salvage-2705");
    let outcome = crate::ship_from_seat::post_exit_commit_as(
        &fx.main,
        &clone,
        "BUG-1",
        "lane-2705",
        false,
        Some("claude"),
        &test_commit,
    );
    assert!(
        matches!(outcome, PostExitCommit::Committed(_)),
        "{outcome:?}"
    );
    let subject = git_ok(&clone, &["log", "-1", "--format=%s"]);
    assert!(
        subject.starts_with("[AI:claude] chore(seat): salvage work "),
        "{subject}"
    );
    assert!(subject.ends_with("(Refs BUG-1)"), "{subject}");
}

const LAUNCH: &str = "1111111111111111111111111111111111111111";
const WORK: &str = "2222222222222222222222222222222222222222";

fn seat(head: &str, origin: OriginHead, uncommitted: Option<usize>) -> SeatState {
    SeatState {
        head: Some(head.into()),
        origin,
        uncommitted,
        head_has_unpushed: Some(true),
    }
}

/// Each case that leaves the edits alone says why; none runs the commit.
#[test]
fn bug_2705_leftovers_the_host_does_not_commit_carry_their_reason() {
    let never = || -> PostExitCommit { panic!("the host commit ran although it was due to skip") };
    let dirty = seat(LAUNCH, OriginHead::At(LAUNCH.into()), Some(2));
    let pushed_dirty = seat(WORK, OriginHead::At(WORK.into()), Some(2));
    for (outcome, success, state, why) in [
        (Outcome::NoPr, false, &dirty, "the lane exited non-zero"),
        (
            Outcome::NoPr,
            true,
            &pushed_dirty,
            "already published this round",
        ),
        (
            Outcome::NoPr,
            true,
            &seat(LAUNCH, OriginHead::Unknown, None),
            "could not be read",
        ),
    ] {
        let result = commit_leftovers(&outcome, success, state, Some(LAUNCH), never);
        let reason = result.unresolved().expect("a skipped commit has a reason");
        assert!(
            reason.starts_with("host post-exit commit skipped: "),
            "{reason}"
        );
        assert!(reason.contains(why), "{reason}");
    }
    for outcome in [
        Outcome::Punted {
            reason: "blocked".into(),
        },
        Outcome::Held { branch: "b".into() },
        Outcome::NoChange,
    ] {
        assert_eq!(
            commit_leftovers(&outcome, true, &dirty, Some(LAUNCH), never),
            LeftoverCommit::NotNeeded
        );
    }
    let clean = seat(LAUNCH, OriginHead::At(LAUNCH.into()), Some(0));
    assert_eq!(
        commit_leftovers(&Outcome::NoPr, true, &clean, Some(LAUNCH), never),
        LeftoverCommit::NotNeeded
    );
}

/// A refused host commit leaves the seat unpublished and its reason (the
/// git/preflight error) reaches the exit, instead of a bare gate refusal.
#[test]
fn bug_2705_failed_host_commit_reason_reaches_the_exit() {
    let dirty = seat(LAUNCH, OriginHead::At(LAUNCH.into()), Some(1));
    let launch_seat = LaunchSeat {
        pr: None,
        head: Some(LAUNCH),
    };
    let (outcome, note) = self_published_seat_exit(
        Outcome::NoPr,
        true,
        &launch_seat,
        &SeatExit {
            read_seat: &|| dirty.clone(),
            post_exit: &|| PostExitCommit::Failed("index.lock exists\nretry later".into()),
            publish: &|| panic!("a dirty seat must not be published"),
            republished: &|| panic!("nothing was published"),
            no_change: &|_| panic!("a dirty seat is never no-change"),
        },
    )
    .unwrap();
    assert_eq!(outcome, Outcome::NoPr);
    assert_eq!(
        note.as_deref(),
        Some("host post-exit commit failed: index.lock exists retry later")
    );
}
