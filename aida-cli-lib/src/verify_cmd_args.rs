//! `aida verify_cmd` clap types, owned by `verify_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// The closed, explicit host-verification command group.
// trace:STORY-1673 | ai:codex
#[derive(Subcommand, Debug)]
pub enum VerifyCommand {
    /// Compare constraint verify blocks with this Linux host. Read-only.
    Constraints {
        /// Emit one match/drift/unknown/invalid result per configured row.
        #[clap(long)]
        json: bool,
        /// Return a nonzero exit only when a row reports drift.
        #[clap(long)]
        fail_on_drift: bool,
        /// With --fail-on-drift, also fail when a row is unknown (host data
        /// unavailable) or invalid (bad block), so a CI gate cannot pass on a
        /// host that could not check anything.
        // trace:TASK-1954 | ai:claude  trace:TASK-1796 | ai:claude
        #[clap(long, requires = "fail_on_drift")]
        fail_on_unknown: bool,
        /// Also list rejected, superseded and archived constraints, which are
        /// never probed. JSON output always includes them.
        // trace:STORY-1673 | ai:claude
        #[clap(long, short = 'v')]
        verbose: bool,
    },
}
