//! Report required Actions workflows that never start jobs; never supplies CI credit.
// trace:STORY-2856 | ai:codex
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path, time::Duration};

#[derive(Default, Serialize, Deserialize)]
struct Episode {
    first: String,
    next: i64,
    notices: u32,
}
impl Episode {
    fn deliver(
        &mut self,
        recovered: bool,
        now: i64,
        send: impl FnOnce() -> Result<bool>,
    ) -> Result<bool> {
        if !send()? {
            return Ok(false);
        }
        if recovered {
            *self = Self::default();
        } else {
            self.next = now + (300_i64 * (1_i64 << self.notices.min(6))).min(14400);
            self.notices = self.notices.saturating_add(1);
        }
        Ok(true)
    }
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    consecutive_runs: usize,
    queued_timeout_secs: i64,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            consecutive_runs: 3,
            queued_timeout_secs: 600,
        }
    }
}

fn timestamp(v: &Value, key: &str) -> Result<i64> {
    Ok(
        DateTime::parse_from_rfc3339(v[key].as_str().context("missing run timestamp")?)?
            .timestamp(),
    )
}
fn rows<'a>(v: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    v[key].as_array().context("missing Actions collection")
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key].as_str().context("missing Actions field")
}

fn head_has_context(name: &str, checks: &[Value], statuses: &[Value]) -> bool {
    checks
        .iter()
        .filter(|c| c["name"] == name)
        .max_by_key(|c| c["id"].as_u64().unwrap_or(0))
        .is_some_and(|c| c["conclusion"] == "success")
        || statuses
            .iter()
            .filter(|s| s["context"] == name)
            .max_by_key(|s| s["id"].as_u64().unwrap_or(0))
            .is_some_and(|s| s["state"] == "success")
}

// Input is newest first. A started job breaks the consecutive startup streak.
fn evaluate(
    episode: &mut Episode,
    runs: &[Value],
    config: &Config,
    now: i64,
) -> Result<Option<bool>> {
    let Some(latest) = runs.first() else {
        return Ok(None);
    };
    if latest["conclusion"] == "success" {
        return Ok((episode.notices > 0).then_some(true));
    }
    let mut failed = Vec::new();
    for run in runs {
        let stalled = run["status"] == "queued"
            && now - timestamp(run, "created_at")? >= config.queued_timeout_secs;
        if !(run["conclusion"] == "startup_failure" || stalled)
            || run["job_count"].as_u64().context("missing job count")? != 0
        {
            break;
        }
        failed.push(run);
    }
    if failed.len() < config.consecutive_runs {
        return Ok(None);
    }
    if episode.first.is_empty() {
        episode.first = text(failed.last().unwrap(), "created_at")?.into();
    }
    Ok((now >= episode.next).then_some(false))
}

/// Called by the supervised train and explicit notify checks. Errors retain prior
/// episodes: unreadable forge data never proves recovery.
pub(crate) fn check(root: &Path) -> Result<()> {
    // Unit tests exercise the observation and delivery logic directly; never
    // contact the live forge from notification or train unit-test callers.
    if cfg!(test) {
        return Ok(());
    }
    let Some(body) = crate::trusted_config::read_trusted_config_toml(root) else {
        return Ok(());
    };
    let config_toml: toml::Value = toml::from_str(&body)?;
    // There is one configured transport command; operators can fan it out to
    // every push channel. Do not advance backoff when no transport is configured.
    if config_toml
        .get("notify")
        .and_then(|n| n.get("command"))
        .and_then(|v| v.as_str())
        .is_none_or(|s| s.trim().is_empty())
    {
        return Ok(());
    }
    let config: Config = config_toml
        .get("ci_startup")
        .cloned()
        .map(toml::Value::try_into)
        .transpose()?
        .unwrap_or_default();
    anyhow::ensure!(
        (1..=20).contains(&config.consecutive_runs)
            && (1..=86400).contains(&config.queued_timeout_secs),
        "invalid ci_startup thresholds"
    );
    let kind = crate::forge::read_forge_config(root)
        .or_else(|| crate::forge::origin_url(root).map(|u| crate::forge::detect_forge_kind(&u)));
    if kind != Some(crate::forge::ForgeKind::GitHub) {
        return Ok(());
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    let output = |endpoint: &str| -> Result<std::process::Output> {
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "startup observation budget exhausted"
        );
        crate::forge::github_startup_api(root, endpoint)
    };
    let api = |endpoint: &str| -> Result<Value> {
        let out = output(endpoint)?;
        anyhow::ensure!(
            out.status.success(),
            "startup observation failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Ok(serde_json::from_slice(&out.stdout)?)
    };
    let repo = api("repos/{owner}/{repo}")?;
    let default_branch = text(&repo, "default_branch")?;
    let branch = default_branch;
    let required = crate::status_prs::train_required_status_checks(branch, output)?;
    if required.is_empty() {
        return Ok(());
    }
    let branch = branch
        .replace('%', "%25")
        .replace('/', "%2F")
        .replace('#', "%23")
        .replace('?', "%3F");
    let workflows = api("repos/{owner}/{repo}/actions/workflows?per_page=100")?;
    let mut workflows = rows(&workflows, "workflows")?.clone();
    anyhow::ensure!(workflows.len() < 100, "workflow inventory truncated");
    let dir = root.join(".aida");
    std::fs::create_dir_all(&dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("ci-startup.lock"))?;
    if lock.try_lock_exclusive().is_err() {
        return Ok(());
    }
    let path = dir.join("ci-startup.json");
    let mut state: BTreeMap<String, Episode> = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => return Err(e.into()),
    };
    // Known incidents and exact custom-context matches take priority over
    // unrelated workflows when the shared observation budget is tight.
    workflows.sort_by_key(|w| {
        let known = w["id"]
            .as_u64()
            .is_some_and(|id| state.get(&id.to_string()).is_some_and(|e| e.notices > 0));
        let named = w["name"]
            .as_str()
            .is_some_and(|n| required.iter().any(|r| r == n));
        (!known, !named)
    });
    let now = Utc::now().timestamp();
    // Persist each accepted delivery independently. A crash after transport can
    // repeat a notice; it cannot suppress the recovery or fabricate delivery.
    for workflow in &workflows {
        let id = workflow["id"].as_u64().context("missing workflow id")?;
        let response = api(&format!(
            "repos/{{owner}}/{{repo}}/actions/workflows/{id}/runs?branch={branch}&per_page=100"
        ))?;
        let mut runs = rows(&response, "workflow_runs")?.clone();
        if runs.is_empty() {
            continue;
        }
        // Retain the episode's earliest startup-failure timestamp visible in
        // this bounded history even when only N job inventories are needed
        // to prove the alert threshold for a custom status publisher.
        let mut first_startup_failure = None;
        for run in &runs {
            if run["conclusion"] == "startup_failure" {
                first_startup_failure = Some(text(run, "created_at")?.to_owned());
            } else {
                break;
            }
        }
        // Map ordinary contexts using actual job names. Custom status
        // publishers use the exact workflow display name as their context.
        // An older started run maps jobs when recent attempts have none.
        let custom = workflow["name"]
            .as_str()
            .filter(|n| required.iter().any(|r| r == n));
        let mut mapped: Vec<String> = custom.into_iter().map(str::to_owned).collect();
        let mut inspected = 0;
        for run in &mut runs {
            inspected += 1;
            let run_id = run["id"].as_u64().context("missing run id")?;
            let jobs = api(&format!(
                "repos/{{owner}}/{{repo}}/actions/runs/{run_id}/jobs?per_page=100"
            ))?;
            let count = jobs["total_count"].as_u64().context("missing jobs count")?;
            run["job_count"] = count.into();
            if count > 0 {
                anyhow::ensure!(count < 100, "workflow job inventory truncated");
                mapped.extend(
                    rows(&jobs, "jobs")?
                        .iter()
                        .filter_map(|j| j["name"].as_str())
                        .filter(|n| required.iter().any(|r| r == n))
                        .map(str::to_owned),
                );
                break;
            }
            if custom.is_some() && inspected >= config.consecutive_runs {
                break;
            }
        }
        runs.truncate(inspected);
        // Status-publishing workflows (including merge-hold-gate) expose a
        // custom status rather than a job with that context. The workflow's
        // exact display name is the convention for that custom context.
        if let Some(name) = workflow["name"]
            .as_str()
            .filter(|n| required.iter().any(|r| r == n))
        {
            if !mapped.iter().any(|n| n == name) {
                mapped.push(name.into());
            }
        }
        mapped.sort();
        mapped.dedup();
        if mapped.is_empty() {
            continue;
        }
        // Only prefix through the mapping run is needed; all preceding runs
        // have verified zero jobs. Sort order is Actions' newest-first order.
        let end = runs
            .iter()
            .position(|r| r["job_count"].as_u64().is_some_and(|n| n > 0))
            .unwrap_or(runs.len() - 1);
        runs.truncate(end + 1);
        let key = id.to_string();
        let episode = state.entry(key).or_default();
        let fresh = episode.first.is_empty();
        let Some(recovered) = evaluate(episode, &runs, &config, now)? else {
            continue;
        };
        if fresh && !recovered {
            if let Some(first) = first_startup_failure {
                episode.first = first;
            }
        }
        let blocked = if recovered {
            0
        } else {
            let prs = api("repos/{owner}/{repo}/pulls?state=open&per_page=100")?;
            let prs = prs.as_array().context("invalid PR inventory")?;
            anyhow::ensure!(prs.len() < 100, "PR inventory truncated");
            let mut count = 0;
            for pr in prs {
                if pr["draft"] == true || pr["base"]["ref"] != default_branch {
                    continue;
                }
                let sha = text(&pr["head"], "sha")?;
                let checks = api(&format!(
                    "repos/{{owner}}/{{repo}}/commits/{sha}/check-runs?per_page=100"
                ))?;
                let checks = rows(&checks, "check_runs")?;
                anyhow::ensure!(checks.len() < 100, "check inventory truncated");
                let statuses = api(&format!(
                    "repos/{{owner}}/{{repo}}/commits/{sha}/status?per_page=100"
                ))?;
                let statuses = rows(&statuses, "statuses")?;
                anyhow::ensure!(statuses.len() < 100, "status inventory truncated");
                if mapped
                    .iter()
                    .all(|name| head_has_context(name, checks, statuses))
                {
                    continue;
                }
                // GitHub's reviewDecision accounts for dismissed/obsolete reviews.
                let number = pr["number"].as_u64().context("missing PR number")?;
                anyhow::ensure!(
                    std::time::Instant::now() < deadline,
                    "startup observation budget exhausted"
                );
                let review =
                    match crate::gate::ci_startup::github_startup_review_decision(root, number) {
                        crate::forge_observed::Observed::Known(value) => value,
                        other => anyhow::bail!("PR review observation unavailable: {other:?}"),
                    };
                if review == "APPROVED" {
                    count += 1;
                }
            }
            count
        };
        let title = if recovered {
            "Required CI recovered"
        } else {
            "Required CI never started"
        };
        let message = format!("Workflow: {}\nRequired checks: {}\nLatest run: {}\nFirst failure: {}\nApproved PRs blocked by missing check: {}\n{}\nRunbook: docs/advanced/ci-startup-failure.md", text(workflow, "name")?, mapped.join(", "), text(&runs[0], "html_url")?, episode.first, blocked, if recovered { "A run succeeded again." } else { "Consecutive runs have zero jobs (startup_failure or queued beyond timeout)." });
        let outcome = crate::notify::send_direct_bounded(
            root,
            &format!(
                "ci-startup-{id}-{}-{}-{}",
                episode.first, episode.notices, recovered
            ),
            title,
            &message,
            Some(Duration::from_secs(3)),
        )?;
        if episode.deliver(recovered, now, || {
            Ok(matches!(
                outcome.delivery(),
                crate::notify::DirectDelivery::Sent | crate::notify::DirectDelivery::Deferred
            ))
        })? {
            let mut file = tempfile::NamedTempFile::new_in(&dir)?;
            use std::io::Write;
            file.write_all(&serde_json::to_vec_pretty(&state)?)?;
            file.as_file().sync_all()?;
            file.persist(&path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(id: u64, conclusion: &str) -> Value {
        serde_json::json!({"id": id, "conclusion": conclusion, "status": "completed", "created_at": "2026-11-16T16:01:00Z", "job_count": 0})
    }
    #[test]
    fn story2856_missing_custom_status_and_latest_check_attempt() {
        let checks = serde_json::json!([
            {"id": 1, "name": "build", "conclusion": "success"},
            {"id": 2, "name": "build", "conclusion": null}
        ]);
        assert!(!head_has_context("build", checks.as_array().unwrap(), &[]));
        let statuses = serde_json::json!([
            {"id": 9, "context": "merge-hold-gate", "state": "pending"},
            {"id": 10, "context": "merge-hold-gate", "state": "success"}
        ]);
        assert!(head_has_context(
            "merge-hold-gate",
            &[],
            statuses.as_array().unwrap()
        ));
        assert!(!head_has_context(
            "missing",
            &[],
            statuses.as_array().unwrap()
        ));
    }
    #[test]
    fn story2856_threshold_backoff_and_recovery() {
        let config = Config::default();
        let mut state = Episode::default();
        let runs = vec![
            run(3, "startup_failure"),
            run(2, "startup_failure"),
            run(1, "startup_failure"),
        ];
        assert_eq!(
            evaluate(&mut state, &runs[..2], &config, 1000).unwrap(),
            None
        );
        assert_eq!(
            evaluate(&mut state, &runs, &config, 1000).unwrap(),
            Some(false)
        );
        state.notices = 1;
        state.next = 1300;
        let mut state: Episode =
            serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(evaluate(&mut state, &runs, &config, 1299).unwrap(), None);
        assert_eq!(
            evaluate(&mut state, &runs, &config, 1300).unwrap(),
            Some(false)
        );
        assert_eq!(
            evaluate(&mut state, &[run(4, "success")], &config, 1301).unwrap(),
            Some(true)
        );
        state = Episode::default();
        assert_eq!(
            evaluate(&mut state, &[run(4, "success")], &config, 1302).unwrap(),
            None
        );
    }
    #[test]
    fn story2856_one_delivery_per_window_and_one_recovery() {
        let config = Config::default();
        let runs = vec![
            run(3, "startup_failure"),
            run(2, "startup_failure"),
            run(1, "startup_failure"),
        ];
        let mut episode = Episode::default();
        let mut sent = Vec::new();
        for now in [1000, 1001, 1299, 1300, 1301, 1899, 1900] {
            if let Some(recovered) = evaluate(&mut episode, &runs, &config, now).unwrap() {
                episode
                    .deliver(recovered, now, || {
                        sent.push((now, recovered));
                        Ok(true)
                    })
                    .unwrap();
            }
            // Simulate a process restart between observations.
            episode = serde_json::from_slice(&serde_json::to_vec(&episode).unwrap()).unwrap();
        }
        assert_eq!(sent, [(1000, false), (1300, false), (1900, false)]);
        for now in [1901, 1902] {
            if let Some(recovered) =
                evaluate(&mut episode, &[run(4, "success")], &config, now).unwrap()
            {
                episode
                    .deliver(recovered, now, || {
                        sent.push((now, recovered));
                        Ok(true)
                    })
                    .unwrap();
            }
        }
        assert_eq!(sent.last(), Some(&(1901, true)));
        assert_eq!(sent.len(), 4);
        let mut episode = Episode::default();
        assert_eq!(
            evaluate(&mut episode, &runs, &config, 2000).unwrap(),
            Some(false)
        );
        assert!(!episode.deliver(false, 2000, || Ok(false)).unwrap());
        assert_eq!(
            evaluate(&mut episode, &runs, &config, 2001).unwrap(),
            Some(false)
        );
    }
    #[test]
    fn story2856_started_jobs_break_streak_and_queued_timeout() {
        let mut runs = vec![
            run(3, "startup_failure"),
            run(2, "startup_failure"),
            run(1, "startup_failure"),
        ];
        runs[1]["job_count"] = 1.into();
        assert_eq!(
            evaluate(
                &mut Episode::default(),
                &runs,
                &Config::default(),
                i64::MAX / 2
            )
            .unwrap(),
            None
        );
        for r in &mut runs {
            r["job_count"] = 0.into();
            r["status"] = "queued".into();
            r["conclusion"] = Value::Null;
        }
        let created = timestamp(&runs[0], "created_at").unwrap();
        assert_eq!(
            evaluate(
                &mut Episode::default(),
                &runs,
                &Config::default(),
                created + 599
            )
            .unwrap(),
            None
        );
        assert_eq!(
            evaluate(
                &mut Episode::default(),
                &runs,
                &Config::default(),
                created + 600
            )
            .unwrap(),
            Some(false)
        );
    }
}
