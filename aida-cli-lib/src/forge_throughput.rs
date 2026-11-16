//! Historical review and FULL CI evidence for throughput, behind the forge boundary.
// trace:STORY-2844 | ai:codex
use super::*;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashSet;

fn pages(endpoint: &str, api: &mut dyn FnMut(&str) -> Result<Vec<u8>>) -> Result<Vec<Value>> {
    let mut rows = Vec::new();
    for page in 1..=100 {
        let body = api(&format!("{endpoint}?per_page=100&page={page}"))?;
        let value: Value = serde_json::from_slice(&body)?;
        let batch = value
            .as_array()
            .context("historical forge response is not an array")?;
        rows.extend(batch.iter().cloned());
        if batch.len() < 100 {
            return Ok(rows);
        }
    }
    anyhow::bail!("historical forge response exceeds 100 pages")
}

fn historical_rework(
    pr: u64,
    landed: DateTime<Utc>,
    api: &mut dyn FnMut(&str) -> Result<Vec<u8>>,
) -> Result<bool> {
    let base = "repos/{owner}/{repo}";
    for review in pages(&format!("{base}/pulls/{pr}/reviews"), api)? {
        let state = review["state"].as_str().context("review lacks state")?;
        if state == "CHANGES_REQUESTED" {
            let at = review["submitted_at"]
                .as_str()
                .context("review lacks submitted_at")?;
            if DateTime::parse_from_rfc3339(at)? <= landed {
                return Ok(true);
            }
        }
    }
    let commits = pages(&format!("{base}/pulls/{pr}/commits"), api)?;
    // GitHub caps this endpoint at 250 commits. Never call a truncated history clean.
    anyhow::ensure!(
        !commits.is_empty() && commits.len() < 250,
        "PR commit history empty or potentially truncated"
    );
    for commit in commits {
        let sha = commit["sha"].as_str().context("PR commit lacks sha")?;
        anyhow::ensure!(is_full_commit_sha(sha), "invalid PR commit SHA");
        let mut checks = Vec::new();
        let mut seen = 0;
        let mut total = None;
        for page in 1..=100 {
            let body = api(&format!(
                "{base}/commits/{sha}/check-runs?filter=all&per_page=100&page={page}"
            ))?;
            let value: Value = serde_json::from_slice(&body)?;
            let (count, parsed) = parse_gh_check_runs(&value, sha)?;
            anyhow::ensure!(
                total.is_none_or(|old| old == count),
                "check-run count changed while paging"
            );
            total = Some(count);
            seen += parsed.len();
            let raw = value["check_runs"]
                .as_array()
                .context("missing check_runs")?;
            for (check, row) in parsed.into_iter().zip(raw) {
                if row["status"].as_str() == Some("completed") {
                    let at = row["completed_at"]
                        .as_str()
                        .context("completed check lacks completed_at")?;
                    if DateTime::parse_from_rfc3339(at)? <= landed {
                        checks.push(check);
                    }
                }
            }
            if seen >= count || raw.len() < 100 {
                anyhow::ensure!(seen == count, "incomplete historical check-runs");
                break;
            }
            anyhow::ensure!(page < 100, "historical check-runs exceed 100 pages");
        }
        if red_full_run(&checks) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn red_full_run(checks: &[CommitCheck]) -> bool {
    let full: HashSet<_> = checks
        .iter()
        .filter(|c| c.name == "ci-tier: full" && c.outcome == CheckOutcome::Passed)
        .filter_map(|c| c.run_id.as_deref())
        .collect();
    full.into_iter().any(|run| {
        !checks.iter().any(|c| {
            c.run_id.as_deref() == Some(run)
                && c.name.starts_with("ci-tier: ")
                && c.name != "ci-tier: full"
        }) && checks
            .iter()
            .any(|c| c.run_id.as_deref() == Some(run) && c.outcome == CheckOutcome::Failed)
    })
}

pub(crate) fn github_rework(root: &Path, pr: u64, landed: DateTime<Utc>) -> Result<bool> {
    anyhow::ensure!(
        checked_forge_kind(root).map_err(anyhow::Error::msg)? == ForgeKind::GitHub,
        "historical throughput rework currently requires GitHub"
    );
    let gh = crate::resolve_forge_cli(ForgeKind::GitHub).context("gh is missing")?;
    historical_rework(pr, landed, &mut |endpoint| {
        commit_ci_api(&gh, root, endpoint)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn throughput_full_failure_survives_green_rerun_and_excludes_fast() {
        let check = |name: &str, outcome, run: &str| CommitCheck {
            name: name.into(),
            outcome,
            run_id: Some(run.into()),
            workflow_id: None,
        };
        let full = check("ci-tier: full", CheckOutcome::Passed, "1");
        let red = check("Build (ubuntu-latest)", CheckOutcome::Failed, "1");
        assert!(red_full_run(&[
            full.clone(),
            red.clone(),
            check("ci-tier: full", CheckOutcome::Passed, "2"),
            check("Build (ubuntu-latest)", CheckOutcome::Passed, "2")
        ]));
        assert!(!red_full_run(&[
            full,
            check("Build (ubuntu-latest)", CheckOutcome::Failed, "3")
        ]));
        assert!(!red_full_run(&[
            red,
            check("ci-tier: fast", CheckOutcome::Passed, "1")
        ]));
    }

    #[test]
    fn throughput_historical_full_failure_cutoff_and_filter_all() {
        let landed = DateTime::parse_from_rfc3339("2026-11-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let sha = "a".repeat(40);
        let fixture = |completed_at: &str| {
            serde_json::json!({
                "total_count": 2,
                "check_runs": [
                    {"name": "ci-tier: full", "head_sha": sha, "status": "completed", "conclusion": "success", "completed_at": "2026-11-15T10:00:00Z", "details_url": "https://github.com/o/r/actions/runs/123/job/1"},
                    {"name": "Build (ubuntu-latest)", "head_sha": sha, "status": "completed", "conclusion": "failure", "completed_at": completed_at, "details_url": "https://github.com/o/r/actions/runs/123/job/2"}
                ]
            })
        };
        for (at, expected) in [
            ("2026-11-15T11:00:00Z", true),
            ("2026-11-15T13:00:00Z", false),
        ] {
            let mut api = |endpoint: &str| {
                let value = if endpoint.contains("/reviews?") {
                    serde_json::json!([])
                } else if endpoint.contains("/commits?") {
                    serde_json::json!([{"sha": sha}])
                } else {
                    assert!(endpoint.contains("filter=all"));
                    fixture(at)
                };
                Ok(serde_json::to_vec(&value).unwrap())
            };
            assert_eq!(historical_rework(42, landed, &mut api).unwrap(), expected);
        }
    }

    #[test]
    fn throughput_historical_refusal_and_read_errors() {
        let landed = DateTime::parse_from_rfc3339("2026-11-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut api = |_: &str| {
            Ok(
                br#"[{"state":"CHANGES_REQUESTED","submitted_at":"2026-11-14T12:00:00Z"}]"#
                    .to_vec(),
            )
        };
        assert!(historical_rework(42, landed, &mut api).unwrap());
        assert!(historical_rework(42, landed, &mut |_| anyhow::bail!("offline")).is_err());
    }
}
