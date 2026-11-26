//! Attempt-specific CI evidence for the train and landing gates.
// trace:BUG-2723 | ai:codex
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub(crate) struct TierRun {
    pub id: u64,
    pub attempt: u64,
    number: u64,
    pub tier: &'static str,
    pub workflow: String,
    pub conclusion: String,
    pub jobs: Vec<Value>,
}

impl TierRun {
    pub fn detail(&self) -> String {
        format!(
            "run {} attempt {} tier {} conclusion {} (selected, not superseded)",
            self.id, self.attempt, self.tier, self.conclusion
        )
    }

    pub fn bucket(&self) -> &'static str {
        bucket(&self.conclusion)
    }
}

pub(crate) fn bucket(conclusion: &str) -> &'static str {
    match conclusion {
        "success" => "pass",
        "neutral" | "skipped" => "skipping",
        "cancelled" => "cancel",
        "queued" | "waiting" | "pending" | "requested" | "in_progress" => "pending",
        _ => "fail",
    }
}

pub(crate) fn outcome(row: &Value) -> Result<&str> {
    let status = row["status"].as_str().context("missing Actions status")?;
    if status == "completed" {
        let conclusion = row["conclusion"]
            .as_str()
            .context("missing completed Actions conclusion")?;
        ensure!(
            matches!(
                conclusion,
                "success"
                    | "skipped"
                    | "neutral"
                    | "failure"
                    | "cancelled"
                    | "timed_out"
                    | "action_required"
                    | "startup_failure"
                    | "stale"
            ),
            "unknown Actions conclusion {conclusion}"
        );
        Ok(conclusion)
    } else {
        ensure!(
            bucket(status) == "pending",
            "unknown Actions status {status}"
        );
        Ok(status)
    }
}

#[derive(Default, Debug)]
pub(crate) struct TierSelection {
    conclusions: BTreeMap<String, String>,
    pub owned: BTreeSet<String>,
    pub selected: Vec<TierRun>,
}

pub(crate) fn list(
    path: &str,
    field: &str,
    api: &mut impl FnMut(&str) -> Result<Vec<u8>>,
) -> Result<Vec<Value>> {
    let sep = if path.contains('?') { '&' } else { '?' };
    let mut rows = Vec::new();
    let mut total = None;
    for page in 1..=10 {
        let endpoint = format!("{path}{sep}per_page=100&page={page}");
        let body: Value = serde_json::from_slice(&api(&endpoint)?)
            .with_context(|| format!("invalid JSON from {endpoint}"))?;
        let count = body["total_count"]
            .as_u64()
            .context("missing Actions total_count")?;
        ensure!(
            *total.get_or_insert(count) == count,
            "Actions count changed during pagination"
        );
        let batch = body[field]
            .as_array()
            .with_context(|| format!("missing Actions {field}"))?;
        rows.extend(batch.iter().cloned());
        if rows.len() as u64 == count {
            return Ok(rows);
        }
        ensure!(
            batch.len() == 100 && (rows.len() as u64) < count,
            "incomplete Actions {field}"
        );
    }
    anyhow::bail!("Actions {field} exceeds page limit")
}

pub(crate) fn latest_runs_at(
    head: &str,
    pr: u64,
    base: &str,
    mut api: impl FnMut(&str) -> Result<Vec<u8>>,
) -> Result<TierSelection> {
    ensure!(
        head.len() == 40 && head.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid pinned Actions SHA"
    );
    let mut result = TierSelection::default();
    let mut groups = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let path = format!("actions/runs?head_sha={head}");
    let runs = list(&path, "workflow_runs", &mut api)?;
    for run in &runs {
        ensure!(
            run["head_sha"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case(head)),
            "Actions run on different head"
        );
        let id = run["id"].as_u64().context("missing Actions run ID")?;
        ensure!(seen.insert(id), "duplicate Actions run {id}");
        if run["event"].as_str().context("missing Actions event")? != "pull_request" {
            // Other triggers never replace PR checks, but their run state can
            // explain a blocking row (e.g. a pull_request_target hold gate).
            if let Ok(state) = outcome(run) {
                result.conclusions.insert(id.to_string(), state.to_owned());
            }
            continue;
        }
        // Commit checks are SHA-scoped. Even ignoring a foreign run here would
        // leave its raw checks eligible to satisfy the exact-tip gate.
        ensure!(
            pr > 0
                && !base.is_empty()
                && run["pull_requests"].as_array().is_some_and(|owners| {
                    matches!(owners.as_slice(), [owner] if owner["number"].as_u64() == Some(pr)
                    && owner["base"]["ref"].as_str() == Some(base))
                }),
            "run {id} is not uniquely associated with PR #{pr} targeting {base}"
        );
        let identity =
            super::ci_run_identity(run, id, Some(head)).context("invalid pinned run identity")?;
        let path = run["path"]
            .as_str()
            .context("missing workflow path")?
            .to_owned();
        let attempt = run["run_attempt"].as_u64().context("missing run attempt")?;
        let number = run["run_number"].as_u64().context("missing run number")?;
        ensure!(
            attempt > 0 && number > 0 && id > 0 && !path.trim().is_empty(),
            "invalid Actions run coordinates"
        );
        let jobs = list(
            &format!("actions/runs/{id}/attempts/{attempt}/jobs"),
            "jobs",
            &mut api,
        )?;
        let mut tier = None;
        let mut ids = BTreeSet::new();
        for job in &jobs {
            ensure!(
                job["run_id"] == id
                    && job["head_sha"]
                        .as_str()
                        .is_some_and(|sha| sha.eq_ignore_ascii_case(head))
                    && job.get("run_attempt").is_none_or(|value| value == attempt),
                "job identity mismatch for run {id} attempt {attempt}"
            );
            ensure!(
                ids.insert(job["id"].as_u64().context("missing job id")?),
                "duplicate job"
            );
            outcome(job)?;
            let marker = match job["name"].as_str().context("missing Actions job name")? {
                "ci-tier: fast" => "FAST",
                "ci-tier: full" => "FULL",
                _ => continue,
            };
            ensure!(tier.replace(marker).is_none(), "ambiguous tier in run {id}");
        }
        let conclusion = outcome(run)?.to_owned();
        result
            .conclusions
            .insert(id.to_string(), conclusion.clone());
        let tier = if let Some(marker) = tier {
            marker
        } else if path == ".github/workflows/ci.yml" && jobs.is_empty() && conclusion == "cancelled"
        {
            "unavailable"
        } else {
            if path == ".github/workflows/ci.yml"
                && !jobs
                    .iter()
                    .any(|j| j["name"] == "label event (CI tier unchanged)")
            {
                anyhow::bail!("tier unavailable for run {id} attempt {attempt} ({conclusion})");
            }
            continue;
        };
        result.owned.insert(id.to_string());
        let candidate = TierRun {
            id,
            number,
            attempt,
            tier,
            conclusion,
            jobs,
            workflow: run["name"]
                .as_str()
                .context("missing workflow name")?
                .to_owned(),
        };
        let tiers = groups
            .entry((identity, path))
            .or_insert_with(BTreeMap::<_, TierRun>::new);
        if tiers
            .get(tier)
            .is_none_or(|old| (number, attempt, id) > (old.number, old.attempt, old.id))
        {
            tiers.insert(tier, candidate);
        }
    }
    // A rerun can replace an attempt without changing the head or run ID.
    // Refuse a mixed snapshot if selection changed while fetching the jobs.
    if !runs.is_empty() {
        ensure!(
            runs == list(&path, "workflow_runs", &mut api)?,
            "Actions runs changed during tier selection"
        );
    }
    for mut tiers in groups.into_values() {
        if let Some(full) = tiers.get("FULL") {
            if full.conclusion == "success"
                && full.jobs.iter().any(|j| {
                    j["name"] == super::CI_AGGREGATOR_CHECK && j["conclusion"] == "success"
                })
            {
                let full_id = full.id;
                tiers.retain(|tier, run| {
                    !matches!(*tier, "FAST" | "unavailable") || run.id >= full_id
                });
            }
        }
        for run in tiers.into_values() {
            if run.conclusion == "success" {
                ensure!(
                    run.jobs
                        .iter()
                        .any(|j| j["name"] == super::CI_AGGREGATOR_CHECK
                            && j["conclusion"] == "success"),
                    "{} lacks a successful Build job",
                    run.detail()
                );
            }
            result.selected.push(run);
        }
    }
    Ok(result)
}

impl TierRun {
    /// A run may be queued with partial jobs, or terminate without reporting its aggregate.
    pub fn rows(&self) -> Result<Vec<crate::ci_gate::CheckRow>> {
        use crate::ci_gate::CheckRow;
        let mut rows = Vec::new();
        for job in &self.jobs {
            rows.push(CheckRow {
                name: job["name"].as_str().context("missing job name")?.into(),
                workflow: self.workflow.clone(),
                run_id: Some(self.id.to_string()),
                job_id: job["id"].as_u64(),
                bucket: if self.bucket() == "pending" {
                    "pending"
                } else if self.conclusion == "cancelled"
                    && job["name"] == super::CI_AGGREGATOR_CHECK
                {
                    "cancel"
                } else {
                    bucket(outcome(job)?)
                }
                .into(),
            });
        }
        if self.bucket() != "pass"
            && !rows
                .iter()
                .any(|row| matches!(row.bucket.as_str(), "fail" | "cancel" | "pending"))
        {
            let state = if self.bucket() == "skipping" {
                "pending"
            } else {
                self.bucket()
            };
            rows.push(CheckRow {
                name: self.detail(),
                workflow: self.workflow.clone(),
                run_id: Some(self.id.to_string()),
                job_id: None,
                bucket: state.into(),
            });
        }
        Ok(rows)
    }
}

impl TierSelection {
    pub fn describe(&self, id: Option<&str>, check_state: &str) -> String {
        let id = id.unwrap_or("unavailable");
        if let Some(run) = self.selected.iter().find(|r| Some(r.id) == id.parse().ok()) {
            return run.detail();
        }
        let conclusion = self
            .conclusions
            .get(id)
            .map(String::as_str)
            .unwrap_or("unavailable");
        format!("run {id} conclusion {conclusion} (check {check_state}, supersession unavailable)")
    }

    pub fn verify_marker(&self, name: &str, id: Option<&str>) -> Result<()> {
        if matches!(name, "ci-tier: fast" | "ci-tier: full") {
            ensure!(
                id.is_some_and(|id| self.owned.contains(id)),
                "tier marker {name} missing from pinned Actions evidence"
            );
        }
        Ok(())
    }

    pub fn judge_commit_checks(
        &self,
        mut checks: super::CommitChecks,
        required: &[&str],
    ) -> Result<super::CommitCi> {
        use super::{CheckOutcome, CommitCheck};
        for c in &checks.checks {
            self.verify_marker(&c.name, c.run_id.as_deref())?;
        }
        checks
            .checks
            .retain(|c| !c.run_id.as_ref().is_some_and(|id| self.owned.contains(id)));
        let mut effective = super::latest_per_name(&checks.checks);
        let mut selected = Vec::new();
        for run in &self.selected {
            for row in run.rows()? {
                let outcome = match row.bucket.as_str() {
                    "pass" => CheckOutcome::Passed,
                    "skipping" => CheckOutcome::Skipped,
                    "pending" => CheckOutcome::Pending,
                    _ => CheckOutcome::Failed,
                };
                selected.push(CommitCheck {
                    name: row.name,
                    outcome,
                    run_id: row.run_id,
                    workflow_id: None,
                    cancelled: row.bucket == "cancel",
                    job_id: row.job_id,
                    run_attempt: Some(run.attempt),
                    run_in_progress: run.bucket() == "pending",
                });
            }
        }
        effective.extend(&selected);
        let mut verdict = super::commit_ci::judge_effective_checks(&effective, required, |c| {
            let state = if c.cancelled {
                "cancelled"
            } else if c.outcome == CheckOutcome::Pending {
                "pending"
            } else {
                "failed"
            };
            format!("{} ({})", c.name, self.describe(c.run_id.as_deref(), state))
        });
        if let super::CommitCi::MissingRequired(name) = &mut verdict {
            for run in &self.selected {
                name.push_str(&format!("; {}", run.detail()));
            }
        }
        Ok(verdict)
    }
}

#[cfg(test)]
pub(crate) mod fixture {
    use super::*;
    use serde_json::json;
    pub const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    pub fn run(id: u64, tier: &str, conclusion: &str) -> Value {
        json!({"id":id, "name":"CI", "path":".github/workflows/ci.yml", "event":"pull_request",
            "pull_requests":[{"number":1,"base":{"ref":"main"}}],
            "head_sha":HEAD,"head_repository":{"id":1},"head_branch":"feature","workflow_id":1,
            "run_attempt":1,"run_number":id,"status":if conclusion=="in_progress" {conclusion} else {"completed"},
            "conclusion":conclusion,"fixture_tier":tier})
    }
    pub fn read(runs: &[Value]) -> Result<TierSelection> {
        latest_runs_at(HEAD, 1, "main", |endpoint| {
            if endpoint == format!("actions/runs?head_sha={HEAD}&per_page=100&page=1") {
                return Ok(serde_json::to_vec(
                    &json!({"total_count":runs.len(),"workflow_runs":runs}),
                )?);
            }
            for run in runs {
                let (id, attempt) = (&run["id"], &run["run_attempt"]);
                if endpoint
                    != format!("actions/runs/{id}/attempts/{attempt}/jobs?per_page=100&page=1")
                {
                    continue;
                }
                if run["fixture_tier"].is_null() {
                    return Ok(serde_json::to_vec(&json!({"total_count":0,"jobs":[]}))?);
                }
                let tier_job = format!("ci-tier: {}", run["fixture_tier"].as_str().unwrap());
                let job = |id, name: &str, status, conclusion| {
                    json!({
                        "id":id, "name":name, "run_id":run["id"], "head_sha":run["head_sha"],
                        "status":status, "conclusion":conclusion
                    })
                };
                let conclusion = run["conclusion"].as_str().unwrap();
                let status = run["status"].as_str().unwrap();
                let aggregate = if conclusion == "cancelled" {
                    "failure"
                } else {
                    conclusion
                };
                let jobs = [
                    job(100, &tier_job, "completed", "success"),
                    job(101, super::super::CI_AGGREGATOR_CHECK, status, aggregate),
                ];
                return Ok(serde_json::to_vec(
                    &json!({"total_count":jobs.len(),"jobs":jobs}),
                )?);
            }
            anyhow::bail!("unexpected endpoint {endpoint}")
        })
    }
}
