//! `aida presence_cmd` clap types, owned by `presence_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Subcommands under `aida presence` — the canonical surface for the operator
/// presence state (collapsing the old top-level `away`/`home`/`presence` verbs
/// into one). Bare `aida presence` shows status. The top-level `aida away` /
/// `aida home` remain as hidden aliases for muscle memory.
// trace:TASK-851 | ai:claude
#[derive(Subcommand, Debug)]
pub enum PresenceCommand {
    /// Mark yourself away from the keyboard (sets the machine-global presence
    /// state with a TTL). Same as the hidden top-level `aida away`.
    // trace:TASK-851 | ai:claude
    Away,
    /// Mark yourself back at the keyboard (clears any away state). Same as the
    /// hidden top-level `aida home`.
    // trace:TASK-851 | ai:claude
    Home,
    /// Show current effective presence: home/away, how long ago it was set,
    /// and TTL-remaining when away. Same as bare `aida presence`.
    // trace:TASK-851 | ai:claude
    Status,
}
