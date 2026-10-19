//! `aida load_cmd` clap types, owned by `load_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Quantitative effort/load views over `.aida/effort-calibration/`.
// trace:STORY-451 | ai:codex
#[derive(Subcommand, Debug, Clone)]
pub enum LoadCommand {
    /// Sum latest effort estimates for queued items.
    Queue,
    /// Sum latest effort estimates for approved/planned/draft backlog items
    /// that are not currently queued.
    Backlog,
    /// Queue + backlog + in-flight effort summary.
    Report,
    /// Estimate-vs-actual effort deltas. `1d` is 8 work-hours; `1w`
    /// is 5 work-days / 40 work-hours.
    Calibration {
        /// Restrict to records since this point: a relative window (`7d`,
        /// `12h`, `2w`, `30m`, or `24 hours ago`), an ISO date (`2026-05-01`,
        /// local midnight), a zone-less ISO datetime (local time), or RFC3339.
        // trace:TASK-1509 | ai:claude
        #[clap(long, value_name = "WINDOW")]
        since: Option<String>,
        /// Group deltas by requirement type.
        #[clap(long)]
        by_type: bool,
        /// Emit machine-readable JSON.
        #[clap(long)]
        json: bool,
    },
}
