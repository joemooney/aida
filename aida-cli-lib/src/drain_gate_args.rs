//! `aida drain_gate` clap types, owned by `drain_gate.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

// trace:TASK-394 | ai:claude
/// Persist the one-time `--no-human` scope acknowledgement as a
/// file marker so an unattended loop doesn't re-prompt per iteration. The
/// pre-flight gate checks the marker in addition to AIDA_NO_HUMAN_ACKNOWLEDGED.
#[derive(Subcommand, Debug, Clone)]
pub enum NoHumanCommand {
    /// Persist the acknowledgement so future `--no-human` drains skip the
    /// scope prompt. Machine-wide (`~/.aida/no-human-acknowledged`) by default;
    /// `--project` scopes it to this repo (`.aida/no-human-acknowledged`).
    Acknowledge {
        /// Scope the acknowledgement to this project instead of the machine.
        #[clap(long)]
        project: bool,
    },
    /// Remove the persistent acknowledgement (the scope prompt returns).
    Revoke {
        /// Revoke the project-scoped marker instead of the machine-wide one.
        #[clap(long)]
        project: bool,
    },
    /// Show whether `--no-human` is currently acknowledged and via which channel.
    Status,
}
