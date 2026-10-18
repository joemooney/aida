//! `aida supervise_cmd` clap types, owned by `supervise_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Lightweight supervisor helpers for product/advisor stop-gap loops.
// trace:STORY-1052 | ai:codex
#[derive(Subcommand, Debug)]
pub enum SuperviseCommand {
    /// Nudge a live advisor about transiently parked work, or fall back to
    /// operator notification when no advisor is live.
    Nudge,
    /// Re-drive transiently-parked specs (typed tooling causes) on a capped,
    /// backed-off loop; leave genuine needs-human parks for escalation. Lets an
    /// unattended drain self-recover. Reads parks from the store and its attempt
    /// history from the event log, so it recovers a park regardless of which
    /// drain parked it or whether that drain crashed.
    // trace:STORY-1051 | ai:claude
    Redrive {
        /// Actually re-drive (default is a dry-run report of what it WOULD do).
        #[clap(long)]
        execute: bool,
        /// Re-drive cap before reclassifying to needs-human (default 3).
        #[clap(long)]
        max_attempts: Option<u32>,
        /// Re-drive at most N specs in this run.
        #[clap(long)]
        max: Option<usize>,
        #[clap(long)]
        json: bool,
    },
    /// Watch a named objective and keep work aligned to it: realign the queue
    /// toward the objective's ready children, fire the redrive + nudge reflexes,
    /// and surface only the items that need a human. Reads the substrate
    /// (events + store + queue) so it is safe to run with no agent awake; it
    /// never drives or merges. Default is a dry-run report; pass --execute to act.
    ///
    /// The redrive reflex is the night shift's re-drive step. It is off unless
    /// this clone turns it on (`redrive = true` for this repo in
    /// ~/.aida/shift-local.toml), keeps the shift's floors, and only puts
    /// transient parks back in the queue for the next drain wave. An --execute
    /// pass holds the night-shift lock while it re-drives, and a shift tick that
    /// fires during that moment is skipped; avoid very short --interval values.
    // trace:STORY-1096 trace:BUG-1621 trace:BUG-1623 | ai:claude
    Watch {
        /// The objective to keep work aligned to (an epic id). When omitted,
        /// falls back to the `[oversight] objective` config value.
        #[clap(long)]
        objective: Option<String>,
        /// Act instead of reporting: realign the queue, re-drive transient
        /// parks (when re-drive is on for this clone), and nudge the advisor or
        /// operator about stuck work. Default is a dry-run report of what it
        /// WOULD do.
        // trace:BUG-1623 | ai:claude
        #[clap(long)]
        execute: bool,
        /// Repeat the pass every N seconds (default: a single pass then exit).
        #[clap(long)]
        interval: Option<u64>,
        #[clap(long)]
        json: bool,
    },
}
