//! `aida cache_cmd` clap types, owned by `cache_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Commands for the SQLite read-cache that projects the git-canonical
/// store for fast list/filter/search queries.
// trace:EPIC-1-001 | ai:claude
// trace:TASK-487 | ai:claude
#[derive(Subcommand, Debug)]
pub enum CacheCommand {
    /// Force a full rebuild of the cache from the git store
    // trace:TASK-1507 | ai:claude
    Rebuild {
        /// Rebuild the history index (used by `aida history`) instead of the requirements cache
        #[clap(long)]
        history: bool,
    },

    /// Show cache state (HEAD comparison, requirement count, last build time)
    Status,

    /// Bring the cache current now; worker/scheduler forms drive the detached refresh protocol
    // trace:TASK-1527 | ai:claude
    Refresh {
        /// Catch up the existing history index inline, without refreshing the requirements cache
        // trace:STORY-1661 | ai:codex
        #[clap(long, conflicts_with_all = ["worker", "if_requested", "cache"])]
        history: bool,

        /// Maintenance tick: catch up the existing history index only when it is behind
        #[clap(long, requires = "history")]
        if_behind: bool,

        /// Run as the detached refresh worker (internal; spawned automatically)
        #[clap(long, hide = true)]
        worker: bool,

        /// Refresh only when a pending refresh request exists (for `aida schedule` jobs)
        #[clap(long)]
        if_requested: bool,

        /// Explicit store root for the worker (internal; never inferred from the cwd)
        #[clap(long, hide = true, value_name = "PATH")]
        store: Option<std::path::PathBuf>,

        /// Explicit cache database path for the worker (internal)
        #[clap(long, hide = true, value_name = "PATH")]
        cache: Option<std::path::PathBuf>,
    },

    /// Cross-check every cached status against the git store and report drift
    // trace:BUG-771 | ai:claude
    Verify {
        /// Rebuild the cache and re-check when drift is found
        #[clap(long)]
        fix: bool,

        /// Emit the divergence report as JSON
        #[clap(long)]
        json: bool,
    },
}
