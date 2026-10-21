//! `aida protocol_cmd` clap types, owned by `protocol_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Inspect the editable per-requirement-type work protocols.
// trace:STORY-1221 | ai:codex
#[derive(Subcommand, Debug)]
pub enum ProtocolCommand {
    /// List every protocol currently stored as META.
    List,
    /// Show one protocol by requirement type (for example `spike` or `bug`).
    Show {
        req_type: String,
        /// Overlay an optional lane protocol (research, docs, or keystone).
        #[clap(long)]
        lane: Option<String>,
    },
    /// Idempotently add any missing built-in type protocols to this store.
    Seed,
}
