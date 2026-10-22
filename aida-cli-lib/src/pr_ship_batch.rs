//! `aida pr ship-batch` (STORY-1887): ship human-gated PRs one at a time, in
//! the operator's order. Each gated merge moves main and makes the next gated
//! PR stale, so every PR is refreshed only when needed, CI and an independent
//! approval are awaited at its exact new head, and the operator confirms each
//! ship at the terminal. The ship itself is the ordinary `aida pr ship` path
//! with both overrides hard-false: the batch adds waiting, never a bypass.
//!
//! The step machine reads the world only through [`BatchPorts`], so its order
//! and every stop are unit-tested without a forge. Every failure stops the
//! batch without shipping; a re-run skips PRs that already merged.
// trace:STORY-1887 | ai:claude

use anyhow::Result;

/// C2: the most PRs one batch accepts.
pub(crate) const MAX_BATCH: usize = 20;
/// Seconds between CI / review polls.
const POLL_SECS: u64 = 30;
/// C1: how often the review wait says what it is waiting for.
const AWAITING_LINE_SECS: u64 = 60;
/// How many times a head may move under the waits before the batch stops.
const MAX_HEAD_MOVES: u32 = 2;
/// How long a refreshed PR may take to show its new head.
const REFRESH_SETTLE_SECS: u64 = 120;
/// `--review-timeout` default.
pub(crate) const DEFAULT_REVIEW_TIMEOUT_SECS: u64 = 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrState {
    Open,
    Merged,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mergeable {
    Clean,
    Conflicting,
    Unknown,
}

/// What the batch needs to know about one PR, read in one forge call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrView {
    pub state: PrState,
    pub mergeable: Mergeable,
    /// The forge says the branch must be brought up to date with its base.
    pub behind: bool,
    pub draft: bool,
    pub head_sha: String,
    pub head_ref: String,
    pub title: String,
    pub body: String,
}

/// CI as observed for one exact head (C3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CiAtHead {
    Pending,
    Passing,
    Failing(String),
    /// The PR's head is no longer the one asked about.
    HeadMoved,
}

/// The review evidence at one exact head, judged by the ship gate's readers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReviewAtHead {
    Approved(String),
    Negative(String),
    Awaiting(Option<String>),
}

/// The ship gate overrides. The batch only ever passes [`ShipOverrides::NONE`];
/// the type exists so tests can assert it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShipOverrides {
    pub stale_check: bool,
    pub stale_approval: bool,
}

impl ShipOverrides {
    pub(crate) const NONE: Self = ShipOverrides {
        stale_check: false,
        stale_approval: false,
    };
}

/// Everything the step machine reads or changes.
pub(crate) trait BatchPorts {
    /// The integrity floor: a human at an interactive terminal.
    fn human_at_terminal(&self) -> bool;
    fn view(&mut self, pr: u64) -> Result<PrView>;
    /// C4: the CI-definition files that make `aida pr ship` refuse this PR's
    /// green, from the stale-definition gate's own predicate.
    fn stale_definition_files(&mut self, pr: u64, view: &PrView) -> Result<Vec<String>>;
    fn update_branch(&mut self, pr: u64, expected_head: &str) -> Result<()>;
    fn ci_at_head(&mut self, pr: u64, head: &str) -> Result<CiAtHead>;
    /// Enqueue a review of the current head; never launches or records one.
    fn enqueue_review(&mut self, pr: u64, view: &PrView);
    fn review_at_head(&mut self, pr: u64, view: &PrView) -> ReviewAtHead;
    /// The merge-hold on `pr`, described with its release condition.
    fn hold(&mut self, pr: u64) -> Option<String>;
    /// Which spec IDs the ship would complete.
    fn completion_preview(&mut self, pr: u64, view: &PrView) -> Option<String>;
    /// One line typed by the operator; `None` at end of input.
    fn ask(&mut self, prompt: &str) -> Option<String>;
    /// Clear `pr`'s merge-hold through `aida merge-hold clear`.
    fn clear_hold(&mut self, pr: u64) -> Result<()>;
    fn ship(&mut self, pr: u64, overrides: ShipOverrides) -> Result<()>;
    fn sleep(&mut self, secs: u64);
    fn elapsed_secs(&self) -> u64;
    fn say(&mut self, line: &str);
    fn record(&mut self, pr: Option<u64>, outcome: &str, detail: &str);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BatchOptions {
    pub review_timeout_secs: u64,
    pub ci_timeout_secs: u64,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrOutcome {
    Shipped,
    AlreadyMerged,
    Stopped(String),
    /// `--dry-run`: what the batch would do.
    Planned(String),
}

/// C2: the PR list is non-empty, at most [`MAX_BATCH`] long, without repeats.
pub(crate) fn validate_order(prs: &[u64]) -> std::result::Result<(), String> {
    if prs.is_empty() {
        return Err("give at least one PR number".to_string());
    }
    if prs.len() > MAX_BATCH {
        return Err(format!(
            "{} PRs given; one batch ships at most {MAX_BATCH}",
            prs.len()
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for pr in prs {
        if !seen.insert(*pr) {
            return Err(format!("PR #{pr} is listed twice; list each PR once"));
        }
    }
    Ok(())
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

/// Run the batch. `Err` means nothing was started (refused input, no human,
/// no `go`); otherwise one outcome per PR reached, ending at the first stop.
pub(crate) fn run_batch(
    ports: &mut dyn BatchPorts,
    prs: &[u64],
    opts: BatchOptions,
) -> std::result::Result<Vec<(u64, PrOutcome)>, String> {
    validate_order(prs)?;
    if !ports.human_at_terminal() {
        let refusal = "`aida pr ship-batch` ships PRs that need a human merge, so it runs only \
                       for a human at an interactive terminal";
        ports.record(None, "refused", refusal);
        return Err(refusal.to_string());
    }
    let order = prs
        .iter()
        .map(|pr| format!("#{pr}"))
        .collect::<Vec<_>>()
        .join(" → ");
    if opts.dry_run {
        ports.say(&format!("Dry run, in this order: {order}"));
        return Ok(prs.iter().map(|pr| (*pr, plan_pr(ports, *pr))).collect());
    }
    ports.say(&format!("Ship in this order: {order}"));
    ports.say("Each PR is refreshed from main if needed, then waits for CI and an independent approval at its new head, then asks you to confirm.");
    if ports.ask("Type 'go' to start: ").as_deref().map(str::trim) != Some("go") {
        return Err("not started: 'go' was not typed".to_string());
    }
    ports.record(None, "started", &order);
    let mut outcomes = Vec::new();
    for pr in prs {
        let outcome = ship_one(ports, *pr, opts);
        let (label, detail) = match &outcome {
            PrOutcome::Shipped => ("shipped", String::new()),
            PrOutcome::AlreadyMerged => ("already-merged", String::new()),
            PrOutcome::Stopped(why) => ("stopped", why.clone()),
            PrOutcome::Planned(plan) => ("planned", plan.clone()),
        };
        ports.record(Some(*pr), label, &detail);
        let stop = matches!(outcome, PrOutcome::Stopped(_));
        outcomes.push((*pr, outcome));
        if stop {
            break;
        }
    }
    Ok(outcomes)
}

/// `--dry-run` for one PR: reads only.
fn plan_pr(ports: &mut dyn BatchPorts, pr: u64) -> PrOutcome {
    let view = match ports.view(pr) {
        Ok(view) => view,
        Err(e) => return PrOutcome::Stopped(format!("could not read PR #{pr}: {e:#}")),
    };
    match view.state {
        PrState::Merged => return PrOutcome::AlreadyMerged,
        PrState::Closed => return PrOutcome::Planned("closed: the batch would stop here".into()),
        PrState::Open => {}
    }
    let mut steps = Vec::new();
    if view.draft {
        steps.push("draft: the batch would stop here".to_string());
    }
    if view.mergeable == Mergeable::Conflicting {
        steps.push("conflicts with main: the batch would stop here".to_string());
    }
    match ports.stale_definition_files(pr, &view) {
        Ok(files) if view.behind || !files.is_empty() => {
            steps.push("would refresh from main".to_string())
        }
        Ok(_) => {}
        Err(e) => steps.push(format!("could not check staleness: {e:#}")),
    }
    let ci = match ports.ci_at_head(pr, &view.head_sha) {
        Ok(CiAtHead::Passing) => "green".to_string(),
        Ok(CiAtHead::Pending) => "pending".to_string(),
        Ok(CiAtHead::Failing(why)) => format!("red ({why})"),
        Ok(CiAtHead::HeadMoved) => "head moved while reading".to_string(),
        Err(e) => format!("unreadable ({e:#})"),
    };
    steps.push(format!("CI at {}: {ci}", short(&view.head_sha)));
    steps.push(match ports.review_at_head(pr, &view) {
        ReviewAtHead::Approved(who) => format!("approved at this head ({who})"),
        ReviewAtHead::Negative(why) => format!("{why}: the batch would stop here"),
        ReviewAtHead::Awaiting(_) => "no approval at this head yet".to_string(),
    });
    if let Some(hold) = ports.hold(pr) {
        steps.push(format!("merge-hold: {hold}"));
    }
    PrOutcome::Planned(steps.join("; "))
}

enum Wait {
    Ready,
    Moved,
    Stop(String),
}

/// One PR, from read to ship.
fn ship_one(ports: &mut dyn BatchPorts, pr: u64, opts: BatchOptions) -> PrOutcome {
    let mut view = match ports.view(pr) {
        Ok(view) => view,
        Err(e) => return PrOutcome::Stopped(format!("could not read PR #{pr}: {e:#}")),
    };
    if let Some(done) = open_or_outcome(pr, &view) {
        return done;
    }
    ports.say(&format!("PR #{pr} {}", view.title));
    let stale = match ports.stale_definition_files(pr, &view) {
        Ok(files) => files,
        Err(e) => {
            return PrOutcome::Stopped(format!("could not check whether PR #{pr} is stale: {e:#}"))
        }
    };
    if view.behind || !stale.is_empty() {
        let why = if stale.is_empty() {
            "it is behind main".to_string()
        } else {
            format!("its green predates a change to {}", stale.join(", "))
        };
        ports.say(&format!("  refreshing from main: {why}"));
        let old_head = view.head_sha.clone();
        if let Err(e) = ports.update_branch(pr, &old_head) {
            return PrOutcome::Stopped(format!("could not refresh PR #{pr}: {e:#}"));
        }
        view = match wait_for_new_head(ports, pr, &old_head) {
            Ok(view) => view,
            Err(why) => return PrOutcome::Stopped(why),
        };
        if let Some(done) = open_or_outcome(pr, &view) {
            return done;
        }
    }
    let mut moves = 0;
    let approval = loop {
        let head = view.head_sha.clone();
        let waited = match wait_for_ci(ports, pr, &head, opts.ci_timeout_secs) {
            Wait::Ready => wait_for_review(ports, pr, &view, opts.review_timeout_secs),
            other => Err(other),
        };
        match waited {
            Ok(approval) => {
                // The head must still be the approved one.
                match ports.view(pr) {
                    Ok(now) if now.head_sha == head => break approval,
                    Ok(now) => view = now,
                    Err(e) => {
                        return PrOutcome::Stopped(format!("could not re-read PR #{pr}: {e:#}"))
                    }
                }
            }
            Err(Wait::Stop(why)) => return PrOutcome::Stopped(why),
            Err(Wait::Moved | Wait::Ready) => match ports.view(pr) {
                Ok(now) => view = now,
                Err(e) => return PrOutcome::Stopped(format!("could not re-read PR #{pr}: {e:#}")),
            },
        }
        moves += 1;
        if moves > MAX_HEAD_MOVES {
            return PrOutcome::Stopped(format!(
                "PR #{pr}'s head kept moving ({moves} times) while waiting; re-run when it is settled"
            ));
        }
        ports.say(&format!(
            "  head moved to {}; waiting again",
            short(&view.head_sha)
        ));
        if let Some(done) = open_or_outcome(pr, &view) {
            return done;
        }
    };
    confirm_and_ship(ports, pr, &view, &approval)
}

/// `Some` when `view` ends this PR's turn before any write.
fn open_or_outcome(pr: u64, view: &PrView) -> Option<PrOutcome> {
    match view.state {
        PrState::Merged => Some(PrOutcome::AlreadyMerged),
        PrState::Closed => Some(PrOutcome::Stopped(format!(
            "PR #{pr} is closed without merging"
        ))),
        PrState::Open if view.draft => Some(PrOutcome::Stopped(format!(
            "PR #{pr} is a draft; mark it ready (`gh pr ready {pr}`) and re-run"
        ))),
        PrState::Open if view.mergeable == Mergeable::Conflicting => Some(PrOutcome::Stopped(
            format!("PR #{pr} conflicts with main; resolve the conflict, then re-run the batch"),
        )),
        PrState::Open => None,
    }
}

fn wait_for_new_head(
    ports: &mut dyn BatchPorts,
    pr: u64,
    old_head: &str,
) -> std::result::Result<PrView, String> {
    let started = ports.elapsed_secs();
    loop {
        match ports.view(pr) {
            Ok(view) if view.head_sha != old_head && !view.head_sha.is_empty() => return Ok(view),
            Ok(_) => {}
            Err(e) => return Err(format!("could not re-read PR #{pr}: {e:#}")),
        }
        if ports.elapsed_secs().saturating_sub(started) >= REFRESH_SETTLE_SECS {
            return Err(format!(
                "PR #{pr}'s head did not change within {REFRESH_SETTLE_SECS}s of the refresh"
            ));
        }
        ports.sleep(10);
    }
}

/// C3: CI for exactly `head`; a green from any other head never counts.
fn wait_for_ci(ports: &mut dyn BatchPorts, pr: u64, head: &str, timeout_secs: u64) -> Wait {
    let started = ports.elapsed_secs();
    let mut said = false;
    loop {
        match ports.ci_at_head(pr, head) {
            Ok(CiAtHead::Passing) => return Wait::Ready,
            Ok(CiAtHead::Failing(why)) => {
                return Wait::Stop(format!("PR #{pr}'s CI is red at {}: {why}", short(head)))
            }
            Ok(CiAtHead::HeadMoved) => return Wait::Moved,
            Ok(CiAtHead::Pending) => {}
            Err(e) => return Wait::Stop(format!("could not read PR #{pr}'s CI: {e:#}")),
        }
        if !said {
            ports.say(&format!("  waiting for CI at {}", short(head)));
            said = true;
        }
        if ports.elapsed_secs().saturating_sub(started) >= timeout_secs {
            return Wait::Stop(format!(
                "PR #{pr}'s CI at {} did not finish within {}m",
                short(head),
                timeout_secs / 60
            ));
        }
        ports.sleep(POLL_SECS);
    }
}

/// C1: enqueue, then wait for an approval at exactly this head, saying once
/// a minute that an independent reviewer has to act.
fn wait_for_review(
    ports: &mut dyn BatchPorts,
    pr: u64,
    view: &PrView,
    timeout_secs: u64,
) -> std::result::Result<String, Wait> {
    let head = short(&view.head_sha).to_string();
    let started = ports.elapsed_secs();
    let mut last_line: Option<u64> = None;
    let mut enqueued = false;
    loop {
        let note = match ports.review_at_head(pr, view) {
            ReviewAtHead::Approved(who) => return Ok(who),
            ReviewAtHead::Negative(why) => {
                return Err(Wait::Stop(format!("PR #{pr} at {head}: {why}")))
            }
            ReviewAtHead::Awaiting(note) => note,
        };
        if !enqueued {
            ports.enqueue_review(pr, view);
            enqueued = true;
        }
        let now = ports.elapsed_secs();
        let waited = now.saturating_sub(started);
        if waited >= timeout_secs {
            return Err(Wait::Stop(format!(
                "no independent approval of PR #{pr} at {head} within {}m; the batch stops here (re-run once it is reviewed)",
                timeout_secs / 60
            )));
        }
        if last_line.is_none_or(|t| now.saturating_sub(t) >= AWAITING_LINE_SECS) {
            let mut line = format!(
                "  PR #{pr} at {head}: awaiting an independent review at this head ({}m)",
                waited / 60
            );
            if let Some(note) = &note {
                line.push_str(&format!(" — {note}"));
            }
            ports.say(&line);
            last_line = Some(now);
        }
        match ports.ci_at_head(pr, &view.head_sha) {
            Ok(CiAtHead::HeadMoved) => return Err(Wait::Moved),
            Ok(CiAtHead::Failing(why)) => {
                return Err(Wait::Stop(format!(
                    "PR #{pr}'s CI went red at {head}: {why}"
                )))
            }
            _ => {}
        }
        ports.sleep(POLL_SECS);
    }
}

fn confirm_and_ship(
    ports: &mut dyn BatchPorts,
    pr: u64,
    view: &PrView,
    approval: &str,
) -> PrOutcome {
    if !ports.human_at_terminal() {
        return PrOutcome::Stopped(
            "no human at an interactive terminal any more; nothing shipped".to_string(),
        );
    }
    let hold = ports.hold(pr);
    ports.say(&format!("  ready: PR #{pr} {}", view.title));
    ports.say(&format!(
        "    head {}, CI green, approved: {approval}",
        short(&view.head_sha)
    ));
    if let Some(preview) = ports.completion_preview(pr, view) {
        ports.say(&format!("    {preview}"));
    }
    if let Some(hold) = &hold {
        ports.say(&format!("    merge-hold: {hold}"));
    }
    let typed = ports.ask(&format!(
        "Type {pr} to ship PR #{pr} (anything else stops the batch): "
    ));
    if typed.as_deref().map(str::trim) != Some(pr.to_string().as_str()) {
        return PrOutcome::Stopped(format!("PR #{pr} was not confirmed; nothing shipped"));
    }
    if hold.is_some() {
        let typed = ports.ask(&format!(
            "PR #{pr} has a merge-hold. Type 'clear' to clear it and ship (anything else stops): "
        ));
        if typed.as_deref().map(str::trim) != Some("clear") {
            return PrOutcome::Stopped(format!(
                "PR #{pr}'s merge-hold was not cleared; nothing shipped"
            ));
        }
        if let Err(e) = ports.clear_hold(pr) {
            return PrOutcome::Stopped(format!("could not clear PR #{pr}'s merge-hold: {e:#}"));
        }
    }
    match ports.ship(pr, ShipOverrides::NONE) {
        Ok(()) => PrOutcome::Shipped,
        Err(e) => PrOutcome::Stopped(format!("`aida pr ship {pr}` refused: {e:#}")),
    }
}

/// The review state at `head`, judged by the readers `aida pr ship` uses.
/// The newest verdict recorded at this head wins; only an approval the ship
/// gate would pin to (no carry-over) and an authorized recorder count.
pub(crate) fn review_state(
    pr: u64,
    candidates: &[crate::review_verdict::RecordedVerdict],
    policy: &crate::pr_ship::ShipApprovalPolicy,
    head: &str,
) -> ReviewAtHead {
    let newest_at_head = candidates
        .iter()
        .filter(|v| !v.is_closed())
        .filter(|v| {
            v.reviewed_sha
                .as_deref()
                .is_some_and(|sha| crate::forge::head_matches_pin(sha.trim(), head))
        })
        .max_by_key(|v| {
            v.recorded_at
                .as_deref()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t.trim()).ok())
                .map(|t| t.with_timezone(&chrono::Utc))
        });
    let Some(newest) = newest_at_head else {
        return ReviewAtHead::Awaiting(None);
    };
    if !newest.kind.approves() {
        return ReviewAtHead::Negative(format!("the reviewer recorded {}", newest.kind.label()));
    }
    if crate::pr_ship::approved_match_head(candidates, Some(head)).is_none() {
        return ReviewAtHead::Awaiting(None);
    }
    if let Some(refusal) = crate::pr_ship::approval_authority_refusal(candidates, policy) {
        return ReviewAtHead::Awaiting(Some(crate::pr_ship::approval_authority_refusal_message(
            pr, &refusal,
        )));
    }
    let who = newest
        .recorded_by
        .clone()
        .unwrap_or_else(|| "recorded".to_string());
    ReviewAtHead::Approved(match newest.summary.as_deref() {
        Some(summary) if !summary.trim().is_empty() => format!("{who}: {}", summary.trim()),
        _ => who,
    })
}

/// `aida pr ship-batch` handler.
pub(crate) fn run(prs: &[u64], review_timeout: Option<u64>, dry_run: bool) -> Result<()> {
    let project_root = crate::find_project_root()?;
    let main_worktree = crate::find_main_worktree_root()?;
    if crate::forge::resolve_forge_kind(&project_root) != crate::forge::ForgeKind::GitHub {
        anyhow::bail!("`aida pr ship-batch` needs a GitHub project: it refreshes branches with GitHub's update-branch");
    }
    let opts = BatchOptions {
        review_timeout_secs: review_timeout.unwrap_or(DEFAULT_REVIEW_TIMEOUT_SECS),
        ci_timeout_secs: crate::ci_idle_timeout::ci_absolute_ceiling_secs(&project_root),
        dry_run,
    };
    let mut ports = GitHubPorts {
        project_root,
        main_worktree,
        started: std::time::Instant::now(),
    };
    let outcomes = run_batch(&mut ports, prs, opts).map_err(anyhow::Error::msg)?;
    println!();
    println!("Summary:");
    let mut stopped = None;
    for (pr, outcome) in &outcomes {
        let line = match outcome {
            PrOutcome::Shipped => "shipped".to_string(),
            PrOutcome::AlreadyMerged => "already merged (skipped)".to_string(),
            PrOutcome::Stopped(why) => {
                stopped = Some(*pr);
                format!("STOPPED: {why}")
            }
            PrOutcome::Planned(plan) => plan.clone(),
        };
        println!("  #{pr}: {line}");
    }
    let untouched: Vec<String> = prs
        .iter()
        .skip(outcomes.len())
        .map(|pr| format!("#{pr}"))
        .collect();
    if !untouched.is_empty() {
        println!("  not reached: {}", untouched.join(" "));
    }
    match stopped {
        Some(pr) if !dry_run => {
            anyhow::bail!("the batch stopped at PR #{pr}; nothing after it was touched")
        }
        _ => Ok(()),
    }
}

/// The real ports: `gh` for reads, the forge layer for update-branch, the
/// ship gate's own readers for staleness and review, and `aida pr ship` /
/// `aida merge-hold clear` for the writes.
struct GitHubPorts {
    project_root: std::path::PathBuf,
    main_worktree: std::path::PathBuf,
    started: std::time::Instant,
}

impl GitHubPorts {
    fn gh_pr_json(&self, pr: u64, fields: &str) -> Result<serde_json::Value> {
        let gh = crate::resolve_forge_cli(crate::forge::ForgeKind::GitHub)
            .ok_or_else(|| anyhow::anyhow!("could not find the GitHub CLI (`gh`)"))?;
        let out = std::process::Command::new(gh)
            .current_dir(&self.project_root)
            .args(["pr", "view", &pr.to_string(), "--json", fields])
            .output()?;
        anyhow::ensure!(
            out.status.success(),
            "gh pr view {pr} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(serde_json::from_slice(&out.stdout)?)
    }
}

fn json_str(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Map `gh pr view --json state,mergeable,mergeStateStatus,isDraft,...`.
pub(crate) fn parse_pr_view(v: &serde_json::Value) -> PrView {
    PrView {
        state: match json_str(v, "state").as_str() {
            "MERGED" => PrState::Merged,
            "CLOSED" => PrState::Closed,
            _ => PrState::Open,
        },
        mergeable: match json_str(v, "mergeable").as_str() {
            "MERGEABLE" => Mergeable::Clean,
            "CONFLICTING" => Mergeable::Conflicting,
            _ => Mergeable::Unknown,
        },
        behind: json_str(v, "mergeStateStatus") == "BEHIND",
        draft: v.get("isDraft").and_then(|d| d.as_bool()).unwrap_or(false),
        head_sha: json_str(v, "headRefOid"),
        head_ref: json_str(v, "headRefName"),
        title: json_str(v, "title"),
        body: json_str(v, "body"),
    }
}

/// C3: classify a rollup only when it belongs to `head`.
pub(crate) fn ci_from_rollup(
    v: &serde_json::Value,
    head: &str,
    cfg: &crate::ci_gate::CiGateConfig,
) -> CiAtHead {
    if !crate::forge::head_matches_pin(head, &json_str(v, "headRefOid")) {
        return CiAtHead::HeadMoved;
    }
    let rollup = v
        .get("statusCheckRollup")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    let status = crate::pr_list::github_rollup_ci_status(&rollup, cfg);
    if status == "Failing" {
        CiAtHead::Failing("a required check failed".to_string())
    } else if status.starts_with("Passing") {
        CiAtHead::Passing
    } else {
        CiAtHead::Pending
    }
}

impl BatchPorts for GitHubPorts {
    fn human_at_terminal(&self) -> bool {
        crate::role::has_integrity_floor_authority()
    }

    fn view(&mut self, pr: u64) -> Result<PrView> {
        let v = self.gh_pr_json(
            pr,
            "state,mergeable,mergeStateStatus,isDraft,headRefOid,headRefName,title,body",
        )?;
        Ok(parse_pr_view(&v))
    }

    fn stale_definition_files(&mut self, pr: u64, view: &PrView) -> Result<Vec<String>> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.project_root)
            .args(["fetch", "--quiet", "origin"])
            .output()?;
        anyhow::ensure!(
            out.status.success(),
            "git fetch origin failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        let (_, warning) =
            crate::pr_cmd::ship_stale_definition_warning(&self.project_root, &view.head_ref, pr);
        Ok(warning.map(|w| w.definition_files).unwrap_or_default())
    }

    fn update_branch(&mut self, pr: u64, expected_head: &str) -> Result<()> {
        crate::forge::forge_for(&self.project_root).update_change_branch(pr, expected_head)
    }

    fn ci_at_head(&mut self, pr: u64, head: &str) -> Result<CiAtHead> {
        let v = self.gh_pr_json(pr, "headRefOid,statusCheckRollup")?;
        let cfg = crate::ci_gate::read_ci_gate_config(&self.project_root);
        Ok(ci_from_rollup(&v, head, &cfg))
    }

    fn enqueue_review(&mut self, _pr: u64, view: &PrView) {
        let outcome = crate::try_auto_queue_pr_review(
            &self.project_root,
            &view.head_ref,
            "(ship-batch)",
            crate::AutoQueueOrigin::PrSkill,
        );
        crate::render_auto_queue_outcome(&outcome);
    }

    fn review_at_head(&mut self, pr: u64, view: &PrView) -> ReviewAtHead {
        let roots = [self.project_root.as_path(), self.main_worktree.as_path()];
        let (candidates, policy) = crate::pr_cmd::merge_approval_inputs(
            &roots,
            &self.main_worktree,
            pr,
            &[],
            Some(&view.head_ref),
            Some(&view.head_sha),
        );
        review_state(pr, &candidates, &policy, &view.head_sha)
    }

    fn hold(&mut self, pr: u64) -> Option<String> {
        if let Some(record) = crate::merge_hold::read_hold_record(&self.main_worktree, pr) {
            return Some(match record.release_condition {
                Some(cond) => format!("{} (release when: {cond})", record.detail),
                None => record.detail,
            });
        }
        if let Some(detail) = crate::merge_hold::read_hold(&self.main_worktree, pr) {
            return Some(detail);
        }
        match crate::merge_hold::label_present(&self.main_worktree, pr) {
            Ok(true) => Some("the `aida:merge-hold` label is set".to_string()),
            Ok(false) => None,
            Err(e) => Some(format!("could not read the hold label: {e}")),
        }
    }

    fn completion_preview(&mut self, _pr: u64, view: &PrView) -> Option<String> {
        let store = crate::load_store_for_lookup(&self.project_root)?;
        let trailer = crate::pr_cmd::ship_completion_trailer(
            &store,
            &view.title,
            &view.head_ref,
            &view.body,
            false,
        );
        Some(crate::pr_ship::format_trailer_provenance(&trailer))
    }

    fn ask(&mut self, prompt: &str) -> Option<String> {
        use std::io::Write;
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line),
        }
    }

    fn clear_hold(&mut self, pr: u64) -> Result<()> {
        let exe = std::env::current_exe()?;
        let status = std::process::Command::new(exe)
            .current_dir(&self.main_worktree)
            .args(["merge-hold", "clear", &pr.to_string()])
            .status()?;
        anyhow::ensure!(status.success(), "`aida merge-hold clear {pr}` failed");
        Ok(())
    }

    fn ship(&mut self, pr: u64, overrides: ShipOverrides) -> Result<()> {
        let method = crate::pr_ship::validate_ship_merge_method(false, None, false)
            .map_err(anyhow::Error::msg)?;
        crate::pr_cmd::pr_ship_with_method(
            Some(pr),
            false,
            false,
            false,
            false,
            None,
            None,
            false,
            overrides.stale_check,
            overrides.stale_approval,
            None,
            false,
            method,
            false,
        )
    }

    fn sleep(&mut self, secs: u64) {
        std::thread::sleep(std::time::Duration::from_secs(secs));
    }

    fn elapsed_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    fn say(&mut self, line: &str) {
        println!("{line}");
    }

    fn record(&mut self, pr: Option<u64>, outcome: &str, detail: &str) {
        let line = serde_json::json!({
            "ts": chrono::Utc::now().to_rfc3339(),
            "event": "ship_batch",
            "pr": pr,
            "outcome": outcome,
            "detail": detail,
        });
        crate::pr_cmd::append_ship_activity_line(&self.main_worktree, &line.to_string());
        if outcome == "refused" {
            crate::events::record_gate_held(&self.main_worktree, "ship-batch", None, pr, detail);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, VecDeque};

    fn open(head: &str) -> PrView {
        PrView {
            state: PrState::Open,
            mergeable: Mergeable::Clean,
            behind: false,
            draft: false,
            head_sha: head.to_string(),
            head_ref: "branch".to_string(),
            title: "title".to_string(),
            body: String::new(),
        }
    }

    /// A scripted world. Views are consumed per PR (the last repeats); CI and
    /// reviews are keyed by head and consumed per poll (the last repeats).
    #[derive(Default)]
    struct Fake {
        human: bool,
        views: BTreeMap<u64, VecDeque<PrView>>,
        stale: BTreeMap<u64, Vec<String>>,
        ci: BTreeMap<String, VecDeque<CiAtHead>>,
        reviews: BTreeMap<String, VecDeque<ReviewAtHead>>,
        holds: BTreeMap<u64, String>,
        answers: VecDeque<String>,
        clock: u64,
        forge_calls: u32,
        updates: Vec<(u64, String)>,
        ci_heads: Vec<(u64, String)>,
        enqueued: Vec<(u64, String)>,
        cleared: Vec<u64>,
        shipped: Vec<(u64, ShipOverrides)>,
        said: Vec<String>,
    }

    impl Fake {
        fn human() -> Self {
            Fake {
                human: true,
                ..Fake::default()
            }
        }
        fn views(mut self, pr: u64, views: Vec<PrView>) -> Self {
            self.views.insert(pr, views.into());
            self
        }
        fn ci(mut self, head: &str, states: Vec<CiAtHead>) -> Self {
            self.ci.insert(head.to_string(), states.into());
            self
        }
        fn review(mut self, head: &str, states: Vec<ReviewAtHead>) -> Self {
            self.reviews.insert(head.to_string(), states.into());
            self
        }
        fn answers(mut self, answers: &[&str]) -> Self {
            self.answers = answers.iter().map(|a| a.to_string()).collect();
            self
        }
    }

    fn next<T: Clone>(q: &mut VecDeque<T>) -> Option<T> {
        if q.len() > 1 {
            q.pop_front()
        } else {
            q.front().cloned()
        }
    }

    fn approved() -> ReviewAtHead {
        ReviewAtHead::Approved("reviewer".to_string())
    }

    impl BatchPorts for Fake {
        fn human_at_terminal(&self) -> bool {
            self.human
        }
        fn view(&mut self, pr: u64) -> Result<PrView> {
            self.forge_calls += 1;
            self.views
                .get_mut(&pr)
                .and_then(next)
                .ok_or_else(|| anyhow::anyhow!("no such PR"))
        }
        fn stale_definition_files(&mut self, pr: u64, _view: &PrView) -> Result<Vec<String>> {
            self.forge_calls += 1;
            Ok(self.stale.get(&pr).cloned().unwrap_or_default())
        }
        fn update_branch(&mut self, pr: u64, expected_head: &str) -> Result<()> {
            self.forge_calls += 1;
            self.updates.push((pr, expected_head.to_string()));
            Ok(())
        }
        fn ci_at_head(&mut self, pr: u64, head: &str) -> Result<CiAtHead> {
            self.forge_calls += 1;
            self.ci_heads.push((pr, head.to_string()));
            Ok(self
                .ci
                .get_mut(head)
                .and_then(next)
                .unwrap_or(CiAtHead::HeadMoved))
        }
        fn enqueue_review(&mut self, pr: u64, view: &PrView) {
            self.enqueued.push((pr, view.head_sha.clone()));
        }
        fn review_at_head(&mut self, _pr: u64, view: &PrView) -> ReviewAtHead {
            self.reviews
                .get_mut(&view.head_sha)
                .and_then(next)
                .unwrap_or(ReviewAtHead::Awaiting(None))
        }
        fn hold(&mut self, pr: u64) -> Option<String> {
            self.holds.get(&pr).cloned()
        }
        fn completion_preview(&mut self, _pr: u64, _view: &PrView) -> Option<String> {
            None
        }
        fn ask(&mut self, _prompt: &str) -> Option<String> {
            self.answers.pop_front()
        }
        fn clear_hold(&mut self, pr: u64) -> Result<()> {
            self.cleared.push(pr);
            Ok(())
        }
        fn ship(&mut self, pr: u64, overrides: ShipOverrides) -> Result<()> {
            self.shipped.push((pr, overrides));
            Ok(())
        }
        fn sleep(&mut self, secs: u64) {
            self.clock += secs;
        }
        fn elapsed_secs(&self) -> u64 {
            self.clock
        }
        fn say(&mut self, line: &str) {
            self.said.push(line.to_string());
        }
        fn record(&mut self, _pr: Option<u64>, _outcome: &str, _detail: &str) {}
    }

    const OPTS: BatchOptions = BatchOptions {
        review_timeout_secs: 600,
        ci_timeout_secs: 600,
        dry_run: false,
    };

    fn ready(f: Fake, pr: u64, head: &str) -> Fake {
        f.views(pr, vec![open(head)])
            .ci(head, vec![CiAtHead::Passing])
            .review(head, vec![approved()])
    }

    // T1: `aida pr ship-batch 7 3 9` parses to the new subcommand, in order.
    // trace:STORY-1887 | ai:claude
    #[test]
    fn t1_ship_batch_parses_in_order() {
        let cli =
            crate::command_registry::try_parse_from(["aida", "pr", "ship-batch", "7", "3", "9"])
                .unwrap();
        match cli.command.registered::<crate::pr_cmd::PrArgs>() {
            crate::pr_cmd::PrArgs {
                inner:
                    crate::cli::PrCommand::ShipBatch {
                        prs,
                        review_timeout,
                        dry_run,
                    },
            } => {
                assert_eq!(prs, vec![7, 3, 9]);
                assert_eq!(review_timeout, None);
                assert!(!dry_run);
            }
            _ => panic!("expected pr ship-batch"),
        }
        let cli = crate::command_registry::try_parse_from([
            "aida",
            "pr",
            "ship-batch",
            "1",
            "--review-timeout",
            "2h",
        ])
        .unwrap();
        match cli.command.registered::<crate::pr_cmd::PrArgs>() {
            crate::pr_cmd::PrArgs {
                inner: crate::cli::PrCommand::ShipBatch { review_timeout, .. },
            } => assert_eq!(review_timeout, Some(7200)),
            _ => panic!("expected pr ship-batch"),
        }
        assert!(crate::command_registry::try_parse_from(["aida", "pr", "ship-batch"]).is_err());
    }

    // C2: duplicates, more than 20, and an empty list are refused before
    // anything else, and nothing touches the forge without 'go'.
    // trace:STORY-1887 | ai:claude
    #[test]
    fn c2_input_hygiene_and_up_front_go() {
        assert!(validate_order(&[1, 2, 1]).unwrap_err().contains("twice"));
        let many: Vec<u64> = (1..=21).collect();
        assert!(validate_order(&many).unwrap_err().contains("at most 20"));
        assert!(validate_order(&[]).is_err());
        assert!(validate_order(&(1..=20).collect::<Vec<u64>>()).is_ok());

        let mut f = Fake::human();
        let err = run_batch(&mut f, &[1, 1], OPTS).unwrap_err();
        assert!(err.contains("twice"));
        assert_eq!(f.forge_calls, 0);

        let mut f = ready(Fake::human(), 1, "aaaaaaaaaaaa1").answers(&["yes"]);
        assert!(run_batch(&mut f, &[1], OPTS).unwrap_err().contains("'go'"));
        assert_eq!(f.forge_calls, 0, "no forge call before 'go'");
        assert!(f.shipped.is_empty());
    }

    // T2: a caller without the integrity floor is refused before any forge
    // call, dry run included.
    // trace:STORY-1887 | ai:claude
    #[test]
    fn t2_non_human_refused_before_any_forge_call() {
        let mut f = ready(Fake::default(), 1, "aaaaaaaaaaaa1").answers(&["go", "1"]);
        let err = run_batch(&mut f, &[1], OPTS).unwrap_err();
        assert!(err.contains("human at an interactive terminal"), "{err}");
        assert_eq!(f.forge_calls, 0);
        assert!(f.shipped.is_empty());
        let mut f = ready(Fake::default(), 1, "aaaaaaaaaaaa1");
        let dry = BatchOptions {
            dry_run: true,
            ..OPTS
        };
        assert!(run_batch(&mut f, &[1], dry).is_err());
        assert_eq!(f.forge_calls, 0);
    }

    // T3 + C3: order is kept; a BEHIND PR is refreshed, its old head's green
    // and old head's approval are ignored, CI and review are awaited at the
    // new head; a red CI stops the batch and later PRs are never touched.
    // trace:STORY-1887 | ai:claude
    #[test]
    fn t3_order_refresh_new_head_and_stop() {
        let mut behind = open("bbbbbbbbbbbb_old");
        behind.behind = true;
        let mut f = ready(Fake::human(), 1, "aaaaaaaaaaaa1")
            .views(2, vec![behind, open("bbbbbbbbbbbb_new")])
            .ci("bbbbbbbbbbbb_old", vec![CiAtHead::Passing])
            .review("bbbbbbbbbbbb_old", vec![approved()])
            .ci(
                "bbbbbbbbbbbb_new",
                vec![CiAtHead::Pending, CiAtHead::Pending, CiAtHead::Passing],
            )
            .review(
                "bbbbbbbbbbbb_new",
                vec![
                    ReviewAtHead::Awaiting(None),
                    ReviewAtHead::Awaiting(None),
                    approved(),
                ],
            )
            .views(3, vec![open("cccccccccccc3")])
            .ci(
                "cccccccccccc3",
                vec![CiAtHead::Failing("Build".to_string())],
            )
            .views(4, vec![open("dddddddddddd4")])
            .answers(&["go", "1", "2", "3", "4"]);
        let out = run_batch(&mut f, &[1, 2, 3, 4], OPTS).unwrap();
        assert_eq!(out[0], (1, PrOutcome::Shipped));
        assert_eq!(out[1], (2, PrOutcome::Shipped));
        assert!(matches!(&out[2], (3, PrOutcome::Stopped(why)) if why.contains("red")));
        assert_eq!(out.len(), 3, "PR 4 is never reached");
        assert_eq!(f.updates, vec![(2, "bbbbbbbbbbbb_old".to_string())]);
        assert!(
            f.ci_heads
                .iter()
                .filter(|(pr, _)| *pr == 2)
                .all(|(_, head)| head == "bbbbbbbbbbbb_new"),
            "C3: CI is only ever read at the refreshed head: {:?}",
            f.ci_heads
        );
        assert_eq!(f.enqueued, vec![(2, "bbbbbbbbbbbb_new".to_string())]);
        assert!(f.ci_heads.iter().all(|(pr, _)| *pr != 4));
        assert_eq!(
            f.shipped.iter().map(|(pr, _)| *pr).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    // C1 + Q3: the review wait names the PR, the head, and that an
    // independent review is awaited, about once a minute, and stops at the
    // timeout without touching later PRs.
    // trace:STORY-1887 | ai:claude
    #[test]
    fn c1_review_wait_is_honest_and_stops_on_timeout() {
        let mut f = Fake::human()
            .views(5, vec![open("eeeeeeeeeeee5")])
            .ci("eeeeeeeeeeee5", vec![CiAtHead::Passing])
            .answers(&["go", "5"]);
        let out = run_batch(&mut f, &[5, 6], OPTS).unwrap();
        assert!(
            matches!(&out[0], (5, PrOutcome::Stopped(why)) if why.contains("no independent approval"))
        );
        assert_eq!(out.len(), 1);
        let awaiting: Vec<_> = f
            .said
            .iter()
            .filter(|l| l.contains("awaiting an independent review at this head"))
            .collect();
        assert!(
            awaiting[0].contains("PR #5 at eeeeeeeeeeee"),
            "{awaiting:?}"
        );
        assert!(
            (9..=11).contains(&awaiting.len()),
            "about one line a minute over 10 minutes: {}",
            awaiting.len()
        );
        assert!(f.shipped.is_empty());
    }

    // A negative verdict at the head stops the batch.
    // trace:STORY-1887 | ai:claude
    #[test]
    fn negative_verdict_at_head_stops() {
        let mut f = Fake::human()
            .views(5, vec![open("eeeeeeeeeeee5")])
            .ci("eeeeeeeeeeee5", vec![CiAtHead::Passing])
            .review(
                "eeeeeeeeeeee5",
                vec![ReviewAtHead::Negative(
                    "the reviewer recorded REJECTED".into(),
                )],
            )
            .answers(&["go", "5"]);
        let out = run_batch(&mut f, &[5], OPTS).unwrap();
        assert!(matches!(&out[0], (5, PrOutcome::Stopped(why)) if why.contains("REJECTED")));
        assert!(f.shipped.is_empty());
    }

    // T4: anything but the exact PR number ships nothing.
    // trace:STORY-1887 | ai:claude
    #[test]
    fn t4_confirmation_must_be_the_exact_pr_number() {
        for answers in [
            vec!["go", "8"],
            vec!["go", "y"],
            vec!["go", ""],
            vec!["go", "78"],
            vec!["go"],
        ] {
            let mut f = ready(Fake::human(), 7, "777777777777a").answers(&answers);
            let out = run_batch(&mut f, &[7], OPTS).unwrap();
            assert!(matches!(&out[0], (7, PrOutcome::Stopped(_))), "{answers:?}");
            assert!(f.shipped.is_empty(), "{answers:?}");
        }
        let mut f = ready(Fake::human(), 7, "777777777777a").answers(&["go", " 7\n"]);
        assert_eq!(
            run_batch(&mut f, &[7], OPTS).unwrap()[0].1,
            PrOutcome::Shipped
        );
    }

    // Q2: a hold is cleared only after this PR was confirmed and 'clear' typed.
    // trace:STORY-1887 | ai:claude
    #[test]
    fn q2_hold_cleared_only_with_confirmation_and_clear() {
        let held = |answers: &[&str]| {
            let mut f = ready(Fake::human(), 9, "999999999999a").answers(answers);
            f.holds.insert(9, "supervised".to_string());
            let out = run_batch(&mut f, &[9], OPTS).unwrap();
            (out, f)
        };
        let (out, f) = held(&["go", "9", "clear"]);
        assert_eq!(out[0].1, PrOutcome::Shipped);
        assert_eq!(f.cleared, vec![9]);
        let (out, f) = held(&["go", "9", "y"]);
        assert!(matches!(out[0].1, PrOutcome::Stopped(_)));
        assert!(f.cleared.is_empty() && f.shipped.is_empty());
        let (_, f) = held(&["go", "no", "clear"]);
        assert!(f.cleared.is_empty() && f.shipped.is_empty());
    }

    // T5: the ship is always made with both overrides false.
    // trace:STORY-1887 | ai:claude
    #[test]
    fn t5_ship_never_overrides() {
        let mut f = ready(ready(Fake::human(), 1, "aaaaaaaaaaaa1"), 2, "bbbbbbbbbbbb2")
            .answers(&["go", "1", "2"]);
        run_batch(&mut f, &[1, 2], OPTS).unwrap();
        assert_eq!(f.shipped.len(), 2);
        assert!(f.shipped.iter().all(|(_, o)| *o == ShipOverrides::NONE));
        assert!(!ShipOverrides::NONE.stale_check && !ShipOverrides::NONE.stale_approval);
    }

    // T6: a merged PR is skipped; closed and conflicting PRs stop the batch.
    // trace:STORY-1887 | ai:claude
    #[test]
    fn t6_merged_skips_closed_and_conflicting_stop() {
        let mut merged = open("111111111111a");
        merged.state = PrState::Merged;
        let mut f =
            ready(Fake::human().views(1, vec![merged]), 2, "222222222222a").answers(&["go", "2"]);
        let out = run_batch(&mut f, &[1, 2], OPTS).unwrap();
        assert_eq!(out[0], (1, PrOutcome::AlreadyMerged));
        assert_eq!(out[1], (2, PrOutcome::Shipped));

        let mut closed = open("111111111111a");
        closed.state = PrState::Closed;
        let mut conflicting = open("111111111111a");
        conflicting.mergeable = Mergeable::Conflicting;
        for (view, word) in [(closed, "closed"), (conflicting, "conflicts")] {
            let mut f = ready(Fake::human().views(1, vec![view]), 2, "222222222222a")
                .answers(&["go", "1", "2"]);
            let out = run_batch(&mut f, &[1, 2], OPTS).unwrap();
            assert!(matches!(&out[0], (1, PrOutcome::Stopped(why)) if why.contains(word)));
            assert_eq!(out.len(), 1);
            assert!(f.shipped.is_empty() && f.updates.is_empty());
        }
    }

    // T7: a head that keeps moving after approval is re-awaited, then stops;
    // a single move is re-awaited and ships at the new head.
    // trace:STORY-1887 | ai:claude
    #[test]
    fn t7_moving_head_rewaits_then_stops() {
        let heads = [
            "h1h1h1h1h1h1",
            "h2h2h2h2h2h2",
            "h3h3h3h3h3h3",
            "h4h4h4h4h4h4",
        ];
        let views = heads.iter().map(|h| open(h)).collect();
        let mut f = Fake::human().views(3, views).answers(&["go", "3"]);
        for h in heads {
            f = f.ci(h, vec![CiAtHead::Passing]).review(h, vec![approved()]);
        }
        let out = run_batch(&mut f, &[3], OPTS).unwrap();
        assert!(
            matches!(&out[0], (3, PrOutcome::Stopped(why)) if why.contains("kept moving")),
            "{out:?}"
        );
        assert!(f.shipped.is_empty());

        let mut f = Fake::human()
            .views(3, vec![open(heads[0]), open(heads[1])])
            .answers(&["go", "3"]);
        for h in &heads[..2] {
            f = f.ci(h, vec![CiAtHead::Passing]).review(h, vec![approved()]);
        }
        assert_eq!(
            run_batch(&mut f, &[3], OPTS).unwrap()[0].1,
            PrOutcome::Shipped
        );
    }

    // C4: staleness from the gate's predicate alone triggers a refresh.
    // trace:STORY-1887 | ai:claude
    #[test]
    fn c4_stale_definition_triggers_refresh() {
        let mut f = ready(Fake::human(), 4, "new_head_0000")
            .views(4, vec![open("old_head_0000"), open("new_head_0000")])
            .answers(&["go", "4"]);
        f.stale
            .insert(4, vec![".github/workflows/ci.yml".to_string()]);
        let out = run_batch(&mut f, &[4], OPTS).unwrap();
        assert_eq!(out[0].1, PrOutcome::Shipped);
        assert_eq!(f.updates, vec![(4, "old_head_0000".to_string())]);
        assert!(f
            .said
            .iter()
            .any(|l| l.contains(".github/workflows/ci.yml")));
    }

    // C3 for the real port: a rollup from another head never counts, and no
    // checks yet is not green.
    // trace:STORY-1887 | ai:claude
    #[test]
    fn c3_rollup_from_another_head_is_head_moved() {
        let cfg = crate::ci_gate::CiGateConfig::default();
        let a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let green = serde_json::json!({
            "headRefOid": a,
            "statusCheckRollup": [{"name": "Build", "status": "COMPLETED", "conclusion": "SUCCESS"}],
        });
        assert_eq!(
            ci_from_rollup(&green, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", &cfg),
            CiAtHead::HeadMoved
        );
        assert_eq!(ci_from_rollup(&green, a, &cfg), CiAtHead::Passing);
        let empty = serde_json::json!({ "headRefOid": a, "statusCheckRollup": [] });
        assert_eq!(ci_from_rollup(&empty, a, &cfg), CiAtHead::Pending);
        let view = parse_pr_view(&serde_json::json!({
            "state": "OPEN", "mergeable": "CONFLICTING", "mergeStateStatus": "BEHIND",
            "isDraft": true, "headRefOid": "abc", "headRefName": "b", "title": "t", "body": "",
        }));
        assert_eq!(view.mergeable, Mergeable::Conflicting);
        assert!(view.behind && view.draft);
    }

    // The GitHub update-branch call is pinned to the head it was read at.
    // trace:STORY-1887 | ai:claude
    #[test]
    fn update_branch_is_pinned_to_the_read_head() {
        let argv = crate::forge::github_update_branch_argv(42, "abc123");
        assert!(argv.contains(&"repos/{owner}/{repo}/pulls/42/update-branch".to_string()));
        assert!(argv.contains(&"expected_head_sha=abc123".to_string()));
    }

    // T8: the batch module is a mandate-protected gate file.
    // trace:STORY-1887 | ai:claude
    #[test]
    fn t8_ship_batch_is_gate_protected() {
        assert_eq!(
            crate::mandate_gates::touches_gates(&["aida-cli-lib/src/pr_ship_batch.rs".to_string()]),
            Some("aida-cli-lib/src/pr_ship_batch.rs".to_string())
        );
    }
}
