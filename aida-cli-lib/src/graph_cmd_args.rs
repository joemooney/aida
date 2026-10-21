//! `aida graph_cmd` clap types, owned by `graph_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum GraphCommand {
    /// Transitive BlockedBy chain for a spec.
    // trace:STORY-1028 | ai:codex
    BlockedBy { id: String },
    /// Transitive Blocks chain for a spec.
    // trace:STORY-1028 | ai:codex
    Blocks { id: String },
    /// Parent/Child descendants with a status rollup.
    // trace:STORY-1028 | ai:codex
    Tree { id: String },
    /// Reverse impact for a spec.
    // trace:STORY-1028 | ai:codex
    Impact { id: String },
}
