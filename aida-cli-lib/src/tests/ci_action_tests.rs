//! TASK-111: pin the `aida session end` CI decision tree. The probe
//! itself shells out to gh, but `decide_ci_action` and `parse_ci_probe`
//! are pure — tests cover every probe state × yes/wait_ci combo so
//! lifecycle changes don't silently corrupt the handoff.
//! trace:TASK-111 | ai:claude
use super::*;

#[test]
fn no_signal_proceeds_silently() {
    let probe = CiProbe::NoSignal("no open PR for branch".to_string());
    assert_eq!(decide_ci_action(&probe, false, false), CiAction::Proceed);
    assert_eq!(decide_ci_action(&probe, true, false), CiAction::Proceed);
    assert_eq!(decide_ci_action(&probe, false, true), CiAction::Proceed);
}

/// TASK-233: `--watch-ci` blocks exactly like `--wait-ci` — the
/// caller passes `wait_ci || watch_ci`, so InProgress yields
/// `CiAction::Wait` and the decision tree is identical once CI is
// terminal. trace:TASK-233 | ai:claude
#[test]
fn watch_ci_blocks_like_wait_ci_on_in_progress() {
    let probe = CiProbe::InProgress { pr_number: 26 };
    // wait || watch == true → Wait (block).
    assert_eq!(decide_ci_action(&probe, true, false), CiAction::Wait);
    // Terminal states re-decided after the block: green proceeds.
    assert_eq!(
        decide_ci_action(&CiProbe::Green { pr_number: 26 }, false, false),
        CiAction::Proceed
    );
    // Red prompts (Cancel) unless --yes.
    assert!(matches!(
        decide_ci_action(
            &CiProbe::Red {
                pr_number: 26,
                failed_summary: "macos".to_string()
            },
            false,
            false
        ),
        CiAction::Cancel(_)
    ));
}

/// BUG-273: live `gh run watch` output is only safe in an interactive
/// terminal. Headless drains and tee-captured logs must use quiet polling.
// trace:BUG-273
#[test]
fn ci_watch_streams_only_for_interactive_non_headless_context() {
    assert!(should_stream_ci_watch(true, false));
    assert!(!should_stream_ci_watch(false, false));
    assert!(!should_stream_ci_watch(true, true));
    assert!(!should_stream_ci_watch(false, true));
}

/// TASK-233: run-id extraction from `gh run list --json databaseId`.
// trace:TASK-233 | ai:claude
#[test]
fn first_run_id_from_gh_json_shapes() {
    assert_eq!(
        first_run_id_from_gh_json(r#"[{"databaseId":12345}]"#),
        Some("12345".to_string())
    );
    // Multiple runs → the first (most recent) wins.
    assert_eq!(
        first_run_id_from_gh_json(r#"[{"databaseId":999},{"databaseId":111}]"#),
        Some("999".to_string())
    );
    // Empty array (no runs yet) → None.
    assert_eq!(first_run_id_from_gh_json("[]"), None);
    // Garbage / non-JSON → None, no panic.
    assert_eq!(first_run_id_from_gh_json("not json"), None);
    assert_eq!(first_run_id_from_gh_json(""), None);
}

#[test]
fn green_always_proceeds() {
    let probe = CiProbe::Green { pr_number: 7 };
    assert_eq!(decide_ci_action(&probe, false, false), CiAction::Proceed);
    assert_eq!(decide_ci_action(&probe, true, false), CiAction::Proceed);
    assert_eq!(decide_ci_action(&probe, false, true), CiAction::Proceed);
}

#[test]
fn pr_no_checks_proceeds_with_info() {
    let probe = CiProbe::PrNoChecks { pr_number: 7 };
    assert_eq!(decide_ci_action(&probe, false, false), CiAction::Proceed);
}

#[test]
fn in_progress_prompts_when_interactive() {
    let probe = CiProbe::InProgress { pr_number: 7 };
    match decide_ci_action(&probe, false, false) {
        CiAction::Cancel(msg) => assert!(msg.contains("PR-7"), "msg: {msg}"),
        other => panic!("expected Cancel, got {:?}", other),
    }
}

#[test]
fn in_progress_waits_with_flag() {
    let probe = CiProbe::InProgress { pr_number: 7 };
    assert_eq!(decide_ci_action(&probe, true, false), CiAction::Wait);
}

#[test]
fn in_progress_proceeds_with_yes() {
    let probe = CiProbe::InProgress { pr_number: 7 };
    assert_eq!(decide_ci_action(&probe, false, true), CiAction::Proceed);
}

#[test]
fn red_cancels_interactively() {
    let probe = CiProbe::Red {
        pr_number: 7,
        failed_summary: "build".to_string(),
    };
    match decide_ci_action(&probe, false, false) {
        CiAction::Cancel(msg) => {
            assert!(msg.contains("PR-7"), "msg: {msg}");
            assert!(msg.contains("RED"), "msg: {msg}");
            assert!(msg.contains("fixups"), "msg: {msg}");
        }
        other => panic!("expected Cancel, got {:?}", other),
    }
}

#[test]
fn red_proceeds_with_yes_but_warns() {
    let probe = CiProbe::Red {
        pr_number: 7,
        failed_summary: "build".to_string(),
    };
    // --yes acknowledges the user is non-interactive; we still
    // print the warning but don't block.
    assert_eq!(decide_ci_action(&probe, false, true), CiAction::Proceed);
}

// --- parse_ci_probe ---

#[test]
fn parse_empty_array_is_no_signal() {
    let probe = parse_ci_probe("[]");
    assert!(matches!(probe, CiProbe::NoSignal(_)));
}

#[test]
fn parse_pr_no_checks() {
    let json = r#"[{"number": 42, "statusCheckRollup": []}]"#;
    match parse_ci_probe(json) {
        CiProbe::PrNoChecks { pr_number } => assert_eq!(pr_number, 42),
        other => panic!("expected PrNoChecks, got {:?}", other),
    }
}

#[test]
fn parse_pr_no_rollup_field() {
    let json = r#"[{"number": 42}]"#;
    match parse_ci_probe(json) {
        CiProbe::PrNoChecks { pr_number } => assert_eq!(pr_number, 42),
        other => panic!("expected PrNoChecks, got {:?}", other),
    }
}

#[test]
fn parse_all_green_checkruns() {
    let json = r#"[{"number": 7, "statusCheckRollup": [
            {"name": "build", "status": "COMPLETED", "conclusion": "SUCCESS"},
            {"name": "test",  "status": "COMPLETED", "conclusion": "SUCCESS"}
        ]}]"#;
    assert_eq!(parse_ci_probe(json), CiProbe::Green { pr_number: 7 });
}

#[test]
fn parse_one_in_progress_is_in_progress() {
    let json = r#"[{"number": 7, "statusCheckRollup": [
            {"name": "build", "status": "COMPLETED",   "conclusion": "SUCCESS"},
            {"name": "test",  "status": "IN_PROGRESS", "conclusion": ""}
        ]}]"#;
    assert_eq!(parse_ci_probe(json), CiProbe::InProgress { pr_number: 7 });
}

#[test]
fn parse_any_failure_is_red() {
    let json = r#"[{"number": 7, "statusCheckRollup": [
            {"name": "build", "status": "COMPLETED", "conclusion": "SUCCESS"},
            {"name": "lint",  "status": "COMPLETED", "conclusion": "FAILURE"}
        ]}]"#;
    match parse_ci_probe(json) {
        CiProbe::Red {
            pr_number,
            failed_summary,
        } => {
            assert_eq!(pr_number, 7);
            assert!(failed_summary.contains("lint"), "summary: {failed_summary}");
        }
        other => panic!("expected Red, got {:?}", other),
    }
}

#[test]
fn parse_status_context_shape() {
    // Older / classic status-API checks come back as
    // {state: SUCCESS|FAILURE|PENDING, context: ...} instead of
    // {status, conclusion, name}. We support both.
    let json = r#"[{"number": 7, "statusCheckRollup": [
            {"context": "ci/circleci", "state": "FAILURE"}
        ]}]"#;
    match parse_ci_probe(json) {
        CiProbe::Red { pr_number, .. } => assert_eq!(pr_number, 7),
        other => panic!("expected Red, got {:?}", other),
    }
}

#[test]
fn parse_red_summary_truncates_when_many_failed() {
    let json = r#"[{"number": 7, "statusCheckRollup": [
            {"name": "a", "status": "COMPLETED", "conclusion": "FAILURE"},
            {"name": "b", "status": "COMPLETED", "conclusion": "FAILURE"},
            {"name": "c", "status": "COMPLETED", "conclusion": "FAILURE"},
            {"name": "d", "status": "COMPLETED", "conclusion": "FAILURE"},
            {"name": "e", "status": "COMPLETED", "conclusion": "FAILURE"}
        ]}]"#;
    match parse_ci_probe(json) {
        CiProbe::Red { failed_summary, .. } => {
            assert!(
                failed_summary.contains("and 2 more"),
                "summary: {failed_summary}"
            );
        }
        other => panic!("expected Red, got {:?}", other),
    }
}

#[test]
fn parse_malformed_is_no_signal() {
    assert!(matches!(parse_ci_probe("not json"), CiProbe::NoSignal(_)));
    assert!(matches!(parse_ci_probe(""), CiProbe::NoSignal(_)));
}
