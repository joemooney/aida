//! `aida queue_misc` clap types, owned by `queue_misc.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Worker-directive introspection. The `aida-worker` shell function reads
/// pending directives from `.aida/worker.cmd` (a FIFO — one directive
/// per line); this subcommand lists them so a user can see "what will the
/// worker do next?" without `cat`ing a runtime file. Pairs with the
/// `aida-worker` function emitted by `aida dev shell-init`.
// trace:TASK-294 | ai:claude
// trace:TASK-487 | ai:claude
#[derive(Subcommand, Debug)]
pub enum WorkerCommand {
    /// List pending directives from `.aida/worker.cmd` in FIFO order — the
    /// next directive the `aida-worker` shell function will act on is at the
    /// top. Prints `No pending directives.` (exit 0) when the file is empty
    /// or absent.
    Directives {
        /// Emit the directive list as JSON instead of the human summary.
        #[clap(long)]
        json: bool,
    },

    /// Garbage-collect stale directives from `.aida/worker.cmd` — prune every
    /// spec-targeted drain line whose target spec is already archived,
    /// Completed, or Rejected, so a later worker run cannot fire an unattended
    /// drain against finished work. Bare drains, batch-scoped drains,
    /// comments, and control directives (pause / exit) are never touched.
    /// Sibling of `aida queue gc` (which prunes dead routed queue entries).
    // trace:BUG-723 | ai:claude
    Gc {
        /// Preview what would be pruned without rewriting the file.
        #[clap(long)]
        dry_run: bool,
    },
}

// Headless-log introspection. A `--no-human` drain writes one JSONL file per
// child session under `.aida/headless-logs/`; this subcommand wraps the right
// block-type filtering so the user gets a clean text stream instead of the
// pages of `null` a naive `jq` filter produces.
// trace:TASK-398 | ai:claude
#[derive(Subcommand, Debug)]
pub enum HeadlessCommand {
    /// Stream a headless-drain JSONL log as clean text — one block of
    /// assistant prose per message, blank line between messages. No daemon,
    /// no parsing into cache.db. With no argument, picks the most-recently-
    /// modified log under `.aida/headless-logs/`; pass a SPEC-ID to pick
    /// the newest log for that spec, or a lease/session id prefix
    /// (e.g. `019e4400`) to disambiguate between concurrent drains.
    // trace:TASK-487 | ai:claude
    Tail {
        /// SPEC-ID (e.g. `TASK-N`) or lease/session id prefix. Optional —
        /// when omitted, the most-recently-modified log is followed.
        target: Option<String>,

        /// List every log file under `.aida/headless-logs/` (kind, spec,
        /// mtime, size, filename) and exit. Useful for picking a lease id
        /// when multiple drains have touched the same spec.
        #[clap(long, conflicts_with_all = ["target", "with_tools", "tools_only", "include_user", "no_follow", "since"])]
        list: bool,

        /// Interleave tool invocations as `[ToolName] <preview>` lines,
        /// alongside the assistant text. Off by default — most users only
        /// want the narration.
        #[clap(long)]
        with_tools: bool,

        /// Print only tool invocations, no assistant text. Useful for
        /// watching what commands the implementer is running.
        #[clap(long, conflicts_with = "list")]
        tools_only: bool,

        /// Also surface `type=="user"` messages (tool_result payloads). Off
        /// by default — headless mode rarely has interactive user content,
        /// and tool_results add a lot of noise.
        #[clap(long)]
        include_user: bool,

        /// Print existing content and exit instead of staying attached with
        /// `tail -f`-style polling. Useful for piping into a pager.
        #[clap(long, short = 'n')]
        no_follow: bool,

        /// Skip entries older than this point: a relative duration (`30s`,
        /// `10m`, `2h`, `1d`, `2w`, or `24 hours ago`; a bare integer means
        /// seconds), an ISO date (`2026-05-01`, local midnight), a zone-less
        /// ISO datetime (local time), or RFC3339. Compared against
        /// `timestamp` fields when the event carries one; events without a
        /// timestamp (assistant messages, system events) pass through
        /// unfiltered.
        // trace:TASK-1509 | ai:claude
        #[clap(long, value_name = "DURATION")]
        since: Option<String>,
    },
}

// trace:STORY-248 | ai:claude
/// Stack-graph introspection. `aida queue work --stack /
/// --base` records each stacked branch's parent in `.aida/stacks.json`;
/// `aida stack show` renders the chain tree; `aida stack list` prints one
/// chain per line for scripting.
// trace:STORY-248 | ai:claude
#[derive(Subcommand, Debug)]
pub enum StackCommand {
    /// Render the stack tree — one indented line per branch, grouped by
    /// chain. Stale entries (branch missing locally + on origin) are
    /// suffixed with `(stale)` and can be cleaned by passing
    /// `--prune-stale`.
    Show {
        /// Emit the chains as JSON instead of the indented tree.
        #[clap(long)]
        json: bool,
        /// Remove stack entries whose branch no longer exists locally or
        /// on origin. Idempotent — safe to re-run.
        #[clap(long)]
        prune_stale: bool,
    },
    /// One line per chain in the form `bottom → mid → top`. Quieter than
    /// `show`; useful for scripts and the statusline.
    List {
        /// Emit the chains as JSON instead of the arrow form.
        #[clap(long)]
        json: bool,
    },
}
