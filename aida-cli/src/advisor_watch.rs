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
const WATCH_PROMPT: &str = "\
You are a forked advisor session running UNATTENDED while the operator is away. \
Do ONLY safe, bounded, reversible work; ESCALATE everything else — never make a \
keystone or destructive decision unsupervised.

Run this garden + triage pass, then exit:
1. `aida doctor --heal --category OBE-briefs --yes` then `--category stale-leases --yes` \
(safe auto-fixes only; REPORT [manual] findings, never --force them).
2. `aida queue list` — for any 'auto-bump missed' item, run `aida db reconcile-status --spec <ID>`.
3. `aida mailbox inbox advisor` — for each unread message: if it is a bounded/mechanical \
request you can settle safely, do it; otherwise leave it and record a one-line escalation \
via `aida findings add` (or a comment on the relevant spec) for the operator.
4. Do NOT merge PRs, approve specs, run drains, or take any action you are not certain is \
safe and reversible.

End with a 3-5 line summary: what you gardened, what mail you handled, what you escalated.";

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
/// lapsed"). `secs_since_last_fork` is `None` before the first fork. Forks on
/// the first away tick, then every `fork_interval_secs`.
pub(crate) fn plan_watch_tick(
    presence: Presence,
    secs_since_last_fork: Option<u64>,
    fork_interval_secs: u64,
) -> WatchTick {
    if matches!(presence, Presence::Home) {
        return WatchTick::Exit("operator is home (returned or away-TTL lapsed)".to_string());
    }
    match secs_since_last_fork {
        None => WatchTick::Fork,
        Some(s) if s >= fork_interval_secs => WatchTick::Fork,
        Some(s) => WatchTick::Skip(format!(
            "away; {s}s since last fork (< {fork_interval_secs}s cadence)"
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

    loop {
        let presence = presence::current_presence(Utc::now());
        let secs = last_fork.map(|t| t.elapsed().as_secs());
        match plan_watch_tick(presence, secs, opts.fork_interval_secs) {
            WatchTick::Exit(reason) => {
                println!("advisor watch exiting: {reason}");
                break;
            }
            WatchTick::Skip(reason) => {
                println!("  · skip: {reason}");
            }
            WatchTick::Fork => {
                if opts.dry_run {
                    preview_fork(project_root, &config);
                } else {
                    fork_and_run(project_root, &config)?;
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

fn fork_and_run(project_root: &Path, config: &AdvisorConfig) -> Result<()> {
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
        WATCH_PROMPT,
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
            plan_watch_tick(Presence::Home, None, 60),
            WatchTick::Exit(_)
        ));
        assert!(matches!(
            plan_watch_tick(Presence::Home, Some(9999), 60),
            WatchTick::Exit(_)
        ));
    }

    #[test]
    fn away_forks_on_the_first_tick() {
        assert_eq!(plan_watch_tick(Presence::Away, None, 60), WatchTick::Fork);
    }

    #[test]
    fn away_forks_once_the_cadence_has_elapsed() {
        assert_eq!(
            plan_watch_tick(Presence::Away, Some(60), 60),
            WatchTick::Fork
        );
        assert_eq!(
            plan_watch_tick(Presence::Away, Some(600), 60),
            WatchTick::Fork
        );
    }

    #[test]
    fn away_skips_before_the_cadence() {
        assert!(matches!(
            plan_watch_tick(Presence::Away, Some(30), 60),
            WatchTick::Skip(_)
        ));
    }
}
