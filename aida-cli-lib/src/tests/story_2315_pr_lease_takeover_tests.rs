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
        PrLeaseCheck::SafeToEnd
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
    assert_eq!(check, PrLeaseCheck::SafeToEnd);
}

#[test]
fn missing_seat_is_ended() {
    assert_eq!(
        classify(OwnerLiveness::Dead, &SeatReading::Missing, None, never),
        PrLeaseCheck::SafeToEnd
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
        holder_record: None,
        pid_namespace: crate::agent_registry::current_pid_namespace(),
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
        lane_jobs: None,
    }
}

/// The full read for a lease already known dead: seat, PR head, ancestry.
fn check_dead(fx: &Fixture) -> PrLeaseCheck {
    let lease = seat_lease(&fx.seat);
    let seat = read_seat(&fx.main, &lease, "feat");
    let pr_head = pr_head_on_origin(&fx.main, "feat");
    classify(OwnerLiveness::Dead, &seat, pr_head.as_deref(), |a, b| {
        is_ancestor(&fx.main, a, b)
    })
}

#[test]
fn clean_seat_at_the_pushed_pr_head_is_ended() {
    let fx = shipped_lane();
    assert_eq!(check_dead(&fx), PrLeaseCheck::SafeToEnd);
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
    assert_eq!(check_dead(&fx), PrLeaseCheck::SafeToEnd);
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
    let seat = read_seat(&fx.main, &lease, "unpushed");
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
        end_action(&lease, "feat", &PrLeaseCheck::SafeToEnd),
        LeaseEndAction::SafeEnd
    );
    assert_eq!(
        end_action(&lease, "feat", &PrLeaseCheck::Live),
        LeaseEndAction::Bail("ownership holder is alive; wait for voluntary handoff".into())
    );
    assert_eq!(
        end_action(&lease, "feat", &PrLeaseCheck::NotProvable("x".into())),
        LeaseEndAction::Bail("ownership retained: x".into())
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
        LeaseEndAction::Bail("PR branch unknown; ownership retained".into())
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
        "impl-a" => PrLeaseCheck::SafeToEnd,
        "impl-b" => PrLeaseCheck::NotProvable("no pid".into()),
        _ => PrLeaseCheck::Refuse("its seat has commits the PR lacks: abc".into()),
    };
    let actions: Vec<LeaseEndAction> = merged
        .iter()
        .map(|l| end_action(l, "feat", &verdicts(l)))
        .collect();
    assert_eq!(actions[0], LeaseEndAction::SafeEnd);
    assert!(matches!(actions[1], LeaseEndAction::Bail(_)));
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
        LeaseEndAction::SafeEnd,
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

// trace:BUG-2450 | ai:codex
fn exited_lease(fx: &Fixture) -> crate::SessionLease {
    use crate::ownership::{Holder, Ownership, Phase, Role, Witness, WitnessKind, WorkKey};
    let mut lease = seat_lease(&fx.seat);
    lease.scope = uuid::Uuid::from_u128(2315).to_string();
    lease.hostname = crate::coordination::hostname();
    lease.active_pid = Some(i32::MAX as u32 - 1);
    lease.active_pid_start_time = Some("linux-starttime:1".into());
    let id = aida_core::process_probe::ProcessIdentity {
        pid: i32::MAX as u32 - 1,
        start: format!("linux:{}:1", aida_core::local_boot_id().unwrap()),
    };
    let ns = aida_core::process_probe::ProbeSource::system()
        .pid_namespace(std::process::id())
        .unwrap();
    let identity = aida_core::proc_identity::ProcIdentity::from_record(&id, Some(ns)).unwrap();
    let admission: aida_core::proc_identity::HolderRecord = serde_json::from_value(serde_json::json!({"version":1,"node_id":aida_core::proc_identity::local_node_id().unwrap(),"identity":identity,"generation":"dead-fixture","refusal":null})).unwrap();
    lease.holder_record = Some(serde_json::to_string(&admission).unwrap());
    let key = WorkKey::Spec {
        spec: crate::gate::canon::SpecKey(uuid::Uuid::from_u128(2315)),
    };
    let store = crate::ownership::store::OwnershipStore::at(fx.main.join(".aida/ownership"));
    store
        .update(&key, "seed", crate::ownership::store::Expect::Any, |_| {
            Some((
                Ownership {
                    key: key.clone(),
                    generation: 0,
                    phase: Phase::Leased,
                    holder: Some(Holder {
                        seat: "fixture".into(),
                        session: lease.id.clone(),
                        role: Role::Implementer,
                        tree: None,
                        witnesses: vec![Witness {
                            kind: WitnessKind::Process {
                                host: lease.hostname.clone(),
                                id,
                                pid_ns: Some(ns),
                            },
                            admission: Some(admission),
                            recorded_at: chrono::Utc::now(),
                        }],
                    }),
                },
                None,
            ))
        })
        .unwrap();
    lease
}

#[test]
fn bug2450_reviewer_reclaims_a_clean_pushed_dead_seat() {
    let fx = shipped_lane();
    let _ambient = crate::test_env::AmbientGuard::hermetic_and(
        &fx.main,
        None,
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    let lease = exited_lease(&fx);
    let record = crate::ownership::reader::lease_record(&fx.main, &lease)
        .unwrap()
        .unwrap();
    assert!(matches!(
        crate::ownership::reader::owner(&record),
        crate::ownership::OwnerVerdict::Dead { .. }
    ));
    assert_eq!(
        plan_lease_end(&fx.main, &lease, Some("feat")),
        LeaseEndAction::SafeEnd
    );
    assert!(fx.seat.exists(), "planning itself never removes a seat");
}

#[test]
fn bug2450_reviewer_reclaims_an_already_pushed_staged_index() {
    let fx = shipped_lane();
    let _ambient = crate::test_env::AmbientGuard::hermetic_and(
        &fx.main,
        None,
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    git(&fx.seat, &["reset", "--soft", "HEAD~1"]);
    let lease = exited_lease(&fx);
    assert_eq!(
        plan_lease_end(&fx.main, &lease, Some("feat")),
        LeaseEndAction::SafeEnd
    );
    std::fs::write(fx.seat.join("work.txt"), "unstaged work").unwrap();
    assert!(matches!(
        plan_lease_end(&fx.main, &lease, Some("feat")),
        LeaseEndAction::Bail(_)
    ));
}

#[test]
fn prc_relaunch_retains_head_not_in_pr_even_if_pushed_elsewhere() {
    let fx = shipped_lane();
    let _ambient = crate::test_env::AmbientGuard::hermetic_and(
        &fx.main,
        None,
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    // A seat's HEAD moved on, but that work is safely on a different remote
    // branch. Comparing only to the PR head falsely refused this clean seat.
    commit(&fx.seat, "later.txt", "pushed elsewhere");
    git(&fx.seat, &["push", "origin", "HEAD:published-elsewhere"]);
    let lease = exited_lease(&fx);
    assert!(matches!(
        plan_lease_end(&fx.main, &lease, Some("feat")),
        LeaseEndAction::Bail(_)
    ));
}

#[test]
fn bug2450_recorded_branch_without_upstream_blocks_force_cleanup_until_discard_is_explicit() {
    let fx = shipped_lane();
    let _ambient = crate::test_env::AmbientGuard::hermetic_and(
        &fx.main,
        None,
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    git(&fx.seat, &["checkout", "-b", "only-local"]);
    let only_copy = commit(&fx.seat, "precious.txt", "only copy");
    git(&fx.seat, &["checkout", "feat"]);
    let mut lease = exited_lease(&fx);
    lease.branch = "only-local".into();
    let error = crate::session_end::guard_remote_commits(&fx.main, &lease, false).unwrap_err();
    let text = error.to_string();
    assert!(
        text.contains(&lease.id) && text.contains("--discard-unpushed"),
        "{text}"
    );
    assert!(matches!(
        plan_lease_end(&fx.main, &lease, Some("feat")),
        LeaseEndAction::Bail(_)
    ));
    assert_eq!(git(&fx.seat, &["rev-parse", "only-local"]), only_copy);
    crate::session_end::guard_remote_commits(&fx.main, &lease, true).unwrap();
    git(&fx.seat, &["push", "origin", "only-local"]);
    // Pushing with no upstream configured is sufficient evidence.
    crate::session_end::guard_remote_commits(&fx.main, &lease, false).unwrap();
    assert_eq!(
        plan_lease_end(&fx.main, &lease, Some("feat")),
        LeaseEndAction::SafeEnd
    );
}

#[test]
fn bug2450_detached_head_unpushed_commits_are_protected() {
    let fx = shipped_lane();
    git(&fx.seat, &["checkout", "--detach"]);
    commit(&fx.seat, "detached.txt", "only in HEAD");
    let lease = exited_lease(&fx);
    assert!(crate::session_end::guard_remote_commits(&fx.main, &lease, false).is_err());
}

#[test]
fn bug2450_unsafe_seat_notice_names_lease_and_preserves_work() {
    let fx = shipped_lane();
    let _ambient = crate::test_env::AmbientGuard::hermetic_and(
        &fx.main,
        None,
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    std::fs::write(fx.seat.join("uncommitted.txt"), "keep me").unwrap();
    let lease = exited_lease(&fx);
    let LeaseEndAction::Bail(message) = plan_lease_end(&fx.main, &lease, Some("feat")) else {
        panic!("dirty seat must refuse");
    };
    notify_advisor(&fx.main, &lease, &message).unwrap();
    notify_advisor(&fx.main, &lease, &message).unwrap();
    let messages = aida_core::mailbox::read_local_messages(&fx.main).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].to,
        aida_core::mailbox::Recipient::Agent("advisor".into())
    );
    assert!(messages[0].body.contains(&lease.id));
    assert!(messages[0].body.contains("uncommitted.txt"));
    assert!(fx.seat.join("uncommitted.txt").exists());
}

// trace:BUG-2450 | ai:codex
#[test]
fn bug2450_force_session_end_refuses_before_changing_the_lease_or_tree() {
    let fx = shipped_lane();
    commit(&fx.seat, "only-local.txt", "never pushed");
    let lease = exited_lease(&fx);
    let path = crate::lease_path(&fx.main, &lease.id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = toml::to_string(&lease).unwrap();
    std::fs::write(&path, &original).unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic(&fx.main, None);
    let error = crate::session_end::session_end(
        Some(&lease.id),
        None,
        None,
        true,
        true,
        false,
        false,
        false,
        true,
        false,
        true,
        false,
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("--discard-unpushed"),
        "{error:#}"
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    assert_eq!(
        std::fs::read_to_string(fx.seat.join("only-local.txt")).unwrap(),
        "never pushed"
    );
}

// trace:BUG-2450 | ai:codex
#[test]
fn bug2450_unrelated_host_branches_do_not_block_end_or_reclaim() {
    let fx = shipped_lane();
    let _ambient = crate::test_env::AmbientGuard::hermetic_and(
        &fx.main,
        None,
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    git(&fx.main, &["checkout", "-b", "unrelated-host-work"]);
    commit(&fx.main, "host-only.txt", "unpublished host work");
    let lease = exited_lease(&fx);
    crate::session_end::guard_remote_commits(&fx.main, &lease, false).unwrap();
    assert_eq!(
        plan_lease_end(&fx.main, &lease, Some("feat")),
        LeaseEndAction::SafeEnd
    );
    // The seat's current branch is protected even if it differs from the lease.
    git(&fx.seat, &["checkout", "-b", "seat-current"]);
    commit(&fx.seat, "seat-only.txt", "unpublished seat work");
    assert!(crate::session_end::guard_remote_commits(&fx.main, &lease, false).is_err());
}

// Exercise session_start's AutoEnd -> Bail wiring, rather than calling the
// notification helper directly. Both attempts must preserve the same lease.
// trace:BUG-2450 | ai:codex
#[test]
fn bug2450_reviewer_launch_mails_once_and_retains_unsafe_exited_lease() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    for dirty in [true, false] {
        let fx = shipped_lane();
        let mut ambient = crate::test_env::AmbientGuard::hermetic_and(
            &fx.main,
            None,
            &[
                ("AIDA_TRUST_PID_NAMESPACE", Some("1")),
                ("AIDA_HEADLESS", Some("1")),
            ],
        );
        std::fs::create_dir_all(fx.main.join(".aida")).unwrap();
        std::fs::write(
            fx.main.join(".aida/config.toml"),
            "[worktree_pool]\nenabled = false\n",
        )
        .unwrap();
        let bin = fx._tmp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        crate::test_exec::write_executable(&bin.join("gh"), "#!/bin/sh\nprintf 'feat\\n'\n");
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        ambient.set_key("PATH", path);
        if dirty {
            std::fs::write(fx.seat.join("precious.txt"), "keep dirty work").unwrap();
        } else {
            commit(&fx.seat, "precious.txt", "keep unpushed work");
        }
        let lease = exited_lease(&fx);
        let path = crate::lease_path(&fx.main, &lease.id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = toml::to_string(&lease).unwrap();
        std::fs::write(&path, &original).unwrap();
        for _ in 0..2 {
            let error = crate::session_start::session_start(
                "PR-42",
                None,
                None,
                false,
                None,
                Some("github"),
                "plain",
                false,
                None,
                false,
                None,
                None,
                false,
                Some("reviewer".into()),
                false,
                Some(false),
                false,
                None,
                false,
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains(&lease.id), "{error:#}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            assert!(fx.seat.join("precious.txt").exists());
        }
        let messages = aida_core::mailbox::read_local_messages(&fx.main).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].to,
            aida_core::mailbox::Recipient::Agent("advisor".into())
        );
        assert!(messages[0].body.contains(&lease.id));
    }
}

// trace:STORY-2325 | ai:codex
#[test]
fn prc_safe_end_retires_dead_pushed_seat_without_force() {
    let fx = shipped_lane();
    let _ambient = crate::test_env::AmbientGuard::hermetic_and(
        &fx.main,
        None,
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    let lease = exited_lease(&fx);
    let cargo_target = fx._tmp.path().join("target/worktrees/seat");
    std::fs::create_dir_all(&cargo_target).unwrap();
    std::fs::create_dir_all(fx.seat.join(".aida")).unwrap();
    std::fs::write(fx.main.join(".git/info/exclude"), ".aida/\n").unwrap();
    std::fs::write(
        fx.seat.join(".aida/session-env.sh"),
        format!("export CARGO_TARGET_DIR='{}'\n", cargo_target.display()),
    )
    .unwrap();
    let path = crate::lease_path(&fx.main, &lease.id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, toml::to_string(&lease).unwrap()).unwrap();
    assert_eq!(
        plan_lease_end(&fx.main, &lease, Some("feat")),
        LeaseEndAction::SafeEnd
    );
    crate::session_end::session_end(
        Some(&lease.id),
        None,
        None,
        true,
        false,
        false,
        false,
        false,
        true,
        false,
        true,
        false,
    )
    .unwrap();
    assert!(!path.exists());
    assert!(!fx.seat.exists());
    assert!(!cargo_target.exists());
}

// trace:STORY-2325 | ai:codex
#[test]
fn prc_ship_handoff_keeps_lease_until_guarded_cleanup() {
    let fx = shipped_lane();
    let lease = {
        let _ambient = crate::test_env::AmbientGuard::hermetic_and(
            &fx.main,
            None,
            &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
        );
        exited_lease(&fx)
    };
    let _ambient = crate::test_env::AmbientGuard::hermetic_and(
        &fx.main,
        None,
        &[
            ("AIDA_TRUST_PID_NAMESPACE", Some("1")),
            ("AIDA_SESSION_ID", Some(&lease.id)),
        ],
    );
    crate::ownership::dual::enable_for_test();
    let path = crate::lease_path(&fx.main, &lease.id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let lease_body = toml::to_string(&lease).unwrap();
    std::fs::write(&path, &lease_body).unwrap();
    std::fs::write(fx.main.join(".git/info/exclude"), ".aida/\n").unwrap();
    let store = crate::ownership::store::OwnershipStore::for_project(&fx.main)
        .unwrap()
        .unwrap();
    let key = crate::ownership::WorkKey::Spec {
        spec: crate::gate::canon::SpecKey(uuid::Uuid::from_u128(2315)),
    };
    store
        .update(&key, "tree", crate::ownership::store::Expect::Any, |cur| {
            let mut r = cur.unwrap().clone();
            r.holder.as_mut().unwrap().tree = Some(crate::ownership::SeatTree {
                path: fx.seat.clone(),
                kind: crate::ownership::TreeKind::Worktree,
                branch: Some("feat".into()),
            });
            Some((r, None))
        })
        .unwrap();
    crate::ownership::dual::handed_off(&fx.main, &fx.seat, "feat", &lease.scope, 42).unwrap();
    assert!(matches!(
        store.load(&key).unwrap().unwrap().phase,
        crate::ownership::Phase::InPr { .. }
    ));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), lease_body);
    assert!(crate::list_leases(&fx.main)
        .iter()
        .any(|l| l.branch == "feat"));
    assert_eq!(
        plan_lease_end(&fx.main, &lease, Some("feat")),
        LeaseEndAction::SafeEnd
    );
    // A live handing-off lease still blocks cleanup, even though the key is free.
    let mut live = lease.clone();
    live.active_pid = Some(std::process::id());
    live.active_pid_start_time = crate::process_probe::process_start_identity(std::process::id());
    live.holder_record = Some(
        serde_json::to_string(&aida_core::proc_identity::HolderRecord::capture(
            std::process::id(),
        ))
        .unwrap(),
    );
    assert_eq!(check_pr_lease(&fx.main, &live, "feat"), PrLeaseCheck::Live);
    // A different handing-off session grants neither recovery nor cleanup.
    store
        .update(
            &key,
            "foreign",
            crate::ownership::store::Expect::Any,
            |cur| {
                let mut record = cur.unwrap().clone();
                if let crate::ownership::Phase::InPr { handed_off_by, .. } = &mut record.phase {
                    *handed_off_by = Some("another-session".into());
                }
                Some((record, None))
            },
        )
        .unwrap();
    assert!(crate::pr_cmd::ship_branch(&fx.seat, &fx.main, true).is_err());
    assert!(matches!(
        plan_lease_end(&fx.main, &lease, Some("feat")),
        LeaseEndAction::Bail(_)
    ));
    store
        .update(
            &key,
            "restore",
            crate::ownership::store::Expect::Any,
            |cur| {
                let mut record = cur.unwrap().clone();
                if let crate::ownership::Phase::InPr { handed_off_by, .. } = &mut record.phase {
                    *handed_off_by = Some(lease.id.clone());
                }
                Some((record, None))
            },
        )
        .unwrap();
    let head = git(&fx.seat, &["rev-parse", "HEAD"]);
    git(&fx.seat, &["update-ref", "refs/heads/feat", "HEAD~1"]);
    assert!(crate::pr_cmd::ship_branch(&fx.seat, &fx.main, true).is_err());
    git(&fx.seat, &["update-ref", "refs/heads/feat", &head]);
    assert_eq!(
        crate::pr_cmd::ship_branch(&fx.seat, &fx.main, false).unwrap(),
        "feat"
    );
    assert!(
        git(&fx.seat, &["branch", "--show-current"]).is_empty(),
        "dry-run stays detached"
    );
    // Both ship entry points use this recovery seam after --no-merge or a hold.
    assert_eq!(
        crate::pr_cmd::ship_branch(&fx.seat, &fx.main, true).unwrap(),
        "feat"
    );
    assert_eq!(git(&fx.seat, &["branch", "--show-current"]), "feat");
    assert_eq!(
        crate::pr_cmd::ship_branch(&fx.seat, &fx.main, true).unwrap(),
        "feat"
    );
    // The resumed branch can hand off again, then take the ordinary cleanup path.
    crate::ownership::dual::handed_off(&fx.main, &fx.seat, "feat", &lease.scope, 42).unwrap();
    assert_eq!(
        plan_lease_end(&fx.main, &lease, Some("feat")),
        LeaseEndAction::SafeEnd
    );
    crate::session_end::session_end(
        Some(&lease.id),
        None,
        None,
        true,
        false,
        false,
        false,
        false,
        true,
        false,
        true,
        false,
    )
    .unwrap();
    assert!(!path.exists());
    assert!(!fx.seat.exists());
}
