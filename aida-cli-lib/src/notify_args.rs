//! `aida notify` clap types, owned by `notify.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum NotifyCommand {
    /// Evaluate configured notification rules and send due alerts.
    Check,

    /// Send a sample notification through the configured command.
    Test,

    /// Show configured rules and per-rule delivery state.
    Status,
}
