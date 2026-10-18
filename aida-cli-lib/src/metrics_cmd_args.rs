//! `aida metrics_cmd` clap types, owned by `metrics_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Dogfood metrics over the recorded telemetry substrate.
///
/// A reporting layer — computes nothing the telemetry logs don't already
/// record.
// trace:STORY-477 | ai:claude
#[derive(Subcommand, Debug)]
pub enum MetricsCommand {
    /// Agent-lift signals: autonomous drain success rate, runs over distinct
    /// specs/builds, stale-base recoveries, and the autonomous-vs-human split.
    /// Reads `~/.aida/auto-complete.jsonl` + `~/.aida/usage.jsonl`.
    // trace:STORY-477 | ai:claude
    AgentLift {
        /// Report over this window: a relative duration (`30d`, `12h`, `2w`,
        /// `45m`, or `24 hours ago`), an ISO date (`2026-05-01`, local
        /// midnight), a zone-less ISO datetime (`2026-05-01T10:00`, local
        /// time), or RFC3339.
        // trace:STORY-477 | ai:claude
        // trace:TASK-1509 | ai:claude
        #[clap(long, value_name = "WINDOW", default_value = "30d")]
        since: String,
        /// Emit Markdown — suitable for pasting into release notes or a case
        /// study. Default is the colorized terminal view.
        // trace:STORY-477 | ai:claude
        #[clap(long)]
        markdown: bool,
        /// Emit a JSON object with the computed signals for machine consumers.
        // trace:STORY-477 | ai:claude
        #[clap(long, conflicts_with = "markdown")]
        json: bool,
    },
    /// Aggregate cycle-time breakdown across every requirement completed in
    /// the window: per-activity count/p50/p90/total, the overall work vs wait
    /// vs unknown split, rework signals (CI failure rate, reviewer bounces,
    /// implementer passes, shelves), and the slowest items with their
    /// dominant span. Each item's timeline is rebuilt the way `aida history
    /// <ID> --timeline` builds it, from the store's transitions and the local
    /// drain feed; forge timestamps are not fetched, so the view stays fast
    /// and offline. Items with no usable data are excluded and counted, with
    /// the reason. Renders human, TOON and JSON.
    // trace:STORY-1479 | ai:claude — plain `//` keeps the marker out of `--help`.
    CycleTime {
        /// Aggregate requirements completed since this point: a relative
        /// duration (`7d`, `12h`, `2w`, `45m`, or `24 hours ago`), an ISO
        /// date (`2026-05-01`, local midnight), a zone-less ISO datetime
        /// (`2026-05-01T10:00`, local time), or RFC3339.
        #[clap(long, value_name = "WINDOW", default_value = "7d")]
        since: String,
        /// Only requirements completed before this point. Same forms as
        /// `--since`; rejected if it resolves earlier than `--since`.
        #[clap(long, value_name = "WINDOW")]
        until: Option<String>,
        /// Only requirements of this type (story, task, bug, …).
        #[clap(long = "type", value_name = "TYPE")]
        req_type: Option<String>,
        /// Comma-separated tag filter; a requirement must carry every listed
        /// tag (case-insensitive).
        #[clap(long, value_name = "TAGS", value_delimiter = ',')]
        tags: Vec<String>,
        /// How many of the slowest requirements to list.
        #[clap(long, value_name = "N", default_value = "5")]
        slowest: usize,
        /// Emit a JSON object with the aggregate breakdown. `--format json`
        /// is the same.
        #[clap(long)]
        json: bool,
    },
    /// AI-lift signals over the git commit corpus: trailer coverage over time,
    /// tool attribution, and confidence-band sparsity.
    // trace:STORY-783 | ai:codex
    AiLift {
        /// Emit Markdown — suitable for pasting into release notes or a case
        /// study. Default is the colorized terminal view.
        // trace:STORY-783 | ai:codex
        #[clap(long)]
        markdown: bool,
        /// Emit a JSON object with aggregate-only computed signals.
        // trace:STORY-783 | ai:codex
        #[clap(long, conflicts_with = "markdown")]
        json: bool,
    },
}
