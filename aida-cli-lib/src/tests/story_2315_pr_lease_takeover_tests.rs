//! STORY-2315 — a relaunch ends a finished lane's dead lease itself when the
//! seat is clean and its HEAD is already in the PR, and refuses only when the
//! seat is dirty or has commits the PR lacks.
//!
//! The regression the spec reports: a lane shipped its PR and exited, leaving a
//! clean seat at the PR head; `queue work --steal` and the review launch then
//! stopped on "could not end dead lease … add --force". The git-backed tests
//! below rebuild that seat and show the check now clears it.
// trace:STORY-2315 | ai:claude

use super::*;
use std::path::Path;

fn never(_: &str, _: &str) -> bool {
    false
}

fn clean_at(head: &str) -> SeatReading {
    SeatReading::Read {
        dirty: Vec::new(),
        unpushed: Vec::new(),
        head: Some(head.to_string()),
    }
}

#[test]
fn live_owner_is_never_ended() {
    assert_eq!(
        classify(OwnerLiveness::Live, &clean_at("a"), Some("a"), never),
        PrLeaseCheck::Live
    );
}

#[test]
fn unprovable_death_keeps_the_previous_handling() {
    assert!(matches!(
        classify(OwnerLiveness::Unknown, &clean_at("a"), Some("a"), never),
        PrLeaseCheck::NotProvable(_)
    ));
}

#[test]
fn dead_lease_with_clean_seat_at_pr_head_is_ended() {
    assert_eq!(
        classify(OwnerLiveness::Dead, &clean_at("abc"), Some("abc"), never),
        PrLeaseCheck::SafeToForceEnd
    );
}

#[test]
fn dead_lease_whose_head_is_behind_the_pr_head_is_ended() {
    let check = classify(
        OwnerLiveness::Dead,
        &clean_at("old"),
        Some("new"),
        |seat, pr| seat == "old" && pr == "new",
    );
    assert_eq!(check, PrLeaseCheck::SafeToForceEnd);
}

#[test]
fn missing_seat_is_ended() {
    assert_eq!(
        classify(OwnerLiveness::Dead, &SeatReading::Missing, None, never),
        PrLeaseCheck::SafeToForceEnd
    );
}

#[test]
fn dirty_seat_refuses_naming_the_changes() {
    let seat = SeatReading::Read {
        dirty: vec![" M src/lib.rs".to_string()],
        unpushed: Vec::new(),
        head: Some("abc".to_string()),
    };
    match classify(OwnerLiveness::Dead, &seat, Some("abc"), never) {
        PrLeaseCheck::Refuse(reason) => {
            assert!(reason.contains("uncommitted"), "{reason}");
            assert!(reason.contains("src/lib.rs"), "{reason}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn seat_ahead_of_the_pr_refuses() {
    match classify(OwnerLiveness::Dead, &clean_at("ahead"), Some("pr"), never) {
        PrLeaseCheck::Refuse(reason) => {
            assert!(reason.contains("commits the PR head"), "{reason}")
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn clone_with_unpushed_commits_refuses() {
    let seat = SeatReading::Read {
        dirty: Vec::new(),
        unpushed: vec!["2 commit(s) on refs/heads/side".to_string()],
        head: Some("abc".to_string()),
    };
    match classify(OwnerLiveness::Dead, &seat, Some("abc"), never) {
        PrLeaseCheck::Refuse(reason) => assert!(reason.contains("refs/heads/side"), "{reason}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn unreadable_seat_is_not_judged() {
    // The spec refuses only dirty seats, commits the PR lacks, or a live
    // process: an unreadable seat keeps plain `session end`'s own guards.
    assert!(matches!(
        classify(
            OwnerLiveness::Dead,
            &SeatReading::Unreadable("timeout".into()),
            Some("a"),
            never
        ),
        PrLeaseCheck::NotProvable(_)
    ));
}

#[test]
fn no_pr_head_on_origin_is_not_judged() {
    assert!(matches!(
        classify(OwnerLiveness::Dead, &clean_at("a"), None, never),
        PrLeaseCheck::NotProvable(_)
    ));
}

#[test]
fn review_branches_parse_to_their_pr() {
    use crate::session_start::ReviewForge;
    assert_eq!(
        parse_review_branch("pr-42"),
        Some((ReviewForge::GitHub, 42))
    );
    assert_eq!(parse_review_branch("mr-7"), Some((ReviewForge::GitLab, 7)));
    assert_eq!(parse_review_branch("pr-"), None);
    assert_eq!(parse_review_branch("pr-12a"), None);
    assert_eq!(parse_review_branch("story-2315-work"), None);
}

// ── git-backed regression ──────────────────────────────────────────────────

fn git(dir: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@localhost",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str, msg: &str) -> String {
    std::fs::write(dir.join(file), msg).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-q", "--no-verify", "-m", msg]);
    git(dir, &["rev-parse", "HEAD"])
}

struct Fixture {
    _tmp: tempfile::TempDir,
    main: std::path::PathBuf,
    seat: std::path::PathBuf,
}

/// A main checkout with a bare origin and a seat worktree on `feat`, whose
/// branch was pushed: the state a lane leaves after opening its PR and exiting.
fn shipped_lane() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin.git");
    let main = tmp.path().join("main");
    let seat = tmp.path().join("seat");
    std::fs::create_dir_all(&main).unwrap();
    git(
        tmp.path(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    git(&main, &["init", "-q", "-b", "main"]);
    commit(&main, "README", "base");
    git(
        &main,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&main, &["push", "-q", "origin", "main"]);
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat",
            seat.to_str().unwrap(),
        ],
    );
    commit(&seat, "work.txt", "the lane's work");
    git(&seat, &["push", "-q", "origin", "feat"]);
    Fixture {
        _tmp: tmp,
        main,
        seat,
    }
}

fn seat_lease(seat: &Path) -> crate::SessionLease {
    crate::SessionLease {
        tree_kind: Default::default(),
        id: "lease-story-2315".into(),
        scope: "STORY-2315".into(),
        slug: "story-2315".into(),
        owner: "tester".into(),
        worktree_path: seat.to_path_buf(),
        branch: "feat".into(),
        started_at: chrono::Utc::now(),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: None,
        creator_pid_start_time: None,
        active_pid: None,
        active_pid_start_time: None,
        creator_pid_ns: crate::gate::review_authority::origin_pid_ns(),
        active_pid_ns: crate::gate::review_authority::origin_pid_ns(),
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
        manual_enter_at: None,
        interrupted_at: None,
        interrupted_reason: None,
        ending_retained_at: None,
        ending_retained_reason: None,
        driver: None,
        child_grant_id: None,
        vendor_session_id: None,
    }
}

/// The full read for a lease already known dead: seat, PR head, ancestry.
fn check_dead(fx: &Fixture) -> PrLeaseCheck {
    let lease = seat_lease(&fx.seat);
    let seat = read_seat(&lease, "feat");
    let pr_head = pr_head_on_origin(&fx.main, "feat");
    classify(OwnerLiveness::Dead, &seat, pr_head.as_deref(), |a, b| {
        is_ancestor(&fx.main, a, b)
    })
}

#[test]
fn clean_seat_at_the_pushed_pr_head_is_ended() {
    let fx = shipped_lane();
    assert_eq!(check_dead(&fx), PrLeaseCheck::SafeToForceEnd);
}

#[test]
fn clean_seat_behind_a_pr_that_moved_on_is_ended() {
    let fx = shipped_lane();
    // A rework pushed on top of the lane's work from elsewhere.
    let other = fx._tmp.path().join("other");
    git(
        &fx.main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            other.to_str().unwrap(),
            "feat",
        ],
    );
    commit(&other, "more.txt", "rework");
    git(&other, &["push", "-q", "origin", "HEAD:feat"]);
    assert_eq!(check_dead(&fx), PrLeaseCheck::SafeToForceEnd);
}

#[test]
fn seat_with_a_commit_the_pr_lacks_refuses() {
    let fx = shipped_lane();
    commit(&fx.seat, "late.txt", "never pushed");
    match check_dead(&fx) {
        PrLeaseCheck::Refuse(reason) => assert!(reason.contains("lacks"), "{reason}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn dirty_seat_refuses() {
    let fx = shipped_lane();
    std::fs::write(fx.seat.join("work.txt"), "edited, not committed").unwrap();
    match check_dead(&fx) {
        PrLeaseCheck::Refuse(reason) => assert!(reason.contains("work.txt"), "{reason}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn branch_never_pushed_is_not_judged() {
    let fx = shipped_lane();
    let lease = seat_lease(&fx.seat);
    let seat = read_seat(&lease, "unpushed");
    assert!(pr_head_on_origin(&fx.main, "unpushed").is_none());
    assert!(matches!(
        classify(OwnerLiveness::Dead, &seat, None, never),
        PrLeaseCheck::NotProvable(_)
    ));
}

// ── the one verdict-to-action mapping both relaunch paths use ───────────────

#[test]
fn end_action_maps_every_verdict() {
    let lease = seat_lease(Path::new("/nonexistent/seat"));
    assert_eq!(
        end_action(&lease, "feat", &PrLeaseCheck::SafeToForceEnd),
        LeaseEndAction::ForceEnd
    );
    assert_eq!(
        end_action(&lease, "feat", &PrLeaseCheck::Live),
        LeaseEndAction::PlainEnd
    );
    assert_eq!(
        end_action(&lease, "feat", &PrLeaseCheck::NotProvable("x".into())),
        LeaseEndAction::PlainEnd
    );
    match end_action(
        &lease,
        "feat",
        &PrLeaseCheck::Refuse("its seat is dirty".into()),
    ) {
        LeaseEndAction::Bail(msg) => {
            assert!(msg.contains("its seat is dirty"), "{msg}");
            assert!(msg.contains("lease-story-2315"), "{msg}");
        }
        other => panic!("expected a bail, got {other:?}"),
    }
}

#[test]
fn no_nameable_pr_branch_keeps_the_plain_end() {
    let lease = seat_lease(Path::new("/nonexistent/seat"));
    assert_eq!(
        plan_lease_end(Path::new("/nonexistent/main"), &lease, None),
        LeaseEndAction::PlainEnd
    );
}

fn lease_on(id: &str, branch: &str) -> crate::SessionLease {
    let mut l = seat_lease(Path::new("/nonexistent/seat"));
    l.id = id.into();
    l.branch = branch.into();
    l
}

#[test]
fn review_launch_collects_every_blocking_lease_and_the_previous_reviewers() {
    let merged = merge_review_conflicts(
        vec![lease_on("impl-a", "feat"), lease_on("impl-b", "feat")],
        vec![lease_on("rev-old", "pr-42"), lease_on("impl-a", "feat")],
    );
    let ids: Vec<&str> = merged.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(ids, ["impl-a", "impl-b", "rev-old"]);
}

#[test]
fn every_collected_lease_gets_its_own_action() {
    // Not only conflicts[0]: each lease is judged, and the reviewer's pr-N
    // lease is judged against the PR's source branch.
    let merged = merge_review_conflicts(
        vec![lease_on("impl-a", "feat"), lease_on("impl-b", "feat")],
        vec![lease_on("rev-old", "pr-42")],
    );
    let verdicts = |l: &crate::SessionLease| match l.id.as_str() {
        "impl-a" => PrLeaseCheck::SafeToForceEnd,
        "impl-b" => PrLeaseCheck::NotProvable("no pid".into()),
        _ => PrLeaseCheck::Refuse("its seat has commits the PR lacks: abc".into()),
    };
    let actions: Vec<LeaseEndAction> = merged
        .iter()
        .map(|l| end_action(l, "feat", &verdicts(l)))
        .collect();
    assert_eq!(actions[0], LeaseEndAction::ForceEnd);
    assert_eq!(actions[1], LeaseEndAction::PlainEnd);
    match &actions[2] {
        LeaseEndAction::Bail(msg) => {
            assert!(msg.contains("rev-old"), "{msg}");
            assert!(msg.contains("`pr-42`"), "{msg}");
            assert!(msg.contains("PR branch `feat`"), "{msg}");
        }
        other => panic!("expected a bail, got {other:?}"),
    }
}

/// The reported failure: a lane shipped its PR and exited, leaving a dead
/// lease on a clean seat at the PR head. Both relaunch paths used to run a
/// plain `session end` on it, which stopped on "could not end dead lease …
/// add --force". Through the shared mapping that lease is now force-ended.
#[test]
fn regression_dead_clean_in_pr_lease_is_force_ended_not_could_not_end() {
    let fx = shipped_lane();
    let lease = seat_lease(&fx.seat);
    let action = end_action(&lease, "feat", &check_dead(&fx));
    assert_eq!(
        action,
        LeaseEndAction::ForceEnd,
        "a plain end here is what produced 'could not end dead lease … add --force'"
    );
}

#[test]
fn refusal_message_names_the_lease_and_the_way_out() {
    let lease = seat_lease(Path::new("/nonexistent/seat"));
    let msg = refusal_message(&lease, "feat", "its seat has 1 uncommitted change(s): x");
    assert!(msg.contains("lease-story-2315"), "{msg}");
    assert!(msg.contains("`feat`"), "{msg}");
    assert!(
        msg.contains("aida session end lease-story-2315 --force"),
        "{msg}"
    );
}

#[test]
fn refusal_message_names_a_reviewers_own_branch() {
    let lease = lease_on("rev-old", "pr-42");
    let msg = refusal_message(&lease, "feat", "dirty");
    assert!(
        msg.contains("holds branch `pr-42` (PR branch `feat`)"),
        "{msg}"
    );
}
