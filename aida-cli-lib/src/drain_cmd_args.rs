//! `aida drain_cmd` clap types, owned by `drain_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Drain-state introspection. An `aida queue work --auto-complete`
/// orchestrator writes `.aida/drain-state.json` while a drain is live; this
/// subcommand reads it so a user inside the orchestrator-spawned session can
/// see what command launched the drain, whether it is a single-spec or a batch
/// drain, how far through it is, and what happens when they exit the session.
// trace:STORY-301 | ai:claude
// trace:TASK-487 | ai:claude
#[derive(Subcommand, Debug)]
pub enum DrainCommand {
    /// Start an autonomous drain through the canonical `queue work
    /// --auto-complete` engine.
    // trace:STORY-1130 | ai:codex
    Start {
        /// Optional selector: a SPEC id, `next`, `nextN`, or `batch:NAME`.
        selector: Option<String>,
        /// Resolve and run against this project instead of the current cwd.
        #[clap(long, value_name = "PATH")]
        project: Option<String>,
        /// Drain a named `batch:NAME`.
        #[clap(long, value_name = "NAME", conflicts_with = "selector")]
        batch: Option<String>,
        /// Drive exactly the queue head (`aida queue work next --auto-complete`).
        #[clap(long, conflicts_with = "selector")]
        once: bool,
        /// Headless mode for the canonical drain. Bare flag means `both`.
        #[clap(long, value_name = "MODE", num_args = 0..=1, default_missing_value = "both")]
        no_human: Option<String>,
        /// Override the drain role.
        #[clap(long, value_name = "NAME")]
        role: Option<String>,
        /// Per-batch failure budget passed through to the engine.
        #[clap(long, value_name = "N")]
        max_failures: Option<usize>,
        /// Stop after this many acted-on specs.
        #[clap(long, value_name = "N")]
        max_iterations: Option<u64>,
        /// Stop after this wall-clock budget (`90s`, `45m`, `2h`).
        #[clap(long, value_name = "DUR")]
        max_runtime: Option<String>,
        /// Cap cumulative reported tokens.
        #[clap(long, value_name = "N")]
        max_tokens: Option<u64>,
        /// Emit JSON progress from the underlying drain.
        #[clap(long)]
        json: bool,
        /// Route the headless agents to a vendor (`claude`, `codex`, `agy`).
        #[clap(long, visible_alias = "agent", value_name = "VENDOR")]
        vendor: Option<String>,
        /// Opaque model override passed through to the selected vendor.
        #[clap(long, value_name = "MODEL")]
        model: Option<String>,
    },

    /// Re-drive a deliberately shelved spec from the phase that failed.
    // trace:TASK-1272 | ai:codex
    Resume {
        /// Shelved requirement to resume.
        spec: String,
        /// Emit JSON progress from the underlying drain.
        #[clap(long)]
        json: bool,
    },

    /// Request that the live drain stop at the next spec boundary.
    // trace:STORY-1130 | ai:codex
    Stop {
        /// Kill the live drain process immediately after writing the stop request.
        #[clap(long)]
        now: bool,
        /// Resolve against this project instead of the current cwd.
        #[clap(long, value_name = "PATH")]
        project: Option<String>,
    },

    /// Show the active drain: the launching command, the members and their
    /// lifecycle state, the current phase, and a prediction of what happens on
    /// session exit. Launcher-held drains (`aida burndown run`, `aida queue
    /// integrate`) are reported from the live drain lock — pid, start time,
    /// and spec set — even though they write no per-phase progress file.
    /// Prints `No drain in progress.` (exit 0) when none is running. A
    /// drain-state file whose orchestrator process is no longer alive is
    /// reported as stale — `--clear` removes it.
    // trace:BUG-759 | ai:claude
    Status {
        /// Emit the drain state as JSON instead of the human summary.
        #[clap(long)]
        json: bool,
        /// Remove a stale drain-state file (one whose orchestrator process is
        /// no longer running). Refuses while the drain is still live — a live
        /// orchestrator removes the file itself on exit.
        #[clap(long, hide = true)]
        clear: bool,
    },

    /// Remove a stale drain-state file.
    // trace:STORY-1028 | ai:codex
    Clear,

    /// Stream the active drain log, using the same resolver and renderer as
    /// `aida tail drain`.
    // trace:TASK-1209 | ai:codex
    Tail {
        /// Pass the raw stream-json events through instead of rendering them.
        #[clap(long)]
        json: bool,
        /// Start from at most the last N lines of what is already in the log.
        #[clap(long, short = 'n', value_name = "N")]
        lines: Option<usize>,
        /// Skip anything older than this: a relative duration (`30s`, `10m`,
        /// `2h`, `1d`, `2w`, or `24 hours ago`; a bare integer means seconds),
        /// an ISO date (`2026-05-01`, local midnight), a zone-less ISO
        /// datetime (local time), or RFC3339.
        // trace:TASK-1509 | ai:claude
        #[clap(long, value_name = "DURATION")]
        since: Option<String>,
        /// Print what is already there and exit instead of following.
        #[clap(long)]
        no_follow: bool,
        /// Interleave tool invocations with the assistant text.
        #[clap(long)]
        with_tools: bool,
        /// Drop the leading `[HH:MM:SS]` event time from each line.
        #[clap(long)]
        no_timestamp: bool,
        /// Annotate idlewatch transitions such as low-information spinning.
        // trace:STORY-998 | ai:codex
        #[clap(long)]
        annotate: bool,
    },
}
