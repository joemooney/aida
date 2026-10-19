//! `aida triage_cmd` clap types, owned by `triage_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Per-scope disposition / triage lease commands (the intake gate).
// trace:TASK-661 | ai:claude
#[derive(Subcommand, Debug)]
pub enum TriageCommand {
    /// Take the disposition lease for a scope before disposing drafts in it.
    /// Refused (naming the holder) if a live advisor already holds the scope.
    /// Idempotent for the same process re-acquiring. Default scope is the
    /// whole project.
    Acquire {
        /// Scope to dispose (e.g. a subsystem name). Omit for the whole
        /// project.
        #[clap(long)]
        scope: Option<String>,
        /// Override the owner id recorded on the lease (defaults to the
        /// shell user identity, same resolution as the queue).
        #[clap(long)]
        user: Option<String>,
    },
    /// Release the disposition lease for a scope you hold (no-op if you don't
    /// hold it).
    Release {
        /// Scope to release. Omit for the whole project.
        #[clap(long)]
        scope: Option<String>,
        /// Override the owner id (defaults to the shell user identity).
        #[clap(long)]
        user: Option<String>,
    },
    /// Show the live disposition leases for this project (dead-holder leases
    /// are reaped on read). `--json` for machine consumers.
    Status {
        /// Emit JSON instead of a table.
        #[clap(long)]
        json: bool,
    },
}
