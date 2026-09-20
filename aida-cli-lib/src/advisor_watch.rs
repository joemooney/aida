//! STORY-586: presence-gated fork-from-live advisor watch loop.
//!
//! While the operator is `away`, fork the live advisor session only when the
//! canonical `aida awaiting` surface is non-empty (SPIKE-11:
//! copy-then-resume the JSONL so the headless pass boots with the live
//! session's full context) and run a scoped pass that gardens the substrate,
//! triages the mailbox, and ESCALATES anything it can't safely settle. Exits
//! when the operator returns (`aida home`) or the away-TTL lapses.
//!
//! Keystone-autonomy posture: opt-in by invocation, never on by default; the
//! forked advisor is scoped to mechanical + escalate (no keystone or
//! destructive actions). The loop itself only forks + sleeps.
//! trace:STORY-586 trace:SPIKE-11 | ai:claude

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use chrono::Utc;

use crate::advisor::{self, AdvisorConfig};
use crate::presence::{self, Presence};
use crate::session::{self, HeadlessVendor};

/// The scoped instruction the forked advisor runs headless each pass. Mechanical
/// gardening + mailbox triage + escalate-the-rest only.
// trace:TASK-781 | ai:claude — questions sweep added to the garden pass (sweep-only)
const WATCH_PROMPT: &str = "\
You are a forked advisor session running UNATTENDED while the operator is away. \
Do ONLY safe, bounded, reversible work; ESCALATE everything else — never make a \
keystone or destructive decision unsupervised.

Run this garden + triage pass, then exit:
1. `aida doctor --heal --category OBE-briefs --yes` then `--category stale-leases --yes` \
(safe auto-fixes only; REPORT [manual] findings, never --force them).
2. `aida queue list` — for any 'auto-bump missed' item, run `aida db reconcile-status --spec <ID>`.
3. `aida questions sweep` — flag specs that likely need a human decision and record a \
DecisionRequest for each (so the operator drains them later as pure picks via `aida \
questions answer`). SWEEP ONLY: detect + record the request — do NOT answer any question \
yourself; answering stays human (`aida questions answer`) or the /aida-decide skill.
4. `aida mailbox inbox advisor` — for each unread message: if it is a bounded/mechanical \
request you can settle safely, do it; otherwise leave it and record a one-line escalation \
via `aida findings add` (or a comment on the relevant spec) for the operator.
5. Do NOT merge PRs, approve specs, answer DecisionRequests, run drains, or take any action \
you are not certain is safe and reversible.

End with a 3-5 line summary: what you gardened, what mail you handled, what you escalated.";

/// Conservative variant (TASK-776, `--triage-only`): still runs the safe garden
/// pass, but only SURFACES/escalates mailbox items — never acts on a request.
const WATCH_PROMPT_TRIAGE: &str = "\
You are a forked advisor session running UNATTENDED while the operator is away, in \
TRIAGE-ONLY mode. Run the safe garden pass, then SURFACE the mailbox — do not act on \
any mailbox request.

1. `aida doctor --heal --category OBE-briefs --yes` then `--category stale-leases --yes` \
(safe auto-fixes only; REPORT [manual] findings, never --force them).
2. `aida queue list` — for any 'auto-bump missed' item, run `aida db reconcile-status --spec <ID>`.
3. `aida questions sweep` — flag specs that likely need a human decision and record a \
DecisionRequest for each, so the operator drains them later as pure picks. SWEEP ONLY: \
detect + record — never answer a question yourself.
4. `aida mailbox inbox advisor` — for each unread message, record a one-line escalation via \
`aida findings add` (or a comment on the relevant spec) for the operator. Do NOT act on the \
requests themselves; leave them for the operator to decide.
5. Do NOT merge PRs, approve specs, answer DecisionRequests, run drains, or take any non-garden action.

End with a 3-5 line summary: what you gardened and what mail you surfaced for the operator.";

/// One tick's decision. Pure — see [`plan_watch_tick`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WatchTick {
    /// Stop the loop, with a human-readable reason.
    Exit(String),
    /// Do nothing this tick, with a reason.
    Skip(String),
    /// Fork-from-live and run the scoped pass.
    Fork,
}

/// Pure tick decision. `presence` is the effective presence (already accounts
/// for the away-TTL, so `Home` covers both "operator returned" and "TTL
/// lapsed") and is the **hard gate** — a `Home` operator never forks, whatever
/// else is true.
///
/// TASK-1245: the substrate is the clock. The live advisor wakes only when the
/// canonical `aida awaiting` surface is non-empty (mail, findings, mergeable
/// PRs, worker directives, shelved work, and other operator gates). A timer may
/// re-check the substrate, but elapsed time is never itself a reason to fork.
// trace:TASK-991 trace:STORY-712 trace:TASK-776 | ai:claude
// trace:TASK-1245 | ai:codex
pub(crate) fn plan_watch_tick(presence: Presence, awaiting_non_empty: bool) -> WatchTick {
    // Hard gate: a present operator is the supervisor — never fork.
    if matches!(presence, Presence::Home) {
        return WatchTick::Exit("operator is home (returned or away-TTL lapsed)".to_string());
    }
    if awaiting_non_empty {
        return WatchTick::Fork;
    }

    WatchTick::Skip("away; `aida awaiting` is empty — advisor stays asleep".to_string())
}

/// Options for [`run_advisor_watch`].
pub(crate) struct WatchOpts {
    /// How often to wake and re-check presence (seconds).
    pub poll_interval_secs: u64,
    /// Legacy compatibility knob. The advisor no longer forks on elapsed time;
    /// it wakes only when `aida awaiting` is non-empty.
    pub fork_interval_secs: u64,
    /// Preview decisions (and fork cost) without forking.
    pub dry_run: bool,
    /// Run a single tick and return (cron / testing).
    pub once: bool,
    /// Conservative mode: the forked advisor still runs the safe garden pass but
    /// SURFACES/escalates mailbox items instead of acting on bounded requests.
    pub triage_only: bool,
}

/// The watch loop. Returns when the operator is home / the away-TTL lapses, or
/// after one tick when `once` is set.
pub(crate) fn run_advisor_watch(project_root: &Path, opts: &WatchOpts) -> Result<()> {
    let config = AdvisorConfig::load(project_root);

    println!(
        "advisor watch {}— wakes the live advisor only when `aida awaiting` is non-empty; exits on `aida home`.",
        if opts.dry_run { "(dry-run) " } else { "" }
    );
    if opts.fork_interval_secs != 1200 {
        println!("  · --fork-interval is ignored by the event-driven advisor heartbeat");
    }

    let prompt = if opts.triage_only {
        WATCH_PROMPT_TRIAGE
    } else {
        WATCH_PROMPT
    };

    loop {
        let presence = presence::current_presence(Utc::now());
        let awaiting = advisor_awaiting_line(project_root);
        match plan_watch_tick(presence, awaiting.is_some()) {
            WatchTick::Exit(reason) => {
                println!("advisor watch exiting: {reason}");
                break;
            }
            WatchTick::Skip(reason) => {
                println!("  · skip: {reason}");
            }
            WatchTick::Fork => {
                if let Some(line) = awaiting.as_deref() {
                    println!("  · {line} — forking now");
                }
                if opts.dry_run {
                    preview_fork(project_root, &config);
                } else {
                    fork_and_run(project_root, &config, prompt)?;
                }
            }
        }
        if opts.once {
            break;
        }
        std::thread::sleep(Duration::from_secs(opts.poll_interval_secs));
    }
    Ok(())
}

/// The advisor heartbeat wakes from the same canonical surface an operator sees
/// with `aida awaiting`, rather than from a parallel mail/event/cadence test.
/// Best-effort and read-only: if the store/cache cannot be opened, stay quiet
/// and let the next poll try again.
// trace:TASK-1245 | ai:codex
fn advisor_awaiting_line(project_root: &Path) -> Option<String> {
    let store_path = super::detect_distributed_store_from(project_root)
        .unwrap_or_else(|| project_root.join(".aida-store"));
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::open(&store_path, &cache_path).ok()?;
    let ctx = super::UserStatusContext {
        session: None,
        role: Some("advisor".to_string()),
        branch: None,
        pr: None,
        queue_head: Vec::new(),
        queue_total: 0,
        agents: Vec::new(),
    };
    super::collect_awaiting_report(project_root, &backend, &ctx, false).compact_line()
}

fn short_uuid(uuid: &str) -> &str {
    &uuid[..uuid.len().min(8)]
}

fn preview_fork(project_root: &Path, config: &AdvisorConfig) {
    let vendor = session::resolve_headless_vendor(project_root);
    if vendor != HeadlessVendor::Claude {
        println!(
            "  · [dry-run] would cold-boot a `{}` advisor and run the garden+triage pass",
            vendor.as_str()
        );
        return;
    }
    match advisor::plan_fork(project_root, config) {
        Some(plan) => println!(
            "  · [dry-run] would fork live advisor {} (~${:.2}) and run the garden+triage pass",
            short_uuid(&plan.live.uuid),
            advisor::estimated_fork_cost_usd(plan.live.jsonl_size_bytes)
        ),
        None => println!(
            "  · [dry-run] no live advisor session to fork — the pass would be skipped (or cold-boot)"
        ),
    }
}

fn fork_and_run(project_root: &Path, config: &AdvisorConfig, prompt: &str) -> Result<()> {
    let vendor = session::resolve_headless_vendor(project_root);
    if vendor != HeadlessVendor::Claude {
        return cold_boot_and_run(project_root, vendor, prompt);
    }
    let Some(plan) = advisor::plan_fork(project_root, config) else {
        println!("  · no live advisor session to fork — skipping this pass");
        return Ok(());
    };
    let bytes = advisor::execute_fork(&plan)?;
    println!(
        "  · forked live advisor {} ({} KB) — running garden+triage headless…",
        short_uuid(&plan.live.uuid),
        bytes / 1024
    );
    let log_path = project_root
        .join(".aida")
        .join("advisor-watch")
        .join(format!("{}.log", plan.fork_uuid));
    let tee = crate::headless_tee::TeeOptions::from_env_and_flag(false).with_label("advisor-watch");
    let status = session::spawn_claude_headless_resume(
        prompt,
        &plan.fork_uuid,
        &log_path,
        project_root,
        &tee,
        false,
    )?;
    if !status.success() {
        eprintln!(
            "  · advisor-watch pass exited with {} (see {})",
            status,
            log_path.display()
        );
    }
    Ok(())
}

/// Codex has no portable transcript-copy/resume contract equivalent to
/// Claude's JSONL fork. Keep the advisor seat available by running the tick as
/// a vendor-native cold boot, rooted in the same project substrate.
// trace:TASK-1279 | ai:codex
fn cold_boot_and_run(project_root: &Path, vendor: HeadlessVendor, prompt: &str) -> Result<()> {
    let run_id = uuid::Uuid::new_v4().to_string();
    println!(
        "  · `{}` has no fork-from-live transport — cold-booting the advisor tick…",
        vendor.as_str()
    );
    let log_path = project_root
        .join(".aida")
        .join("advisor-watch")
        .join(format!("{run_id}.log"));
    let tee = crate::headless_tee::TeeOptions::from_env_and_flag(false).with_label("advisor-watch");
    let status = session::spawn_vendor_headless_with_seat(
        vendor,
        aida_core::agents_config::AgentSeat::Advisor,
        prompt,
        &run_id,
        &log_path,
        &tee,
        false,
    )?;
    if !status.success() {
        eprintln!(
            "  · advisor-watch pass exited with {} (see {})",
            status,
            log_path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The five existing decision cases, ported to the slice-3 signature with no
    // live event stream (`actionable_event = false`, `event_stream_live = false`)
    // — the degenerate-fallback path, which must behave EXACTLY as before.

    #[test]
    fn home_exits_the_loop_even_before_first_fork() {
        assert!(matches!(
            plan_watch_tick(Presence::Home, false),
            WatchTick::Exit(_)
        ));
        assert!(matches!(
            plan_watch_tick(Presence::Home, true),
            WatchTick::Exit(_)
        ));
    }

    #[test]
    fn away_forks_only_when_awaiting_is_non_empty() {
        assert_eq!(plan_watch_tick(Presence::Away, true), WatchTick::Fork);
        assert!(matches!(
            plan_watch_tick(Presence::Away, false),
            WatchTick::Skip(_)
        ));
    }

    #[test]
    fn presence_present_never_forks() {
        // Presence is the hard gate — Home exits over every other trigger.
        assert!(matches!(
            plan_watch_tick(Presence::Home, true),
            WatchTick::Exit(_)
        ));
        assert!(matches!(
            plan_watch_tick(Presence::Home, false),
            WatchTick::Exit(_)
        ));
    }

    #[test]
    fn elapsed_time_alone_never_wakes_the_advisor() {
        assert!(matches!(
            plan_watch_tick(Presence::Away, false),
            WatchTick::Skip(_)
        ));
    }

    // Event-stream waiting for integrator watch lives in `crate::event_wait`;
    // advisor watch now keys off the canonical `aida awaiting` report.
}
