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
fn parse_zero_pr_number_is_no_signal() {
    let probe = parse_ci_probe(r#"[{"number": 0, "statusCheckRollup": []}]"#);
    assert!(
        matches!(probe, CiProbe::NoSignal(_)),
        "PR-0 must not be treated as a reviewable PR: {probe:?}"
    );
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

// --- BUG-1455: a partial rollup must never read as a terminal verdict ---
//
// `gh`'s rollup carries no `isRequired` flag, so `parse_ci_probe` cannot
// distinguish a required check from an optional one by name. What it CAN
// always tell is concluded vs. still-running, so the table below is framed
// on that axis: any check still in progress keeps the verdict open,
// regardless of what has already concluded. `merge-hold-gate` (fails by
// construction while a supervised hold is active) and `Build` (the
// build/test check that actually decides code health) are the two real
// check names from the observed incident, standing in for "a fast-failing
// gate-style check" and "the code-health check" respectively.

/// The exact rollup observed live on BUG-1291 / PR #2001: `merge-hold-gate`
/// has already concluded FAILURE while `Build` is still IN_PROGRESS. Before
/// the fix this returned `Red` — a terminal verdict — 32 seconds after the
/// CI phase started, while the check that actually measures code health
/// hadn't reported in yet.
// trace:BUG-1455 | ai:claude
#[test]
fn optional_fail_with_required_pending_is_not_terminal() {
    let json = r#"[{"number": 2001, "statusCheckRollup": [
            {"name": "merge-hold-gate", "status": "COMPLETED",   "conclusion": "FAILURE"},
            {"name": "Build",           "status": "IN_PROGRESS", "conclusion": ""}
        ]}]"#;
    assert_eq!(
        parse_ci_probe(json),
        CiProbe::InProgress { pr_number: 2001 },
        "a concluded failure must not end the wait while another check is still running"
    );
}

/// Once nothing is left running, a concluded failure is reported as it
/// always was: the fast-fail case a required check going red with no other
/// check pending.
// trace:BUG-1455 | ai:claude
#[test]
fn required_fail_with_nothing_pending_is_red() {
    let json = r#"[{"number": 2001, "statusCheckRollup": [
            {"name": "merge-hold-gate", "status": "COMPLETED", "conclusion": "FAILURE"},
            {"name": "Build",           "status": "COMPLETED", "conclusion": "SUCCESS"}
        ]}]"#;
    match parse_ci_probe(json) {
        CiProbe::Red {
            pr_number,
            failed_summary,
        } => {
            assert_eq!(pr_number, 2001);
            assert!(
                failed_summary.contains("merge-hold-gate"),
                "summary: {failed_summary}"
            );
        }
        other => panic!("expected Red, got {other:?}"),
    }
}

/// A check still queued/running with nothing concluded yet is the ordinary
/// in-progress case, unaffected by the fix.
// trace:BUG-1455 | ai:claude
#[test]
fn required_pending_with_nothing_concluded_is_in_progress() {
    let json = r#"[{"number": 2001, "statusCheckRollup": [
            {"name": "Build", "status": "QUEUED", "conclusion": ""}
        ]}]"#;
    assert_eq!(
        parse_ci_probe(json),
        CiProbe::InProgress { pr_number: 2001 }
    );
}

/// Every check concluded successfully — Green, unaffected by the fix.
// trace:BUG-1455 | ai:claude
#[test]
fn all_pass_is_green() {
    let json = r#"[{"number": 2001, "statusCheckRollup": [
            {"name": "merge-hold-gate", "status": "COMPLETED", "conclusion": "SUCCESS"},
            {"name": "Build",           "status": "COMPLETED", "conclusion": "SUCCESS"}
        ]}]"#;
    assert_eq!(parse_ci_probe(json), CiProbe::Green { pr_number: 2001 });
}

/// An unreadable/empty rollup is `NoSignal`, never a false Green or Red.
// trace:BUG-1455 | ai:claude
#[test]
fn unknown_rollup_is_no_signal() {
    assert!(matches!(parse_ci_probe("not json"), CiProbe::NoSignal(_)));
    assert!(matches!(
        parse_ci_probe(r#"[{"number": 2001}]"#),
        CiProbe::PrNoChecks { pr_number: 2001 }
    ));
}

// BUG-1250: the exact stderr emitted by `gh` for a connect failure must be
// classified as retryable; exhaustion must close the gate, never proceed.
// trace:BUG-1250 | ai:codex
#[test]
fn gh_connect_error_retries_then_becomes_unavailable() {
    let reason = "gh pr list failed: error connecting to api.github.com\ncheck your internet connection or https://githubstatus.com";
    let patterns: Vec<String> = crate::network_retry::default_transient_patterns()
        .iter()
        .map(|pattern| (*pattern).to_string())
        .collect();

    assert_eq!(
        decide_ci_probe_failure(reason, 1, 3, &patterns),
        CiProbeFailureAction::Retry
    );
    assert_eq!(
        decide_ci_probe_failure(reason, 2, 3, &patterns),
        CiProbeFailureAction::Retry
    );
    assert_eq!(
        decide_ci_probe_failure(reason, 3, 3, &patterns),
        CiProbeFailureAction::Unavailable
    );
}

#[test]
fn hard_ci_probe_failure_is_immediately_unavailable() {
    let patterns: Vec<String> = crate::network_retry::default_transient_patterns()
        .iter()
        .map(|pattern| (*pattern).to_string())
        .collect();
    assert_eq!(
        decide_ci_probe_failure("gh pr list failed: HTTP 401", 1, 3, &patterns),
        CiProbeFailureAction::Unavailable
    );
}

// --- TASK-1453: absolute-ceiling verdict — Red-with-known-failure vs honest NoSignal ---
//
// `ci_ceiling_verdict_from_rollup` is the pure decision `wait_for_ci_terminal`
// consults only once it has already hit its absolute ceiling. It must tell a
// stuck-pending-forever check sitting next to an already-concluded failure
// (report Red, name both) apart from stuck-pending alone (stay honest
// NoSignal — the caller falls back to its existing message).

/// The exact shape this follows from: `merge-hold-gate` concluded FAILURE,
/// `Build` never concludes. At the ceiling this must surface as Red with
/// `merge-hold-gate` named and `Build` listed as still pending — not the
/// uninformative "giving up" NoSignal.
// trace:TASK-1453 | ai:claude
#[test]
fn ceiling_with_known_failure_and_stuck_pending_is_red_with_summary() {
    let json = r#"[{"number": 2001, "statusCheckRollup": [
            {"name": "merge-hold-gate", "status": "COMPLETED",   "conclusion": "FAILURE"},
            {"name": "Build",           "status": "IN_PROGRESS", "conclusion": ""}
        ]}]"#;
    match ci_ceiling_verdict_from_rollup(json) {
        Some(CiProbe::Red {
            pr_number,
            failed_summary,
        }) => {
            assert_eq!(pr_number, 2001);
            assert!(
                failed_summary.contains("merge-hold-gate"),
                "summary: {failed_summary}"
            );
            assert!(
                failed_summary.contains("Build"),
                "stuck check should still be named as pending: {failed_summary}"
            );
        }
        other => panic!("expected Some(Red), got {other:?}"),
    }
}

/// Nothing concluded — every check is still pending/queued. This is the
/// genuine "we truly don't know" case, so the ceiling must NOT invent a Red
/// verdict; the caller keeps its existing NoSignal.
// trace:TASK-1453 | ai:claude
#[test]
fn ceiling_with_stuck_pending_alone_stays_none() {
    let json = r#"[{"number": 2001, "statusCheckRollup": [
            {"name": "Build", "status": "IN_PROGRESS", "conclusion": ""}
        ]}]"#;
    assert_eq!(ci_ceiling_verdict_from_rollup(json), None);
}

/// A concluded failure with nothing else pending is still Red (no spurious
/// "still pending" note appended).
// trace:TASK-1453 | ai:claude
#[test]
fn ceiling_with_only_concluded_failure_is_red_without_pending_note() {
    let json = r#"[{"number": 2001, "statusCheckRollup": [
            {"name": "lint", "status": "COMPLETED", "conclusion": "FAILURE"}
        ]}]"#;
    match ci_ceiling_verdict_from_rollup(json) {
        Some(CiProbe::Red { failed_summary, .. }) => {
            assert!(
                !failed_summary.contains("still pending"),
                "summary: {failed_summary}"
            );
        }
        other => panic!("expected Some(Red), got {other:?}"),
    }
}

/// Malformed / empty / non-GitHub-shaped JSON degrades to `None` — the safe
/// fallback that preserves today's NoSignal behavior.
// trace:TASK-1453 | ai:claude
#[test]
fn ceiling_verdict_degrades_to_none_on_unparsable_json() {
    assert_eq!(ci_ceiling_verdict_from_rollup(""), None);
    assert_eq!(ci_ceiling_verdict_from_rollup("[]"), None);
    assert_eq!(ci_ceiling_verdict_from_rollup("not json"), None);
}
