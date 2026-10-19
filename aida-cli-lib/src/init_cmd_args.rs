//! `aida init_cmd` clap types, owned by `init_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

// trace:STORY-830 | ai:codex
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitFootprint {
    /// Full team-adoption scaffold: agent packs, docs, hooks, MCP config.
    Full,
    /// Minimal project footprint: git-canonical store, config, cache, gitignore.
    Minimal,
    /// Entry memory lane: store, config, cache, gitignore, reflex skills, guidance.
    // trace:STORY-1093 | ai:codex
    MemoryLane,
}
