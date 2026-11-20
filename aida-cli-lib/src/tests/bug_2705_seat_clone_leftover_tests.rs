//! BUG-2705: a self-publishing (Claude) lane in a seat clone that exits
//! cleanly with its edits uncommitted gets the host post-exit commit before
//! its seat is published and before the fast gate requires a clean committed
//! seat. When the host does not commit, the lane says exactly why.
// trace:BUG-2705 | ai:claude

use super::seat_publish_tests::{
    bwrap_live_or_skip, git_ok, no_commit, opts, seat_with_publication_pending, seed_lane_spec,
    self_push_body, started_lane_configured, test_bundle, test_commit, RefusedLaunch,
    REFUSED_BRANCH, REFUSED_LEASE,
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

/// Helper level: [`self_published_seat_exit`] with injected effects commits
/// the leftovers as a `[AI:claude]` commit and publishes it. The production
/// wiring and the real gate are covered by the `bug_2705_real_lane_*` tests
/// below.
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

// ── Through the real lane exit driver ─────────────────────────────────────
//
// Each case below runs `standalone_implementer::run_for_test`, the runner
// `aida queue work` launches, against a leased seat clone and a mock Claude
// child: the host's own seat exit, post-exit commit, publication and pushed
// fast gate run as in production.

/// The child's uncommitted edit, which the configured fast check looks for in
/// the tree it is handed.
const LEFTOVER: &str = "leftover-2705.txt";

/// Trusted default-branch config: a forge-less publication and a fast check
/// that passes only when the child's edit is in the seat tree.
fn gate_config(forge: &str) -> String {
    format!(
        "\n[forge]\nprovider = \"{forge}\"\n\n[validation]\nfast_check = \
         \"grep -qx child-edit {LEFTOVER} && echo gate-saw-committed-tree\"\n"
    )
}

/// The mock child's shell: leave `LEFTOVER` uncommitted, then `extra`.
fn leave_edit(clone: &Path, extra: &str) -> String {
    format!(
        "cd '{}' && printf 'child-edit\\n' > {LEFTOVER}\n{extra}",
        clone.display()
    )
}

/// Run the lane through the real runner; its result and emitted lines.
fn run_lane(lane: &RefusedLaunch) -> (anyhow::Result<()>, Vec<String>) {
    let grant = crate::gate::seat_authority::HeadlessChildGrant::Baseline {
        cause: "test".into(),
    };
    crate::lane_result::take_emitted_lines();
    let result =
        crate::standalone_implementer::run_for_test(crate::standalone_implementer::Launch {
            project_root: &lane.fx.main,
            vendor: crate::session::HeadlessVendor::Claude,
            prompt: "go",
            session_id: "019e0000-0000-7000-8000-000000002705",
            spec: "BUG-1",
            lease_id: REFUSED_LEASE,
            branch: REFUSED_BRANCH,
            contained: false,
            child_grant: &grant,
            host_main: Some(&lane.fx.main),
        });
    (result, crate::lane_result::take_emitted_lines())
}

/// The finished lane-result receipts the host persisted for this lane.
fn lane_receipts(main: &Path) -> Vec<serde_json::Value> {
    let dir = crate::main_worktree_root_from(main).join(".aida/lane-results");
    std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .map(|e| serde_json::from_slice(&std::fs::read(e.path()).unwrap()).unwrap())
                .filter(|r: &serde_json::Value| !r["result"].is_null())
                .collect()
        })
        .unwrap_or_default()
}

/// The one finished receipt's result kind and detail.
fn finished(main: &Path) -> (String, String) {
    let receipts = lane_receipts(main);
    let [receipt] = receipts.as_slice() else {
        panic!("expected one finished lane receipt, got {receipts:?}");
    };
    (
        receipt["result"].as_str().unwrap_or_default().to_owned(),
        receipt["detail"].as_str().unwrap_or_default().to_owned(),
    )
}

/// Run `f` with the mock forge reporting PR `pr` at origin's branch tip.
fn with_pr_at_origin<T>(lane: &RefusedLaunch, pr: u64, f: impl FnOnce() -> T) -> T {
    crate::standalone_implementer::TEST_PR_AT_ORIGIN
        .with(|p| *p.borrow_mut() = Some((pr, lane.fx.origin.clone())));
    let out = f();
    crate::standalone_implementer::TEST_PR_AT_ORIGIN.with(|p| *p.borrow_mut() = None);
    out
}

/// BUG-2705 AC1/AC3: a Claude lane on a seat clone exits 0 with its edits
/// uncommitted. The host commits them, publishes that head, and the
/// configured fast check runs on the committed tree (it finds the child's
/// edit; the gate snapshot refuses a dirty seat, so a pass means the host
/// commit ran first) and is bound to the published head; the lane's receipt
/// records the new head as pushed.
#[cfg(unix)]
#[test]
fn bug_2705_real_lane_exit_commits_leftovers_gates_the_committed_tree_and_publishes() {
    if crate::test_env::run_path_isolated_test() || !bwrap_live_or_skip() {
        return;
    }
    let lane = started_lane_configured(
        true,
        &gate_config("pure-git"),
        |_, clone| leave_edit(clone, ""),
        |fx, _| {
            seed_lane_spec(fx, aida_core::RequirementStatus::InProgress);
            Vec::new()
        },
    );
    let launch = git_ok(&lane.clone, &["rev-parse", "HEAD"]);
    let (result, lines) = with_pr_at_origin(&lane, 7, || run_lane(&lane));

    let tip = git_ok(&lane.clone, &["rev-parse", "HEAD"]);
    assert_ne!(tip, launch, "the host made no commit: {result:?} {lines:?}");
    assert_eq!(git_ok(&lane.clone, &["status", "--porcelain"]), "");
    let subject = git_ok(&lane.clone, &["log", "-1", "--format=%s"]);
    assert!(subject.starts_with("[AI:claude] "), "{subject}");
    assert!(subject.contains("BUG-1)"), "{subject}");
    assert_eq!(
        git_ok(&lane.clone, &["show", &format!("{tip}:{LEFTOVER}")]),
        "child-edit"
    );
    assert_eq!(
        git_ok(
            &lane.fx.origin,
            &["rev-parse", &format!("refs/heads/{REFUSED_BRANCH}")]
        ),
        tip,
        "the committed head was not published"
    );
    assert_eq!(
        crate::lane_fast_gate::marker(&lane.fx.main, REFUSED_BRANCH),
        "pass",
        "{lines:?}"
    );
    // The lane's one result line binds the gate's pass to the committed,
    // published head.
    assert!(
        lines.iter().any(|l| l.starts_with("implementer: pushed ")
            && l.contains(&format!(" head={tip} "))
            && l.contains(" fast_gate=pass")),
        "{lines:?}"
    );
    assert_eq!(finished(&lane.fx.main).0, "pushed", "{result:?} {lines:?}");
}

/// The child leaves a held `index.lock` in its seat, so git refuses the host
/// post-exit commit.
const LOCK_INDEX: &str = ": > .git/index.lock";

/// BUG-2705 AC1: the host commit is refused (a held index lock) and the old
/// PR head is still blocked (conflicting), so the lane is routed as an empty
/// rework. The git refusal and the gate's dirty-seat reason reach the lane's
/// persisted result record, not only stderr.
#[cfg(unix)]
#[test]
fn bug_2705_real_lane_refused_commit_with_blocked_pr_records_both_reasons() {
    if crate::test_env::run_path_isolated_test() || !bwrap_live_or_skip() {
        return;
    }
    let lane = started_lane_configured(
        false,
        &gate_config("github"),
        |_, clone| leave_edit(clone, LOCK_INDEX),
        |fx, clone| {
            seed_lane_spec(fx, aida_core::RequirementStatus::InProgress);
            // The forge still has the launch head, conflicting, unchecked.
            let head = git_ok(clone, &["rev-parse", "HEAD"]);
            let bin = fx.root.join("gh-bin");
            std::fs::create_dir_all(&bin).unwrap();
            crate::test_exec::write_executable(
                &bin.join("gh"),
                format!(
                    "#!/bin/sh\ncase \"$2\" in\n\
                     view) printf '%s' '{{\"state\":\"OPEN\",\"headRefOid\":\"{head}\",\"mergeable\":\"CONFLICTING\"}}' ;;\n\
                     checks) echo 'no checks reported on the rework branch' >&2; exit 1 ;;\n\
                     *) exit 1 ;;\nesac\n"
                ),
            );
            vec![
                (
                    "PATH",
                    Some(format!("{}:/usr/local/bin:/usr/bin:/bin", bin.display())),
                ),
                ("AIDA_USER", Some("queue-fixture-user".into())),
            ]
        },
    );
    let head = git_ok(&lane.clone, &["rev-parse", "HEAD"]);
    crate::standalone_implementer::TEST_PR.with(|p| *p.borrow_mut() = Some(Some((7, Some(head)))));
    let (result, lines) = run_lane(&lane);
    crate::standalone_implementer::TEST_PR.with(|p| *p.borrow_mut() = None);

    let error = result.expect_err("an empty rework is not a success");
    assert_eq!(
        crate::exit_code_for_error(&error),
        crate::standalone_implementer::EMPTY_REWORK_EXIT_CODE,
        "{error:#} {lines:?}"
    );
    let (kind, detail) = finished(&lane.fx.main);
    assert_eq!(kind, "no-change-with-reason", "{detail}");
    assert!(
        detail.contains("host post-exit commit failed") && detail.contains("index.lock"),
        "the refused commit's git error is missing from the record: {detail}"
    );
    assert!(
        detail.contains("fast gate: fast gate needs a clean committed seat")
            && detail.contains(LEFTOVER),
        "the gate's dirty-seat reason is missing from the record: {detail}"
    );
}

/// BUG-2705 AC1: a lane that pushed its own head and then left edits gets no
/// host commit (it already published this round); its pushed record still
/// says why the seat stayed dirty and why the gate could not run.
#[cfg(unix)]
#[test]
fn bug_2705_real_lane_pushed_then_dirty_records_the_skip_reason() {
    if crate::test_env::run_path_isolated_test() || !bwrap_live_or_skip() {
        return;
    }
    let lane = started_lane_configured(
        false,
        &gate_config("pure-git"),
        |_, clone| {
            format!(
                "{}\n{}",
                self_push_body(clone, "[AI:claude] fix: complete (BUG-1)"),
                leave_edit(clone, "")
            )
        },
        |fx, _| {
            seed_lane_spec(fx, aida_core::RequirementStatus::InProgress);
            Vec::new()
        },
    );
    let (result, lines) = with_pr_at_origin(&lane, 7, || run_lane(&lane));
    assert_ne!(
        git_ok(&lane.clone, &["status", "--porcelain"]),
        "",
        "the host must not commit after the lane's own publication"
    );
    let (kind, detail) = finished(&lane.fx.main);
    assert_eq!(kind, "pushed", "{result:?} {lines:?} {detail}");
    assert!(
        detail.contains("host post-exit commit skipped: the lane already published this round"),
        "{detail}"
    );
    assert!(
        detail.contains("fast gate: fast gate needs a clean committed seat"),
        "{detail}"
    );
}
