//! `aida land` clap types, owned by `land.rs`.
// trace:STORY-2768 | ai:claude

use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum LandCommand {
    /// Print every open change's landing state and the one action it calls
    /// for. Read-only: nothing is dispatched, cleared or merged.
    Status,
}
