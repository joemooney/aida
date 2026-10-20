//! `aida orchestrator_cmd` clap types, owned by `orchestrator_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Orchestrator-run introspection. The `--auto-complete` orchestrator passes a
/// corroboration token to its phase children; these subcommands let a child —
/// or a skill running inside one — verify whether it is a *genuine* orchestrator
/// child rather than guessing from a bare `AIDA_AUTO_COMPLETE` env var.
// trace:BUG-233 | ai:claude
#[derive(Subcommand, Debug)]
pub enum OrchestratorCommand {
    /// Print the corroborated orchestrator context of the current process:
    /// `orchestrated` when `AIDA_AUTO_COMPLETE` is set AND its
    /// `AIDA_AUTO_COMPLETE_TOKEN` names a live orchestrator run, else
    /// `interactive`. Skills branch their orchestrator-aware behavior off this
    /// word instead of trusting the bare env var.
    Status {
        /// Emit `{"context","corroborated","reason"}` JSON instead of the
        /// bare status word.
        #[clap(long)]
        json: bool,
    },
}
