//! `aida maintenance_schedule` clap types, owned by `maintenance_schedule.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Native no-daemon job registry (`aida schedule`, alias `aida cron`). Reads
/// `[schedule]` jobs from `.aida/config.toml` (project) and
/// `~/.aida/schedule.toml` (global; project wins by name). A SUBSTRATE job
/// (`command = "session reap"`) is run by `tick` through AIDA's built-in
/// allow-list; a SEAT job (`prompt = "..."`, `seats = ["advisor"]`) is never
/// executed here — it becomes due and is delivered to whoever holds the seat
/// (`aida awaiting --notice`, the pickup prompt, the launch context), who
/// reports back with `done`. Schedules: `every = "30m"`, `on = ["PrMerged"]`,
/// `when = "mail.oldest_unread_age > 15m"`. Runs are ledgered per job on the
/// aida-store branch (`schedule/<job>.yaml`).
// trace:STORY-1047 | ai:codex
// trace:STORY-1226 | ai:claude
#[derive(Subcommand, Debug)]
pub enum MaintenanceScheduleCommand {
    /// Evaluate every enabled job: run due substrate jobs, mark due seat jobs.
    Tick {
        /// Hook path: skip jobs that need network or heavier external effects.
        #[clap(long)]
        hook: bool,
    },

    /// Force one job now (substrate: execute; seat: print its prompt), or
    /// every enabled substrate job.
    Run {
        /// Configured job name.
        name: Option<String>,
    },

    /// Show every configured job with kind, seats, schedule, status, last
    /// run, next due — plus legacy `.aida/schedules.toml` cadence entries.
    Status {
        /// Emit JSON instead of a table.
        #[clap(long)]
        json: bool,
    },

    /// List the merged registry (project + global) with source layer and
    /// last reporter.
    List {
        /// Only jobs that apply to this seat (role), e.g. `advisor`.
        #[clap(long, value_name = "ROLE")]
        seat: Option<String>,
        /// Emit JSON instead of a table.
        #[clap(long)]
        json: bool,
    },

    /// What is due right now: seat jobs for a seat (or every seat), plus due
    /// substrate jobs when no seat filter is given.
    Due {
        /// Only seat jobs routed to this seat (role), e.g. `advisor`.
        #[clap(long, value_name = "ROLE")]
        seat: Option<String>,
        /// Emit JSON instead of due-lines.
        #[clap(long)]
        json: bool,
    },

    /// Report a seat job as done: ledgers who/when on the aida-store branch
    /// and clears the due flag.
    Done {
        /// Configured job name.
        job: String,
        /// Short note recorded with the run.
        #[clap(long)]
        note: Option<String>,
    },

    /// Print crontab lines for enabled substrate jobs with an `every`.
    EmitCron,

    /// Install the crontab entry that drives `aida schedule tick` for this
    /// repo every 15 minutes (Linux/macOS only; idempotent — safe to re-run).
    /// This is what `aida init`'s TTY prompt calls, and the fix hint `aida
    /// doctor` prints when it finds registered jobs but no installed driver.
    // trace:STORY-1463 | ai:claude
    InstallCron,

    /// Remove this repo's crontab entry installed by `install-cron` (or the
    /// `aida init` prompt). Idempotent — a no-op when nothing is installed;
    /// never touches another repo's entry.
    // trace:STORY-1463 | ai:claude
    UninstallCron,

    /// Install a systemd user timer that runs `aida schedule tick` for this
    /// repo (Linux only; idempotent — safe to re-run, and it repairs an older
    /// unit). The timer is enabled and verified first; only then is this
    /// repo's crontab entry removed. Needs a human at an interactive terminal
    /// and an explicit yes. `install-cron` likewise removes this timer.
    // trace:TASK-1491 | ai:claude
    InstallSystemd,

    /// Disable and delete this repo's systemd user timer. Only the timer is
    /// disabled: a tick already running finishes. Unit files that aida did
    /// not write are left alone. Idempotent.
    // trace:TASK-1491 | ai:claude
    UninstallSystemd,
}
