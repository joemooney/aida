//! Clap types for `aida review draft`.
// trace:BUG-2669 | ai:claude
use clap::Subcommand;

/// What `aida review draft` does.
#[derive(Subcommand, Debug)]
pub enum ReviewDraftCommand {
    /// List this project's registered reviewer draft handoffs.
    ///
    /// Each row names the pull request and head the reviewer was launched
    /// for, the receipt's state, whether the launcher sealed it, and which
    /// files the draft directory holds now. Read-only.
    List {
        /// Include handoffs registered for every project on this host.
        #[clap(long)]
        all: bool,

        /// Emit the handoffs as JSON.
        #[clap(long)]
        json: bool,
    },
}
