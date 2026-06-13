//! STORY-586: presence-gated fork-from-live advisor watch loop.
//!
//! While the operator is `away`, periodically fork the live advisor session
//! (SPIKE-11: copy-then-resume the JSONL so the headless pass boots with the
//! live session's full context) and run a scoped pass that gardens the
//! substrate, triages the mailbox, and ESCALATES anything it can't safely
//! settle. Exits when the operator returns (`aida home`) or the away-TTL
//! lapses.
//!
//! Keystone-autonomy posture: opt-in by invocation, never on by default; the
//! forked advisor is scoped to mechanical + escalate (no keystone or
//! destructive actions). The loop itself only forks + sleeps.
//! trace:STORY-586 trace:SPIKE-11 | ai:claude

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;

use crate::advisor::{self, AdvisorConfig};
use crate::presence::{self, Presence};
use crate::session;

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
/// lapsed"). `has_unread_mail` forks immediately (event-driven — TASK-776; the
/// loop's poll sleep rate-limits how often this fires). Otherwise forks on the
/// first away tick and then every `fork_interval_secs` (`secs_since_last_fork`
/// is `None` before the first fork).
pub(crate) fn plan_watch_tick(
    presence: Presence,
    has_unread_mail: bool,
    secs_since_last_fork: Option<u64>,
    fork_interval_secs: u64,
) -> WatchTick {
    if matches!(presence, Presence::Home) {
        return WatchTick::Exit("operator is home (returned or away-TTL lapsed)".to_string());
    }
    if has_unread_mail {
        return WatchTick::Fork;
    }
    match secs_since_last_fork {
        None => WatchTick::Fork,
        Some(s) if s >= fork_interval_secs => WatchTick::Fork,
        Some(s) => WatchTick::Skip(format!(
            "away; no unread mail; {s}s since last fork (< {fork_interval_secs}s cadence)"
        )),
    }
}

/// Options for [`run_advisor_watch`].
pub(crate) struct WatchOpts {
    /// How often to wake and re-check presence (seconds).
    pub poll_interval_secs: u64,
    /// How often to actually fork-and-run (seconds).
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
    let mut last_fork: Option<Instant> = None;

    println!(
        "advisor watch {}— forks the live advisor every {}s while away; exits on `aida home`.",
        if opts.dry_run { "(dry-run) " } else { "" },
        opts.fork_interval_secs
    );

    let prompt = if opts.triage_only {
        WATCH_PROMPT_TRIAGE
    } else {
        WATCH_PROMPT
    };

    loop {
        let presence = presence::current_presence(Utc::now());
        let secs = last_fork.map(|t| t.elapsed().as_secs());
        let has_unread = advisor_unread_count(project_root) > 0;
        match plan_watch_tick(presence, has_unread, secs, opts.fork_interval_secs) {
            WatchTick::Exit(reason) => {
                println!("advisor watch exiting: {reason}");
                break;
            }
            WatchTick::Skip(reason) => {
                println!("  · skip: {reason}");
            }
            WatchTick::Fork => {
                if has_unread {
                    println!("  · unread advisor mail — forking now");
                }
                if opts.dry_run {
                    preview_fork(project_root, &config);
                } else {
                    fork_and_run(project_root, &config, prompt)?;
                }
                last_fork = Some(Instant::now());
            }
        }
        if opts.once {
            break;
        }
        std::thread::sleep(Duration::from_secs(opts.poll_interval_secs));
    }
    Ok(())
}

/// Count unread messages addressed to the `advisor` (direct + broadcast),
/// merging the local + canonical mailbox layers against the advisor's read
/// watermark. Used for the event-driven fork trigger (TASK-776). Best-effort —
/// any read failure yields 0 (never blocks the loop). trace:TASK-776 | ai:claude
fn advisor_unread_count(project_root: &Path) -> usize {
    let store_root = project_root.join(".aida-store");
    let local = crate::mailbox_store::read_local_messages(project_root).unwrap_or_default();
    let canonical = crate::mailbox_store::read_canonical_messages(&store_root).unwrap_or_default();
    let merged = aida_core::mailbox::merge_dedup(&local, &canonical);
    let mark = crate::mailbox_store::read_watermark(project_root, "advisor");
    let (unread, _urgent) = aida_core::mailbox::unread_counts("advisor", &merged, mark);
    unread
}

fn short_uuid(uuid: &str) -> &str {
    &uuid[..uuid.len().min(8)]
}

fn preview_fork(project_root: &Path, config: &AdvisorConfig) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_exits_the_loop_even_before_first_fork() {
        assert!(matches!(
            plan_watch_tick(Presence::Home, false, None, 60),
            WatchTick::Exit(_)
        ));
        assert!(matches!(
            plan_watch_tick(Presence::Home, false, Some(9999), 60),
            WatchTick::Exit(_)
        ));
    }

    #[test]
    fn away_forks_on_the_first_tick() {
        assert_eq!(
            plan_watch_tick(Presence::Away, false, None, 60),
            WatchTick::Fork
        );
    }

    #[test]
    fn away_forks_once_the_cadence_has_elapsed() {
        assert_eq!(
            plan_watch_tick(Presence::Away, false, Some(60), 60),
            WatchTick::Fork
        );
        assert_eq!(
            plan_watch_tick(Presence::Away, false, Some(600), 60),
            WatchTick::Fork
        );
    }

    #[test]
    fn away_skips_before_the_cadence_with_no_mail() {
        assert!(matches!(
            plan_watch_tick(Presence::Away, false, Some(30), 60),
            WatchTick::Skip(_)
        ));
    }

    #[test]
    fn unread_mail_forks_immediately_even_before_the_cadence() {
        // TASK-776: event-driven — unread advisor mail beats the idle timer.
        assert_eq!(
            plan_watch_tick(Presence::Away, true, Some(1), 60),
            WatchTick::Fork
        );
    }

    #[test]
    fn home_wins_over_unread_mail() {
        // Presence is the hard gate — never fork once the operator is back.
        assert!(matches!(
            plan_watch_tick(Presence::Home, true, Some(1), 60),
            WatchTick::Exit(_)
        ));
    }
}
