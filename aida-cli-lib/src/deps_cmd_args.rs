//! `aida deps_cmd` clap types, owned by `deps_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Dependency-inference tooling. Read-only: surfaces likely dependency
/// edges for human confirmation; it never writes the graph itself.
// trace:STORY-447 | ai:claude
#[derive(Subcommand, Debug)]
pub enum DepsCommand {
    /// List likely dependencies inferred from the trace graph, without
    /// writing anything. For each spec, ranks other specs that share
    /// trace-link files (≥2 shared files: high; 1: medium) and, weaker,
    /// share a parent. Confirm a real edge by hand with
    /// `aida edit <id> --blocked-by <dep>`.
    // trace:STORY-447 | ai:claude
    Sweep {
        /// Limit the sweep to a single source spec (SPEC-ID or UUID)
        /// instead of every spec in the store.
        #[clap(long, value_name = "SPEC")]
        for_spec: Option<String>,

        /// Emit the result as JSON for agents / scripts.
        #[clap(long)]
        json: bool,
    },
}
