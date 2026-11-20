//! The one commit-keyed CI reader: every check the forge reports on one
//! commit SHA, read by SHA (never through a branch or a PR, which resolve to
//! whatever head they have now). Every CLI, API or parse failure is an
//! `Err`, never an empty or "no CI" answer, so a caller cannot read a failed
//! query as clean.
// trace:TASK-1945 | ai:claude
use anyhow::{Context, Result};

/// Where one check on a commit stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckOutcome {
    /// Completed as success.
    Passed,
    /// Completed as neutral or skipped, or (GitLab) failed but allowed to:
    /// not a failure, and not a success either.
    Skipped,
    /// Queued, running, waiting or awaiting a manual action.
    Pending,
    /// Completed as anything else (failure, cancelled, timed out, ...).
    Failed,
}

/// One check (a GitHub check run or commit status, a GitLab job status).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitCheck {
    pub name: String,
    pub outcome: CheckOutcome,
    /// The workflow run that owns a GitHub Actions check run (from its
    /// details link); `None` for commit statuses and other forges.
    // trace:STORY-2809 | ai:claude
    pub run_id: Option<String>,
    /// GitHub workflow identity, verified from the owning Actions run.
    pub workflow_id: Option<String>,
    /// The check was cancelled rather than failing on its own. It still
    /// judges as [`CheckOutcome::Failed`] for every gate; only the red-main
    /// watch tells the two apart (a cancelled run is not a red main).
    // trace:STORY-2897 | ai:claude
    pub cancelled: bool,
    /// The attempt of the owning GitHub Actions run (1 for the first run, 2
    /// after one re-run), verified from that run; `None` elsewhere.
    // trace:STORY-2897 | ai:claude
    pub run_attempt: Option<u64>,
    /// The owning GitHub Actions run has not completed (a re-run started
    /// after this row was listed): its failure is not a finished attempt.
    // trace:STORY-2897 | ai:claude
    pub run_in_progress: bool,
}

/// Every check the forge reports on `sha`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitChecks {
    pub sha: String,
    pub checks: Vec<CommitCheck>,
}

/// The verdict on a commit's checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommitCi {
    /// At least one check passed, every other one passed or was skipped,
    /// and every required check passed.
    Green,
    /// No check ran on the commit: none is reported, or every one was skipped
    /// (a label-event run executes no job).
    NoChecks,
    /// A required check is absent, skipped or neutral: it did not report
    /// success, so the commit's CI is unknown.
    MissingRequired(String),
    /// Some checks have not finished; names them.
    Pending(Vec<String>),
    /// Some checks failed; names them.
    Failed(Vec<String>),
}

/// The check AIDA's own CI reports once every lane has passed (the
/// `build-result` job, whose single step is "Require every CI lane").
/// Tier replacement checks this name against the replacement run.
// trace:TASK-1945 | ai:claude (TASK-2201)
pub(crate) const CI_AGGREGATOR_CHECK: &str = "Build (ubuntu-latest)";

/// The name the CI workflow's tier job takes on a label event that leaves the
/// tier alone (`.github/workflows/ci.yml`): every job of such a run is
/// skipped, so the run says nothing about the commit.
// trace:STORY-2809 | ai:claude
const LABEL_EVENT_MARKER: &str = "label event (CI tier unchanged)";

/// Map cancelled FAST runs to their newest FULL replacement on the same head.
/// Check names nominate candidates; Actions metadata establishes ownership.
// trace:BUG-2692 | ai:codex
pub(crate) fn replaced_fast_runs<'a>(
    rows: impl IntoIterator<Item = (&'a str, &'a str)>,
    expected_head: Option<&str>,
    mut metadata: impl FnMut(u64) -> Result<serde_json::Value>,
) -> std::collections::BTreeMap<u64, u64> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut fast = BTreeSet::new();
    let mut full = BTreeSet::new();
    for (name, run) in rows {
        if let Ok(id) = run.parse::<u64>() {
            match name {
                "ci-tier: fast" => {
                    fast.insert(id);
                }
                "ci-tier: full" => {
                    full.insert(id);
                }
                _ => {}
            }
        }
    }
    let mut identity = |id| {
        let run = metadata(id).ok()?;
        if run["id"].as_u64()? != id || run["event"].as_str()? != "pull_request" {
            return None;
        }
        let sha = run["head_sha"].as_str()?;
        if sha.len() != 40
            || !sha.bytes().all(|byte| byte.is_ascii_hexdigit())
            || expected_head.is_some_and(|head| !sha.eq_ignore_ascii_case(head))
        {
            return None;
        }
        Some((
            (
                run["workflow_id"].as_u64()?,
                sha.to_ascii_lowercase(),
                run["head_repository"]["id"].as_u64()?,
                run["head_branch"].as_str()?.to_owned(),
            ),
            match run.get("conclusion")? {
                serde_json::Value::Null => false,
                value => value.as_str()? == "cancelled",
            },
        ))
    };
    if fast.is_empty() || full.is_empty() {
        return BTreeMap::new();
    }
    let successors: Vec<_> = full
        .into_iter()
        .rev()
        .filter_map(|id| {
            let (key, cancelled) = identity(id)?;
            (!cancelled).then_some((id, key))
        })
        .collect();
    fast.into_iter()
        .filter_map(|id| {
            let (key, cancelled) = identity(id)?;
            if !cancelled {
                return None;
            }
            successors
                .iter()
                .find(|(new, other)| *new > id && *other == key)
                .map(|(new, _)| (id, *new))
        })
        .collect()
}

pub(crate) fn is_replaced_run(
    replaced: &std::collections::BTreeMap<u64, u64>,
    run_id: Option<&str>,
) -> bool {
    run_id
        .and_then(|id| id.parse::<u64>().ok())
        .is_some_and(|id| replaced.contains_key(&id))
}

/// The rows that count: every row except those of a workflow run that
/// executed nothing or was superseded.
///
/// A run executed nothing when every job in it was skipped, or when its tier
/// job is the label-event marker. Such a run is dropped whole: it neither
/// counts nor supersedes. Its skipped rows repeat names the real run on the
/// commit reports (a skipped matrix or release job), so a later label event
/// would otherwise displace the run that did the work.
///
/// Among the runs that executed, a run is superseded when a newer one reports
/// a check of the same name in the same verified workflow; it then contributes nothing, whatever else it
/// reported. A `ci:full` label event restarts a PR's CI, and the cancelled run
/// leaves rows the newer run never repeats: its own tier marker (`ci-tier:
/// fast` against the newer run's `ci-tier: full`) and matrix jobs cancelled
/// before their names expanded. Rows with no owning run (commit statuses) are
/// kept as reported.
// trace:STORY-2809 | ai:claude
// trace:STORY-2809 | ai:codex
pub(crate) fn latest_per_name(checks: &[CommitCheck]) -> Vec<&CommitCheck> {
    use std::collections::{HashMap, HashSet};
    let order = |c: &CommitCheck| c.run_id.as_deref().and_then(|id| id.parse::<u64>().ok());
    let mut executed: HashSet<u64> = HashSet::new();
    let mut label_event: HashSet<u64> = HashSet::new();
    for check in checks {
        if let Some(run) = order(check) {
            if check.outcome != CheckOutcome::Skipped {
                executed.insert(run);
            }
            if check.name == LABEL_EVENT_MARKER {
                label_event.insert(run);
            }
        }
    }
    let ran = |run: &u64| executed.contains(run) && !label_event.contains(run);
    let mut newest: HashMap<(&str, &str), u64> = HashMap::new();
    for check in checks {
        if let (Some(run), Some(workflow)) = (
            order(check).filter(|run| ran(run)),
            check.workflow_id.as_deref(),
        ) {
            let slot = newest.entry((workflow, check.name.as_str())).or_insert(run);
            *slot = (*slot).max(run);
        }
    }
    let superseded: HashSet<u64> = checks
        .iter()
        .filter_map(|c| {
            let workflow = c.workflow_id.as_deref()?;
            order(c).filter(|run| {
                ran(run)
                    && newest
                        .get(&(workflow, c.name.as_str()))
                        .is_some_and(|latest| latest > run)
            })
        })
        .collect();
    checks
        .iter()
        .filter(|c| order(c).is_none_or(|run| ran(&run) && !superseded.contains(&run)))
        .collect()
}

/// Judge `checks`: a failure outranks a pending check, which outranks a
/// required check that did not succeed. A skipped check passes only when it
/// is not required, and only beside a check that passed: a commit on which
/// nothing ran is never green. A workflow run that executed nothing, or that
/// a newer one on the commit superseded, does not count.
pub(crate) fn judge_commit_checks(checks: &CommitChecks, required: &[&str]) -> CommitCi {
    let latest = latest_per_name(&checks.checks);
    let named = |outcome: CheckOutcome| -> Vec<String> {
        latest
            .iter()
            .filter(|c| c.outcome == outcome)
            .map(|c| c.name.clone())
            .collect()
    };
    let failed = named(CheckOutcome::Failed);
    if !failed.is_empty() {
        return CommitCi::Failed(failed);
    }
    let pending = named(CheckOutcome::Pending);
    if !pending.is_empty() {
        return CommitCi::Pending(pending);
    }
    // Only skipped rows are left, or none: nothing ran on the commit.
    // trace:STORY-2809 | ai:claude
    if !latest.iter().any(|c| c.outcome == CheckOutcome::Passed) {
        return CommitCi::NoChecks;
    }
    if let Some(missing) = required.iter().find(|name| {
        !latest
            .iter()
            .any(|c| c.name == **name && c.outcome == CheckOutcome::Passed)
    }) {
        return CommitCi::MissingRequired(missing.to_string());
    }
    CommitCi::Green
}

/// Pages read before a check listing is refused as unbounded.
const MAX_PAGES: u32 = 10;
const PER_PAGE: usize = 100;

fn json(bytes: &[u8], what: &str) -> Result<serde_json::Value> {
    serde_json::from_slice(bytes).with_context(|| format!("invalid {what} JSON"))
}

fn require_sha(found: Option<&str>, sha: &str, what: &str) -> Result<()> {
    let found = found.with_context(|| format!("{what} names no commit"))?;
    anyhow::ensure!(
        found.eq_ignore_ascii_case(sha),
        "{what} is for commit {found}, not {sha}"
    );
    Ok(())
}

/// GitHub: every check run (`commits/{sha}/check-runs`, paginated) and
/// every commit status (`commits/{sha}/status`) on `sha` in `repo`
/// (`owner/name`). `api` runs `gh api <endpoint>` and returns its stdout,
/// or `Err` when the command fails.
// trace:STORY-2809 | ai:codex
pub(crate) fn github_commit_checks(
    repo: &str,
    sha: &str,
    api: &mut dyn FnMut(&str) -> Result<Vec<u8>>,
) -> Result<CommitChecks> {
    let mut checks = Vec::new();
    let mut workflows = std::collections::HashMap::<String, (String, Option<u64>, bool)>::new();
    let mut total = None;
    let mut seen = 0usize;
    for page in 1..=MAX_PAGES {
        let body = api(&format!(
            "repos/{repo}/commits/{sha}/check-runs?per_page={PER_PAGE}&page={page}"
        ))?;
        let (count, runs) = parse_gh_check_runs(&json(&body, "check-runs")?, sha)?;
        anyhow::ensure!(
            total.is_none_or(|t| t == count),
            "check-run count changed while paging"
        );
        total = Some(count);
        let rows = runs.len();
        seen += rows;
        for mut check in runs {
            if let Some(run) = check.run_id.as_deref() {
                if !workflows.contains_key(run) {
                    let body = api(&format!("repos/{repo}/actions/runs/{run}"))?;
                    let value = json(&body, "workflow run")?;
                    require_sha(
                        value.get("head_sha").and_then(|v| v.as_str()),
                        sha,
                        "workflow run",
                    )?;
                    anyhow::ensure!(
                        value
                            .get("id")
                            .and_then(|v| v.as_u64())
                            .map(|id| id.to_string())
                            .as_deref()
                            == Some(run),
                        "workflow run identity mismatch"
                    );
                    let workflow = value
                        .get("workflow_id")
                        .and_then(|v| v.as_u64())
                        .context("workflow run has no workflow_id")?;
                    let attempt = value.get("run_attempt").and_then(|v| v.as_u64());
                    let in_progress =
                        value.get("status").and_then(|v| v.as_str()) != Some("completed");
                    workflows.insert(
                        run.to_string(),
                        (workflow.to_string(), attempt, in_progress),
                    );
                }
                if let Some((workflow, attempt, in_progress)) = workflows.get(run) {
                    check.workflow_id = Some(workflow.clone());
                    check.run_attempt = *attempt;
                    check.run_in_progress = *in_progress;
                }
            }
            checks.push(check);
        }
        if seen >= count || rows < PER_PAGE {
            anyhow::ensure!(
                seen == count,
                "check-runs listed {seen} of {count} runs; the listing is incomplete"
            );
            let status = api(&format!(
                "repos/{repo}/commits/{sha}/status?per_page={PER_PAGE}"
            ))?;
            checks.extend(parse_gh_commit_status(
                &json(&status, "commit status")?,
                sha,
            )?);
            return Ok(CommitChecks {
                sha: sha.to_string(),
                checks,
            });
        }
    }
    anyhow::bail!("check-runs on {sha} exceed {MAX_PAGES} pages")
}

/// One page of `commits/{sha}/check-runs`: the total count and its runs.
/// Every run must be on `sha`.
pub(crate) fn parse_gh_check_runs(
    value: &serde_json::Value,
    sha: &str,
) -> Result<(usize, Vec<CommitCheck>)> {
    let total = value
        .get("total_count")
        .and_then(|v| v.as_u64())
        .context("check-runs response has no total_count")? as usize;
    let runs = value
        .get("check_runs")
        .and_then(|v| v.as_array())
        .context("check-runs response has no check_runs")?;
    let mut out = Vec::new();
    for run in runs {
        let name = run
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|n| !n.is_empty())
            .context("check run has no name")?;
        require_sha(
            run.get("head_sha").and_then(|v| v.as_str()),
            sha,
            &format!("check run `{name}`"),
        )?;
        let status = run
            .get("status")
            .and_then(|v| v.as_str())
            .with_context(|| format!("check run `{name}` has no status"))?;
        let conclusion = run.get("conclusion").and_then(|v| v.as_str());
        let outcome = if status != "completed" {
            CheckOutcome::Pending
        } else {
            match conclusion {
                Some("success") => CheckOutcome::Passed,
                Some("neutral" | "skipped") => CheckOutcome::Skipped,
                Some(_) => CheckOutcome::Failed,
                None => anyhow::bail!("completed check run `{name}` has no conclusion"),
            }
        };
        out.push(CommitCheck {
            name: name.to_string(),
            outcome,
            workflow_id: None,
            cancelled: outcome == CheckOutcome::Failed && conclusion == Some("cancelled"),
            run_attempt: None,
            run_in_progress: false,
            run_id: run
                .get("details_url")
                .and_then(|v| v.as_str())
                .and_then(crate::ci_gate::run_id_from_check_link),
        });
    }
    Ok((total, out))
}

/// `commits/{sha}/status`: the latest status per context on `sha`. A
/// commit with no statuses yields none (GitHub reports such a commit as
/// `pending` overall, which is not a check).
pub(crate) fn parse_gh_commit_status(
    value: &serde_json::Value,
    sha: &str,
) -> Result<Vec<CommitCheck>> {
    require_sha(
        value.get("sha").and_then(|v| v.as_str()),
        sha,
        "commit status",
    )?;
    let total = value
        .get("total_count")
        .and_then(|v| v.as_u64())
        .context("commit status has no total_count")? as usize;
    let statuses = value
        .get("statuses")
        .and_then(|v| v.as_array())
        .context("commit status has no statuses")?;
    anyhow::ensure!(
        statuses.len() == total,
        "commit status listed {} of {total} contexts; the listing is incomplete",
        statuses.len()
    );
    statuses
        .iter()
        .map(|status| {
            let name = status
                .get("context")
                .and_then(|v| v.as_str())
                .filter(|n| !n.is_empty())
                .context("commit status has no context")?;
            let outcome = match status.get("state").and_then(|v| v.as_str()) {
                Some("success") => CheckOutcome::Passed,
                Some("pending") => CheckOutcome::Pending,
                Some("failure" | "error") => CheckOutcome::Failed,
                other => anyhow::bail!("commit status `{name}` has unknown state {other:?}"),
            };
            Ok(CommitCheck {
                name: name.to_string(),
                outcome,
                run_id: None,
                workflow_id: None,
                cancelled: false,
                run_attempt: None,
                run_in_progress: false,
            })
        })
        .collect()
}

/// GitLab: every job status on `sha` in `project` (`group/name`), from
/// `projects/:id/repository/commits/:sha/statuses` (paginated). `api` runs
/// `glab api <endpoint>`.
pub(crate) fn gitlab_commit_checks(
    project: &str,
    sha: &str,
    api: &mut dyn FnMut(&str) -> Result<Vec<u8>>,
) -> Result<CommitChecks> {
    let project = project.replace('/', "%2F");
    let mut rows = Vec::new();
    for page in 1..=MAX_PAGES {
        let body = api(&format!(
            "projects/{project}/repository/commits/{sha}/statuses?all=true&per_page={PER_PAGE}&page={page}"
        ))?;
        let value = json(&body, "commit statuses")?;
        let page_rows = value
            .as_array()
            .context("commit statuses response is not a list")?;
        let count = page_rows.len();
        rows.extend(page_rows.iter().cloned());
        if count < PER_PAGE {
            return Ok(CommitChecks {
                sha: sha.to_string(),
                checks: parse_glab_commit_statuses(&rows, sha)?,
            });
        }
    }
    anyhow::bail!("commit statuses on {sha} exceed {MAX_PAGES} pages")
}

/// GitLab job statuses on `sha`, keeping the newest (highest id) per name.
/// An allowed failure counts as skipped.
pub(crate) fn parse_glab_commit_statuses(
    rows: &[serde_json::Value],
    sha: &str,
) -> Result<Vec<CommitCheck>> {
    let mut latest: std::collections::BTreeMap<String, (u64, CheckOutcome, bool)> =
        std::collections::BTreeMap::new();
    for row in rows {
        let name = row
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|n| !n.is_empty())
            .context("commit status has no name")?;
        require_sha(
            row.get("sha").and_then(|v| v.as_str()),
            sha,
            &format!("commit status `{name}`"),
        )?;
        let id = row
            .get("id")
            .and_then(|v| v.as_u64())
            .with_context(|| format!("commit status `{name}` has no id"))?;
        let allowed = row
            .get("allow_failure")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let outcome = match row.get("status").and_then(|v| v.as_str()) {
            Some("success") => CheckOutcome::Passed,
            Some("skipped") => CheckOutcome::Skipped,
            Some("failed") if allowed => CheckOutcome::Skipped,
            Some("failed" | "canceled") => CheckOutcome::Failed,
            Some(
                "created"
                | "pending"
                | "running"
                | "manual"
                | "scheduled"
                | "preparing"
                | "waiting_for_resource",
            ) => CheckOutcome::Pending,
            other => anyhow::bail!("commit status `{name}` has unknown status {other:?}"),
        };
        let cancelled = row.get("status").and_then(|v| v.as_str()) == Some("canceled");
        let entry = latest
            .entry(name.to_string())
            .or_insert((id, outcome, cancelled));
        if id >= entry.0 {
            *entry = (id, outcome, cancelled);
        }
    }
    Ok(latest
        .into_iter()
        .map(|(name, (_, outcome, cancelled))| CommitCheck {
            name,
            outcome,
            run_id: None,
            workflow_id: None,
            cancelled,
            run_attempt: None,
            run_in_progress: false,
        })
        .collect())
}

/// GitHub: the steps of job `job` in workflow run `run` (its latest
/// attempt) that did not succeed: failed, cancelled, timed out or not
/// finished. A skipped step is not one. `Err` when the run's job listing
/// cannot be read, is incomplete, or names `job` other than exactly once.
// trace:STORY-2912 | ai:claude
pub(crate) fn github_run_job_failed_steps(
    repo: &str,
    run: &str,
    job: &str,
    api: &mut dyn FnMut(&str) -> Result<Vec<u8>>,
) -> Result<Vec<String>> {
    failed_steps_of(&github_run_jobs(repo, run, api)?, job)
}

/// GitHub: every job of workflow run `run` (its latest attempt), read page
/// by page; `Err` when a page cannot be read or the listing is incomplete.
// trace:STORY-2897 | ai:claude
pub(crate) fn github_run_jobs(
    repo: &str,
    run: &str,
    api: &mut dyn FnMut(&str) -> Result<Vec<u8>>,
) -> Result<Vec<serde_json::Value>> {
    anyhow::ensure!(
        !run.is_empty() && run.bytes().all(|b| b.is_ascii_digit()),
        "invalid CI run id `{run}`"
    );
    let mut jobs = Vec::new();
    let mut total = None;
    for page in 1..=MAX_PAGES {
        let body = api(&format!(
            "repos/{repo}/actions/runs/{run}/jobs?filter=latest&per_page={PER_PAGE}&page={page}"
        ))?;
        let value = json(&body, "workflow jobs")?;
        let count = value
            .get("total_count")
            .and_then(|v| v.as_u64())
            .context("workflow jobs response has no total_count")? as usize;
        anyhow::ensure!(
            total.is_none_or(|t| t == count),
            "job count changed while paging"
        );
        total = Some(count);
        let rows = value
            .get("jobs")
            .and_then(|v| v.as_array())
            .context("workflow jobs response has no jobs")?
            .clone();
        let n = rows.len();
        jobs.extend(rows);
        if jobs.len() >= count || n < PER_PAGE {
            anyhow::ensure!(
                jobs.len() == count,
                "workflow run {run} listed {} of {count} jobs; the listing is incomplete",
                jobs.len()
            );
            return Ok(jobs);
        }
    }
    anyhow::bail!("jobs of workflow run {run} exceed {MAX_PAGES} pages")
}

/// The not-succeeded steps of the one job named `job` in `jobs` (GitHub
/// `actions/runs/{id}/jobs` rows).
// trace:STORY-2912 | ai:claude
pub(crate) fn failed_steps_of(jobs: &[serde_json::Value], job: &str) -> Result<Vec<String>> {
    let named: Vec<&serde_json::Value> = jobs
        .iter()
        .filter(|j| j.get("name").and_then(|v| v.as_str()) == Some(job))
        .collect();
    let [one] = named.as_slice() else {
        anyhow::bail!("the run has {} jobs named `{job}`, not one", named.len());
    };
    anyhow::ensure!(
        one.get("status").and_then(|v| v.as_str()) == Some("completed"),
        "job `{job}` has not completed"
    );
    let steps = one
        .get("steps")
        .and_then(|v| v.as_array())
        .with_context(|| format!("job `{job}` lists no steps"))?;
    let mut failed = Vec::new();
    for step in steps {
        let name = step
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|n| !n.is_empty())
            .with_context(|| format!("a step of job `{job}` has no name"))?;
        match step.get("conclusion").and_then(|v| v.as_str()) {
            Some("success" | "skipped" | "neutral") => {}
            _ => failed.push(name.to_string()),
        }
    }
    Ok(failed)
}

#[cfg(test)]
#[path = "tests/task_1945_commit_ci_tests.rs"]
mod tests;
