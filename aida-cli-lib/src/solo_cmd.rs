//! `aida solo` command cluster — the solo loop (`solo_cycle`,
//! `run_solo_loop`) and its dispatcher (`handle_solo_command`), extracted
//! from `lib.rs` (SPIKE-78 / STORY-771; pure movement, no behavior change).
// trace:STORY-771 | ai:claude

use crate::*;

/// STORY-625: one cycle of the solo loop — the safe-backlog pipeline, composed
/// from the existing (individually-safe) commands by shelling out to this same
/// binary. Order: garden (hygiene) → assess+queue (advisor sign-off, keystone
/// parked) → implement (headless drain) → integrate (merge Done PRs, keystone
/// parked). A non-zero step is logged but does NOT kill the loop (a shelved
/// phase is retried next tick). With `dry_run`, each step is PRINTED, not run.
/// trace:STORY-625 | ai:claude
pub(crate) fn solo_cycle(dry_run: bool) -> Result<()> {
    // BUG-562: use the hardened resolver, NOT raw current_exe(). The solo loop is
    // long-running; a `cargo build` that swaps the binary mid-run makes Linux
    // report current_exe() as "<path> (deleted)", which Command::new cannot spawn
    // — every cycle step then ENOENTs. resolve_aida_exe() falls back to the
    // on-PATH `aida` (the live binary) when the exe path is gone. trace:BUG-562
    let exe = resolve_aida_exe();
    // Each step is (label, args-to-`aida`). Garden first — this is what reaps the
    // stale leases / OBE briefs the operator otherwise saw pile up unattended.
    let steps: &[(&str, &[&str])] = &[
        (
            "garden — reap stale leases",
            &["doctor", "--heal", "--category", "stale-leases", "--yes"],
        ),
        (
            "garden — reap OBE briefs",
            &["doctor", "--heal", "--category", "OBE-briefs", "--yes"],
        ),
        (
            "garden — reconcile Done→Completed",
            &["db", "reconcile-status"],
        ),
        (
            "assess + queue (advisor sign-off; keystone parked)",
            &["intake", "--apply"],
        ),
        (
            "implement (headless drain of the queued set)",
            &["burndown", "run"],
        ),
        (
            "integrate (merge Done PRs; keystone parked)",
            &["queue", "integrate"],
        ),
    ];
    for (label, args) in steps {
        if dry_run {
            println!(
                "  {} {} — aida {}",
                "would run:".dimmed(),
                label,
                args.join(" ")
            );
            continue;
        }
        println!(
            "  {} {} — aida {}",
            crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
            label,
            args.join(" ").dimmed()
        );
        // STORY-627: spawn the step and poll it, printing a heartbeat every
        // ~30s while it runs. The long steps (`intake --apply`, `burndown run`)
        // shell out to `claude -p` for minutes — without this an operator watches
        // silence and can't tell a working loop from a wedged one.
        // trace:STORY-627 | ai:claude
        match std::process::Command::new(&exe).args(*args).spawn() {
            Ok(mut child) => {
                let started = std::time::Instant::now();
                let mut next_beat = std::time::Duration::from_secs(SOLO_HEARTBEAT_SECS);
                let status = loop {
                    match child.try_wait() {
                        Ok(Some(s)) => break Ok(s),
                        Ok(None) => {
                            if started.elapsed() >= next_beat {
                                let mins = started.elapsed().as_secs() / 60;
                                println!(
                                    "    {} working… ({}m) — {} to stop",
                                    "·".dimmed(),
                                    mins,
                                    "Ctrl-C or `aida solo stop`".cyan()
                                );
                                next_beat += std::time::Duration::from_secs(SOLO_HEARTBEAT_SECS);
                            }
                            std::thread::sleep(std::time::Duration::from_millis(500));
                        }
                        Err(e) => break Err(e),
                    }
                };
                match status {
                    Ok(s) if s.success() => {}
                    Ok(s) => eprintln!(
                        "    {} step exited {} — continuing (retried next tick)",
                        "note:".yellow(),
                        s.code().unwrap_or(-1)
                    ),
                    Err(e) => eprintln!(
                        "    {} step wait failed: {e} — continuing",
                        "note:".yellow()
                    ),
                }
            }
            Err(e) => eprintln!(
                "    {} step failed to spawn: {e} — continuing",
                "note:".yellow()
            ),
        }
    }
    Ok(())
}

/// STORY-627: how often the per-step progress heartbeat prints while a long
/// `solo_cycle` step (e.g. `claude -p`) runs. trace:STORY-627 | ai:claude
pub(crate) const SOLO_HEARTBEAT_SECS: u64 = 30;

/// STORY-627: while the inter-cycle sleep waits, poll the solo flag this often so
/// `aida solo stop` lands within seconds, not a full cycle. trace:STORY-627
pub(crate) const SOLO_STOP_POLL_SECS: u64 = 2;

/// STORY-625: the solo LOOP — the single leave-it-running command that works the
/// safe backlog end-to-end on a cadence (subsumes `aida queue integrate
/// --watch`). Sets the solo flag on entry; each tick re-checks it and exits when
/// it's cleared (`aida solo --off`) or the TTL lapses; Ctrl-C also stops it.
/// `--dry-run` runs ONE tick that prints the cycle and exits, so the loop is
/// verifiable without a live drain. trace:STORY-625 | ai:claude
pub(crate) fn run_solo_loop(dry_run: bool, interval: u64) -> Result<()> {
    // STORY-627: acquire the per-repo solo lock so a second `aida solo run`
    // refuses while a live one holds it (and a Ctrl-C-killed loop is
    // stale-reclaimed). A dry-run is a single non-integrating tick — no lock.
    // The guard's Drop releases the lock on a clean exit / flag-clear.
    // trace:STORY-627 | ai:claude
    let lock_guard = if dry_run {
        None
    } else {
        let root =
            find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        Some(solo_lock::acquire_solo_lock(&root)?)
    };

    // Entering the loop turns solo on (default TTL) so the flag + statusline
    // reflect the active pipeline. A dry-run does NOT flip the real flag.
    if !dry_run && !presence::current_solo(chrono::Utc::now()) {
        presence::set_solo(presence::DEFAULT_SOLO_TTL_SECS)?;
    }
    println!(
        "{} solo loop {} (interval {}s) — garden → assess/queue → implement → integrate → repeat. \
         {} to stop.",
        crate::glyph(crate::glyphs::Glyph::Robot),
        if dry_run {
            "DRY-RUN".yellow().bold()
        } else {
            "running".green().bold()
        },
        interval,
        "Ctrl-C or `aida solo stop`".cyan()
    );
    loop {
        let now = chrono::Utc::now();
        if !dry_run && !presence::current_solo(now) {
            println!(
                "{} solo flag cleared (stop or TTL) — loop stopped.",
                "■".dimmed()
            );
            break;
        }
        println!("\n{} cycle @ {}", "──".dimmed(), now.to_rfc3339().dimmed());
        // STORY-638: refresh the shared cross-clone solo claim's heartbeat each
        // tick so a long-running loop never ages past its TTL and gets reclaimed
        // by another clone. No-op for a dry-run / local-only. trace:STORY-638
        if let Some(g) = lock_guard.as_ref() {
            g.heartbeat();
        }
        solo_cycle(dry_run)?;
        if dry_run {
            println!("\n{} dry-run: one cycle shown; not looping.", "■".dimmed());
            break;
        }
        println!(
            "{} cycle complete; sleeping {}s ({} to stop)",
            "·".dimmed(),
            interval,
            "Ctrl-C / `aida solo stop`".cyan()
        );
        // STORY-627 responsive stop: poll the solo flag in short increments
        // during the inter-cycle sleep so `aida solo stop` lands within seconds,
        // not a full cycle + sleep. Break the moment the flag clears.
        // trace:STORY-627 | ai:claude
        if solo_sleep_until_stop(
            interval,
            SOLO_STOP_POLL_SECS,
            &mut |secs| std::thread::sleep(std::time::Duration::from_secs(secs)),
            &mut || presence::current_solo(chrono::Utc::now()),
        ) {
            println!(
                "{} solo flag cleared during sleep — loop stopped.",
                "■".dimmed()
            );
            break;
        }
    }
    Ok(())
}

/// STORY-627: the inter-cycle sleep, broken into `poll_secs` increments so the
/// solo flag is re-checked frequently and `aida solo stop` lands within seconds.
/// Returns `true` if the flag cleared mid-sleep (caller should stop the loop),
/// `false` if the full `interval` elapsed with the flag still set. Both `sleep`
/// and the flag check (`still_solo`) are injected so the poll cadence is
/// unit-testable without real wall-clock waits or touching `~/.aida/solo.toml`.
/// trace:STORY-627 | ai:claude
pub(crate) fn solo_sleep_until_stop(
    interval: u64,
    poll_secs: u64,
    sleep: &mut impl FnMut(u64),
    still_solo: &mut impl FnMut() -> bool,
) -> bool {
    let poll = poll_secs.max(1);
    let mut remaining = interval;
    while remaining > 0 {
        let chunk = remaining.min(poll);
        sleep(chunk);
        remaining -= chunk;
        if !still_solo() {
            return true;
        }
    }
    false
}

/// `aida solo [--off | --status | --ttl <DURATION> | --watch [--dry-run]]` —
/// enter/exit/show solo mode (the visible work-state flag, STORY-624) or run the
/// solo LOOP (STORY-625). State lives in `~/.aida/solo.toml` with a safety TTL;
/// the statusline surfaces it. trace:STORY-624 trace:STORY-625 | ai:claude
/// STORY-627: the resolved solo action after folding the canonical verb and the
/// legacy `--watch`/`--off`/`--status` flag aliases into one. The verb wins when
/// present; otherwise the flag aliases apply (silently, so nothing breaks).
/// Pure so the verb→action mapping is unit-testable. trace:STORY-627 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SoloEffect {
    /// Run the loop (`solo run` / `--watch`).
    Run,
    /// Stop a running loop (`solo stop` / `--off`).
    Stop,
    /// Print state (`solo status` / `--status`).
    Status,
    /// No verb, no alias → enter solo MODE (set the flag).
    EnterMode,
}

/// Fold the canonical verb and the legacy flag aliases into a single effect.
/// The verb takes precedence; otherwise `--watch`→Run, `--off`→Stop,
/// `--status`→Status, and (with none of them) EnterMode. `--off` is checked
/// before `--status` to match the prior arm order. trace:STORY-627 | ai:claude
pub(crate) fn resolve_solo_effect(
    action: Option<SoloAction>,
    off: bool,
    status: bool,
    watch: bool,
) -> SoloEffect {
    if let Some(a) = action {
        return match a {
            SoloAction::Run => SoloEffect::Run,
            SoloAction::Stop => SoloEffect::Stop,
            SoloAction::Status => SoloEffect::Status,
        };
    }
    if watch {
        SoloEffect::Run
    } else if off {
        SoloEffect::Stop
    } else if status {
        SoloEffect::Status
    } else {
        SoloEffect::EnterMode
    }
}

pub(crate) fn handle_solo_command(
    action: Option<SoloAction>,
    off: bool,
    status: bool,
    ttl: Option<&str>,
    watch: bool,
    dry_run: bool,
    interval: u64,
) -> Result<()> {
    let effect = resolve_solo_effect(action, off, status, watch);
    let now = chrono::Utc::now();
    if effect == SoloEffect::Run {
        return run_solo_loop(dry_run, interval);
    }
    if effect == SoloEffect::Status {
        // STORY-627: corroborate the flag against the loop lock so `status` can
        // tell a live loop from a Ctrl-C-orphaned flag. trace:STORY-627
        let root =
            find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        match solo_lock::probe_lock(&root) {
            solo_lock::LockStatus::Running(l) => {
                println!(
                    "{} solo loop {} (pid {}, since {})",
                    crate::glyph(crate::glyphs::Glyph::Robot),
                    "RUNNING".green().bold(),
                    l.pid,
                    l.started_at_utc
                );
            }
            solo_lock::LockStatus::Stale(l) => {
                println!(
                    "{} solo loop flag set but no live process (stale lock pid {}). \
                     Run `aida solo stop` to clear it.",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    l.pid
                );
            }
            solo_lock::LockStatus::None => {
                if presence::current_solo(now) {
                    println!(
                        "{} solo mode {}",
                        crate::glyph(crate::glyphs::Glyph::Robot),
                        "ON".green().bold()
                    );
                } else {
                    println!("solo mode {}", "off".dimmed());
                }
            }
        }
        return Ok(());
    }
    if effect == SoloEffect::Stop {
        // STORY-627: clear the flag AND signal a live loop pid so stop lands even
        // mid-step (when the loop is blocked inside a long `claude -p` step) and
        // the flag-left-ON-after-Ctrl-C gap is closed. trace:STORY-627
        presence::clear_solo()?;
        let root =
            find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        match solo_lock::probe_lock(&root) {
            solo_lock::LockStatus::Running(l) => {
                if solo_lock::signal_stop(l.pid) {
                    println!(
                        "solo mode {} — signalled the running loop (pid {}).",
                        "off".bold(),
                        l.pid
                    );
                } else {
                    println!(
                        "solo mode {} (loop pid {} already gone).",
                        "off".bold(),
                        l.pid
                    );
                }
            }
            solo_lock::LockStatus::Stale(_) | solo_lock::LockStatus::None => {
                println!("solo mode {}", "off".bold());
            }
        }
        return Ok(());
    }
    let ttl_secs = match ttl {
        Some(s) => presence::parse_duration_secs(s)
            .ok_or_else(|| anyhow::anyhow!("invalid --ttl '{s}'; use e.g. 8h, 30m, 2h30m"))?,
        None => presence::DEFAULT_SOLO_TTL_SECS,
    };
    presence::set_solo(ttl_secs)?;
    println!(
        "{} solo mode {} — advisor+integrator working the safe backlog end-to-end; \
         keystone/architecture is parked for the operator. `{}` to exit.",
        crate::glyph(crate::glyphs::Glyph::Robot),
        "ON".green().bold(),
        "aida solo stop".cyan()
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests/solo_tests.rs"]
mod solo_tests;
