//! TASK-1945: the shared commit-keyed CI reader, over recorded GitHub and
//! GitLab response shapes and a fake API runner. No network.
// trace:TASK-1945 | ai:claude
use super::*;
use serde_json::json;

const SHA: &str = "e33fec11d936d4cc6ce450719a506a68932915eb";
const OTHER: &str = "b63474e3eb000000000000000000000000000000";

fn run(name: &str, status: &str, conclusion: Option<&str>) -> serde_json::Value {
    json!({"name": name, "status": status, "conclusion": conclusion, "head_sha": SHA})
}

fn no_statuses() -> serde_json::Value {
    json!({"state": "pending", "sha": SHA, "total_count": 0, "statuses": []})
}

/// A fake `gh api`: serves `pages` of check runs, then `status`, and
/// records every endpoint asked for.
fn github(
    pages: Vec<serde_json::Value>,
    status: serde_json::Value,
) -> (Result<CommitChecks>, Vec<String>) {
    let mut asked = Vec::new();
    let mut api = |endpoint: &str| -> Result<Vec<u8>> {
        asked.push(endpoint.to_string());
        if endpoint.contains("/status?") {
            return Ok(serde_json::to_vec(&status).unwrap());
        }
        let page: usize = endpoint.rsplit("page=").next().unwrap().parse().unwrap();
        Ok(serde_json::to_vec(&pages[page - 1]).unwrap())
    };
    let result = github_commit_checks("owner/repo", SHA, &mut api);
    (result, asked)
}

#[test]
fn github_check_runs_and_statuses_are_read_by_sha() {
    let page = json!({"total_count": 4, "check_runs": [
        run("CI (lib-0)", "completed", Some("success")),
        run("Release (ubuntu)", "completed", Some("skipped")),
        run("Lint", "completed", Some("neutral")),
        run(CI_AGGREGATOR_CHECK, "completed", Some("success")),
    ]});
    let status = json!({"state": "success", "sha": SHA, "total_count": 1,
        "statuses": [{"context": "deploy/preview", "state": "success"}]});
    let (checks, asked) = github(vec![page], status);
    let checks = checks.unwrap();
    assert_eq!(checks.sha, SHA);
    assert_eq!(checks.checks.len(), 5);
    assert_eq!(
        judge_commit_checks(&checks, &[CI_AGGREGATOR_CHECK]),
        CommitCi::Green
    );
    assert!(asked[0].starts_with(&format!("repos/owner/repo/commits/{SHA}/check-runs?")));
    assert!(asked[1].starts_with(&format!("repos/owner/repo/commits/{SHA}/status?")));
}

#[test]
fn github_pending_failed_and_empty_are_not_green() {
    let (checks, _) = github(
        vec![json!({"total_count": 2, "check_runs": [
            run("CI (lib-0)", "in_progress", None),
            run(CI_AGGREGATOR_CHECK, "queued", None),
        ]})],
        no_statuses(),
    );
    assert_eq!(
        judge_commit_checks(&checks.unwrap(), &[]),
        CommitCi::Pending(vec!["CI (lib-0)".into(), CI_AGGREGATOR_CHECK.into()])
    );
    for conclusion in [
        "failure",
        "cancelled",
        "timed_out",
        "action_required",
        "stale",
    ] {
        let (checks, _) = github(
            vec![
                json!({"total_count": 1, "check_runs": [run("CI", "completed", Some(conclusion))]}),
            ],
            no_statuses(),
        );
        assert_eq!(
            judge_commit_checks(&checks.unwrap(), &[]),
            CommitCi::Failed(vec!["CI".into()]),
            "{conclusion}"
        );
    }
    let (checks, _) = github(
        vec![json!({"total_count": 0, "check_runs": []})],
        no_statuses(),
    );
    assert_eq!(
        judge_commit_checks(&checks.unwrap(), &[]),
        CommitCi::NoChecks
    );
    // A failing commit status fails the commit even when every run passed.
    let (checks, _) = github(
        vec![json!({"total_count": 1, "check_runs": [run("CI", "completed", Some("success"))]})],
        json!({"state": "failure", "sha": SHA, "total_count": 1,
            "statuses": [{"context": "ci/legacy", "state": "error"}]}),
    );
    assert_eq!(
        judge_commit_checks(&checks.unwrap(), &[]),
        CommitCi::Failed(vec!["ci/legacy".into()])
    );
}

/// A run cancelled and superseded on the same commit never reads as red:
/// only the newest run of each check name is judged.
// trace:STORY-2809 | ai:claude
#[test]
fn a_cancelled_earlier_run_does_not_outvote_the_latest_run_of_a_check() {
    let row = |name: &str, outcome, run: Option<&str>| CommitCheck {
        name: name.to_string(),
        outcome,
        run_id: run.map(str::to_string),
        workflow_id: Some("ci".to_string()),
    };
    let on = |checks: Vec<CommitCheck>| CommitChecks {
        sha: SHA.to_string(),
        checks,
    };
    let superseded = on(vec![
        row("CI (lib-0)", CheckOutcome::Failed, Some("10")),
        row("Build", CheckOutcome::Failed, Some("10")),
        row("CI (lib-0)", CheckOutcome::Passed, Some("11")),
        row("Build", CheckOutcome::Passed, Some("11")),
    ]);
    assert_eq!(
        judge_commit_checks(&superseded, &["Build"]),
        CommitCi::Green
    );
    // The newest run is the one judged, in either direction and while it runs.
    let regressed = on(vec![
        row("Build", CheckOutcome::Passed, Some("10")),
        row("Build", CheckOutcome::Failed, Some("11")),
    ]);
    assert_eq!(
        judge_commit_checks(&regressed, &["Build"]),
        CommitCi::Failed(vec!["Build".into()])
    );
    let rerunning = on(vec![
        row("Build", CheckOutcome::Failed, Some("10")),
        row("Build", CheckOutcome::Pending, Some("11")),
    ]);
    assert_eq!(
        judge_commit_checks(&rerunning, &["Build"]),
        CommitCi::Pending(vec!["Build".into()])
    );
    // A run that no newer run repeats a check of, and a check with no owning
    // run, still count.
    let leftover = on(vec![
        row("CI (lib-0)", CheckOutcome::Failed, Some("10")),
        row("Build", CheckOutcome::Passed, Some("11")),
        row("external", CheckOutcome::Failed, None),
    ]);
    assert_eq!(
        judge_commit_checks(&leftover, &["Build"]),
        CommitCi::Failed(vec!["CI (lib-0)".into(), "external".into()])
    );
}

/// Re-applying `ci:full` starts a FAST run (the `unlabeled` event) that the
/// FULL run cancels within seconds. The cancelled run leaves its tier marker
/// and an unexpanded matrix job, names the FULL run never reports. The whole
/// superseded run is dropped, so the approved head reads green.
// trace:STORY-2809 | ai:claude
#[test]
fn a_fast_run_cancelled_by_the_full_run_contributes_nothing() {
    // trace:STORY-b6b1278890774ef993d2ebc327862470-732.acbd1c12 | ai:claude
    const FAST: &str = "43603377514";
    const FULL: &str = "43603377600";
    let row = |name: &str, outcome, run: &str| CommitCheck {
        name: name.to_string(),
        outcome,
        run_id: Some(run.to_string()),
        workflow_id: Some("ci".to_string()),
    };
    let cancelled_fast = || {
        vec![
            row("ci-tier: fast", CheckOutcome::Failed, FAST),
            row("CI (${{ matrix.shard }})", CheckOutcome::Failed, FAST),
            row("Build (ubuntu-latest)", CheckOutcome::Failed, FAST),
        ]
    };
    let full = |build| {
        vec![
            row("ci-tier: full", CheckOutcome::Passed, FULL),
            row("CI (lib-0)", CheckOutcome::Passed, FULL),
            row("Build (ubuntu-latest)", build, FULL),
        ]
    };
    let on = |first: Vec<CommitCheck>, second: Vec<CommitCheck>| CommitChecks {
        sha: SHA.to_string(),
        checks: first.into_iter().chain(second).collect(),
    };
    for required in [&["Build (ubuntu-latest)"][..], &[]] {
        assert_eq!(
            judge_commit_checks(&on(cancelled_fast(), full(CheckOutcome::Passed)), required),
            CommitCi::Green
        );
    }
    // The order the forge lists the rows in does not matter.
    assert_eq!(
        judge_commit_checks(&on(full(CheckOutcome::Passed), cancelled_fast()), &[]),
        CommitCi::Green
    );
    // The newer run is still the one judged.
    assert_eq!(
        judge_commit_checks(&on(cancelled_fast(), full(CheckOutcome::Pending)), &[]),
        CommitCi::Pending(vec!["Build (ubuntu-latest)".into()])
    );
    // A run of another workflow shares no check name with the FULL run, so
    // its failure stands.
    let mut other = on(cancelled_fast(), full(CheckOutcome::Passed));
    other
        .checks
        .push(row("docs", CheckOutcome::Failed, "43603377000"));
    assert_eq!(
        judge_commit_checks(&other, &[]),
        CommitCi::Failed(vec!["docs".into()])
    );
}

#[test]
fn required_check_absent_is_unknown_not_green() {
    let (checks, _) = github(
        vec![
            json!({"total_count": 1, "check_runs": [run("CI (lib-0)", "completed", Some("success"))]}),
        ],
        no_statuses(),
    );
    assert_eq!(
        judge_commit_checks(&checks.unwrap(), &[CI_AGGREGATOR_CHECK]),
        CommitCi::MissingRequired(CI_AGGREGATOR_CHECK.into())
    );
    // A skipped or neutral aggregator (its `if:` did not fire) is not a
    // success: unknown, never green. A skipped optional check still passes
    // beside a check that ran.
    for conclusion in ["skipped", "neutral"] {
        let (checks, _) = github(
            vec![json!({"total_count": 3, "check_runs": [
                run("CI (lib-0)", "completed", Some("success")),
                run("Release (ubuntu)", "completed", Some("skipped")),
                run(CI_AGGREGATOR_CHECK, "completed", Some(conclusion)),
            ]})],
            no_statuses(),
        );
        let checks = checks.unwrap();
        assert_eq!(
            judge_commit_checks(&checks, &[CI_AGGREGATOR_CHECK]),
            CommitCi::MissingRequired(CI_AGGREGATOR_CHECK.into()),
            "{conclusion}"
        );
        assert_eq!(judge_commit_checks(&checks, &[]), CommitCi::Green);
        // With nothing but skipped checks, nothing ran: never green.
        // trace:STORY-2809 | ai:claude
        let (checks, _) = github(
            vec![json!({"total_count": 2, "check_runs": [
                run("Release (ubuntu)", "completed", Some("skipped")),
                run(CI_AGGREGATOR_CHECK, "completed", Some(conclusion)),
            ]})],
            no_statuses(),
        );
        let checks = checks.unwrap();
        for required in [&[CI_AGGREGATOR_CHECK][..], &[]] {
            assert_eq!(
                judge_commit_checks(&checks, required),
                CommitCi::NoChecks,
                "{conclusion}"
            );
        }
    }
}

/// A label event that leaves the tier alone (a merge-hold toggle) starts a
/// run whose every job is skipped. It is newer than the real run on the head
/// and repeats some of its check names, and it must neither displace that
/// run nor read as green by itself. Each case holds with and without a
/// required-check list.
// trace:STORY-2809 | ai:claude
#[test]
fn a_label_event_run_never_supersedes_the_run_that_executed() {
    // trace:STORY-b6b1278890774ef993d2ebc327862470-732.accf1a65 | ai:claude
    const REAL: &str = "43603377600";
    const LABEL: &str = "43603379999";
    const BUILD: &str = "Build (ubuntu-latest)";
    let row = |name: &str, outcome, run: &str| CommitCheck {
        name: name.to_string(),
        outcome,
        run_id: Some(run.to_string()),
        workflow_id: Some("ci".to_string()),
    };
    let real = |build| {
        vec![
            row("ci-tier: full", CheckOutcome::Passed, REAL),
            row("CI (lib-0)", CheckOutcome::Passed, REAL),
            row("CI (fast gates)", CheckOutcome::Skipped, REAL),
            row("Release (${{ matrix.os }})", CheckOutcome::Skipped, REAL),
            row(BUILD, build, REAL),
        ]
    };
    let label_event = || {
        [
            "label event (CI tier unchanged)",
            "label event (fast gates not rerun)",
            "label event (fast compile not rerun)",
            "CI (${{ matrix.shard }})",
            "label event (Build not rerun)",
            "Release (${{ matrix.os }})",
        ]
        .into_iter()
        .map(|name| row(name, CheckOutcome::Skipped, LABEL))
        .collect::<Vec<_>>()
    };
    let on = |first: Vec<CommitCheck>, second: Vec<CommitCheck>| CommitChecks {
        sha: SHA.to_string(),
        checks: first.into_iter().chain(second).collect(),
    };
    for required in [&[BUILD][..], &[]] {
        // (a) A green real run, then a label event: still green, in either
        // listing order.
        assert_eq!(
            judge_commit_checks(&on(real(CheckOutcome::Passed), label_event()), required),
            CommitCi::Green,
            "{required:?}"
        );
        assert_eq!(
            judge_commit_checks(&on(label_event(), real(CheckOutcome::Passed)), required),
            CommitCi::Green,
            "{required:?}"
        );
        // (b) A red real run, then a label event: still red.
        assert_eq!(
            judge_commit_checks(&on(real(CheckOutcome::Failed), label_event()), required),
            CommitCi::Failed(vec![BUILD.into()]),
            "{required:?}"
        );
        // A real run in flight stays pending.
        assert_eq!(
            judge_commit_checks(&on(real(CheckOutcome::Pending), label_event()), required),
            CommitCi::Pending(vec![BUILD.into()]),
            "{required:?}"
        );
        // (c) Only a label-event run on the head: nothing ran, not green.
        assert_eq!(
            judge_commit_checks(&on(label_event(), Vec::new()), required),
            CommitCi::NoChecks,
            "{required:?}"
        );
        // The marker alone marks the run, whatever its other jobs report.
        let mut odd = label_event();
        odd.push(row(BUILD, CheckOutcome::Passed, LABEL));
        assert_eq!(
            judge_commit_checks(&on(odd.clone(), Vec::new()), required),
            CommitCi::NoChecks,
            "{required:?}"
        );
        assert_eq!(
            judge_commit_checks(&on(real(CheckOutcome::Failed), odd), required),
            CommitCi::Failed(vec![BUILD.into()]),
            "{required:?}"
        );
    }
}

#[test]
fn github_pages_until_the_total_is_read() {
    let names: Vec<String> = (0..150).map(|i| format!("CI ({i})")).collect();
    let runs = |range: std::ops::Range<usize>| -> Vec<serde_json::Value> {
        names[range]
            .iter()
            .map(|n| run(n, "completed", Some("success")))
            .collect()
    };
    let (checks, asked) = github(
        vec![
            json!({"total_count": 150, "check_runs": runs(0..100)}),
            json!({"total_count": 150, "check_runs": runs(100..150)}),
        ],
        no_statuses(),
    );
    assert_eq!(checks.unwrap().checks.len(), 150);
    assert_eq!(asked.len(), 3, "two run pages and the status: {asked:?}");
    // A listing that stops short of its own total is refused.
    let (checks, _) = github(
        vec![json!({"total_count": 150, "check_runs": runs(0..50)})],
        no_statuses(),
    );
    assert!(checks.unwrap_err().to_string().contains("incomplete"));
}

#[test]
fn every_failure_is_an_error_never_no_ci() {
    // The CLI failed.
    let mut failing = |_: &str| -> Result<Vec<u8>> { anyhow::bail!("gh: HTTP 502") };
    assert!(github_commit_checks("owner/repo", SHA, &mut failing)
        .unwrap_err()
        .to_string()
        .contains("HTTP 502"));
    assert!(gitlab_commit_checks("group/repo", SHA, &mut failing).is_err());
    // Not JSON.
    let mut garbage = |_: &str| -> Result<Vec<u8>> { Ok(b"<html>".to_vec()) };
    assert!(github_commit_checks("owner/repo", SHA, &mut garbage).is_err());
    // A run on another commit.
    let mut other =
        json!({"total_count": 1, "check_runs": [run("CI", "completed", Some("success"))]});
    other["check_runs"][0]["head_sha"] = json!(OTHER);
    let (checks, _) = github(vec![other], no_statuses());
    assert!(checks.unwrap_err().to_string().contains(OTHER));
    // A status listing for another commit.
    let (checks, _) = github(
        vec![json!({"total_count": 0, "check_runs": []})],
        json!({"state": "success", "sha": OTHER, "total_count": 0, "statuses": []}),
    );
    assert!(checks.is_err());
    // A completed run with no conclusion.
    let (checks, _) = github(
        vec![json!({"total_count": 1, "check_runs": [run("CI", "completed", None)]})],
        no_statuses(),
    );
    assert!(checks.is_err());
}

fn job(id: u64, name: &str, status: &str) -> serde_json::Value {
    json!({"id": id, "name": name, "status": status, "sha": SHA, "allow_failure": false})
}

#[test]
fn gitlab_statuses_keep_the_newest_job_per_name() {
    let rows = vec![
        job(1, "test", "failed"),
        job(5, "test", "success"),
        job(2, "build", "running"),
        json!({"id": 3, "name": "flaky", "status": "failed", "sha": SHA, "allow_failure": true}),
    ];
    let mut asked = Vec::new();
    let mut api = |endpoint: &str| -> Result<Vec<u8>> {
        asked.push(endpoint.to_string());
        Ok(serde_json::to_vec(&rows).unwrap())
    };
    let checks = gitlab_commit_checks("group/sub/repo", SHA, &mut api).unwrap();
    assert!(asked[0].starts_with(&format!(
        "projects/group%2Fsub%2Frepo/repository/commits/{SHA}/statuses?"
    )));
    assert_eq!(
        judge_commit_checks(&checks, &[]),
        CommitCi::Pending(vec!["build".into()])
    );
    let outcomes: Vec<_> = checks
        .checks
        .iter()
        .map(|c| (c.name.as_str(), c.outcome))
        .collect();
    assert_eq!(
        outcomes,
        vec![
            ("build", CheckOutcome::Pending),
            ("flaky", CheckOutcome::Skipped),
            ("test", CheckOutcome::Passed),
        ]
    );
    let mut other = job(9, "test", "success");
    other["sha"] = json!(OTHER);
    assert!(parse_glab_commit_statuses(&[other], SHA).is_err());
    assert!(parse_glab_commit_statuses(&[job(9, "test", "mystery")], SHA).is_err());
}

/// The required aggregator is the check run of CI's `build-result` job. Its
/// name is one expression of a fixed shape: a label event that leaves the CI
/// tier alone reports under a name of its own, every other run reports the
/// aggregator constant. No matrix, so the constant cannot drift from the
/// workflow.
// trace:BUG-2578 | ai:claude
#[test]
fn aggregator_const_matches_the_ci_workflow() {
    // The label-event condition, as ci.yml repeats it on every gated job.
    const LABEL_EVENT: &str = "((github.event.action == 'labeled' || github.event.action == 'unlabeled') && github.event.label.name != 'ci:full')";
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.github/workflows/ci.yml");
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let job = &workflow["jobs"]["build-result"];
    let name = job["name"].as_str().unwrap();
    // `${{ <label event> && '<label name>' || '<aggregator>' }}`, nothing else.
    let branches = name
        .strip_prefix("${{ ")
        .and_then(|rest| rest.strip_suffix("' }}"))
        .and_then(|rest| rest.strip_prefix(LABEL_EVENT))
        .and_then(|rest| rest.strip_prefix(" && '"))
        .and_then(|rest| rest.split_once("' || '"));
    let Some((label_name, other_name)) = branches else {
        panic!("build-result's name is not the expected expression: {name}");
    };
    assert!(
        !label_name.contains('\'') && !other_name.contains('\''),
        "each branch is one string literal: {name}"
    );
    assert_eq!(other_name, CI_AGGREGATOR_CHECK, "{name}");
    // An empty label name is falsy, so the expression would fall through to
    // the aggregator name.
    assert!(!label_name.is_empty(), "{name}");
    assert_ne!(label_name, CI_AGGREGATOR_CHECK, "{name}");
    // The run that reports the label name is the one that skips the job.
    assert_eq!(
        job["if"].as_str(),
        Some(format!("${{{{ always() && !{LABEL_EVENT} }}}}").as_str()),
        "{job:?}"
    );
    assert!(
        job.get("strategy").is_none(),
        "a matrix would change the check name"
    );
    let steps = job["steps"].as_sequence().unwrap();
    assert!(
        steps
            .iter()
            .any(|s| s["name"].as_str() == Some("Require every CI lane")),
        "build-result is the job that requires every CI lane"
    );
}

/// AIDA's own readiness requires the aggregator through its project config
/// (TASK-2201): the configured name is the workflow's job name.
// trace:TASK-1945 | ai:claude (TASK-2201)
#[test]
fn project_config_requires_the_aggregator() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let config: toml::Value =
        toml::from_str(&std::fs::read_to_string(root.join(".aida/config.toml")).unwrap()).unwrap();
    let required = config["review_watch"]["required_checks"]
        .as_array()
        .unwrap();
    assert_eq!(
        required
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![CI_AGGREGATOR_CHECK]
    );
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-408.ac0a4b5e | ai:codex
#[test]
fn work_ci_native_status_is_a_required_train_and_pr_source() {
    const CONTEXT: &str = "aida/work-ci";
    // The relay publishes ordinary native statuses. Exercise the actual
    // commit readers used by verified trains/readiness, including a newer
    // pending attempt superseding a successful result for the same SHA.
    for (state, gl_state, outcome) in [
        ("success", "success", CheckOutcome::Passed),
        ("pending", "pending", CheckOutcome::Pending),
        ("failure", "failed", CheckOutcome::Failed),
    ] {
        let (gh, _) = github(
            vec![json!({"total_count": 0, "check_runs": []})],
            json!({"sha": SHA, "total_count": 1, "statuses": [
                {"context": CONTEXT, "state": state}
            ]}),
        );
        let mut api = |_: &str| {
            Ok(serde_json::to_vec(&json!([
                {"id": 1, "sha": SHA, "name": CONTEXT, "status": "success"},
                {"id": 2, "sha": SHA, "name": CONTEXT, "status": gl_state}
            ]))
            .unwrap())
        };
        let gl = gitlab_commit_checks("1", SHA, &mut api).unwrap();
        for checks in [gh.unwrap(), gl] {
            assert_eq!(checks.checks[0].outcome, outcome);
            assert_eq!(
                judge_commit_checks(&checks, &[CONTEXT]) == CommitCi::Green,
                outcome == CheckOutcome::Passed
            );
            let status = crate::forge::ci_status_from_commit_checks(Ok(checks));
            assert_eq!(
                status.failing_checks.is_empty(),
                outcome != CheckOutcome::Failed
            );
        }
        // PR ship classifies this context through the same required-check
        // rows as workflows. Even an informational wildcard cannot waive it.
        let bucket = match outcome {
            CheckOutcome::Passed => "pass",
            CheckOutcome::Pending => "pending",
            _ => "fail",
        };
        let wire =
            json!([{"name": CONTEXT, "bucket": bucket, "workflow": "", "link": ""}]).to_string();
        let required = crate::forge::required_checks_from_output(true, &wire, "").unwrap();
        assert_eq!(required, vec![CONTEXT]);
        let rows = crate::ci_gate::parse_check_rows(&wire).unwrap();
        let cfg = crate::ci_gate::CiGateConfig {
            informational_checks: vec!["*".into()],
            informational_workflows: vec![],
        };
        let verdict = crate::ci_gate::classify_red(&rows, &required, &cfg, false, false);
        assert_eq!(verdict.real.is_empty(), outcome != CheckOutcome::Failed);
        assert_eq!(verdict.pending.is_empty(), outcome != CheckOutcome::Pending);
    }
}

// trace:TASK-3232 | ai:codex
/// Cross the real bare-repo relay, receipt, HTTP status writer and native
/// reader boundaries before judging the evidence used by PR/train gates.
#[test]
fn aida_sized_relay_native_statuses_reach_required_pr_and_train_gates() {
    let binary = std::env::var_os("AIDA_TEST_BIN")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("CARGO_TARGET_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target")
                })
                .join("debug/aida")
        });
    assert!(
        binary.is_file(),
        "build the CLI first or set AIDA_TEST_BIN: {}",
        binary.display()
    );
    let temp = tempfile::tempdir().unwrap();
    let evidence = temp.path().join("native-statuses.json");
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/test_work_ci_relay.py");
    let mut command = std::process::Command::new("python3");
    command
        .arg(script)
        .args(["WorkCiTests.test_aida_sized_multi_sha_templates_and_native_gate_evidence"])
        .env("AIDA_TEST_BIN", binary)
        .env("AIDA_CI_GATE_EVIDENCE", &evidence);
    let (status, output, stderr) = aida_core::transport_policy::bounded_with_stderr(
        command,
        &[],
        1024 * 1024,
        std::time::Instant::now() + std::time::Duration::from_secs(480),
    )
    .unwrap();
    assert!(
        status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output),
        String::from_utf8_lossy(&stderr)
    );
    println!("{}", String::from_utf8_lossy(&output));
    let revisions: serde_json::Value =
        serde_json::from_slice(&std::fs::read(evidence).unwrap()).unwrap();
    let revisions = revisions.as_array().unwrap();
    assert_eq!(revisions.len(), 3);
    for (index, revision) in revisions.iter().enumerate() {
        let sha = revision["sha"].as_str().unwrap();
        let statuses = revision["statuses"].as_array().unwrap();
        for (phase, status) in statuses.iter().enumerate() {
            assert_eq!(status["name"], "aida/work-ci");
            let expected = if phase == 0 {
                CheckOutcome::Pending
            } else if index == 1 {
                CheckOutcome::Failed
            } else {
                CheckOutcome::Passed
            };
            // These are the POST bodies captured by the home API, not invented
            // pass/fail rows. Preserve the SHA captured from the POST endpoint;
            // supply only the forge-owned monotonic id read metadata.
            let mut row = status.clone();
            row["id"] = json!(phase + 1);
            assert_eq!(row["sha"], sha);
            let wire = serde_json::to_vec(&json!([row])).unwrap();
            let mut api = |_: &str| Ok(wire.clone());
            let checks = gitlab_commit_checks("2", sha, &mut api).unwrap();
            assert_eq!(checks.checks[0].outcome, expected);
            assert_eq!(
                judge_commit_checks(&checks, &["aida/work-ci"]) == CommitCi::Green,
                expected == CheckOutcome::Passed
            );
            let native = crate::forge::ci_status_from_commit_checks(Ok(checks));
            assert_eq!(
                native.state == crate::gate::dispatch_types::CiState::Success,
                expected == CheckOutcome::Passed
            );
            // PR ship's required-row classifier must retain a failure/pending
            // verdict even if the operator has an informational wildcard.
            let bucket = match expected {
                CheckOutcome::Passed => "pass",
                CheckOutcome::Pending => "pending",
                _ => "fail",
            };
            let wire =
                json!([{"name": status["name"], "bucket": bucket, "workflow": "", "link": ""}])
                    .to_string();
            let required = crate::forge::required_checks_from_output(true, &wire, "").unwrap();
            let rows = crate::ci_gate::parse_check_rows(&wire).unwrap();
            let cfg = crate::ci_gate::CiGateConfig {
                informational_checks: vec!["*".into()],
                informational_workflows: vec![],
            };
            let verdict = crate::ci_gate::classify_red(&rows, &required, &cfg, false, false);
            assert_eq!(verdict.real.is_empty(), expected != CheckOutcome::Failed);
            assert_eq!(
                verdict.pending.is_empty(),
                expected != CheckOutcome::Pending
            );
        }
    }
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-732.acbd1c12 | ai:codex
#[test]
fn overlapping_job_names_in_distinct_workflows_never_supersede_failures() {
    let row = |name: &str, outcome, run: &str, workflow: Option<&str>| CommitCheck {
        name: name.into(),
        outcome,
        run_id: Some(run.into()),
        workflow_id: workflow.map(str::to_string),
    };
    for workflow in [Some("other"), None] {
        let checks = CommitChecks {
            sha: SHA.into(),
            checks: vec![
                row("Build", CheckOutcome::Failed, "10", Some("ci")),
                row("security", CheckOutcome::Failed, "10", Some("ci")),
                row("Build", CheckOutcome::Passed, "11", workflow),
            ],
        };
        assert_eq!(
            judge_commit_checks(&checks, &[]),
            CommitCi::Failed(vec!["Build".into(), "security".into()])
        );
    }
}

// trace:STORY-2809 | ai:codex
#[test]
fn github_commit_ci_verifies_workflow_identity_once_per_run() {
    let mut asked = Vec::new();
    let checks = github_commit_checks("o/r", SHA, &mut |endpoint| {
        asked.push(endpoint.to_string());
        Ok(serde_json::to_vec(&if endpoint.contains("/check-runs?") {
            json!({"total_count": 2, "check_runs": [
                {"name":"Build", "head_sha": SHA, "status":"completed", "conclusion":"success", "details_url":"https://github.com/o/r/actions/runs/11/job/1"},
                {"name":"test", "head_sha": SHA, "status":"completed", "conclusion":"success", "details_url":"https://github.com/o/r/actions/runs/11/job/2"}]})
        } else if endpoint.ends_with("/actions/runs/11") {
            json!({"id":11,"workflow_id":42,"head_sha":SHA})
        } else { no_statuses() }).unwrap())
    }).unwrap();
    assert!(checks
        .checks
        .iter()
        .all(|c| c.workflow_id.as_deref() == Some("42")));
    assert_eq!(
        asked
            .iter()
            .filter(|s| s.contains("/actions/runs/"))
            .count(),
        1
    );
    assert!(github_commit_checks("o/r", SHA, &mut |endpoint| {
        if endpoint.contains("/actions/runs/") { anyhow::bail!("unavailable"); }
        Ok(serde_json::to_vec(&json!({"total_count":1,"check_runs":[{"name":"Build","head_sha":SHA,"status":"completed","conclusion":"success","details_url":"https://github.com/o/r/actions/runs/11/job/1"}]})).unwrap())
    }).is_err());
}
