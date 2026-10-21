//! `aida server_cmd` clap types, owned by `server_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum ServerCommand {
    /// Check server status
    Status,

    /// List requirements from server
    List {
        /// Filter by status (exact match on a single stored status; the
        /// server-side filter accepts no comma sets, aliases, or lenses)
        // trace:BUG-1771 | ai:claude — the REST filter is an exact match on
        // the Debug-formatted status, so this surface must not advertise the
        // local list command's set/alias/lens syntax.
        #[clap(long)]
        status: Option<String>,

        /// Filter by feature
        #[clap(long)]
        feature: Option<String>,

        /// Limit results
        #[clap(long, default_value = "100")]
        limit: i32,
    },

    /// Get a requirement from server
    Get {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
    },

    /// Ping server to check connectivity
    Ping,
}
