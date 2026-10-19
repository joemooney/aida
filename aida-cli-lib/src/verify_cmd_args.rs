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
        /// Also list rejected, superseded and archived constraints, which are
        /// never probed. JSON output always includes them.
        // trace:STORY-1673 | ai:claude
        #[clap(long, short = 'v')]
        verbose: bool,
    },
}
