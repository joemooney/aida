//! `aida upgrade_release` clap types, owned by `upgrade_release.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum ReleaseCommand {
    /// Bump the patch version.
    // trace:STORY-1028 | ai:codex
    Patch,
    /// Bump the minor version.
    // trace:STORY-1028 | ai:codex
    Minor,
    /// Bump the major version.
    // trace:STORY-1028 | ai:codex
    Major,
    /// Preview the planned release without acting.
    // trace:STORY-1028 | ai:codex
    Check,
}
