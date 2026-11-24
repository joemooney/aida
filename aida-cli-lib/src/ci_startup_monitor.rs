//! Report required Actions workflows that never start jobs; never supplies CI credit.
// trace:STORY-2856 | ai:codex
// trace:STORY-2856 | ai:claude
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::Path,
    process::Output,
    time::Duration,
};

/// Actions returns at most this many runs per page; a full page means older
/// history exists that this pass did not see.
const RUN_PAGE: usize = 100;
/// Bound on pages read for one paginated inventory or history search.
const MAX_PAGES: usize = 10;

// Shrink each successive page by one row. Page p then overlaps the previous
// window by p-1 rows, covering that many deletions between reads. MAX_PAGES
// keeps these windows advancing and bounds the cost of discovery.
// trace:BUG-2592 | ai:codex
fn page_size(page: usize) -> usize {
    RUN_PAGE - (page - 1)
}

fn paged(base: &str, page: usize) -> String {
    let separator = if base.contains('?') { '&' } else { '?' };
    let url = format!("{base}{separator}per_page={}", page_size(page));
    if page == 1 {
        url
    } else {
        format!("{url}&page={page}")
    }
}

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
    mapping_scan: Option<MappingScan>,
    recoveries: VecDeque<Recovery>,
}

// trace:BUG-2694 | ai:codex
#[derive(Serialize, Deserialize)]
struct Recovery {
    rule: String,
    message: String,
    accepted: BTreeSet<String>,
}

#[derive(Serialize, Deserialize)]
struct MappingScan {
    branch: String,
    next_page: usize,
    candidate: Option<u64>,
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
    send: &'a mut dyn FnMut(&str, &str, &str, &mut BTreeSet<String>) -> Result<bool>,
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
    // queued or running runs now sit above it. Recording a durable recovery
    // resets the episode so the same success does not enqueue it twice.
    if episode.notices > 0 {
        let boundary = if episode.latest_failure != 0 {
            // A rerun keeps the run id, and the run's status and conclusion
            // describe its latest attempt. The alerted failure itself now
            // succeeding is therefore a successful later attempt: include it.
            // trace:BUG-2689 | ai:claude
            runs.iter()
                .position(|r| r["id"].as_u64() == Some(episode.latest_failure))
                .map_or(runs.len(), |p| p + usize::from(kinds[p] == Kind::Succeeded))
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

/// Inventory progress and the first observed head of each eligible PR.
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
    Inventory {
        page: usize,
        seen: BTreeSet<u64>,
    },
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
        *progress = Some(BlockedScan {
            branch: branch.to_owned(),
            mapped: mapped.to_vec(),
            pending: Vec::new(),
            count: 0,
            step: PrStep::Inventory {
                page: 1,
                seen: BTreeSet::new(),
            },
        });
    }
    let scan = progress.as_mut().expect("scan initialized above");
    // Checkpoint only fully parsed pages. Keep even excluded PR numbers in
    // the seen set so shifting pages cannot replace the first observation.
    // trace:BUG-2694 | ai:codex
    while let PrStep::Inventory { page, seen } = &mut scan.step {
        anyhow::ensure!((1..=MAX_PAGES).contains(page), "invalid PR inventory page");
        let response = api_json(port, &paged("repos/{owner}/{repo}/pulls?state=open", *page))?;
        let rows = response.as_array().context("invalid PR inventory")?;
        let complete = rows.len() < page_size(*page);
        if !complete && *page >= MAX_PAGES {
            *progress = None;
            anyhow::bail!("PR inventory truncated");
        }
        let mut numbers = BTreeSet::new();
        let mut heads = Vec::new();
        for pr in rows {
            let number = pr["number"].as_u64().context("missing PR number")?;
            if !seen.contains(&number)
                && numbers.insert(number)
                && pr["draft"] != true
                && pr["base"]["ref"] == branch
            {
                heads.push(PrHead {
                    number,
                    sha: text(&pr["head"], "sha")?.to_owned(),
                });
            }
        }
        seen.extend(numbers);
        scan.pending.extend(heads);
        *page += 1;
        if complete {
            scan.pending.reverse();
            scan.step = PrStep::Checks;
        }
    }
    while let Some(PrHead { number, sha }) = scan.pending.last() {
        match &scan.step {
            PrStep::Inventory { .. } => unreachable!("inventory completed above"),
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

/// Id of the newest run (newest first) that started a job.
fn newest_started(runs: &[Value], cx: &Pass) -> Result<Option<u64>> {
    for run in runs {
        if matches!(
            classify(run, cx.config, cx.now)?,
            Kind::Started | Kind::Succeeded
        ) {
            return run_id(run).map(Some);
        }
    }
    Ok(None)
}

/// Observe one workflow: one run-list read (plus older pages only to map jobs
/// on fresh state), at most one job read when the newest started run changed,
/// and the blocked-PR inventory only when a notice is due.
fn observe_workflow(port: &mut Port, cx: &Pass, workflow: &Value, state: &mut State) -> Result<()> {
    let id = workflow["id"].as_u64().context("missing workflow id")?;
    let key = id.to_string();
    let episode = state.entry(key.clone()).or_default();
    let name = text(workflow, "name")?;
    let base = format!("repos/{{owner}}/{{repo}}/actions/workflows/{id}/runs");
    let response = api_json(port, &paged(&base, 1))?;
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
    let mut started = newest_started(&runs, cx)?;
    // Look further back only on fresh mapping state. Checkpoint both the
    // cursor and the candidate in case the jobs read needs another pass.
    // trace:BUG-2694 | ai:codex
    if started.is_none() && !complete && episode.jobs.is_empty() {
        if episode
            .mapping_scan
            .as_ref()
            .is_none_or(|scan| scan.branch != cx.branch)
        {
            episode.mapping_scan = Some(MappingScan {
                branch: cx.branch.to_owned(),
                next_page: 2,
                candidate: None,
            });
        }
        let scan = episode
            .mapping_scan
            .as_mut()
            .expect("mapping scan initialized above");
        while scan.candidate.is_none() && scan.next_page <= MAX_PAGES {
            anyhow::ensure!(scan.next_page >= 2, "invalid mapping page");
            let older = api_json(port, &paged(&base, scan.next_page))?;
            let older = rows(&older, "workflow_runs")?;
            let mut eligible = Vec::new();
            for run in older {
                if protected_provenance(run, cx.branch)? {
                    eligible.push(run.clone());
                }
            }
            scan.candidate = newest_started(&eligible, cx)?;
            scan.next_page = if older.len() < page_size(scan.next_page) {
                MAX_PAGES + 1
            } else {
                scan.next_page + 1
            };
        }
        started = scan.candidate;
    } else {
        episode.mapping_scan = None;
    }
    if let Some(started) = started {
        if episode.mapping_run != started {
            let jobs = api_json(
                port,
                &format!("repos/{{owner}}/{{repo}}/actions/runs/{started}/jobs?per_page=100"),
            )?;
            let count = jobs["total_count"].as_u64().context("missing jobs count")?;
            anyhow::ensure!(count < 100, "workflow job inventory truncated");
            episode.jobs = rows(&jobs, "jobs")?
                .iter()
                .filter_map(|j| j["name"].as_str())
                .map(str::to_owned)
                .collect();
            episode.mapping_run = started;
            episode.mapping_scan = None;
        }
    }
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
        episode.mapping_scan = None;
        episode.checked = cx.now;
        return Ok(());
    }
    let Some(recovered) = evaluate(episode, &runs, complete, cx.config, cx.now)? else {
        episode.blocked_scan = None;
        episode.mapping_scan = None;
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
    episode.mapping_scan = None;
    episode.checked = cx.now;
    if recovered {
        let first_pending = episode.recoveries.is_empty();
        episode.recoveries.push_back(Recovery {
            rule,
            message,
            accepted: BTreeSet::new(),
        });
        episode.reset_episode();
        // Commit the observation before attempting transport so the retry
        // can outlive the successful run's visibility in the run list.
        (port.persist)(state)?;
        if first_pending {
            return deliver_recovery(port, state, &key);
        }
        return Ok(());
    }
    let delivered = episode.deliver(false, cx.now, || {
        let mut accepted = BTreeSet::new();
        (port.send)(&rule, "Required CI never started", &message, &mut accepted)?;
        Ok(!accepted.is_empty())
    })?;
    if delivered {
        (port.persist)(state)?;
    }
    Ok(())
}

fn deliver_recovery(port: &mut Port, state: &mut State, key: &str) -> Result<()> {
    let queue = &mut state
        .get_mut(key)
        .expect("workflow episode exists")
        .recoveries;
    let Some(mut recovery) = queue.pop_front() else {
        return Ok(());
    };
    let result = (port.send)(
        &recovery.rule,
        "Required CI recovered",
        &recovery.message,
        &mut recovery.accepted,
    );
    if !matches!(result, Ok(true)) {
        queue.push_back(recovery);
    }
    (port.persist)(state)?;
    result.map(|_| ())
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
        if let Some(id) = workflow["id"].as_u64() {
            let key = id.to_string();
            if state
                .get(&key)
                .is_some_and(|episode| !episode.recoveries.is_empty())
            {
                if let Err(error) = deliver_recovery(port, state, &key) {
                    if outcome.is_ok() {
                        outcome = Err(error);
                    }
                }
            }
        }
        if let Err(error) = observe_workflow(port, &cx, workflow, state) {
            let exhausted = error.downcast_ref::<BudgetExhausted>().is_some();
            if let Some(id) = workflow["id"].as_u64() {
                let episode = state.entry(id.to_string()).or_default();
                if !exhausted || episode.blocked_scan.is_some() || episode.mapping_scan.is_some() {
                    episode.checked = now;
                }
                if !exhausted {
                    // A candidate may have been deleted, or an inventory page
                    // may have shifted after an error. Rediscover on retry.
                    episode.mapping_scan = None;
                    if episode
                        .blocked_scan
                        .as_ref()
                        .is_some_and(|scan| matches!(scan.step, PrStep::Inventory { .. }))
                    {
                        episode.blocked_scan = None;
                    }
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
    // Alerts go to `[notify] command` and every `[seat_health] push_commands`
    // channel. Do not observe or advance backoff when neither is configured.
    // trace:BUG-2689 | ai:claude
    if !crate::notify::alert_transport_configured(&body)? {
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
    let mut send =
        |rule: &str, title: &str, message: &str, accepted: &mut BTreeSet<String>| -> Result<bool> {
            crate::notify::send_alert_all_channels(
                root,
                rule,
                title,
                message,
                Some(Duration::from_secs(3)),
                accepted,
            )
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
