//! `aida focus_cmd` clap types, owned by `focus_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum FocusCommand {
    /// Clear the current focus.
    // trace:STORY-1028 | ai:codex
    Clear,
    /// Show the current focus.
    // trace:STORY-1028 | ai:codex
    Show,
}
