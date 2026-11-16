//! STORY-2809: full-tier CI evidence on one exact commit, and the label
//! request that starts a full-tier run. Fake check listings and a fake forge
//! CLI runner; no network. `AC<n>` names the story's acceptance criterion a
//! test covers.
// trace:STORY-2809 | ai:claude
use super::*;
use crate::forge::CheckOutcome::{Failed, Passed, Pending, Skipped};
use crate::forge::CommitCheck;

const SHA: &str = "e33fec11d936d4cc6ce450719a506a68932915eb";

fn checks(rows: &[(&str, CheckOutcome, Option<&str>)]) -> CommitChecks {
    CommitChecks {
        sha: SHA.to_string(),
        checks: rows
            .iter()
            .map(|(name, outcome, run)| CommitCheck {
                name: name.to_string(),
                outcome: *outcome,
                run_id: run.map(str::to_string),
                workflow_id: Some("ci".to_string()),
            })
            .collect(),
    }
}

/// A finished FAST run: its aggregate is green, and it proves nothing.
fn fast_run(run: &'static str) -> Vec<(&'static str, CheckOutcome, Option<&'static str>)> {
    vec![
        ("ci-tier: fast", Passed, Some(run)),
        ("CI (fast gates)", Passed, Some(run)),
        ("CI (fast compile)", Passed, Some(run)),
        ("Build (ubuntu-latest)", Passed, Some(run)),
    ]
}

fn full_run(
    run: &'static str,
    shard: CheckOutcome,
    aggregate: Option<CheckOutcome>,
) -> Vec<(&'static str, CheckOutcome, Option<&'static str>)> {
    let mut rows = vec![
        ("ci-tier: full", Passed, Some(run)),
        ("CI (lib-0)", Passed, Some(run)),
        ("CI (cli-1)", shard, Some(run)),
        ("CI (fast gates)", Skipped, Some(run)),
    ];
    if let Some(outcome) = aggregate {
        rows.push(("Build (ubuntu-latest)", outcome, Some(run)));
    }
    rows
}

/// AC2: tieredness is the repository's, not the commit's. A tiered
/// repository refuses a head whose checks carry no `ci-tier` check at all.
#[test]
fn a_tiered_repository_refuses_a_head_with_checks_but_no_tier_check() {
    // trace:STORY-b6b1278890774ef993d2ebc327862470-732.accf1a65 | ai:claude
    let unmarked = checks(&[("Build (ubuntu-latest)", Passed, Some("1"))]);
    assert_eq!(judge(&unmarked, true), FullTier::Missing);
    let why = admission_refusal(&judge(&unmarked, true), SHA).unwrap();
    assert!(why.contains("no full-tier CI run exists"), "{why}");
    assert_eq!(judge(&checks(&[]), true), FullTier::Missing);

    // An untiered repository has no full run to require, but a head with no
    // check at all is still refused, and a tier marker on the commit counts.
    assert_eq!(judge(&unmarked, false), FullTier::Untiered);
    assert_eq!(admission_refusal(&FullTier::Untiered, SHA), None);
    assert_eq!(judge(&checks(&[]), false), FullTier::Missing);
    assert_eq!(judge(&checks(&fast_run("10")), false), FullTier::Missing);
}

/// AC2: the workflow files decide whether the repository is tiered.
#[test]
fn a_repository_is_tiered_when_a_workflow_defines_the_tier_job() {
    // trace:STORY-b6b1278890774ef993d2ebc327862470-732.accf1a65 | ai:claude
    let dir = tempfile::tempdir().unwrap();
    assert!(!repo_is_tiered(dir.path()).unwrap());
    let workflows = dir.path().join(".github/workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(
        workflows.join("lint.yml"),
        "jobs:\n  lint:\n    name: Lint\n",
    )
    .unwrap();
    assert!(!repo_is_tiered(dir.path()).unwrap());
    std::fs::write(
        workflows.join("ci.yml"),
        "jobs:\n  tier:\n    name: \"ci-tier: ${{ 'full' }}\"\n",
    )
    .unwrap();
    assert!(repo_is_tiered(dir.path()).unwrap());
    // AIDA's own workflow is tiered.
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    assert!(repo_is_tiered(&repo).unwrap());
}

/// AC2: a refusal names the missing run.
#[test]
fn a_green_fast_run_is_not_a_full_tier_run() {
    // trace:STORY-b6b1278890774ef993d2ebc327862470-732.accf1a65 | ai:claude
    assert_eq!(judge(&checks(&fast_run("10")), true), FullTier::Missing);
    let why = admission_refusal(&FullTier::Missing, SHA).unwrap();
    assert!(
        why.contains("no full-tier CI run exists at its head e33fec1"),
        "{why}"
    );
    assert!(
        why.contains("`ci-tier: full`") && why.contains("`ci:full`"),
        "{why}"
    );
}

/// AC2: only a green full-tier run at the exact head admits.
#[test]
fn a_full_run_is_green_only_when_its_own_aggregate_passed() {
    // trace:STORY-b6b1278890774ef993d2ebc327862470-732.accf1a65 | ai:claude
    let mut rows = fast_run("10");
    rows.extend(full_run("11", Passed, Some(Passed)));
    assert_eq!(
        judge(&checks(&rows), true),
        FullTier::Green { run: "11".into() }
    );
    assert_eq!(admission_refusal(&judge(&checks(&rows), true), SHA), None);

    // Lanes still running, and the aggregate that registers after them.
    let mut rows = fast_run("10");
    rows.extend(full_run("11", Pending, None));
    assert_eq!(
        judge(&checks(&rows), true),
        FullTier::Pending {
            run: "11".into(),
            waiting: vec!["CI (cli-1)".into()]
        }
    );
    let mut rows = fast_run("10");
    rows.extend(full_run("11", Passed, None));
    assert_eq!(
        judge(&checks(&rows), true),
        FullTier::Pending {
            run: "11".into(),
            waiting: vec!["Build (ubuntu-latest)".into()]
        }
    );

    // The FAST run's green aggregate never stands in for a red full run.
    let mut rows = fast_run("10");
    rows.extend(full_run("11", Failed, Some(Failed)));
    let tier = judge(&checks(&rows), true);
    assert_eq!(
        tier,
        FullTier::Failed {
            run: "11".into(),
            failing: vec!["CI (cli-1)".into(), "Build (ubuntu-latest)".into()]
        }
    );
    let why = admission_refusal(&tier, SHA).unwrap();
    assert!(
        why.contains("run 11") && why.contains("CI (cli-1)"),
        "{why}"
    );
}

/// AC2
#[test]
fn a_green_rerun_at_the_same_head_outranks_an_earlier_red_or_cancelled_run() {
    // trace:STORY-b6b1278890774ef993d2ebc327862470-732.accf1a65 | ai:claude
    let mut rows = full_run("11", Failed, Some(Failed));
    rows.extend(full_run("12", Passed, Some(Passed)));
    assert_eq!(
        judge(&checks(&rows), true),
        FullTier::Green { run: "12".into() }
    );
    // A newer run still in flight is reported ahead of the older failure.
    let mut rows = full_run("11", Failed, Some(Failed));
    rows.extend(full_run("12", Pending, None));
    assert!(matches!(judge(&checks(&rows), true), FullTier::Pending { run, .. } if run == "12"));
}

/// A label event that leaves the tier alone: every job skipped, the tier job
/// under its label-event name.
fn label_event_run(run: &'static str) -> Vec<(&'static str, CheckOutcome, Option<&'static str>)> {
    vec![
        ("label event (CI tier unchanged)", Skipped, Some(run)),
        ("label event (fast gates not rerun)", Skipped, Some(run)),
        ("CI (${{ matrix.shard }})", Skipped, Some(run)),
        ("label event (Build not rerun)", Skipped, Some(run)),
    ]
}

/// AC2: a later label-event run at the same head (a merge-hold toggle)
/// neither unconfirms nor replaces the full run, and proves nothing alone.
#[test]
fn a_label_event_run_leaves_the_full_tier_verdict_alone() {
    // trace:STORY-b6b1278890774ef993d2ebc327862470-732.accf1a65 | ai:claude
    let with_label_event = |mut rows: Vec<_>| {
        rows.extend(label_event_run("13"));
        checks(&rows)
    };
    let green = with_label_event(full_run("12", Passed, Some(Passed)));
    assert_eq!(judge(&green, true), FullTier::Green { run: "12".into() });
    assert_eq!(
        crate::forge::judge_commit_checks(&green, &["Build (ubuntu-latest)"]),
        crate::forge::CommitCi::Green
    );
    let red = with_label_event(full_run("12", Failed, Some(Failed)));
    assert!(matches!(judge(&red, true), FullTier::Failed { run, .. } if run == "12"));
    let alone = with_label_event(Vec::new());
    assert_eq!(judge(&alone, true), FullTier::Missing);
    assert_eq!(
        crate::forge::judge_commit_checks(&alone, &[]),
        crate::forge::CommitCi::NoChecks
    );
}

/// AC2
#[test]
fn a_marker_without_a_run_or_with_a_second_tier_proves_nothing() {
    // trace:STORY-b6b1278890774ef993d2ebc327862470-732.accf1a65 | ai:claude
    let unowned = checks(&[
        ("ci-tier: full", Passed, None),
        ("Build (ubuntu-latest)", Passed, None),
    ]);
    assert_eq!(judge(&unowned, true), FullTier::Missing);
    let mut rows = full_run("11", Passed, Some(Passed));
    rows.push(("ci-tier: fast", Passed, Some("11")));
    assert_eq!(judge(&checks(&rows), true), FullTier::Missing);
}

/// AC2
#[test]
fn check_runs_carry_the_workflow_run_that_owns_them() {
    // trace:STORY-b6b1278890774ef993d2ebc327862470-732.accf1a65 | ai:claude
    let page = serde_json::json!({"total_count": 2, "check_runs": [
        {"name": "ci-tier: full", "status": "completed", "conclusion": "success", "head_sha": SHA,
         "details_url": "https://github.com/o/r/actions/runs/777/job/5"},
        {"name": "external", "status": "completed", "conclusion": "success", "head_sha": SHA,
         "details_url": "https://example.test/build/9"},
    ]});
    let (_, runs) = crate::forge::parse_gh_check_runs(&page, SHA).unwrap();
    assert_eq!(runs[0].run_id.as_deref(), Some("777"));
    assert_eq!(runs[1].run_id, None);
}

fn project(origin: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut commands = vec![vec!["init", "-q"]];
    if let Some(url) = origin {
        commands.push(vec!["remote", "add", "origin", url]);
    }
    for args in commands {
        assert!(crate::test_git::git_command()
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    dir
}

fn github_project() -> tempfile::TempDir {
    let dir = project(Some("git@github.com:o/r.git"));
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(
        dir.path().join(".aida/config.toml"),
        "[forge]\nprovider = \"github\"\n",
    )
    .unwrap();
    dir
}

fn view(labels: &[&str], rollup: serde_json::Value) -> serde_json::Value {
    let labels: Vec<_> = labels
        .iter()
        .map(|name| serde_json::json!({"name": name}))
        .collect();
    serde_json::json!({"labels": labels, "statusCheckRollup": rollup})
}

fn check_run(name: &str, status: &str) -> serde_json::Value {
    serde_json::json!({"__typename": "CheckRun", "name": name, "status": status})
}

/// Run a request against a fake `gh`; returns the outcome and every call.
fn request(view: serde_json::Value, add_ok: bool) -> (Result<bool, String>, Vec<String>) {
    let dir = github_project();
    let calls = std::cell::RefCell::new(Vec::new());
    let record = |cli: &str, args: &[String]| {
        calls.borrow_mut().push(format!("{cli} {}", args.join(" ")));
    };
    let outcome = request_full_ci_with(
        dir.path(),
        41,
        |_, cli, args| {
            record(cli, args);
            Ok((true, view.to_string()))
        },
        |_, cli, args| {
            record(cli, args);
            let ok = add_ok || !args.contains(&"--add-label".to_string());
            Ok((ok, "label not found".to_string()))
        },
    );
    (outcome, calls.into_inner())
}

const VIEW: &str = "gh pr view 41 -R o/r --json labels,statusCheckRollup";

/// AC1: an unlabelled PR gets the label, in one edit and with no removal.
#[test]
fn a_request_adds_the_label_only_when_the_pr_does_not_carry_it() {
    // trace:STORY-b6b1278890774ef993d2ebc327862470-732.acbd1c12 | ai:claude
    for rollup in [
        serde_json::Value::Null,
        serde_json::json!([check_run("ci-tier: fast", "COMPLETED")]),
        // A FAST run in flight gives way to the full run the label starts.
        serde_json::json!([check_run("CI (fast gates)", "IN_PROGRESS")]),
    ] {
        let (outcome, calls) = request(view(&["merge-hold"], rollup), true);
        assert_eq!(outcome, Ok(true));
        assert_eq!(
            calls,
            vec![VIEW, "gh pr edit 41 -R o/r --add-label ci:full"]
        );
    }
}

/// AC1: no label event while CI is in flight on the head, and none on a head
/// that already has a full-tier run: either would cancel a run.
#[test]
fn a_request_never_relabels_a_head_whose_run_it_would_cancel() {
    // trace:STORY-b6b1278890774ef993d2ebc327862470-732.acbd1c12 | ai:claude
    for (labels, rollup) in [
        (
            vec!["ci:full"],
            serde_json::json!([check_run("CI (fast gates)", "IN_PROGRESS")]),
        ),
        (
            vec!["ci:full"],
            serde_json::json!([{"__typename": "StatusContext", "context": "x", "state": "PENDING"}]),
        ),
        (
            vec!["ci:full"],
            serde_json::json!([
                check_run("ci-tier: full", "COMPLETED"),
                check_run("CI (lib-0)", "QUEUED")
            ]),
        ),
        // The label was dropped, but the head's full run is still there.
        (
            vec![],
            serde_json::json!([check_run("ci-tier: full", "COMPLETED")]),
        ),
    ] {
        let (outcome, calls) = request(view(&labels, rollup), true);
        assert_eq!(outcome, Ok(false));
        assert_eq!(calls, vec![VIEW]);
    }
}

/// AC1: a label left by an earlier head's approval is re-applied, because
/// adding a label the PR already carries starts nothing.
#[test]
fn a_request_reapplies_a_stale_label_when_nothing_is_running() {
    // trace:STORY-b6b1278890774ef993d2ebc327862470-732.acbd1c12 | ai:claude
    let rollup = serde_json::json!([
        check_run("ci-tier: fast", "COMPLETED"),
        {"__typename": "StatusContext", "context": "x", "state": "SUCCESS"}
    ]);
    let (outcome, calls) = request(view(&["ci:full"], rollup), true);
    assert_eq!(outcome, Ok(true));
    assert_eq!(
        calls,
        vec![
            VIEW,
            "gh pr edit 41 -R o/r --remove-label ci:full",
            "gh pr edit 41 -R o/r --add-label ci:full",
        ]
    );
}

/// AC1
#[test]
fn a_failed_view_or_label_add_is_reported_and_a_project_without_github_asks_nothing() {
    // trace:STORY-b6b1278890774ef993d2ebc327862470-732.acbd1c12 | ai:claude
    let (outcome, _) = request(view(&[], serde_json::json!([])), false);
    let err = outcome.unwrap_err();
    assert!(
        err.contains("--add-label ci:full") && err.contains("label not found"),
        "{err}"
    );
    let (outcome, calls) = request(serde_json::json!({"labels": 3}), true);
    assert!(outcome.unwrap_err().contains("`labels`"));
    assert_eq!(calls, vec![VIEW]);

    let dir = github_project();
    let err = request_full_ci_with(
        dir.path(),
        41,
        |_, _, _| Ok((false, "HTTP 401".to_string())),
        |_, _, _| panic!("no edit after a failed view"),
    )
    .unwrap_err();
    assert!(
        err.contains("gh pr view 41") && err.contains("HTTP 401"),
        "{err}"
    );

    let plain = project(None);
    let requested = request_full_ci_with(
        plain.path(),
        41,
        |_, _, _| panic!("no forge call without a GitHub forge"),
        |_, _, _| panic!("no forge call without a GitHub forge"),
    );
    assert_eq!(requested, Ok(false));
}

/// Which PR `aida review record` asks a full-tier run for, with the request
/// injected.
fn requested_after(verdict: &str, findings: &[String], pr: Option<u64>) -> Vec<u64> {
    let asked = std::cell::RefCell::new(Vec::new());
    crate::request_full_ci_after_approval(
        std::path::Path::new("/nonexistent"),
        verdict,
        findings,
        pr,
        |_, n| {
            asked.borrow_mut().push(n);
            Ok(true)
        },
    );
    asked.into_inner()
}

fn deferred_finding() -> String {
    serde_json::json!({"text": "F", "severity": "minor", "disposition": "deferred",
        "rationale": "safe follow-up"})
    .to_string()
}

/// AC1: an approval recorded with `--pr` starts the full-tier run, and so
/// does a verdict whose every finding is deferred.
#[test]
fn an_approving_verdict_with_a_pr_requests_full_tier_ci() {
    // trace:STORY-b6b1278890774ef993d2ebc327862470-732.acbd1c12 | ai:claude
    assert_eq!(requested_after("approved", &[], Some(41)), vec![41]);
    assert_eq!(
        requested_after("approved", &[deferred_finding()], Some(41)),
        vec![41]
    );
    assert_eq!(
        requested_after("request-changes", &[deferred_finding()], Some(41)),
        vec![41]
    );
}

/// AC1: nothing else starts one, and a failed request does not fail the
/// verdict.
#[test]
fn a_non_approving_verdict_or_no_pr_requests_nothing() {
    // trace:STORY-b6b1278890774ef993d2ebc327862470-732.acbd1c12 | ai:claude
    let blocking = ["a blocking finding".to_string()];
    for verdict in [
        "request-changes",
        "rejected",
        "waiting",
        "approved-with-nits",
    ] {
        assert!(
            requested_after(verdict, &blocking, Some(41)).is_empty(),
            "{verdict}"
        );
    }
    for verdict in ["rejected", "waiting"] {
        assert!(
            requested_after(verdict, &[], Some(41)).is_empty(),
            "{verdict}"
        );
    }
    assert!(requested_after("approved", &blocking, Some(41)).is_empty());
    assert!(requested_after("approved", &[], None).is_empty());
    crate::request_full_ci_after_approval(
        std::path::Path::new("/nonexistent"),
        "approved",
        &[],
        Some(41),
        |_, _| Err("gh is not signed in".to_string()),
    );
}

/// Docs: the landing page states the gate in the words the train prints, and
/// `make check-ci-list` describes the approved-head tier.
#[test]
fn the_landing_page_and_the_ci_gate_listing_state_the_gate() {
    // trace:STORY-b6b1278890774ef993d2ebc327862470-732.Docs | ai:claude
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let page = std::fs::read_to_string(repo.join("docs/advanced/landing.md")).unwrap();
    let refusal = admission_refusal(&FullTier::Missing, SHA).unwrap();
    for stated in [
        "no full-tier CI run exists at its head",
        FULL_MARKER,
        AGGREGATE,
        FULL_LABEL,
        "--skip-full-check",
        "train-admit-skip-full-check",
    ] {
        assert!(
            page.contains(stated),
            "landing.md does not state `{stated}`"
        );
    }
    assert!(refusal.contains("no full-tier CI run exists at its head"));
    let listing = std::fs::read_to_string(repo.join("scripts/check-ci-gates.py")).unwrap();
    assert!(listing.contains("Approved-head tier:") && listing.contains(FULL_LABEL));
}

/// AC2: the override is logged.
#[test]
fn the_skip_event_records_the_pr_head_and_reason() {
    // trace:STORY-b6b1278890774ef993d2ebc327862470-732.accf1a65 | ai:claude
    let event: serde_json::Value =
        serde_json::from_str(&skip_event("2026-11-14T12:00:00Z", 41, SHA, "why")).unwrap();
    assert_eq!(event["command"], "aida pr train");
    assert_eq!(event["step"], "train-admit-skip-full-check");
    assert_eq!(event["pr"], 41);
    assert_eq!(event["head_sha"], SHA);
    assert_eq!(event["detail"], "why");
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-732.accf1a65 | ai:codex
#[test]
fn full_tier_discovery_without_a_workflows_directory_is_untiered() {
    let dir = tempfile::tempdir().unwrap();
    let checks = checks(&[("lint", Passed, Some("1"))]);
    assert_eq!(
        judge(&checks, repo_is_tiered(dir.path()).unwrap()),
        FullTier::Untiered
    );
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-732.accf1a65 | ai:codex
#[test]
fn full_tier_discovery_refuses_directory_entry_and_file_read_errors() {
    use std::io::{Error, ErrorKind};
    let denied = || Error::new(ErrorKind::PermissionDenied, "unreadable workflow evidence");
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".github")).unwrap();
    std::fs::write(dir.path().join(".github/workflows"), "not a directory").unwrap();
    assert!(repo_is_tiered(dir.path()).is_err());
    assert!(repo_is_tiered_with([Err(denied())], |_| Ok(String::new())).is_err());
    let paths = [
        Ok(std::path::PathBuf::from("ci.yml")),
        Ok(std::path::PathBuf::from("other.yml")),
    ];
    assert!(
        repo_is_tiered_with(paths, |p| if p.ends_with("ci.yml") {
            Ok("ci-tier: full".into())
        } else {
            Err(denied())
        })
        .is_err(),
        "even a tier marker must not hide later read failures"
    );
}

// trace:TASK-3105 | ai:codex
#[test]
fn only_a_verified_completed_cancelled_full_run_is_requested_again() {
    let dir = github_project();
    for labels in [vec![], vec!["ci:full"]] {
        for (status, conclusion, retries) in [
            ("completed", "cancelled", true),
            ("completed", "success", false),
            ("completed", "failure", false),
            ("in_progress", "cancelled", false),
            ("completed", "", false),
        ] {
            let marker = serde_json::json!({"name": FULL_MARKER, "status": "COMPLETED",
                "detailsUrl": "https://github.com/o/r/actions/runs/777/job/5"});
            let edits = std::cell::RefCell::new(Vec::new());
            let outcome = request_full_ci_with(
                dir.path(),
                41,
                |_, _, args| {
                    if args[0] == "pr" {
                        Ok((true, view(&labels, serde_json::json!([marker])).to_string()))
                    } else {
                        assert_eq!(
                            args,
                            &[
                                "run",
                                "view",
                                "777",
                                "-R",
                                "o/r",
                                "--json",
                                "status,conclusion"
                            ]
                        );
                        Ok((
                            true,
                            serde_json::json!({"status": status, "conclusion": conclusion})
                                .to_string(),
                        ))
                    }
                },
                |_, _, args| {
                    edits.borrow_mut().push(args.to_vec());
                    Ok((true, String::new()))
                },
            );
            assert_eq!(outcome, Ok(retries));
            let edits = edits.into_inner();
            assert_eq!(
                edits.len(),
                if retries {
                    if labels.is_empty() {
                        1
                    } else {
                        2
                    }
                } else {
                    0
                }
            );
            if retries {
                assert!(edits.last().unwrap().iter().any(|a| a == "--add-label"));
                if !labels.is_empty() {
                    assert!(edits[0].iter().any(|a| a == "--remove-label"));
                }
            }
        }
    }
}

// trace:TASK-3105 | ai:codex
#[test]
fn ambiguous_or_running_full_rollups_cannot_trigger_a_retry_probe() {
    let marker = serde_json::json!({"name": FULL_MARKER, "status": "COMPLETED",
        "detailsUrl": "https://github.com/o/r/actions/runs/777/job/5"});
    for rows in [
        serde_json::json!([marker, marker]),
        serde_json::json!([marker, check_run("other", "IN_PROGRESS")]),
        serde_json::json!([marker, {"state": "PENDING"}]),
        serde_json::json!([check_run(FULL_MARKER, "COMPLETED")]),
    ] {
        let (outcome, calls) = request(view(&["ci:full"], rows), true);
        assert_eq!(outcome, Ok(false));
        assert_eq!(calls, vec![VIEW]);
    }
}
