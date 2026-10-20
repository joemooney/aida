//! `aida lock_cmd` clap types, owned by `lock_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Advisor-directed worktree lock.
///
/// Binds a worktree to an authorizing advisor by stamping `authorized_by` on
/// the session lease that covers it (it reuses the existing lease registry —
/// no new lock directory). The lock is a manual check: nothing refuses
/// automatically on a mismatched lock — an agent (or a script) opts in by
/// running `aida lock verify` itself and branching on the exit code.
// trace:STORY-711 | ai:claude
// trace:BUG-637 | ai:claude
#[derive(Subcommand, Debug)]
pub enum LockCommand {
    /// Authorize a worktree: stamp `authorized_by = <advisor>` on the session
    /// lease covering it. Finds the existing lease for the worktree (from
    /// `aida session start` / `aida worktree enter` / `aida agent new`) and
    /// patches it in place; if no lease covers the worktree yet, creates a
    /// minimal lock-only lease entry so `aida lock verify`/`status` have
    /// something to read. Re-running re-authorizes (overwrites any prior
    /// `authorized_by`).
    Acquire {
        /// Worktree path to lock.
        #[clap(value_name = "WORKTREE")]
        worktree: PathBuf,

        /// The authorizing advisor's session/agent id.
        #[clap(long)]
        advisor: String,
    },

    /// Verify a worktree's lock against a token and print the verdict.
    /// Exits **0** on `Unlocked` or `Authorized`, **non-zero** on `Refused` —
    /// scriptable as the manual bouncer (`aida lock verify <wt> --as <id> ||
    /// refuse-to-proceed`).
    Verify {
        /// Worktree to check. Defaults to the current directory.
        #[clap(value_name = "WORKTREE")]
        worktree: Option<PathBuf>,

        /// The calling agent's authorization token, to compare against the
        /// worktree's `authorized_by` lock (if any). Omit to check as an
        /// unauthenticated caller (fails safe: refused under a live lock).
        #[clap(long)]
        r#as: Option<String>,
    },

    /// Clear the `authorized_by` lock on a worktree (no-op if it carries
    /// none). Leaves the underlying session lease intact; only removes the
    /// lock-only entry this command itself created when nothing else is on
    /// it.
    Release {
        /// Worktree to unlock.
        #[clap(value_name = "WORKTREE")]
        worktree: PathBuf,
    },

    /// List every worktree in this project that currently carries an
    /// `authorized_by` lock.
    Status,
}
