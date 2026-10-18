//! `aida wiki` clap types, owned by `wiki.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

// trace:EPIC-72 trace:TASK-1439 | ai:antigravity
#[derive(Subcommand, Clone, Debug, PartialEq, Eq)]
pub enum WikiCommand {
    /// Build static HTML living wiki projection from canonical specs and exposition sidecars
    Build {
        /// Output directory for generated wiki (defaults to .aida/wiki)
        #[clap(long)]
        out: Option<PathBuf>,
    },
    /// Serve the generated wiki on loopback only (127.0.0.1); it is never
    /// reachable from the network, and there is no host override.
    // trace:TASK-1470 | ai:claude
    Serve {
        /// Port to bind on 127.0.0.1 (defaults to 8420)
        #[clap(long, default_value = "8420")]
        port: u16,

        /// Path to wiki directory (defaults to .aida/wiki)
        #[clap(long)]
        dir: Option<PathBuf>,
    },
}
