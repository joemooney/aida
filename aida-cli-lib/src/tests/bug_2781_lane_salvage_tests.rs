//! BUG-2781 AC4: a lane that ends non-zero after committing is salvage-pushed
//! from host git; uncommitted-only work and a non-fast-forward are retained.
// trace:BUG-2781 | ai:claude

use super::seat_publish_tests::{git_ok, seat_with_commit, test_bundle};
use super::tests::{fixture, ref_exists};
use crate::headless_background::{OriginHead, SeatState};
use crate::lane_salvage::{
    failed_exit_note, on_failed_exit, Bundle, FailedExit, Host, Lane, UNVERIFIED,
};
use crate::standalone_implementer::LaunchSeat;
use crate::standalone_implementer::Outcome;
use std::cell::RefCell;
use std::path::Path;

fn lane<'a>(
    main: &'a Path,
    clone: &'a Path,
    branch: &'a str,
    id: &'a str,
    pr: Option<u64>,
) -> Lane<'a> {
    Lane {
        main,
        clone,
        spec: "BUG-1",
        branch,
        lane: id,
        launch_head: None,
        open_pr: pr,
    }
}

fn head_movable(_: &Path, _: &str) -> anyhow::Result<()> {
    Ok(())
}

/// The host a salvage reads, with a PR head nothing protects.
fn host<'a>(bundle: Bundle<'a>) -> Host<'a> {
    Host {
        bundle,
        pr_head: &head_movable,
        store: None,
    }
}

fn committed(tip: &str) -> SeatState {
    SeatState {
        head: Some(tip.into()),
        origin: OriginHead::Absent,
        uncommitted: Some(0),
        head_has_unpushed: Some(true),
    }
}

fn no_imports(main: &Path) {
    let refs = git_ok(
        main,
        &[
            "for-each-ref",
            "--format=%(refname)",
            crate::ship_from_seat::QUARANTINE_PREFIX,
            "refs/aida/publication/",
        ],
    );
    assert_eq!(refs, "", "host import refs outlived the salvage");
}

#[cfg(unix)]
fn sigterm() -> String {
    use std::os::unix::process::ExitStatusExt as _;
    crate::lane_salvage::exit_cause(
        std::process::ExitStatus::from_raw(15),
        &crate::lane_salvage::EndEvidence::default(),
    )
}

#[cfg(unix)]
#[test]
fn bug_2781_sigterm_after_commit_salvage_pushes_to_a_salvage_ref() {
    let fx = fixture();
    let id = "c0ffee278101";
    let (clone, tip) = seat_with_commit(&fx, "salvage", id, "seat-salvage", "fix: x (BUG-1)");
    let said = RefCell::new(Vec::new());
    let note = on_failed_exit(
        &lane(&fx.main, &clone, "seat-salvage", id, None),
        &Outcome::NoPr,
        &committed(&tip),
        &sigterm(),
        &host(&test_bundle),
        &|text, pr| said.borrow_mut().push((text.to_owned(), pr)),
    )
    .expect("salvaged");
    let target = format!("refs/heads/salvage/{id}");
    assert_eq!(git_ok(&fx.origin, &["rev-parse", &target]), tip);
    assert!(
        !ref_exists(&fx.origin, "refs/heads/seat-salvage"),
        "no PR: the claimed branch stays untouched"
    );
    assert!(note.starts_with(UNVERIFIED), "{note}");
    let said = said.into_inner();
    assert_eq!(said.len(), 1);
    assert!(
        said[0].0.contains("SIGTERM") && said[0].0.contains(UNVERIFIED),
        "{:?}",
        said[0]
    );
    no_imports(&fx.main);
}

#[cfg(unix)]
#[test]
fn bug_2781_open_pr_branch_is_fast_forwarded() {
    let fx = fixture();
    let id = "c0ffee278102";
    let (clone, tip) = seat_with_commit(&fx, "prff", id, "seat-prff", "fix: y (BUG-1)");
    git_ok(
        &fx.main,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "push",
            "-q",
            "origin",
            "refs/remotes/origin/main:refs/heads/seat-prff",
        ],
    );
    let said = RefCell::new(Vec::new());
    on_failed_exit(
        &lane(&fx.main, &clone, "seat-prff", id, Some(7)),
        &Outcome::PrOpened { pr: 7, head: None },
        &committed(&tip),
        &sigterm(),
        &host(&test_bundle),
        &|text, pr| said.borrow_mut().push((text.to_owned(), pr)),
    )
    .expect("salvaged");
    assert_eq!(
        git_ok(&fx.origin, &["rev-parse", "refs/heads/seat-prff"]),
        tip
    );
    assert_eq!(said.into_inner()[0].1, Some(7), "announced on the PR");
    no_imports(&fx.main);
}

#[cfg(unix)]
#[test]
fn bug_2781_uncommitted_only_pushes_nothing() {
    let fx = fixture();
    let id = "c0ffee278103";
    let (clone, tip) = seat_with_commit(&fx, "dirty", id, "seat-dirty", "fix: z (BUG-1)");
    std::fs::write(clone.join("edit.txt"), "edit\n").unwrap();
    let edits = SeatState {
        head: Some(tip.clone()),
        origin: OriginHead::Absent,
        uncommitted: Some(1),
        head_has_unpushed: Some(true),
    };
    let mut at_launch = lane(&fx.main, &clone, "seat-dirty", id, None);
    at_launch.launch_head = Some(&tip);
    let note = on_failed_exit(
        &at_launch,
        &Outcome::NoPr,
        &edits,
        &sigterm(),
        &host(&|_, _, _| panic!("bundled a seat with nothing committed")),
        &|_, _| panic!("announced a salvage that did not happen"),
    );
    assert_eq!(note, None);
    assert!(!ref_exists(&fx.origin, &format!("refs/heads/salvage/{id}")));
    assert_eq!(
        git_ok(&clone, &["status", "--porcelain", "edit.txt"]),
        "?? edit.txt",
        "retained"
    );
}

#[cfg(unix)]
#[test]
fn bug_2781_non_fast_forward_is_refused_and_retained() {
    let fx = fixture();
    let id = "c0ffee278104";
    let (clone, tip) = seat_with_commit(&fx, "diverged", id, "seat-diverged", "fix: w (BUG-1)");
    let other = git_ok(
        &fx.main,
        &[
            "-c",
            "user.name=Other",
            "-c",
            "user.email=other@example.com",
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "commit-tree",
            "origin/main^{tree}",
            "-p",
            "origin/main",
            "-m",
            "someone else's push",
        ],
    );
    git_ok(
        &fx.main,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "push",
            "-q",
            "origin",
            &format!("{other}:refs/heads/seat-diverged"),
        ],
    );
    let note = on_failed_exit(
        &lane(&fx.main, &clone, "seat-diverged", id, Some(8)),
        &Outcome::PrOpened { pr: 8, head: None },
        &committed(&tip),
        &sigterm(),
        &host(&test_bundle),
        &|_, _| panic!("announced a refused salvage"),
    )
    .expect("a refusal note");
    assert!(
        note.contains("refused") && note.contains("fast-forward"),
        "{note}"
    );
    assert_eq!(
        git_ok(&fx.origin, &["rev-parse", "refs/heads/seat-diverged"]),
        other
    );
    assert_eq!(
        git_ok(&clone, &["rev-parse", "HEAD"]),
        tip,
        "the commits stay in the seat"
    );
    no_imports(&fx.main);
}

/// The run()-level step for a seat clone on `branch`, with `launch` as the
/// seat looked at launch; returns the folded note and what was announced.
#[cfg(unix)]
fn run_failed_exit(
    fx: &super::tests::Fixture,
    clone: &Path,
    branch: &str,
    id: &str,
    launch: &LaunchSeat<'_>,
    outcome: &Outcome,
    seat: SeatState,
) -> (Option<String>, Vec<(String, Option<u64>)>) {
    run_failed_exit_as(fx, clone, (branch, id), launch, outcome, seat, false)
}

/// [`run_failed_exit`], salvaging to the side branch only when `side_only`.
#[cfg(unix)]
fn run_failed_exit_as(
    fx: &super::tests::Fixture,
    clone: &Path,
    (branch, id): (&str, &str),
    launch: &LaunchSeat<'_>,
    outcome: &Outcome,
    seat: SeatState,
    side_only: bool,
) -> (Option<String>, Vec<(String, Option<u64>)>) {
    let said = RefCell::new(Vec::new());
    let note = failed_exit_note(
        &FailedExit {
            main: &fx.main,
            clone,
            spec: "BUG-1",
            branch,
            lane: id,
            seat_clone: true,
            launch_seat: launch,
            side_branch_only: side_only,
        },
        &sigterm(),
        outcome,
        &|| seat.clone(),
        &host(&test_bundle),
        &|text, pr| said.borrow_mut().push((text.to_owned(), pr)),
        Some("host post-exit commit skipped".into()),
    );
    (note, said.into_inner())
}

#[cfg(unix)]
fn pr_branch_at_main(fx: &super::tests::Fixture, branch: &str) {
    git_ok(
        &fx.main,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "push",
            "-q",
            "origin",
            &format!("refs/remotes/origin/main:refs/heads/{branch}"),
        ],
    );
}

// The PR the lane launched on is the target when the lane opened none; the
// note folds into the leftover and the attempt records it as unverified.
#[cfg(unix)]
#[test]
fn bug_2781_run_step_targets_the_launch_pr_and_records_unverified() {
    let fx = fixture();
    let id = "c0ffee278105";
    let (clone, tip) = seat_with_commit(&fx, "launchpr", id, "seat-launchpr", "fix: v (BUG-1)");
    pr_branch_at_main(&fx, "seat-launchpr");
    let launch_pr = (11, None);
    let launch = LaunchSeat {
        pr: Some(&launch_pr),
        head: None,
    };
    let (note, said) = run_failed_exit(
        &fx,
        &clone,
        "seat-launchpr",
        id,
        &launch,
        &Outcome::NoPr,
        committed(&tip),
    );
    let note = note.expect("a note");
    assert_eq!(
        git_ok(&fx.origin, &["rev-parse", "refs/heads/seat-launchpr"]),
        tip,
        "fast-forwarded the launch PR's branch"
    );
    assert!(!ref_exists(&fx.origin, &format!("refs/heads/salvage/{id}")));
    assert_eq!(said.len(), 1);
    assert_eq!(said[0].1, Some(11), "announced on the launch PR");
    assert!(
        note.starts_with("host post-exit commit skipped; lane ended by SIGTERM (15)"),
        "{note}"
    );
    assert!(
        note.contains(&format!("{UNVERIFIED}: pushed {tip} to seat-launchpr")),
        "{note}"
    );
    // The attempt's durable detail carries the salvage label.
    let tmp = tempfile::tempdir().unwrap();
    let attempt =
        crate::lane_result::Attempt::begin(tmp.path(), "BUG-1", Some("seat-launchpr")).unwrap();
    crate::standalone_implementer::persist_exit_notes(
        &attempt,
        Some(&note),
        &fx.main,
        "seat-launchpr",
    )
    .unwrap();
    let receipts: Vec<_> = std::fs::read_dir(tmp.path().join(".aida/lane-results"))
        .unwrap()
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect();
    assert_eq!(receipts.len(), 1);
    let receipt: serde_json::Value = serde_json::from_str(&receipts[0]).unwrap();
    let detail = receipt["detail"].as_str().unwrap();
    assert!(detail.contains(UNVERIFIED), "{detail}");
    no_imports(&fx.main);
}

// A PR this lane opened wins over the one it launched on.
#[cfg(unix)]
#[test]
fn bug_2781_run_step_prefers_the_pr_the_lane_opened() {
    let fx = fixture();
    let id = "c0ffee278106";
    let (clone, tip) = seat_with_commit(&fx, "opened", id, "seat-opened", "fix: u (BUG-1)");
    pr_branch_at_main(&fx, "seat-opened");
    let launch_pr = (11, None);
    let launch = LaunchSeat {
        pr: Some(&launch_pr),
        head: None,
    };
    let (_, said) = run_failed_exit(
        &fx,
        &clone,
        "seat-opened",
        id,
        &launch,
        &Outcome::PrOpened { pr: 12, head: None },
        committed(&tip),
    );
    assert_eq!(said.len(), 1);
    assert_eq!(said[0].1, Some(12));
    assert_eq!(
        git_ok(&fx.origin, &["rev-parse", "refs/heads/seat-opened"]),
        tip
    );
}

// No PR: the salvage ref; the claimed branch is never created.
#[cfg(unix)]
#[test]
fn bug_2781_run_step_without_a_pr_targets_the_salvage_ref() {
    let fx = fixture();
    let id = "c0ffee278107";
    let (clone, tip) = seat_with_commit(&fx, "nopr", id, "seat-nopr", "fix: t (BUG-1)");
    let (note, said) = run_failed_exit(
        &fx,
        &clone,
        "seat-nopr",
        id,
        &LaunchSeat::default(),
        &Outcome::NoPr,
        committed(&tip),
    );
    let target = format!("salvage/{id}");
    assert_eq!(
        git_ok(&fx.origin, &["rev-parse", &format!("refs/heads/{target}")]),
        tip
    );
    assert!(!ref_exists(&fx.origin, "refs/heads/seat-nopr"));
    assert_eq!(said[0].1, None);
    assert!(said[0].0.starts_with(UNVERIFIED), "{}", said[0].0);
    assert!(
        said[0].0.contains("not merge-train eligible"),
        "{}",
        said[0].0
    );
    assert!(note.unwrap().contains(&format!("pushed {tip} to {target}")));
}

// The launch head is passed through: a seat still at it salvages nothing.
#[cfg(unix)]
#[test]
fn bug_2781_run_step_passes_the_launch_head() {
    let fx = fixture();
    let id = "c0ffee278108";
    let (clone, tip) = seat_with_commit(&fx, "athead", id, "seat-athead", "fix: s (BUG-1)");
    let launch = LaunchSeat {
        pr: None,
        head: Some(&tip),
    };
    let (note, said) = run_failed_exit(
        &fx,
        &clone,
        "seat-athead",
        id,
        &launch,
        &Outcome::NoPr,
        committed(&tip),
    );
    assert!(said.is_empty(), "{said:?}");
    assert_eq!(
        note.as_deref(),
        Some(
            "host post-exit commit skipped; lane ended by SIGTERM (15); source: OOM unknown \
             (kernel log unreadable)"
        )
    );
    assert!(!ref_exists(&fx.origin, &format!("refs/heads/salvage/{id}")));
}

// STORY-3134: a usage-limit stop salvages to `salvage/<lane>`, never to the
// PR branch it launched on, and never by force.
// trace:STORY-3134 | ai:claude
#[cfg(unix)]
#[test]
fn story_3134_usage_limit_stop_salvages_to_the_side_branch_only() {
    let fx = fixture();
    let id = "c0ffee313401";
    let (clone, tip) = seat_with_commit(&fx, "outage", id, "seat-outage", "fix: o (BUG-1)");
    pr_branch_at_main(&fx, "seat-outage");
    let before = git_ok(&fx.origin, &["rev-parse", "refs/heads/seat-outage"]);
    let launch_pr = (12, None);
    let launch = LaunchSeat {
        pr: Some(&launch_pr),
        head: None,
    };
    let branch = ("seat-outage", id);
    let (note, said) = run_failed_exit_as(
        &fx,
        &clone,
        branch,
        &launch,
        &Outcome::NoPr,
        committed(&tip),
        true,
    );
    let salvage = format!("refs/heads/salvage/{id}");
    assert_eq!(git_ok(&fx.origin, &["rev-parse", &salvage]), tip);
    let pr_head = git_ok(&fx.origin, &["rev-parse", "refs/heads/seat-outage"]);
    assert_eq!(pr_head, before, "the PR branch is untouched");
    assert_eq!(said.len(), 1);
    assert_eq!(said[0].1, None, "nothing posted on the PR");
    let note = note.unwrap();
    assert!(
        note.contains(&format!("pushed {tip} to salvage/{id}")),
        "{note}"
    );
}

// A seat that is not a self-publishing clone only names the cause.
#[cfg(unix)]
#[test]
fn bug_2781_run_step_outside_a_seat_clone_only_names_the_cause() {
    let fx = fixture();
    let note = failed_exit_note(
        &FailedExit {
            main: &fx.main,
            clone: &fx.main,
            spec: "BUG-1",
            branch: "b",
            lane: "l",
            seat_clone: false,
            launch_seat: &LaunchSeat::default(),
            side_branch_only: false,
        },
        "exited with code 1",
        &Outcome::NoPr,
        &|| panic!("read a seat that is not a clone"),
        &host(&|_, _, _| panic!("bundled a seat that is not a clone")),
        &|_, _| panic!("announced"),
        None,
    );
    assert_eq!(note.as_deref(), Some("lane exited with code 1"));
}

// A branch the seat's lease does not own is refused before any bundle.
#[cfg(unix)]
#[test]
fn bug_2781_branch_the_lease_does_not_own_is_refused() {
    let fx = fixture();
    let id = "c0ffee278109";
    let (clone, tip) = seat_with_commit(&fx, "owned", id, "seat-owned", "fix: r (BUG-1)");
    let note = on_failed_exit(
        &lane(&fx.main, &clone, "someone-elses", id, None),
        &Outcome::NoPr,
        &committed(&tip),
        &sigterm(),
        &host(&|_, _, _| panic!("bundled a branch the lease does not own")),
        &|_, _| panic!("announced a refused salvage"),
    )
    .expect("a refusal note");
    assert!(
        note.starts_with("salvage-push refused, commits retained"),
        "{note}"
    );
    assert!(note.contains("another seat"), "{note}");
    assert!(!ref_exists(&fx.origin, &format!("refs/heads/salvage/{id}")));
    assert!(!ref_exists(&fx.origin, "refs/heads/someone-elses"));
    assert_eq!(git_ok(&clone, &["rev-parse", "HEAD"]), tip, "retained");
    no_imports(&fx.main);
}

// A publication veto (an operator hold) refuses the salvage.
#[cfg(unix)]
#[test]
fn bug_2781_publication_veto_is_refused() {
    let fx = fixture();
    let id = "c0ffee278110";
    let (clone, tip) = seat_with_commit(&fx, "vetoed", id, "seat-vetoed", "fix: q (BUG-1)");
    crate::punt::write_hold_signal(
        &crate::punt::hold_signal_path(&fx.main, "BUG-1"),
        &crate::punt::HoldSignal {
            spec: "BUG-1".into(),
            spec_key: None,
            branch: "seat-vetoed".into(),
            reason: Some("manual gate".into()),
        },
    )
    .unwrap();
    let note = on_failed_exit(
        &lane(&fx.main, &clone, "seat-vetoed", id, None),
        &Outcome::NoPr,
        &committed(&tip),
        &sigterm(),
        &host(&|_, _, _| panic!("bundled a vetoed seat")),
        &|_, _| panic!("announced a vetoed salvage"),
    )
    .expect("a refusal note");
    assert!(
        note.starts_with("salvage-push refused, commits retained"),
        "{note}"
    );
    assert!(note.contains("implementer held (manual gate)"), "{note}");
    assert!(!ref_exists(&fx.origin, &format!("refs/heads/salvage/{id}")));
    assert_eq!(git_ok(&clone, &["rev-parse", "HEAD"]), tip, "retained");
    no_imports(&fx.main);
}

/// Protect PR heads in the fixture and record an approval of `pr` at `sha`.
#[cfg(unix)]
fn approve(fx: &super::tests::Fixture, pr: u64, sha: &str) {
    let config = crate::config_path_for_project(&fx.main);
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    let mut body = std::fs::read_to_string(&config).unwrap_or_default();
    body.push_str("\n[pr-head]\nprotect = true\n");
    std::fs::write(&config, body).unwrap();
    let verdicts = fx.main.join(".aida/review-verdicts");
    std::fs::create_dir_all(&verdicts).unwrap();
    std::fs::write(
        verdicts.join(format!("PR-{pr}.json")),
        format!(r#"{{"verdict":"APPROVED","reviewed_sha":"{sha}","recorded_by":"reviewer-a"}}"#),
    )
    .unwrap();
}

/// A salvage onto `pr`'s open branch, with the BUG-2618 guard read from the
/// fixture's recorded evidence at the branch's live head on origin.
#[cfg(unix)]
fn salvage_onto_pr(
    fx: &super::tests::Fixture,
    clone: &Path,
    branch: &str,
    id: &str,
    pr: u64,
    tip: &str,
) -> (Option<String>, Vec<(String, Option<u64>)>) {
    let origin = fx.origin.clone();
    let guard = move |main: &Path, branch: &str| {
        let head = git_ok(&origin, &["rev-parse", &format!("refs/heads/{branch}")]);
        crate::pr_head_guard::check(main, &format!("PR-{pr}"), pr, &head)
    };
    let said = RefCell::new(Vec::new());
    let note = on_failed_exit(
        &lane(&fx.main, clone, branch, id, Some(pr)),
        &Outcome::PrOpened { pr, head: None },
        &committed(tip),
        &sigterm(),
        &Host {
            bundle: &test_bundle,
            pr_head: &guard,
            store: None,
        },
        &|text, pr| said.borrow_mut().push((text.to_owned(), pr)),
    );
    (note, said.into_inner())
}

// Review round 4, finding 1: a PR head approved at its live head is never
// moved by a salvage; the work goes to the salvage ref and says why.
#[cfg(unix)]
#[test]
fn bug_2781_approved_pr_head_is_not_moved_and_salvages_to_the_salvage_ref() {
    let fx = fixture();
    let id = "c0ffee278111";
    let (clone, tip) = seat_with_commit(&fx, "approved", id, "seat-approved", "fix: p (BUG-1)");
    pr_branch_at_main(&fx, "seat-approved");
    let head = git_ok(&fx.origin, &["rev-parse", "refs/heads/seat-approved"]);
    approve(&fx, 21, &head);
    let (note, said) = salvage_onto_pr(&fx, &clone, "seat-approved", id, 21, &tip);
    assert_eq!(
        git_ok(&fx.origin, &["rev-parse", "refs/heads/seat-approved"]),
        head,
        "the approved PR head did not move"
    );
    let salvage = format!("salvage/{id}");
    assert_eq!(
        git_ok(&fx.origin, &["rev-parse", &format!("refs/heads/{salvage}")]),
        tip
    );
    let note = note.expect("a note");
    assert!(
        note.starts_with(&format!("{UNVERIFIED}: pushed {tip} to {salvage}")),
        "{note}"
    );
    assert!(
        note.contains("PR #21 head not moved") && note.contains("recorded approval"),
        "{note}"
    );
    assert_eq!(said.len(), 1);
    assert!(
        said[0].0.contains(&salvage) && said[0].0.contains("PR #21 head not moved"),
        "{:?}",
        said[0]
    );
    no_imports(&fx.main);
}

// The same guard with the approval at an older head lets the salvage
// fast-forward the PR branch.
#[cfg(unix)]
#[test]
fn bug_2781_unapproved_pr_head_is_fast_forwarded() {
    let fx = fixture();
    let id = "c0ffee278112";
    let (clone, tip) = seat_with_commit(&fx, "unapproved", id, "seat-unapproved", "fix: o (BUG-1)");
    pr_branch_at_main(&fx, "seat-unapproved");
    approve(&fx, 22, "0123456789abcdef0123456789abcdef01234567");
    let (note, said) = salvage_onto_pr(&fx, &clone, "seat-unapproved", id, 22, &tip);
    assert_eq!(
        git_ok(&fx.origin, &["rev-parse", "refs/heads/seat-unapproved"]),
        tip
    );
    assert!(!ref_exists(&fx.origin, &format!("refs/heads/salvage/{id}")));
    let note = note.expect("a note");
    assert_eq!(
        note,
        format!("{UNVERIFIED}: pushed {tip} to seat-unapproved")
    );
    assert_eq!(said[0].1, Some(22));
    no_imports(&fx.main);
}

// Review round 4, finding 2: an imported commit without a spec trailer, or
// one naming another spec only, refuses the salvage and keeps the work.
#[cfg(unix)]
#[test]
fn bug_2781_imported_commit_without_the_lane_trailer_is_refused() {
    for (n, subject, why) in [
        (13, "fix: no trailer", "carries no `(SPEC-ID)`"),
        (14, "fix: elsewhere (BUG-9)", "you named BUG-1"),
    ] {
        let fx = fixture();
        let id = format!("c0ffee2781{n}");
        let name = format!("trailer{n}");
        let branch = format!("seat-trailer{n}");
        let (clone, tip) = seat_with_commit(&fx, &name, &id, &branch, subject);
        let note = on_failed_exit(
            &lane(&fx.main, &clone, &branch, &id, None),
            &Outcome::NoPr,
            &committed(&tip),
            &sigterm(),
            &host(&test_bundle),
            &|_, _| panic!("announced a refused salvage"),
        )
        .expect("a refusal note");
        assert!(
            note.starts_with("salvage-push refused, commits retained") && note.contains(why),
            "{note}"
        );
        assert!(!ref_exists(&fx.origin, &format!("refs/heads/salvage/{id}")));
        assert_eq!(git_ok(&clone, &["rev-parse", "HEAD"]), tip, "retained");
        no_imports(&fx.main);
    }
}
