//! `aida usage_cmd` clap types, owned by `usage_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum UsageCommand {
    /// Rebuild the local per-message token ledger from retained vendor artifacts.
    // trace:TASK-1427 | ai:codex
    Rebuild {
        /// Project whose local derived ledger is rebuilt (defaults to current project).
        #[clap(long, value_name = "PATH")]
        project: Option<std::path::PathBuf>,
        /// Emit the rebuild coverage summary as JSON.
        #[clap(long)]
        json: bool,
    },
    /// Show measured token usage and attribution coverage for one spec.
    // trace:TASK-1427 | ai:codex
    Show {
        #[clap(value_name = "SPEC-ID")]
        spec: String,
        /// Comma-separated dimensions: phase,vendor,model,round.
        #[clap(long, default_value = "phase,vendor,round")]
        group_by: String,
        /// Add offline local cost estimates from the pinned dated rate table.
        // trace:TASK-1434 | ai:codex
        #[clap(long)]
        cost: bool,
        /// Per-write author/session/harness context with session usage at write time.
        // trace:STORY-1674 | ai:codex
        #[clap(long, conflicts_with = "cost")]
        provenance: bool,
        #[clap(long)]
        json: bool,
        #[clap(long, conflicts_with = "json")]
        toon: bool,
    },
    /// Rank command shapes by latency, slowest first.
    // trace:STORY-1028 | ai:codex
    Slowest,
    /// Show commands not used since this point: a relative duration (`30d`,
    /// `2w`, or `24 hours ago`), an ISO date (local midnight), a zone-less
    /// ISO datetime (local time), or RFC3339.
    // trace:STORY-1028 | ai:codex
    // trace:TASK-1509 | ai:claude
    Unused {
        #[clap(value_name = "DURATION")]
        duration: String,
    },
    /// Show commands with the highest error rate.
    // trace:STORY-1028 | ai:codex
    Errors,
    /// Show the raw recent usage event stream.
    // trace:STORY-1028 | ai:codex
    Events,
    /// Compact one-line-per-invocation timeline: local timestamp, duration,
    /// command shape, and exit status/failure mark — newest first. The
    /// scan-first companion to `events` (which prints the full raw fields);
    /// use this to eyeball what ran immediately before a slow command.
    /// Honors --since/--cmd/--slower-than/--limit/--json like `events`.
    // trace:TASK-1481 | ai:claude
    Timeline,
    /// Show autonomous drain telemetry.
    // trace:STORY-1028 | ai:codex
    Drains {
        /// List every recent orchestrator failure in full.
        #[clap(long)]
        failures: bool,
        /// Show which phases fail most often.
        #[clap(long, conflicts_with = "failures")]
        pattern: bool,
    },
    /// Show the deterministic project-health telemetry catalog.
    // trace:STORY-1028 | ai:codex
    Health,
}
