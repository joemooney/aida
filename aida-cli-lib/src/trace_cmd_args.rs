//! `aida trace_cmd` clap types, owned by `trace_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Commands for code-to-requirement traceability
#[derive(Subcommand, Debug)]
pub enum TraceCommand {
    /// Add a trace link from code to a requirement
    Add {
        /// Requirement ID (UUID or SPEC-ID)
        #[clap(long)]
        req: String,

        /// Path to the source file
        #[clap(long)]
        file: String,

        /// Symbol name (function, struct, module, etc.)
        #[clap(long)]
        symbol: Option<String>,

        /// Starting line number
        #[clap(long)]
        line_start: Option<u32>,

        /// Ending line number
        #[clap(long)]
        line_end: Option<u32>,

        /// Artifact type: source, test, config, doc
        #[clap(long, short = 't', default_value = "source")]
        r#type: String,

        /// Notes about this trace link
        #[clap(long)]
        notes: Option<String>,

        /// Git commit hash where this was implemented
        #[clap(long)]
        commit: Option<String>,
    },

    /// List trace links for a requirement or file
    List {
        /// Requirement ID (UUID or SPEC-ID) - lists all trace links for this requirement
        #[clap(long)]
        req: Option<String>,

        /// File path - lists all trace links for this file
        #[clap(long)]
        file: Option<String>,
    },

    /// Remove a trace link
    Remove {
        /// Requirement ID (UUID or SPEC-ID)
        #[clap(long)]
        req: String,

        /// Trace link ID to remove
        #[clap(long)]
        link_id: String,
    },

    /// Scan source files for AIDA trace annotations
    Scan {
        /// Path to scan (file or directory, defaults to current directory)
        path: Option<String>,

        /// File extensions to scan (comma-separated, e.g., "rs,py,ts")
        #[clap(long, default_value = "rs")]
        extensions: String,

        /// Add discovered trace links to requirements database
        #[clap(long)]
        update: bool,

        /// Show verbose output
        #[clap(long, short = 'v')]
        verbose: bool,
    },

    /// Sweep git commits for requirement references
    Sweep {
        /// Number of commits to scan (default: all)
        #[clap(long)]
        limit: Option<u32>,

        /// Branch to scan (default: current)
        #[clap(long)]
        branch: Option<String>,

        /// Only show commits, don't update database
        #[clap(long)]
        dry_run: bool,

        /// Show verbose output
        #[clap(long, short = 'v')]
        verbose: bool,
    },

    /// CI spec-id validity gate: walk a commit range, resolve every
    /// `(SPEC-ID)` subject trailer against the live requirement graph, and
    /// exit non-zero when any commit references a SPEC-ID that does not exist
    /// or is rejected (a dead/dangling provenance link). Mechanical/release
    /// commits with no trailer are exempt; plan commits are skipped.
    Gate {
        /// Git revision range to scan (e.g. `origin/main..HEAD`). Defaults to
        /// `<default-branch>..HEAD` (the commits this branch adds), falling
        /// back to `HEAD~20..HEAD` when no default branch resolves.
        #[clap(long)]
        range: Option<String>,

        /// Emit machine-readable JSON instead of human text.
        #[clap(long)]
        json: bool,
    },

    /// CI trace-COVERAGE check: walk a diff and report which changed source
    /// hunks carry the required code-to-spec provenance (an inline trace
    /// comment OR a live commit `(SPEC-ID)` trailer), per the deterministic
    /// coverage definition + exemptions (tests / generated / docs / config /
    /// vendored / pure-deletion / fmt-only / trivial). Distinct from `gate`,
    /// which validates the trailer spec-ids; this checks that the changed CODE
    /// is traced. Report-only by default (CI succeeds); `--block` fails CI on
    /// any uncovered coverable hunk.
    Coverage {
        /// Git revision range to scan (e.g. `origin/main..HEAD`). Defaults to
        /// `<default-branch>..HEAD` (the commits this branch adds), falling
        /// back to `HEAD~20..HEAD` when no default branch resolves.
        #[clap(long)]
        range: Option<String>,

        /// Emit machine-readable JSON instead of human text.
        #[clap(long)]
        json: bool,

        /// Fail (exit non-zero) when any coverable changed hunk is uncovered.
        /// Default is report-only: the report prints but CI succeeds.
        #[clap(long)]
        block: bool,
    },

    /// Trace-ROT detector: scan source for inline code-to-spec trace markers,
    /// resolve each referenced id against the live requirement graph, and flag
    /// dangling traces — markers whose target no longer exists (deleted,
    /// renumbered) or resolves to a rejected spec. Reports total / resolved /
    /// dangling + a rot rate. This makes a stale trace go "red" like a failing
    /// type — the counterpart to `gate` (validates commit trailers) and
    /// `coverage` (checks the code is traced). Report-only by default (exit 0);
    /// `--block` exits non-zero when any dangling trace exists, for CI gating.
    Check {
        /// Path to scan (file or directory). Defaults to the project root.
        path: Option<String>,

        /// Emit machine-readable JSON instead of human text.
        #[clap(long)]
        json: bool,

        /// Fail (exit non-zero) when any dangling trace exists. Default is
        /// report-only: the report prints but the command succeeds.
        #[clap(long)]
        block: bool,
    },
}
