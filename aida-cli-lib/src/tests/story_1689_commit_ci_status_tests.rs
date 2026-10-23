//! STORY-1689: `ci_status(CiTarget::Commit)` and `Forge::commit_checks`
//! read a commit's CI by its SHA through the shared reader, on both forges.
//! Fake API runners and a fake CLI script; no network.
// trace:STORY-1689 | ai:claude
use super::*;
use serde_json::json;

const SHA: &str = "e33fec11d936d4cc6ce450719a506a68932915eb";

fn checks(outcomes: &[(&str, CheckOutcome)]) -> Result<CommitChecks> {
    Ok(CommitChecks {
        sha: SHA.to_string(),
        checks: outcomes
            .iter()
            .map(|(name, outcome)| CommitCheck {
                name: name.to_string(),
                outcome: *outcome,
            })
            .collect(),
    })
}

#[test]
fn commit_ci_status_maps_each_verdict() {
    use CheckOutcome::*;
    let green = ci_status_from_commit_checks(checks(&[("a", Passed), ("b", Skipped)]));
    assert_eq!(green.state, CiState::Success);
    let pending = ci_status_from_commit_checks(checks(&[("a", Passed), ("b", Pending)]));
    assert_eq!(pending.state, CiState::Pending);
    let failed = ci_status_from_commit_checks(checks(&[("a", Failed), ("b", Pending)]));
    assert_eq!(failed.state, CiState::Failed);
    assert_eq!(failed.failing_checks, vec!["a".to_string()]);
}

#[test]
fn commit_ci_status_without_evidence_is_no_signal_never_success() {
    let empty = ci_status_from_commit_checks(checks(&[]));
    assert_eq!(empty.state, CiState::None);
    let err = ci_status_from_commit_checks(Err(anyhow::anyhow!("gh api failed")));
    assert_eq!(err.state, CiState::None);
}

#[test]
fn github_commit_read_asks_for_the_sha_in_this_repository() {
    let mut asked = Vec::new();
    let mut api = |endpoint: &str| -> Result<Vec<u8>> {
        asked.push(endpoint.to_string());
        Ok(if endpoint.contains("/status?") {
            serde_json::to_vec(&json!({"sha": SHA, "total_count": 0, "statuses": []}))?
        } else {
            serde_json::to_vec(&json!({"total_count": 1, "check_runs": [
                {"name": CI_AGGREGATOR_CHECK, "status": "completed",
                 "conclusion": "success", "head_sha": SHA}]}))?
        })
    };
    let status =
        ci_status_from_commit_checks(github_commit_checks("{owner}/{repo}", SHA, &mut api));
    assert_eq!(status.state, CiState::Success);
    assert!(
        asked[0].starts_with(&format!(
            "repos/{{owner}}/{{repo}}/commits/{SHA}/check-runs?"
        )),
        "{asked:?}"
    );
}

#[test]
fn gitlab_commit_read_asks_for_the_sha_not_a_ref() {
    let mut asked = Vec::new();
    let mut api = |endpoint: &str| -> Result<Vec<u8>> {
        asked.push(endpoint.to_string());
        Ok(serde_json::to_vec(&json!([
            {"id": 1, "name": "build", "sha": SHA, "status": "failed"},
            {"id": 2, "name": "build", "sha": SHA, "status": "success"},
        ]))?)
    };
    let status = ci_status_from_commit_checks(gitlab_commit_checks(":id", SHA, &mut api));
    assert_eq!(
        status.state,
        CiState::Success,
        "the newest job per name wins"
    );
    assert!(
        asked[0].starts_with(&format!("projects/:id/repository/commits/{SHA}/statuses?")),
        "{asked:?}"
    );
    assert!(!asked[0].contains("ref="), "{asked:?}");
}

#[test]
fn commit_checks_refuses_a_non_sha_before_any_cli_call() {
    let dir = tempfile::tempdir().unwrap();
    for forge in [
        Box::new(GitHubForge::new(dir.path())) as Box<dyn Forge>,
        Box::new(GitLabForge::new(dir.path())),
    ] {
        let err = forge.commit_checks("main; rm -rf /").unwrap_err();
        assert!(err.to_string().contains("invalid commit"), "{err:#}");
        assert_eq!(
            forge
                .ci_status(CiTarget::Commit("main".into()))
                .unwrap()
                .state,
            CiState::None
        );
    }
}

#[test]
fn pure_git_has_no_commit_ci() {
    let dir = tempfile::tempdir().unwrap();
    let forge = forge_for_kind(dir.path(), ForgeKind::None);
    assert!(forge.commit_checks(SHA).is_err());
}

#[cfg(unix)]
#[test]
fn commit_ci_api_runs_a_get_and_fails_closed_on_a_cli_error() {
    // System binaries stand in for the forge CLI: no script is written, so
    // no exec can race a still-open file (ETXTBSY).
    let dir = tempfile::tempdir().unwrap();
    let echo = std::path::Path::new("/bin/echo");
    let out = commit_ci_api(echo, dir.path(), "repos/x/y/commits/abc/status").unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap().trim(),
        "api -X GET repos/x/y/commits/abc/status"
    );
    let err = commit_ci_api(std::path::Path::new("/bin/false"), dir.path(), "x").unwrap_err();
    assert!(err.to_string().contains("failed"), "{err:#}");
}
