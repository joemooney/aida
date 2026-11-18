//! BUG-2436: a finished standalone implementer lane (PR open, tree clean,
//! `HEAD` on origin) has its lease ended by its launcher, so the review
//! watcher does not refuse the PR as owned by a live author.
// trace:BUG-2436 | ai:claude

use super::*;
use crate::headless_background::OriginHead;

const HEAD: &str = "1111111111111111111111111111111111111111";

fn seat(origin: OriginHead, uncommitted: Option<usize>) -> SeatState {
    SeatState {
        head: Some(HEAD.to_string()),
        origin,
        uncommitted,
        head_has_unpushed: Some(true),
    }
}

fn pr_opened() -> Outcome {
    Outcome::PrOpened {
        pr: 7,
        head: Some(HEAD.to_string()),
    }
}

#[test]
fn a_clean_pushed_pr_opened_lane_ends_its_lease() {
    assert!(finished_lease_ends(
        &pr_opened(),
        &seat(OriginHead::At(HEAD.to_string()), Some(0))
    ));
}

#[test]
fn a_lane_with_anything_left_in_its_seat_keeps_its_lease() {
    let pushed = || OriginHead::At(HEAD.to_string());
    for (why, outcome, seat) in [
        ("dirty tree", pr_opened(), seat(pushed(), Some(2))),
        ("unreadable tree", pr_opened(), seat(pushed(), None)),
        (
            "origin behind HEAD",
            pr_opened(),
            seat(OriginHead::At("2".repeat(40)), Some(0)),
        ),
        (
            "branch not on origin",
            pr_opened(),
            seat(OriginHead::Absent, Some(0)),
        ),
        (
            "origin unknown",
            pr_opened(),
            seat(OriginHead::Unknown, Some(0)),
        ),
        ("no PR", Outcome::NoPr, seat(pushed(), Some(0))),
        (
            "held",
            Outcome::Held {
                branch: "b".to_string(),
            },
            seat(pushed(), Some(0)),
        ),
        (
            "punted",
            Outcome::Punted {
                reason: "fork".to_string(),
            },
            seat(pushed(), Some(0)),
        ),
    ] {
        assert!(!finished_lease_ends(&outcome, &seat), "{why}");
    }
    let no_head = SeatState {
        head: None,
        origin: OriginHead::Unknown,
        uncommitted: Some(0),
        head_has_unpushed: Some(true),
    };
    assert!(!finished_lease_ends(&pr_opened(), &no_head), "no HEAD");
}

#[test]
fn ending_the_lease_runs_session_end_from_the_main_checkout() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("aida.log");
    let aida = tmp.path().join("aida");
    crate::test_exec::write_executable(
        &aida,
        format!(
            "#!/bin/sh\nprintf '%s|%s\\n' \"$(pwd)\" \"$*\" >> '{}'\n",
            log.display()
        ),
    );
    let _aida = crate::kernel::test_aida_exe::set(aida);
    end_finished_lease(tmp.path(), "0123456789ab").unwrap();
    let call = std::fs::read_to_string(&log).unwrap();
    assert!(
        call.trim_end()
            .ends_with("|session end 0123456789ab --yes --skip-ci"),
        "{call}"
    );
}

#[test]
fn a_refused_session_end_is_an_error_not_a_panic() {
    let tmp = tempfile::tempdir().unwrap();
    let aida = tmp.path().join("aida");
    crate::test_exec::write_executable(&aida, "#!/bin/sh\nexit 1\n");
    let _aida = crate::kernel::test_aida_exe::set(aida);
    let err = end_finished_lease(tmp.path(), "0123456789ab").unwrap_err();
    assert!(
        format!("{err:#}").contains("session end 0123456789ab"),
        "{err:#}"
    );
}

// trace:BUG-2488 | ai:codex
// trace:BUG-b6b1278890774ef993d2ebc327862470-253.acf66e50 | ai:codex
#[test]
fn bug_2488_no_change_ends_only_a_readable_clean_seat() {
    assert!(finished_lease_ends(
        &Outcome::NoChange,
        &seat(OriginHead::Absent, Some(0))
    ));
    for dirty in [None, Some(1)] {
        assert!(!finished_lease_ends(
            &Outcome::NoChange,
            &seat(OriginHead::Absent, dirty)
        ));
    }
    assert!(matches!(
        classify_no_change(Outcome::NoPr, false, || panic!(
            "failed exit must not be reclassified"
        ))
        .unwrap(),
        Outcome::NoPr
    ));
    assert!(matches!(
        classify_no_change(Outcome::NoPr, true, || Ok(true)).unwrap(),
        Outcome::NoPr
    ));
}

// trace:BUG-2643 | ai:codex
#[test]
fn bug_2643_published_lane_retention_refusal_is_a_durable_cleanup_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let tree = tmp.path().join("kept-seat");
    std::fs::create_dir(&tree).unwrap();
    let aida = tmp.path().join("aida");
    crate::test_exec::write_executable(
        &aida,
        "#!/bin/sh\necho 'mail retention refused: fresh writer-admission scan could not verify every mail writer' >&2\nexit 1\n",
    );
    let _aida = crate::kernel::test_aida_exe::set(aida);
    let attempt = crate::lane_result::Attempt::begin(tmp.path(), "BUG-2643", None).unwrap();
    attempt.bind("BUG-2643", "work", None, Some("old")).unwrap();
    attempt.started(123);
    let kind = attempt
        .finish_outcome(&pr_opened(), true, false, false)
        .unwrap();
    assert_eq!(kind, crate::lane_result::Kind::Pushed);
    assert!(finished_lease_ends(
        &pr_opened(),
        &seat(OriginHead::At(HEAD.into()), Some(0))
    ));
    attempt
        .record_cleanup(&tree, end_finished_lease(tmp.path(), "0123456789ab"))
        .unwrap();
    let line = attempt.line().unwrap();
    assert!(line.starts_with("implementer: pushed "), "{line}");
    assert!(line.contains("cleanup=warning"), "{line}");
    assert!(line.contains("writer-admission scan"), "{line}");
    assert!(!line.contains("explicit-punt"), "{line}");
    assert!(tree.is_dir());
    let receipts = std::fs::read_dir(tmp.path().join(".aida/lane-results")).unwrap();
    let path = receipts.into_iter().next().unwrap().unwrap().path();
    let receipt: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(receipt["result"], "pushed");
    assert_eq!(receipt["retained_tree"], tree.to_str().unwrap());
    assert!(receipt["cleanup_warning"]
        .as_str()
        .unwrap()
        .contains("mail retention refused"));
}
