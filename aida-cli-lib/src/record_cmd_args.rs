//! `aida record_cmd` clap types, owned by `record_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Inspect / prune the durable processing-record audit trail.
// trace:STORY-582 | ai:claude
#[derive(Subcommand, Debug)]
pub enum RecordCommand {
    /// List the processing records for one spec (or every spec carrying any,
    /// when no id is given).
    List {
        /// Spec id to inspect. Omit to walk every spec with a record.
        spec: Option<String>,
    },

    /// Trim processing records, keeping the spec. Records can be pruned later
    /// without losing the spec or its history. Propose-by-default — pass
    /// `--apply` to write.
    Prune {
        /// Restrict to one spec. Omit to sweep every spec.
        #[clap(long)]
        spec: Option<String>,

        /// Drop records older than this many days (e.g. `90`). When omitted,
        /// every record on the matched spec(s) is pruned.
        #[clap(long)]
        older_than: Option<u64>,

        /// Actually write the prune. Without it, the command only reports what
        /// it would remove (propose-by-default).
        #[clap(long)]
        apply: bool,
    },
}
