//! `aida solo_cmd` clap types, owned by `solo_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// The canonical `aida solo <action>` verbs. The legacy
/// `--watch`/`--off`/`--status` flags map onto these as silent aliases.
// trace:STORY-627 | ai:claude
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoloAction {
    /// Start the solo loop in the foreground (== legacy `--watch`).
    Run,
    /// Stop a running solo loop (== legacy `--off`).
    Stop,
    /// Print whether a solo loop / solo mode is active (== legacy `--status`).
    Status,
}
