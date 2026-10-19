//! `aida shift` clap types, owned by `shift.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// The night shift: a scheduler-run tick that launches bounded drain waves
/// while nobody is at the keyboard. Off unless enabled for this clone.
// trace:STORY-1218 | ai:claude
#[derive(Subcommand, Debug, Clone)]
pub enum ShiftCommand {
    /// Run one night-shift check: reap finished sessions, then launch the next
    /// bounded drain wave only if every safety guard passes. Refusals exit 0.
    /// Registered as the `night-shift` job of `aida schedule tick`; safe to run
    /// by hand. Does nothing unless the night shift is enabled for this clone.
    Tick {
        /// Print the exact wave command, the specs it would include, every
        /// guard with its value, and the enable source, then exit without
        /// writing anything (no tags, no state, no event, no launch).
        #[clap(long)]
        dry_run: bool,
        /// Emit the tick report as JSON.
        #[clap(long)]
        json: bool,
    },
    /// Show whether the night shift is on for this clone, where that setting
    /// lives, which scheduler driver runs it (cron, systemd, or both), the last
    /// check and wave, and the guard verdicts.
    Status {
        /// Emit JSON instead of text.
        #[clap(long)]
        json: bool,
    },
    /// Turn the night shift on for THIS clone only. The switch is written to
    /// `~/.aida/shift-local.toml` (keyed by this repo's path), never to the
    /// committed project config, and the `night-shift` job is registered.
    /// Needs a human at an interactive terminal and an explicit yes; an agent
    /// session cannot enable it.
    Enable,
    /// Turn the night shift off for this clone.
    Disable,
    /// Allow launches again after the night shift stopped itself because
    /// consecutive waves made no progress. Needs a human at an interactive
    /// terminal and an explicit yes.
    Resume,
    /// Turn the night shift on for this clone AND install the scheduler driver
    /// that runs it: `--systemd-user` (a systemd user timer, Linux) or
    /// `--cron` (a crontab entry). Installing one driver removes this repo's
    /// other one after the new one is verified. Needs a human at an
    /// interactive terminal and an explicit yes.
    // trace:TASK-1491 | ai:claude
    Install {
        /// Install a systemd user timer (Linux).
        #[clap(long, conflicts_with = "cron", required_unless_present = "cron")]
        systemd_user: bool,
        /// Install a crontab entry.
        #[clap(long)]
        cron: bool,
    },
}
