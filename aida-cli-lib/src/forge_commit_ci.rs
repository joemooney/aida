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
    /// At least one check, every one passed or skipped, and every
    /// required check passed.
    Green,
    /// No check at all is reported on the commit.
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
pub(crate) const CI_AGGREGATOR_CHECK: &str = "Build (ubuntu-latest)";

/// Judge `checks`: a failure outranks a pending check, which outranks a
/// required check that did not succeed. A skipped check passes only when it
/// is not required. An empty set is never green.
pub(crate) fn judge_commit_checks(checks: &CommitChecks, required: &[&str]) -> CommitCi {
    let named = |outcome: CheckOutcome| -> Vec<String> {
        checks
            .checks
            .iter()
            .filter(|c| c.outcome == outcome)
            .map(|c| c.name.clone())
            .collect()
    };
    if checks.checks.is_empty() {
        return CommitCi::NoChecks;
    }
    let failed = named(CheckOutcome::Failed);
    if !failed.is_empty() {
        return CommitCi::Failed(failed);
    }
    let pending = named(CheckOutcome::Pending);
    if !pending.is_empty() {
        return CommitCi::Pending(pending);
    }
    if let Some(missing) = required.iter().find(|name| {
        !checks
            .checks
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
pub(crate) fn github_commit_checks(
    repo: &str,
    sha: &str,
    api: &mut dyn FnMut(&str) -> Result<Vec<u8>>,
) -> Result<CommitChecks> {
    let mut checks = Vec::new();
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
        checks.extend(runs);
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
        .map(|(name, (_, outcome))| CommitCheck { name, outcome })
        .collect())
}

#[cfg(test)]
#[path = "tests/task_1945_commit_ci_tests.rs"]
mod tests;
