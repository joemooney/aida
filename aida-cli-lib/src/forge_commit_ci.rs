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
/// Readiness reads required checks from each project's config; this pins
/// AIDA's configured name to its workflow in tests.
// trace:TASK-1945 | ai:claude (TASK-2201)
#[cfg(test)]
pub(crate) const CI_AGGREGATOR_CHECK: &str = "Build (ubuntu-latest)";

/// The name the CI workflow's tier job takes on a label event that leaves the
/// tier alone (`.github/workflows/ci.yml`): every job of such a run is
/// skipped, so the run says nothing about the commit.
// trace:STORY-2809 | ai:claude
const LABEL_EVENT_MARKER: &str = "label event (CI tier unchanged)";

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
fn latest_per_name(checks: &[CommitCheck]) -> Vec<&CommitCheck> {
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
    let mut workflows = std::collections::HashMap::<String, String>::new();
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
                    workflows.insert(run.to_string(), workflow.to_string());
                }
                check.workflow_id = workflows.get(run).cloned();
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
        let outcome = if status != "completed" {
            CheckOutcome::Pending
        } else {
            match run.get("conclusion").and_then(|v| v.as_str()) {
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
    let mut latest: std::collections::BTreeMap<String, (u64, CheckOutcome)> =
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
        let entry = latest.entry(name.to_string()).or_insert((id, outcome));
        if id >= entry.0 {
            *entry = (id, outcome);
        }
    }
    Ok(latest
        .into_iter()
        .map(|(name, (_, outcome))| CommitCheck {
            name,
            outcome,
            run_id: None,
            workflow_id: None,
        })
        .collect())
}

#[cfg(test)]
#[path = "tests/task_1945_commit_ci_tests.rs"]
mod tests;
