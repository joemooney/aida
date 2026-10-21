//! `aida fleet` clap types, owned by `mandate_drive.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::Subcommand;

/// `aida fleet` — fleet operations.
// trace:STORY-1700 | ai:claude
#[derive(Subcommand, Debug)]
pub enum FleetAction {
    /// The drive-mandate tick (scheduled every 15m while a drive mandate is
    /// active). Without `--tick`, shows what a tick would route, changing
    /// nothing.
    Drive {
        /// Run one tick: route stalled lanes and ledger every action.
        #[clap(long)]
        tick: bool,
        /// Print the tick report as JSON.
        #[clap(long)]
        json: bool,
    },
}
