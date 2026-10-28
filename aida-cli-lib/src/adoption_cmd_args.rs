//! `aida adoption` clap types, owned by `adoption_cmd.rs`.
// trace:STORY-1741 | ai:claude

use clap::{Parser, Subcommand};

/// Adopt AIDA without product-branch files: shared settings and guidance
/// live in `adoption/` on the aida-store branch and are projected into
/// this worktree's git-ignored `.aida/`.
#[derive(Parser, Debug)]
#[clap(name = "adoption")]
pub(crate) struct AdoptionArgs {
    #[clap(subcommand)]
    pub inner: AdoptionCommand,
}

#[derive(Subcommand, Debug)]
pub(crate) enum AdoptionCommand {
    /// Show which plane revision this worktree's projection holds.
    Status,
    /// Project the current plane revision into this worktree.
    Refresh,
    /// For CI and agent environments: project the plane only if it is at
    /// exactly `--revision`, then verify the projection before use.
    Bootstrap {
        /// The full commit id of the plane revision that was reviewed.
        #[clap(long, value_name = "COMMIT")]
        revision: String,
    },
    /// Dry-run migration: list the AIDA files tracked on the product
    /// branch, what can move to the plane, and what could be removed.
    /// Writes nothing.
    Inventory,
    /// Remove this worktree's projection and the AIDA git hooks this
    /// build installed. Keeps the store, the plane and every product file.
    Uninstall {
        /// Show what would be removed without removing anything.
        #[clap(long)]
        dry_run: bool,
    },
}
