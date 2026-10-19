//! `aida rules_cmd` clap types, owned by `rules_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Claude Code path-gated rules sync.
// trace:SPIKE-31 | ai:claude
#[derive(Subcommand, Debug)]
pub enum RulesCommand {
    /// Reconcile `.claude/rules/aida-specs/` against the current spec
    /// graph: write a rule for every active spec with trace comments,
    /// remove rules whose spec is no longer active.
    Sync {
        /// Compute what would change without touching disk.
        #[clap(long)]
        dry_run: bool,

        // trace:SPIKE-35 | ai:claude
        /// Also emit a project-root `REVIEW.md` that
        /// aggregates every active spec's acceptance criteria as
        /// the highest-priority injection into Anthropic's managed
        /// Code Review pipeline. One file, committed.
        // trace:SPIKE-35 | ai:claude
        #[clap(long = "review-md")]
        review_md: bool,
    },
}
