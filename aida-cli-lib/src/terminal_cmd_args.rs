//! `aida terminal_cmd` clap types, owned by `terminal_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Terminal integrations that install user-local helper files.
// trace:STORY-995 | ai:codex
#[derive(Subcommand, Debug)]
pub enum TerminalCommand {
    /// Install the AIDA Terminator plugin.
    Install {
        /// Helper to install. Currently only `terminator` is supported.
        target: String,
    },
}
