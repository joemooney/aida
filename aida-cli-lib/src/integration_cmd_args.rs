//! `aida integration` clap types, owned by `integration_cmd.rs` and
//! re-exported from `cli.rs`. trace:STORY-2737 | ai:claude

use clap::Subcommand;

/// Subcommands under `aida integration`: the integration-branch flow, where
/// work lands fast on `integration` and `main` is the batched checkpoint.
// trace:STORY-2737 | ai:claude
#[derive(Subcommand, Debug)]
pub enum IntegrationCommand {
    /// Tag the open specs whose completing commits are on `origin/integration`
    /// but not yet on `origin/main` with `landed-in-integration`, and remove
    /// the tag from specs that are closed or no longer ahead of `main`.
    /// Reads the local `origin/` refs; run `git fetch` first if they may be
    /// stale. Completion itself still happens when the commit reaches `main`.
    Tags {
        /// Print the tag changes without making them.
        #[arg(long)]
        dry_run: bool,
    },
}
