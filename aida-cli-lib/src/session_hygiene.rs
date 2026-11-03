//! `aida session hygiene` — the scheduled, model-free cleanup pass.
//!
//! On 2026-11-03 `aida ps` had piled up about 51 stale, 9 orphaned and 28
//! salvageable rows, and the disk reached 99% with 305.7 GiB reclaimable,
//! because session reap, agent gc and worktree reclaim only ran when an
//! advisor swept by hand. This command runs all three in one pass, so the
//! project registers it as an hourly substrate job (`session-hygiene` in
//! `.aida/config.toml`):
//!
//! 1. **Reap** finished sessions: spec Done or Completed, branch merged,
//!    process exited. This is the same scan and per-lease reap as
//!    `aida session reap --yes`, via [`session_reap::reap_quiet_outcomes`]. It
//!    never releases a protected-checkout lease, which is an operator-typed
//!    act (BUG-2197).
//! 2. **Agent gc**: dead-PID registry rows. A row in a pid namespace this
//!    process cannot observe reads unknown, never dead, so it is kept.
//! 3. **Reclaim** worktree `target/` caches idle for at least
//!    `--idle-hours` (default 24), with every `aida worktree reclaim` rail
//!    on: no live lease, no open PR, no main checkout, no store, and no agent
//!    scratch (job `tmp/` and session scratch are not regenerable).
//!
//! It never commits, stashes or deletes uncommitted work. A salvageable tree
//! (dead process, dirty worktree) is only *reported*.
//!
//! **Routing.** The run exits 0 and stays silent to the seats unless it has
//! to keep something a seat must act on. Today those are: salvageable work
//! idle for at least `--idle-hours`, a reap refused because mail retention
//! is not activated, and a reap, gc or delete that failed. Then it exits
//! non-zero and its last stderr line names each item and the act it needs.
//! The scheduler routes that line to the advisor through
//! `session-hygiene-route` (`on = ["CronJobFailed:session-hygiene"]`).
//!
//! **Budget.** The scheduler kills a child at its timeout, and a killed run
//! routes as "did not finish". The reclaim step is skipped (noted, exit 0)
//! once [`SOFT_BUDGET`] is spent, so a slow reap scan defers the reclaim to
//! the next hourly run instead of routing noise.
// trace:STORY-2351 | ai:claude

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::dispatch_health_ps::{self, DispatchState};
use crate::worktree_reclaim::{self, human_bytes, ReclaimOptions};

/// Default idle threshold for reclaiming a cache and for routing
/// salvageable work.
pub(crate) const DEFAULT_IDLE_HOURS: u64 = 24;

/// Once this much wall time is spent, the reclaim step defers to the next run.
/// The scheduler's kill budget for this job is
/// [`crate::maintenance_schedule`]'s hygiene timeout, well above this.
pub(crate) const SOFT_BUDGET: Duration = Duration::from_secs(180);

/// `aida session hygiene` options.
// trace:STORY-2351 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct HygieneOptions {
    pub dry_run: bool,
    pub idle_hours: u64,
}

/// The reclaim options the hygiene pass uses. Every rail stays on, and the
/// idle window is the hygiene threshold, so only caches idle at least that
/// long are candidates.
// trace:STORY-2351 | ai:claude
pub(crate) fn reclaim_options(opts: &HygieneOptions) -> ReclaimOptions {
    ReclaimOptions {
        apply: !opts.dry_run,
        min_age_mins: opts.idle_hours.saturating_mul(60),
        target_pct: None,
        include_live: false,
        include_open_prs: false,
        json: false,
        scratch_age_days: crate::scratch_reclaim::DEFAULT_MIN_AGE_DAYS,
        no_scratch: true,
    }
}

/// A salvageable row from `aida ps`: dead process, uncommitted work.
// trace:STORY-2351 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SalvageCandidate {
    pub session: String,
    pub worktree: PathBuf,
    /// Seconds since the newest uncommitted file was written, or the
    /// lease's age when no dirty path could be read.
    pub idle_secs: u64,
    /// The `aida ps` next-command hint for the row.
    pub hint: Option<String>,
}

/// Something the pass kept that a seat must act on.
// trace:STORY-2351 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeepItem {
    pub what: String,
    pub act: String,
}

/// What one hygiene pass did.
// trace:STORY-2351 | ai:claude
#[derive(Debug, Default)]
pub(crate) struct HygieneOutcome {
    /// `(session, outcome)` per lease the reap touched, as `reap_one` words it.
    pub reap: Vec<(String, String)>,
    /// Agent-registry rows removed, or why gc failed.
    pub agent_gc: Option<Result<usize, String>>,
    /// Caches deleted (or, on a dry run, that would be).
    pub reclaimed: Vec<(PathBuf, u64)>,
    pub reclaim_failed: Vec<(PathBuf, String)>,
    /// Why reclaim did not run, when it did not.
    pub reclaim_skipped: Option<String>,
    pub salvageable: Vec<SalvageCandidate>,
}

/// The pure routing decision: what the pass kept that a seat must act on.
/// Anything not listed here is either done or will resolve on its own (a
/// live session reaps once it exits; a fresh salvageable tree may still be
/// recovered by its owner).
// trace:STORY-2351 | ai:claude
pub(crate) fn keep_items(outcome: &HygieneOutcome, idle_hours: u64) -> Vec<KeepItem> {
    let idle_secs = idle_hours.saturating_mul(3600);
    let mut items = Vec::new();
    for (session, line) in &outcome.reap {
        if line.contains(aida_core::mailbox_lifecycle::ACTIVATION_COMMAND) {
            items.push(KeepItem {
                what: format!("session {session} kept: mail retention is not activated"),
                act: format!(
                    "run `{}`, then activate retention so the next hygiene run can reap it",
                    aida_core::mailbox_lifecycle::ACTIVATION_COMMAND
                ),
            });
        } else if line.starts_with("failed") {
            items.push(KeepItem {
                what: format!("session {session} reap {line}"),
                act: "inspect it and run `aida session reap` by hand".to_string(),
            });
        }
    }
    if let Some(Err(e)) = &outcome.agent_gc {
        items.push(KeepItem {
            what: format!("agent gc failed: {e}"),
            act: "run `aida agent gc` by hand to see the error".to_string(),
        });
    }
    for (path, err) in &outcome.reclaim_failed {
        items.push(KeepItem {
            what: format!("cache {} could not be deleted: {err}", path.display()),
            act: "check its permissions, then run `aida worktree reclaim --apply`".to_string(),
        });
    }
    for c in &outcome.salvageable {
        if c.idle_secs < idle_secs {
            continue;
        }
        items.push(KeepItem {
            what: format!(
                "salvageable work in {} (session {}) untouched for {}h",
                c.worktree.display(),
                c.session,
                c.idle_secs / 3600
            ),
            act: c.hint.clone().unwrap_or_else(|| {
                format!(
                    "inspect it with `git -C {} status`, then commit it on its branch or discard it",
                    c.worktree.display()
                )
            }),
        });
    }
    items
}

/// The one line the scheduler routes: the child's last stderr line.
// trace:STORY-2351 | ai:claude
pub(crate) fn route_line(items: &[KeepItem]) -> String {
    let body = items
        .iter()
        .map(|i| format!("{} — {}", i.what, i.act))
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "session hygiene kept {} item(s) that need the advisor: {body}",
        items.len()
    )
}

/// The human summary printed on stdout.
// trace:STORY-2351 | ai:claude
pub(crate) fn render_summary(outcome: &HygieneOutcome, dry_run: bool) -> String {
    let mut out = String::new();
    let verb = if dry_run { "would reap" } else { "reaped" };
    let reaped = outcome
        .reap
        .iter()
        .filter(|(_, l)| l.starts_with("reaped"))
        .count();
    out.push_str(&format!("reap      : {verb} {reaped} session(s)\n"));
    for (session, line) in &outcome.reap {
        if !line.starts_with("reaped") {
            out.push_str(&format!("  {session}  {line}\n"));
        }
    }
    match &outcome.agent_gc {
        Some(Ok(n)) => out.push_str(&format!(
            "agent gc  : {} {n} dead registry row(s)\n",
            if dry_run { "would remove" } else { "removed" }
        )),
        Some(Err(e)) => out.push_str(&format!("agent gc  : FAILED — {e}\n")),
        None => out.push_str("agent gc  : not run\n"),
    }
    if let Some(why) = &outcome.reclaim_skipped {
        out.push_str(&format!("reclaim   : skipped — {why}\n"));
    } else {
        let bytes: u64 = outcome.reclaimed.iter().map(|(_, b)| b).sum();
        out.push_str(&format!(
            "reclaim   : {} {} across {} idle cache(s)\n",
            if dry_run { "would free" } else { "freed" },
            human_bytes(bytes),
            outcome.reclaimed.len()
        ));
        for (path, err) in &outcome.reclaim_failed {
            out.push_str(&format!("  FAILED {}  — {err}\n", path.display()));
        }
    }
    out.push_str(&format!(
        "salvage   : {} salvageable tree(s) left untouched\n",
        outcome.salvageable.len()
    ));
    out
}

/// Salvageable rows from the same classifier `aida ps` uses.
// trace:STORY-2351 | ai:claude
fn salvage_candidates(project_root: &Path) -> Vec<SalvageCandidate> {
    let (rows, _orphans) = crate::gather_running_work(project_root);
    rows.into_iter()
        .filter_map(|row| {
            let dispatch = row.dispatch.as_ref()?;
            if dispatch.state != DispatchState::Salvageable {
                return None;
            }
            let probe = dispatch_health_ps::probe_worktree(&row.lease.worktree_path);
            let lease_age = chrono::Utc::now()
                .signed_duration_since(row.lease.started_at)
                .num_seconds()
                .max(0) as u64;
            Some(SalvageCandidate {
                session: row.lease.id.chars().take(8).collect(),
                worktree: row.lease.worktree_path.clone(),
                idle_secs: probe.dirty_newest_mtime_age_secs.unwrap_or(lease_age),
                hint: dispatch.hint.clone(),
            })
        })
        .collect()
}

/// The reclaim step, in-process, worktree caches only.
// trace:STORY-2351 | ai:claude
fn reclaim(project_root: &Path, opts: &HygieneOptions, outcome: &mut HygieneOutcome) {
    let ropts = reclaim_options(opts);
    let survey = match worktree_reclaim::survey_from_env(project_root, &ropts) {
        Ok(s) => s,
        Err(e) => {
            outcome.reclaim_skipped = Some(format!("could not survey caches: {e:#}"));
            return;
        }
    };
    let plan = worktree_reclaim::plan(&survey);
    if opts.dry_run {
        outcome.reclaimed = plan
            .reclaim
            .iter()
            .map(|r| (r.target.clone(), r.bytes))
            .collect();
        return;
    }
    match worktree_reclaim::apply(&plan, None, |_| None, |p| std::fs::remove_dir_all(p)) {
        Ok(done) => {
            outcome.reclaimed = done.deleted;
            outcome.reclaim_failed = done.failed;
        }
        Err(e) => outcome
            .reclaim_failed
            .push((plan.main_root.clone(), format!("{e:#}"))),
    }
}

/// `aida session hygiene`.
// trace:STORY-2351 | ai:claude
pub(crate) fn run(opts: &HygieneOptions) -> Result<()> {
    let started = Instant::now();
    let project_root = crate::main_worktree_root_from(&crate::find_project_root()?);
    let mut outcome = HygieneOutcome::default();

    if opts.dry_run {
        outcome.agent_gc = Some(
            crate::agent_registry::gc_dead_agents(&project_root, true, None)
                .map(|r| r.removed_count())
                .map_err(|e| format!("{e:#}")),
        );
        outcome.reap = crate::session_reap::scan_reapable(&project_root)
            .reapable
            .into_iter()
            .map(|row| (row.session, format!("reaped (dry run) — {}", row.reason)))
            .collect();
    } else {
        let quiet = crate::session_reap::reap_quiet_outcomes(&project_root);
        outcome.agent_gc = Some(quiet.agent_gc);
        outcome.reap = quiet.leases;
    }

    outcome.salvageable = salvage_candidates(&project_root);

    if started.elapsed() >= SOFT_BUDGET {
        outcome.reclaim_skipped = Some(format!(
            "the {}s budget was spent on reap; deferred to the next run",
            SOFT_BUDGET.as_secs()
        ));
    } else {
        reclaim(&project_root, opts, &mut outcome);
    }

    print!("{}", render_summary(&outcome, opts.dry_run));
    let items = keep_items(&outcome, opts.idle_hours);
    if items.is_empty() {
        return Ok(());
    }
    for item in &items {
        println!("KEEP  {} — {}", item.what, item.act);
    }
    // The scheduler routes the child's last stderr line; the error is it.
    anyhow::bail!("{}", route_line(&items))
}

#[cfg(test)]
#[path = "tests/story_2351_session_hygiene_tests.rs"]
mod story_2351_session_hygiene_tests;
