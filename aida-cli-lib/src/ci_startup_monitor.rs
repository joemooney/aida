//! Report required Actions workflows that never start jobs; never supplies CI credit.
// trace:STORY-2856 | ai:codex
// trace:STORY-2856 | ai:claude
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path, process::Output, time::Duration};

/// Actions returns at most this many runs per page; a full page means older
/// history exists that this pass did not see.
const RUN_PAGE: usize = 100;

/// Per-workflow persisted state. Every field defaults so state files written
/// by earlier versions (`first`, `next`, `notices` only) still load.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Episode {
    /// Earliest observed failure (`created_at`) of the open episode.
    first: String,
    /// The streak reached past the visible run history, so the true start may
    /// be earlier than `first`.
    first_partial: bool,
    next: i64,
    notices: u32,
    /// Newest failed run id of the open episode. Only a success newer than it
    /// is recovery; a streak that no longer contains it is a new episode.
    latest_failure: u64,
    /// Last completed observation; least-recently-checked workflows go first
    /// so a budget-exhausted pass resumes where the previous one stopped.
    checked: i64,
    /// Started run whose job names were last read, and those names.
    mapping_run: u64,
    jobs: Vec<String>,
    blocked_scan: Option<BlockedScan>,
}
type State = BTreeMap<String, Episode>;

impl Episode {
    fn active(&self) -> bool {
        self.notices > 0 || !self.first.is_empty()
    }
    fn reset_episode(&mut self) {
        self.first.clear();
        self.first_partial = false;
        self.next = 0;
        self.notices = 0;
        self.latest_failure = 0;
        self.blocked_scan = None;
    }
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
            self.reset_episode();
        } else {
            self.blocked_scan = None;
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

/// Side effects of one observation pass. Production binds these to the
/// bounded `gh` transport, the notify channel and the state file; tests bind
/// them to API-shaped fixtures.
struct Port<'a> {
    api: &'a mut dyn FnMut(&str) -> Result<Output>,
    review: &'a mut dyn FnMut(u64, &str) -> Result<Option<String>>,
    send: &'a mut dyn FnMut(&str, &str, &str) -> Result<bool>,
    persist: &'a mut dyn FnMut(&State) -> Result<()>,
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
fn run_id(v: &Value) -> Result<u64> {
    v["id"].as_u64().context("missing run id")
}
fn api_json(port: &mut Port, endpoint: &str) -> Result<Value> {
    let out = (port.api)(endpoint)?;
    anyhow::ensure!(
        out.status.success(),
        "startup observation failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(serde_json::from_slice(&out.stdout)?)
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

/// Whether a run reports on the protected default branch. Push-like events
/// carry the branch in `head_branch`. `pull_request_target` runs execute the
/// base branch's workflow definition but carry the PR's source branch in
/// `head_branch`, and Actions leaves `pull_requests` empty for startup
/// failures and fork PRs, so those are attributed by the protected target when
/// named and to the default branch otherwise.
fn protected_provenance(run: &Value, branch: &str) -> Result<bool> {
    Ok(match text(run, "event")? {
        "pull_request_target" | "pull_request" => {
            let prs = rows(run, "pull_requests")?;
            prs.is_empty() || prs.iter().any(|p| p["base"]["ref"] == branch)
        }
        _ => run["head_branch"] == branch,
    })
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    /// Not yet resolved (fresh queue, environment approval); neither counts
    /// toward nor breaks a streak.
    Pending,
    /// Never started a job: `startup_failure`, or still queued past the
    /// timeout. Actions moves a run to `in_progress` as soon as any job
    /// starts, so both are verified by the run's own fields.
    Failed,
    /// A job started (in progress or a non-startup conclusion).
    Started,
    Succeeded,
}
fn classify(run: &Value, config: &Config, now: i64) -> Result<Kind> {
    Ok(match text(run, "status")? {
        "completed" => match run["conclusion"].as_str() {
            Some("startup_failure") => Kind::Failed,
            Some("success") => Kind::Succeeded,
            _ => Kind::Started,
        },
        "queued" | "requested" | "pending" => {
            if now - timestamp(run, "created_at")? >= config.queued_timeout_secs {
                Kind::Failed
            } else {
                Kind::Pending
            }
        }
        "waiting" => Kind::Pending,
        _ => Kind::Started,
    })
}

/// Decide what to send for one workflow. `runs` is newest first; `complete`
/// says the whole history is visible. `Some(false)` is an outage notice due
/// now, `Some(true)` a recovery notice.
fn evaluate(
    episode: &mut Episode,
    runs: &[Value],
    complete: bool,
    config: &Config,
    now: i64,
) -> Result<Option<bool>> {
    let kinds = runs
        .iter()
        .map(|r| classify(r, config, now))
        .collect::<Result<Vec<_>>>()?;
    // Recovery: any success newer than the alerted streak, even when newer
    // queued or running runs now sit above it. Exactly once: delivery resets
    // the episode, so the same success never recovers twice.
    if episode.notices > 0 {
        let boundary = if episode.latest_failure != 0 {
            runs.iter()
                .position(|r| r["id"].as_u64() == Some(episode.latest_failure))
                .unwrap_or(runs.len())
        } else {
            kinds
                .iter()
                .position(|k| *k == Kind::Failed)
                .unwrap_or(runs.len())
        };
        if kinds[..boundary].contains(&Kind::Succeeded) {
            return Ok(Some(true));
        }
    }
    // The streak: newest resolved runs that never started, skipping pending.
    let mut streak = Vec::new();
    let mut ended = false;
    for (run, kind) in runs.iter().zip(&kinds) {
        match kind {
            Kind::Pending => continue,
            Kind::Failed => streak.push(run),
            Kind::Started | Kind::Succeeded => {
                ended = true;
                break;
            }
        }
    }
    if streak.len() < config.consecutive_runs {
        if episode.notices == 0 {
            episode.reset_episode();
        }
        return Ok(None);
    }
    let ids = streak
        .iter()
        .map(|r| run_id(r))
        .collect::<Result<Vec<_>>>()?;
    let visible = runs
        .iter()
        .any(|r| r["id"].as_u64() == Some(episode.latest_failure));
    // A streak that no longer contains the episode's failure is a new episode
    // (its start must not inherit the old one), unless that failure aged out of
    // an incomplete history the streak still runs into.
    let continues = episode.latest_failure != 0
        && (ids.contains(&episode.latest_failure) || (!visible && !ended && !complete));
    if !continues {
        episode.blocked_scan = None;
        episode.first.clear();
        episode.first_partial = false;
    }
    let oldest = streak.last().expect("streak meets a positive threshold");
    let earliest = text(oldest, "created_at")?;
    let known = DateTime::parse_from_rfc3339(&episode.first).map_or(i64::MAX, |t| t.timestamp());
    if timestamp(oldest, "created_at")? <= known {
        episode.first = earliest.to_owned();
        episode.first_partial = !ended && !complete;
    }
    episode.latest_failure = ids[0];
    Ok((now >= episode.next).then_some(false))
}

/// The open-PR inventory and head SHAs are pinned when a notice becomes due.
#[derive(Serialize, Deserialize)]
struct BlockedScan {
    branch: String,
    mapped: Vec<String>,
    pending: Vec<PrHead>,
    count: usize,
    step: PrStep,
}

#[derive(Serialize, Deserialize)]
struct PrHead {
    number: u64,
    sha: String,
}

#[derive(Default, Serialize, Deserialize)]
enum PrStep {
    #[default]
    Checks,
    Statuses(Vec<String>),
    Review,
}

fn count_blocked(
    port: &mut Port,
    branch: &str,
    mapped: &[String],
    progress: &mut Option<BlockedScan>,
) -> Result<usize> {
    if progress
        .as_ref()
        .is_none_or(|scan| scan.branch != branch || scan.mapped != mapped)
    {
        let prs = api_json(port, "repos/{owner}/{repo}/pulls?state=open&per_page=100")?;
        let prs = prs.as_array().context("invalid PR inventory")?;
        anyhow::ensure!(prs.len() < 100, "PR inventory truncated");
        let pending = prs
            .iter()
            .rev()
            .filter(|pr| pr["draft"] != true && pr["base"]["ref"] == branch)
            .map(|pr| {
                Ok(PrHead {
                    number: pr["number"].as_u64().context("missing PR number")?,
                    sha: text(&pr["head"], "sha")?.to_owned(),
                })
            })
            .collect::<Result<_>>()?;
        *progress = Some(BlockedScan {
            branch: branch.to_owned(),
            mapped: mapped.to_vec(),
            pending,
            count: 0,
            step: PrStep::Checks,
        });
    }
    let scan = progress.as_mut().expect("scan initialized above");
    while let Some(PrHead { number, sha }) = scan.pending.last() {
        match &scan.step {
            PrStep::Checks => {
                let checks = api_json(
                    port,
                    &format!("repos/{{owner}}/{{repo}}/commits/{sha}/check-runs?per_page=100"),
                )?;
                let checks = rows(&checks, "check_runs")?;
                anyhow::ensure!(checks.len() < 100, "check inventory truncated");
                scan.step = PrStep::Statuses(
                    mapped
                        .iter()
                        .filter(|name| !head_has_context(name, checks, &[]))
                        .cloned()
                        .collect(),
                );
            }
            PrStep::Statuses(missing) => {
                let statuses = api_json(
                    port,
                    &format!("repos/{{owner}}/{{repo}}/commits/{sha}/status?per_page=100"),
                )?;
                let statuses = rows(&statuses, "statuses")?;
                anyhow::ensure!(statuses.len() < 100, "status inventory truncated");
                if missing
                    .iter()
                    .all(|name| head_has_context(name, &[], statuses))
                {
                    scan.pending.pop();
                    scan.step = PrStep::Checks;
                } else {
                    scan.step = PrStep::Review;
                }
            }
            PrStep::Review => {
                // GitHub accounts for dismissed reviews, but its decision is
                // for the current PR head. Reject a mixed-head count.
                let Some(decision) = (port.review)(*number, sha)? else {
                    *progress = None;
                    anyhow::bail!("PR head changed during blocked-PR scan");
                };
                scan.count += usize::from(decision == "APPROVED");
                scan.pending.pop();
                scan.step = PrStep::Checks;
            }
        }
    }
    Ok(scan.count)
}

struct Pass<'a> {
    branch: &'a str,
    required: &'a [String],
    config: &'a Config,
    now: i64,
}

/// Observe one workflow: one run-list read, at most one job read when the
/// newest started run changed, and the blocked-PR inventory only when a
/// notice is due.
fn observe_workflow(port: &mut Port, cx: &Pass, workflow: &Value, state: &mut State) -> Result<()> {
    let id = workflow["id"].as_u64().context("missing workflow id")?;
    let name = text(workflow, "name")?;
    let response = api_json(
        port,
        &format!("repos/{{owner}}/{{repo}}/actions/workflows/{id}/runs?per_page={RUN_PAGE}"),
    )?;
    let all = rows(&response, "workflow_runs")?;
    let complete = all.len() < RUN_PAGE;
    let mut runs = Vec::new();
    for run in all {
        if protected_provenance(run, cx.branch)? {
            runs.push(run.clone());
        }
    }
    // Map ordinary contexts using the newest started run's job names, cached
    // per run. Custom status publishers (merge-hold-gate) use the exact
    // workflow display name as their context.
    let started = runs
        .iter()
        .map(|r| Ok((r, classify(r, cx.config, cx.now)?)))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .find(|(_, k)| matches!(k, Kind::Started | Kind::Succeeded))
        .map(|(r, _)| run_id(r))
        .transpose()?;
    let key = id.to_string();
    if let Some(started) = started {
        if state.get(&key).is_none_or(|e| e.mapping_run != started) {
            let jobs = api_json(
                port,
                &format!("repos/{{owner}}/{{repo}}/actions/runs/{started}/jobs?per_page=100"),
            )?;
            let count = jobs["total_count"].as_u64().context("missing jobs count")?;
            anyhow::ensure!(count < 100, "workflow job inventory truncated");
            let episode = state.entry(key.clone()).or_default();
            episode.mapping_run = started;
            episode.jobs = rows(&jobs, "jobs")?
                .iter()
                .filter_map(|j| j["name"].as_str())
                .map(str::to_owned)
                .collect();
        }
    }
    let episode = state.entry(key).or_default();
    let mut mapped: Vec<String> = cx
        .required
        .iter()
        .filter(|r| *r == name || episode.jobs.contains(r))
        .cloned()
        .collect();
    mapped.sort();
    mapped.dedup();
    if mapped.is_empty() || runs.is_empty() {
        episode.blocked_scan = None;
        episode.checked = cx.now;
        return Ok(());
    }
    let Some(recovered) = evaluate(episode, &runs, complete, cx.config, cx.now)? else {
        episode.blocked_scan = None;
        episode.checked = cx.now;
        return Ok(());
    };
    let blocked = if recovered {
        episode.blocked_scan = None;
        0
    } else {
        count_blocked(port, cx.branch, &mapped, &mut episode.blocked_scan)?
    };
    episode.blocked_scan = None;
    let title = if recovered {
        "Required CI recovered"
    } else {
        "Required CI never started"
    };
    let first = if episode.first_partial {
        format!(
            "{} or earlier (older runs are beyond the visible history)",
            episode.first
        )
    } else {
        episode.first.clone()
    };
    let message = format!(
        "Workflow: {name}\nRequired checks: {}\nLatest run: {}\nFirst failure: {first}\nApproved PRs blocked by missing check: {blocked}\n{}\nRunbook: docs/advanced/ci-startup-failure.md",
        mapped.join(", "),
        text(&runs[0], "html_url")?,
        if recovered {
            "A run succeeded again."
        } else {
            "Consecutive runs have zero jobs (startup_failure or queued beyond timeout)."
        }
    );
    let rule = format!(
        "ci-startup-{id}-{}-{}-{recovered}",
        episode.first, episode.notices
    );
    let delivered = episode.deliver(recovered, cx.now, || (port.send)(&rule, title, &message))?;
    episode.checked = cx.now;
    // Persist each accepted delivery independently. A crash after transport
    // can repeat a notice; it cannot suppress the recovery or fabricate one.
    if delivered {
        (port.persist)(state)?;
    }
    Ok(())
}

/// One observation pass. Errors (including an exhausted budget) stop the
/// pass but still persist per-workflow progress, so the next pass starts with
/// the workflows this one did not reach. Unreadable data never proves recovery.
fn observe(port: &mut Port, config: &Config, state: &mut State, now: i64) -> Result<()> {
    let repo = api_json(port, "repos/{owner}/{repo}")?;
    let branch = text(&repo, "default_branch")?.to_owned();
    let required = {
        let api = &mut *port.api;
        crate::status_prs::train_required_status_checks(&branch, |e| api(e))?
    };
    if required.is_empty() {
        return Ok(());
    }
    let workflows = api_json(port, "repos/{owner}/{repo}/actions/workflows?per_page=100")?;
    let mut workflows = rows(&workflows, "workflows")?.clone();
    anyhow::ensure!(workflows.len() < 100, "workflow inventory truncated");
    // Least recently attempted first. Open incidents win ties.
    workflows.sort_by_key(|w| {
        let episode = w["id"].as_u64().and_then(|id| state.get(&id.to_string()));
        (
            episode.map_or(i64::MIN, |e| e.checked),
            !episode.is_some_and(Episode::active),
        )
    });
    let cx = Pass {
        branch: &branch,
        required: &required,
        config,
        now,
    };
    // An exhausted budget ends the pass. If a workflow got as far as
    // counting PRs, move it behind peers so a long scan cannot starve them.
    // Other per-workflow errors also use up the workflow's turn.
    let mut outcome = Ok(());
    for workflow in &workflows {
        if let Err(error) = observe_workflow(port, &cx, workflow, state) {
            let exhausted = error.downcast_ref::<BudgetExhausted>().is_some();
            if let Some(id) = workflow["id"].as_u64() {
                let episode = state.entry(id.to_string()).or_default();
                if !exhausted || episode.blocked_scan.is_some() {
                    episode.checked = now;
                }
            }
            if outcome.is_ok() {
                outcome = Err(error);
            }
            if exhausted {
                break;
            }
        }
    }
    (port.persist)(state)?;
    outcome
}

/// The shared observation budget ran out mid-pass.
#[derive(Debug)]
struct BudgetExhausted;
impl std::fmt::Display for BudgetExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("startup observation budget exhausted")
    }
}
impl std::error::Error for BudgetExhausted {}

/// Called by the supervised train and explicit notify checks. Errors retain prior
/// episodes: unreadable forge data never proves recovery.
pub(crate) fn check(root: &Path) -> Result<()> {
    // Unit tests drive `observe` through API-shaped ports; never contact the
    // live forge from notification or train unit-test callers.
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
    let mut state: State = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => return Err(e.into()),
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    let within = || -> Result<()> {
        if std::time::Instant::now() >= deadline {
            return Err(BudgetExhausted.into());
        }
        Ok(())
    };
    let mut api = |endpoint: &str| -> Result<Output> {
        within()?;
        crate::forge::github_startup_api(root, endpoint)
    };
    let mut review = |number: u64, head: &str| -> Result<Option<String>> {
        within()?;
        match crate::gate::ci_startup::github_startup_review_decision(root, number, head) {
            crate::forge_observed::Observed::Known(value) => Ok(value),
            other => anyhow::bail!("PR review observation unavailable: {other:?}"),
        }
    };
    let mut send = |rule: &str, title: &str, message: &str| -> Result<bool> {
        let outcome = crate::notify::send_direct_bounded(
            root,
            rule,
            title,
            message,
            Some(Duration::from_secs(3)),
        )?;
        Ok(matches!(
            outcome.delivery(),
            crate::notify::DirectDelivery::Sent | crate::notify::DirectDelivery::Deferred
        ))
    };
    let mut persist = |state: &State| -> Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(&dir)?;
        use std::io::Write;
        file.write_all(&serde_json::to_vec_pretty(state)?)?;
        file.as_file().sync_all()?;
        file.persist(&path)?;
        Ok(())
    };
    let mut port = Port {
        api: &mut api,
        review: &mut review,
        send: &mut send,
        persist: &mut persist,
    };
    observe(&mut port, &config, &mut state, Utc::now().timestamp())
}

#[cfg(all(test, unix))]
#[path = "ci_startup_monitor_tests.rs"]
mod tests;
