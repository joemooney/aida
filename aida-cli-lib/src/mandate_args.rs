//! `aida mandate` clap types, owned by `mandate.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// `aida mandate` — the fire-fighting mandate.
// trace:STORY-1698 | ai:claude
#[derive(Subcommand, Debug)]
pub enum MandateAction {
    /// Start a time-boxed mandate that lets the advisor seat ship held PRs
    /// with an attested, independent approval and green CI. Start and stop
    /// require a human at an interactive terminal; a same-uid process that
    /// forges files in the AIDA home is outside the threat model (the same
    /// as session grants).
    Start {
        /// Cover shipping held PRs (the only scope today; required).
        #[clap(long)]
        ship_held: bool,
        /// How long it lasts, e.g. `8h` or `90m` (default 8h, at most 24h).
        /// A mandate cannot be extended; stop it and start another.
        #[clap(long)]
        ttl: Option<String>,
        /// Why the mandate exists, recorded with it.
        #[clap(long)]
        reason: Option<String>,
        /// Also cover sensitive specs (security, containment, authority,
        /// guided, keystone). Operator-only specs are never covered.
        #[clap(long)]
        include_supervised: bool,
    },
    /// End the active mandate now. Human at an interactive terminal only.
    Stop,
    /// Show the current mandate, if any.
    Status {
        /// Print JSON.
        #[clap(long)]
        json: bool,
    },
}
