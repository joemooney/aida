//! `aida upgrade_cmd` clap types, owned by `upgrade_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum UpgradeCommand {
    /// Compare current/sibling versions to latest release.
    // trace:STORY-1028 | ai:codex
    Check,
    /// Print unreleased commit diff from a developer build.
    // trace:STORY-1028 | ai:codex
    Diff,
}
