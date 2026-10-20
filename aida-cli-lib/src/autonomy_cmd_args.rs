//! `aida autonomy_cmd` clap types, owned by `autonomy_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Autonomy overview, mode ladder, and maturity/calibration views.
// trace:STORY-439 trace:TASK-340 | ai:claude
#[derive(Subcommand, Debug, Clone)]
pub enum AutonomyCommand {
    /// Calibration views over `.aida/complexity-calibration/`.
    // trace:STORY-439 | ai:claude
    #[clap(subcommand)]
    Calibration(CalibrationSubcommand),
    /// Human-intervention maturity report: how many times drains had to stop
    /// and ask a human, rolled up per day so the trend is readable. The
    /// count trending toward zero as the autonomy machinery matures is the
    /// honest maturity signal.
    // trace:TASK-340 | ai:claude
    Report {
        /// Cap the number of dated rows printed (newest first). Default 30.
        #[clap(long, value_name = "N", default_value_t = 30)]
        last: usize,
        /// Emit a machine-readable JSON object instead of the human table.
        #[clap(long)]
        json: bool,
    },
}

/// Subcommands under `aida autonomy calibration`.
// trace:STORY-439 | ai:claude — plain `//` keeps the marker out of `--help`.
#[derive(Subcommand, Debug, Clone)]
pub enum CalibrationSubcommand {
    /// Surface specs where pickup-predicted complexity diverged most
    /// from reviewer-assessed implementation complexity. The
    /// substrate-gap signal — a class of work the agents systematically
    /// misjudge. Records ranked by biggest gap first; ties broken by
    /// recency.
    // trace:STORY-439 | ai:claude
    Mismatches {
        /// Restrict to records since this point: a relative window (`7d`,
        /// `12h`, `2w`, `30m`, or `24 hours ago`), an ISO date (`2026-05-01`,
        /// local midnight), a zone-less ISO datetime (local time), or
        /// RFC3339. Filters by the newest timestamp across the three slots.
        // trace:TASK-1509 | ai:claude
        #[clap(long, value_name = "WINDOW")]
        since: Option<String>,
        /// Cap the rows printed. Default 50.
        #[clap(long, value_name = "N", default_value_t = 50)]
        last: usize,
        /// Emit a machine-readable JSON array instead of the human table.
        #[clap(long)]
        json: bool,
    },
}
