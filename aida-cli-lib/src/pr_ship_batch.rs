//! `aida pr ship-batch` (STORY-1887): ship human-gated PRs one at a time, in
//! the operator's order. Each gated merge moves main and makes the next gated
//! PR stale, so every PR is refreshed only when needed, CI and an independent
//! approval are awaited at its exact new head, and the operator confirms each
//! ship at the terminal. The ship itself is the ordinary `aida pr ship` path
//! with both overrides hard-false: the batch adds waiting, never a bypass.
//!
//! Gate authority (human-only check, typed confirmation, guarded hold-clear +
//! ship handoff, same-head review selector, exact-head CI rollup classifier)
//! lives in [`crate::gate::pr_ship_batch`].
// trace:STORY-1887 | ai:claude
// trace:STORY-1889 | ai:agy

use crate::gate::pr_ship_batch::{
    ci_from_rollup, confirm_and_ship, confirm_batch_start, ensure_human_at_terminal,
    open_or_outcome, parse_pr_view, review_state, short, validate_order, BatchPorts, CiAtHead,
    Mergeable, PrOutcome, PrState, PrView, ReviewAtHead, ShipOverrides, Staleness,
};
use anyhow::Result;

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
pub(crate) struct BatchOptions {
    pub review_timeout_secs: u64,
    pub ci_timeout_secs: u64,
    pub dry_run: bool,
}

/// Run the batch. `Err` means nothing was started (refused input, no human,
/// no `go`); otherwise one outcome per PR reached, ending at the first stop.
pub(crate) fn run_batch(
    ports: &mut dyn BatchPorts,
    prs: &[u64],
    opts: BatchOptions,
) -> std::result::Result<Vec<(u64, PrOutcome)>, String> {
    validate_order(prs)?;
    ensure_human_at_terminal(ports)?;
    let order = prs
        .iter()
        .map(|pr| format!("#{pr}"))
        .collect::<Vec<_>>()
        .join(" → ");
    if opts.dry_run {
        ports.say(&format!("Dry run, in this order: {order}"));
        return Ok(prs.iter().map(|pr| (*pr, plan_pr(ports, *pr))).collect());
    }
    confirm_batch_start(ports, &order)?;
    ports.record(None, "started", &order);
    let mut outcomes = Vec::new();
    for pr in prs {
        let outcome = ship_one(ports, *pr, opts);
        let (label, detail) = match &outcome {
            PrOutcome::Shipped => ("shipped", String::new()),
            PrOutcome::AlreadyMerged => ("already-merged", String::new()),
            PrOutcome::Stopped(why) => ("stopped", why.clone()),
            PrOutcome::Planned(_) => {
                unreachable!("ship_one never plans; a dry run returns before this loop")
            }
        };
        ports.record(Some(*pr), label, &detail);
        let stop = matches!(outcome, PrOutcome::Stopped(_));
        outcomes.push((*pr, outcome));
        if stop {
            break;
        }
    }
    ports.record(None, "finished", &finished_summary(&outcomes));
    Ok(outcomes)
}

/// The `finished` record's detail: what shipped, what was skipped, where the
/// batch stopped.
// trace:STORY-1887 | ai:claude
pub(crate) fn finished_summary(outcomes: &[(u64, PrOutcome)]) -> String {
    let shipped = outcomes
        .iter()
        .filter(|(_, o)| *o == PrOutcome::Shipped)
        .count();
    let merged = outcomes
        .iter()
        .filter(|(_, o)| *o == PrOutcome::AlreadyMerged)
        .count();
    let mut out = format!("{shipped} shipped, {merged} already merged");
    if let Some((pr, _)) = outcomes
        .iter()
        .find(|(_, o)| matches!(o, PrOutcome::Stopped(_)))
    {
        out.push_str(&format!(", stopped at #{pr}"));
    }
    out
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
    // TASK-2157: a dry run changes nothing, so it does not fetch; the
    // stale-definition check reads the local `origin/*` refs as they are.
    // trace:STORY-1887 | ai:claude
    match ports.staleness(pr, &view, false) {
        Ok(stale) if view.behind || stale.behind || !stale.definition_files.is_empty() => {
            steps.push("would refresh from main".to_string())
        }
        Ok(_) => {}
        Err(e) => steps.push(format!("could not check staleness: {e:#}")),
    }
    // TASK-2180: the hold is read once; CI is judged with it, and an
    // unreadable hold is reported as the hold, not as CI.
    // trace:TASK-2180 | ai:claude
    let hold = ports.hold(pr);
    if let Ok(held) = &hold {
        let ci = match ci_with_hold(ports, pr, &view.head_sha, held.is_some()) {
            Ok(CiAtHead::Passing) => "green".to_string(),
            Ok(CiAtHead::Pending) => "pending".to_string(),
            Ok(CiAtHead::Failing(why)) => format!("red ({why})"),
            Ok(CiAtHead::HeadMoved) => "head moved while reading".to_string(),
            Err(why) => format!("unreadable ({why})"),
        };
        steps.push(format!("CI at {}: {ci}", short(&view.head_sha)));
    }
    steps.push(match ports.review_at_head(pr, &view) {
        ReviewAtHead::Approved(who) => format!("approved at this head ({who})"),
        ReviewAtHead::Negative(why) => format!("{why}: the batch would stop here"),
        ReviewAtHead::Awaiting(_) => "no approval at this head yet".to_string(),
    });
    match hold {
        Ok(Some(hold)) => steps.push(format!("merge-hold: {hold}")),
        Ok(None) => {}
        Err(e) => steps.push(format!(
            "could not read the hold label ({e:#}): the batch would stop here"
        )),
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
    let stale = match ports.staleness(pr, &view, true) {
        Ok(stale) => stale,
        Err(e) => {
            return PrOutcome::Stopped(format!("could not check whether PR #{pr} is stale: {e:#}"))
        }
    };
    // trace:STORY-1690 | ai:claude
    if view.behind || stale.behind || !stale.definition_files.is_empty() {
        let why = if stale.definition_files.is_empty() {
            "it is behind main".to_string()
        } else {
            format!(
                "its green predates a change to {}",
                stale.definition_files.join(", ")
            )
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

/// CI at `head`, judged with the PR's hold state. Each read error names
/// what could not be read: the hold label or the CI rollup.
// trace:TASK-2172 | ai:claude
fn read_ci(
    ports: &mut dyn BatchPorts,
    pr: u64,
    head: &str,
) -> std::result::Result<CiAtHead, String> {
    let held = ports
        .hold(pr)
        .map_err(|e| format!("could not read the hold label on PR #{pr} ({e:#})"))?
        .is_some();
    ci_with_hold(ports, pr, head, held)
}

/// CI at `head` for a hold state the caller already read.
// trace:TASK-2180 | ai:claude
fn ci_with_hold(
    ports: &mut dyn BatchPorts,
    pr: u64,
    head: &str,
    held: bool,
) -> std::result::Result<CiAtHead, String> {
    ports
        .ci_at_head(pr, head, held)
        .map_err(|e| format!("could not read PR #{pr}'s CI: {e:#}"))
}

/// C3: CI for exactly `head`; a green from any other head never counts.
fn wait_for_ci(ports: &mut dyn BatchPorts, pr: u64, head: &str, timeout_secs: u64) -> Wait {
    let started = ports.elapsed_secs();
    let mut said = false;
    loop {
        // trace:TASK-2172 | ai:claude
        match read_ci(ports, pr, head) {
            Ok(CiAtHead::Passing) => return Wait::Ready,
            Ok(CiAtHead::Failing(why)) => {
                return Wait::Stop(format!("PR #{pr}'s CI is red at {}: {why}", short(head)))
            }
            Ok(CiAtHead::HeadMoved) => return Wait::Moved,
            Ok(CiAtHead::Pending) => {}
            Err(why) => return Wait::Stop(why),
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
    // TASK-2194: a CI read error while waiting is said when it starts or
    // changes, and the timeout names it, so a stalled wait says why.
    // trace:TASK-1951 trace:TASK-2194 | ai:claude
    let mut ci_error: Option<String> = None;
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
            let mut why = format!(
                "no independent approval of PR #{pr} at {head} within {}m; the batch stops here (re-run once it is reviewed)",
                timeout_secs / 60
            );
            if let Some(err) = &ci_error {
                why.push_str(&format!("; the last CI read failed: {err}"));
            }
            return Err(Wait::Stop(why));
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
        match read_ci(ports, pr, &view.head_sha) {
            Ok(CiAtHead::HeadMoved) => return Err(Wait::Moved),
            Ok(CiAtHead::Failing(why)) => {
                return Err(Wait::Stop(format!(
                    "PR #{pr}'s CI went red at {head}: {why}"
                )))
            }
            Ok(_) => ci_error = None,
            Err(why) => {
                if ci_error.as_deref() != Some(why.as_str()) {
                    ports.say(&format!("  {why}; retrying while the review is awaited"));
                }
                ci_error = Some(why);
            }
        }
        ports.sleep(POLL_SECS);
    }
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

impl BatchPorts for GitHubPorts {
    fn human_at_terminal(&self) -> bool {
        crate::gate::role_authority::has_integrity_floor_authority()
    }

    fn view(&mut self, pr: u64) -> Result<PrView> {
        let v = self.gh_pr_json(
            pr,
            "state,mergeable,mergeStateStatus,isDraft,headRefOid,headRefName,title,body",
        )?;
        Ok(parse_pr_view(&v))
    }

    fn staleness(&mut self, pr: u64, view: &PrView, fetch: bool) -> Result<Staleness> {
        if fetch {
            // TASK-2001: a host fetch keeps origin's stored token out of argv.
            // trace:TASK-1952 | trace:TASK-2001 | ai:claude
            let out = aida_core::git_ops::host_remote_git_command(&self.project_root, "origin")
                .args(["fetch", "--quiet", "origin"])
                .output()?;
            anyhow::ensure!(
                out.status.success(),
                "git fetch origin failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let (_, warning) = crate::gate::ci_freshness::ship_stale_definition_warning(
            &self.project_root,
            &view.head_ref,
            pr,
        );
        Ok(warning
            .map(|w| Staleness {
                behind: true,
                definition_files: w.definition_files,
            })
            .unwrap_or_default())
    }

    fn update_branch(&mut self, pr: u64, expected_head: &str) -> Result<()> {
        crate::forge::forge_for(&self.project_root).update_change_branch(pr, expected_head)
    }

    fn ci_at_head(&mut self, pr: u64, head: &str, held: bool) -> Result<CiAtHead> {
        let v = self.gh_pr_json(pr, "headRefOid,statusCheckRollup")?;
        let cfg = crate::ci_gate::read_ci_gate_config(&self.project_root);
        Ok(ci_from_rollup(&v, head, &cfg, held))
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
        // TASK-2156: the same candidate IDs the ship gate builds.
        // trace:STORY-1887 | ai:claude
        let ids = crate::pr_cmd::ship_gate_known_ids(
            &self.project_root,
            &view.head_ref,
            Some(&view.title),
        );
        let (candidates, policy) = crate::gate::approval::merge_approval_inputs(
            &roots,
            &self.main_worktree,
            pr,
            &ids,
            Some(&view.head_ref),
            Some(&view.head_sha),
        );
        review_state(pr, &candidates, &policy, &view.head_sha)
    }

    fn hold(&mut self, pr: u64) -> Result<Option<String>> {
        if let Some(record) =
            crate::gate::merge_hold_core::read_hold_record(&self.main_worktree, pr)
        {
            return Ok(Some(match record.release_condition {
                Some(cond) => format!("{} (release when: {cond})", record.detail),
                None => record.detail,
            }));
        }
        if let Some(detail) = crate::gate::merge_hold_core::read_hold(&self.main_worktree, pr) {
            return Ok(Some(detail));
        }
        match crate::merge_hold::label_present(&self.main_worktree, pr) {
            Ok(true) => Ok(Some("the `aida:merge-hold` label is set".to_string())),
            Ok(false) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("{e}")),
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
        let status = std::process::Command::new(crate::resolve_aida_exe())
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
        // TASK-2155: every step also reaches the events feed, so
        // `aida history events --kind ship-batch-step` shows the run.
        // trace:STORY-1887 | ai:claude
        crate::events::emit(&self.main_worktree, &ship_batch_event(pr, outcome, detail));
        if outcome == "refused" {
            crate::events::record_gate_held(&self.main_worktree, "ship-batch", None, pr, detail);
        }
    }
}

/// The events-feed record of one batch step (pure; [`GitHubPorts`] emits it).
// trace:STORY-1887 | ai:claude
pub(crate) fn ship_batch_event(
    pr: Option<u64>,
    outcome: &str,
    detail: &str,
) -> crate::events::Event {
    let mut ev = crate::events::Event::new(
        None,
        "",
        crate::events::EventKind::ShipBatchStep {
            pr,
            outcome: outcome.to_string(),
            detail: detail.trim().to_string(),
        },
    );
    ev.seat = crate::events::active_seat();
    ev
}
