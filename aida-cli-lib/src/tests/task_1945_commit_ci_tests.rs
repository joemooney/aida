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
    // success: unknown, never green. A skipped optional check still passes.
    for conclusion in ["skipped", "neutral"] {
        let (checks, _) = github(
            vec![json!({"total_count": 2, "check_runs": [
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

/// The required aggregator is the check run of CI's `build-result` job,
/// whose name is fixed (no matrix expression), so the constant cannot drift
/// from the workflow.
#[test]
fn aggregator_const_matches_the_ci_workflow() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.github/workflows/ci.yml");
    let workflow: serde_yaml::Value =
        serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let job = &workflow["jobs"]["build-result"];
    assert_eq!(job["name"].as_str(), Some(CI_AGGREGATOR_CHECK), "{job:?}");
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
