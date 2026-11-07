//! Clap types for `aida own` (STORY-2325).
// trace:STORY-2325 | ai:claude

/// `aida own` subcommands.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum OwnCommand {
    /// Show the ownership records: who holds each piece of work, the phase
    /// it is in, and whether its holder is still running. Read-only.
    Show {
        /// Only records whose key contains this text (for example `PR-12`).
        key: Option<String>,

        /// Also compare every session lease with its record: today's
        /// liveness reading beside the record's, and count where they differ.
        #[clap(long)]
        compare: bool,

        /// Emit JSON.
        #[clap(long)]
        json: bool,
    },

    /// Release an exact ownership key (for example train/origin-12).
    /// Requires a human at a host terminal. Journals the override and leaves
    /// all leases, worktrees and cargo targets for guarded session teardown.
    Release { key: String },

    /// One-time rewrite of the existing session, review and train records
    /// into ownership records. Only adds records; never changes, archives or
    /// deletes an existing record, worktree or branch.
    Migrate {
        /// Plan and report; write nothing.
        #[clap(long, conflicts_with = "execute", required_unless_present = "execute")]
        dry_run: bool,

        /// Write the records. Needs an interactive terminal in the host
        /// shell, outside any sandbox, with no drain running.
        #[clap(long)]
        execute: bool,

        /// Emit JSON.
        #[clap(long)]
        json: bool,
    },
}
