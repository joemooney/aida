//! Read-only flow report arguments. trace:STORY-2699 | ai:codex
use clap::{Parser, Subcommand};
#[derive(Parser, Debug)]
#[command(
    name = "flow",
    about = "Measure landed PR flow from forge and gate-ledger evidence"
)]
pub(crate) struct FlowArgs {
    #[command(subcommand)]
    pub command: FlowCommand,
}
#[derive(Subcommand, Debug)]
pub(crate) enum FlowCommand {
    /// Report PRs landed on origin/main in a time window (GitHub).
    Report {
        /// Inclusive RFC3339 timestamp or YYYY-MM-DD (UTC).
        #[arg(long)]
        since: String,
        /// Inclusive RFC3339 timestamp or YYYY-MM-DD (UTC); defaults to now.
        #[arg(long)]
        until: Option<String>,
        /// Emit the report as JSON (same as --format json).
        #[arg(long)]
        json: bool,
    },
}
