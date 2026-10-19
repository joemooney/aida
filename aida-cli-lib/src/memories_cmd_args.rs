//! `aida memories_cmd` clap types, owned by `memories_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Starter-memory-pack substrate-drift discovery.
///
/// The opt-in memory pack (`aida init --with-memories`) ships generic
/// AIDA-using discipline as Claude Code project memories. The pack grows
/// over time inside the `aida` binary, but a project that scaffolded it
/// months ago has no way to learn it's behind — `--refresh` only helps if
/// you already KNOW to run it. These commands close that discoverability gap.
// trace:STORY-410 | ai:claude
#[derive(Subcommand, Debug)]
pub enum MemoriesCommand {
    /// Compare the local memory pack to this binary's embedded master and
    /// report drift (missing, stale, edited, up-to-date). Reads only — never
    /// writes. The fix it recommends is `aida init --with-memories --refresh`.
    // trace:STORY-410 | ai:claude
    Check {
        /// List every item in each category. Default summarizes (max 5 per
        /// category).
        #[clap(long, short = 'v')]
        verbose: bool,

        /// Emit a machine-readable JSON report instead of the text summary.
        #[clap(long)]
        json: bool,
    },
}
