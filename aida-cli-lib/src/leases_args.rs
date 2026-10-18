//! `aida leases` clap types, owned by `leases.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use crate::cli::Command;
use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Fasttrack-lane introspection subcommands.
///
/// The bare `aida fasttrack <title>` filing form lives on the parent
/// `Command::Fasttrack` variant; this enum holds the read-only lane views that
/// hang off it.
// trace:TASK-905 | ai:claude — plain `//` keeps the marker out of `--help`.
#[derive(Subcommand, Debug)]
pub enum FasttrackCommand {
    /// Show each lane item's stage: requested to shipped.
    ///
    /// A derived projection over the current fasttrack bucket plus legacy
    /// fasttrack tags — no new store. Each item's stage
    /// (requested / accepted / queued / running / blocked / punted / shipped /
    /// rejected) is read off its existing status, queue membership, active
    /// lease, punt ledger, and merged state. Cache-fast: it reuses the same
    /// summaries `aida list` reads, not the full-store `aida status` scan.
    Status {
        /// Emit the projection as JSON for machine consumers.
        #[clap(long)]
        json: bool,
    },
}
