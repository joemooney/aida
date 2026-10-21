//! `aida burndown_run` clap types, owned by `burndown_run.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Developer commands for working *on* AIDA itself (pyenv-style activation
// trace:STORY-527 | ai:claude
/// `aida burndown` — plan an autonomous backlog drain.
#[derive(Subcommand, Debug)]
pub enum BurndownCommand {
    /// Check whether this machine has enough disk, memory, and a real caller
    /// binary for an unattended window. Read-only; exits non-zero on failure.
    // trace:TASK-1298 | ai:codex
    Readiness {
        /// Planned unattended duration in hours.
        #[clap(long, default_value_t = 20)]
        hours: u64,
        /// Maximum concurrent build/implementation lanes.
        #[clap(long, default_value_t = 4)]
        lanes: usize,
        /// Approximate number of specs expected in the window.
        #[clap(long, default_value_t = 20)]
        specs: usize,
        /// Emit the complete report as JSON.
        #[clap(long)]
        json: bool,
    },
    /// Resolve which specs are ready to fan out vs parked, applying the
    /// pickability gate: bounded (not an epic), unblocked (no unsatisfied
    /// BlockedBy), decision-free (no pending question), not parking-tagged.
    /// Read-only — the run itself is the /aida-burndown skill in Claude Code,
    /// not a CLI subcommand. A "selector" (--status/--tag/--batch) narrows
    /// which specs the plan considers.
    Plan {
        /// Which specs to consider (the "selector"). Default: approved specs.
        #[clap(long, default_value = "approved")]
        status: String,
        /// Only specs carrying this tag.
        #[clap(long)]
        tag: Option<String>,
        /// Only specs in this batch (matches the `batch:<name>` tag).
        #[clap(long)]
        batch: Option<String>,
        /// Only specs of this requirement type (e.g. task, story, bug, doc).
        /// The plan otherwise considers WORK types only — the knowledge-class
        /// records (decision, vision, term, principle) are authored, not
        /// implemented, so they are never offered as drain candidates unless
        /// you name one here.
        // trace:BUG-784 | ai:claude
        #[clap(long, value_name = "TYPE")]
        r#type: Option<String>,
        /// Curation view: show approved + pickable specs that are NOT yet
        /// queued — the advisor's "what could I bless next" aid. Read-only;
        /// never auto-queues. (The default view's ready set requires queue
        /// membership — queueing a spec IS the advisor sign-off.)
        // trace:STORY-546 | ai:claude
        #[clap(long)]
        candidates: bool,
        /// How to order the ready set for this run: `priority` (high before
        /// medium before low, ties broken by queue-insertion time) or `queue`
        /// (strict queue-insertion order — oldest queued first). Overrides
        /// `[burndown] order` in config for this invocation only.
        // trace:TASK-1175 | ai:claude
        #[clap(long, value_name = "ORDER")]
        order: Option<String>,
        /// Machine-readable JSON
        /// (`{ready, awaiting_signoff, serialize_held, supervised, parked}`).
        // trace:TASK-149 | ai:claude
        #[clap(long)]
        json: bool,
    },
    /// Explain why EVERY open spec is still open — not just the candidate set.
    /// Where `plan` shows ready-vs-parked for the specs a burndown would fan
    /// out, `explain` classifies all open specs (drafts, epics, deferred,
    /// held-for-review, blocked, in-flight, vision) into a bucket + one-line
    /// reason, derived purely from store signals. The post-burndown "why is
    /// each of these still here?" view.
    // trace:STORY-547 | ai:claude
    Explain {
        /// Machine-readable JSON (`[{spec, bucket, reason, needs_human}]`).
        #[clap(long)]
        json: bool,
    },
    /// Kick off and walk away: drain the advisor-blessed ready set in a headless
    /// Claude Code session. Launches `claude -p "/aida-burndown <selector>"`,
    /// which fans out worktree-isolated implementers in parallel, integrates
    /// their PRs, and loops until drained. Drains ONLY the queued + pickable set
    /// (advisor sign-off = queue membership) — never an unqueued spec.
    ///
    /// This is the autonomous, decision-FREE half of the drain. Its symmetric
    /// complement is `aida questions answer` (the interactive loop) — the
    /// HUMAN-decision drain that answers the parked, decision-REQUIRED specs and
    /// unparks them back into this ready set.
    // trace:STORY-555 | ai:claude
    // trace:STORY-545 | ai:claude
    Run {
        /// Which specs to consider (the "selector"). Default: approved.
        #[clap(long, default_value = "approved")]
        status: String,
        /// Only specs carrying this tag.
        #[clap(long)]
        tag: Option<String>,
        /// Only specs in this batch (matches the `batch:<name>` tag).
        #[clap(long)]
        batch: Option<String>,
        /// Cap the total number of specs drained this run (passed to the skill
        /// as `--max`). Default: drain until the blessed set is empty.
        #[clap(long)]
        max: Option<usize>,
        /// Parallel wave size — how many implementers fan out at once.
        #[clap(long)]
        concurrency: Option<usize>,
        /// How to order the ready set for this drain: `priority` (high before
        /// medium before low, ties broken by queue-insertion time) or `queue`
        /// (strict queue-insertion order — oldest queued first). Overrides
        /// `[burndown] order` in config, and is exported to the headless drain
        /// so every wave it re-resolves uses the same order.
        // trace:TASK-1175 | ai:claude
        #[clap(long, value_name = "ORDER")]
        order: Option<String>,
        /// Claude permission mode for the headless drain. Defaults to
        /// `bypassPermissions` so the unattended drain can push/merge/fan-out
        /// without stalling on prompts. Override with e.g. `acceptEdits`.
        #[clap(long)]
        permission_mode: Option<String>,
        /// Show the blessed ready set + the exact `claude -p` command that
        /// would run, then exit without launching.
        #[clap(long)]
        dry_run: bool,
        /// Stream live per-event progress instead of waiting silently. Launches
        /// the headless drain with `--output-format stream-json --verbose
        /// --include-partial-messages`, tees the JSONL to
        /// `.aida/burndown/<drain-id>.jsonl`, and renders a human-readable
        /// progress line per event to your terminal. AUTO-PROCEED is unchanged —
        /// this only adds visibility, never alters the drain's control flow. The
        /// teed JSONL is also the machine-readable substrate for future drain-
        /// status tooling.
        // trace:TASK-804 | ai:claude
        #[clap(long)]
        verbose: bool,
        /// Force the legacy buffered launch even when `[burndown] verbose =
        /// true` is configured. `--verbose` wins if both flags are passed.
        // trace:TASK-1159 | ai:codex
        #[clap(long)]
        quiet: bool,
        // trace:STORY-647 trace:STORY-1353 | ai:claude
        /// Bypass the dispatch-authority guardrail on starting an autonomous
        /// drain (product, advisor, integrator role, or a live orchestrator
        /// normally required). A guardrail, not security — the bypass is
        /// recorded in history.
        #[clap(long)]
        force: bool,
        /// Which vendor CLI backs this drain: `claude`, `codex`, or `agy`
        /// (alias: `--agent`). NOTE: `burndown run`'s implementer fan-out is the Claude
        /// harness's native worktree-isolated subagent primitive, so the
        /// fan-out itself is Claude-only; this flag exports `AIDA_HEADLESS_VENDOR`
        /// for the drain so any vendor-agnostic per-spec orchestration inherits
        /// it. For a fully non-Claude drain, run `aida queue work <SPEC>
        /// --auto-complete --vendor <v>` per spec instead. Resolution order:
        /// this flag > the `AIDA_HEADLESS_VENDOR` env > the `[orchestrator]
        /// headless_vendor` / `[agents] vendor` config knob > `claude`.
        // trace:TASK-1116 | ai:claude
        #[clap(long, visible_alias = "agent", value_name = "VENDOR")]
        vendor: Option<String>,
        /// Host each fanned-out implementer in its own titled terminal window
        /// instead of a background process. Currently supports tmux: pass
        /// `--panes` (defaults to tmux) or `--panes tmux`. Exported to the drain
        /// so any `aida queue work --auto-complete` it spawns hosts its
        /// implementer in a titled window under a dedicated `aida-drain` tmux
        /// session. Degrades gracefully: outside tmux the drain falls back to the
        /// background spawn with a one-line notice, never failing.
        // trace:TASK-1120 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, value_name = "HOST", num_args = 0..=1, default_missing_value = "tmux")]
        panes: Option<String>,
        /// Refuse to launch unless this dev build of aida matches (or is
        /// ahead of) the default branch HEAD. Without it, a stale dev build
        /// only prints a one-line warning and the wave runs it. Also enabled
        /// by `[drain] require_head = true` in `.aida/config.toml`. Released
        /// binaries are never checked.
        // trace:STORY-1414 | ai:claude
        #[clap(long)]
        require_head: bool,
    },
    /// Is a drain running, and what is it doing? The read-side companion to
    /// `burndown run`. Reads the global drain lock — pid, start time, the
    /// launching command, host — and corroborates it against a PID-liveness
    /// probe (running vs a crashed/stale lock). Also lists the in-flight leased
    /// worktrees (the implementers fanned out under the drain) and points at the
    /// live event log (`.aida/burndown/<drain-id>.jsonl`) you can tail.
    /// Read-only; exits 0 whether or not a drain is running.
    // trace:TASK-806 | ai:claude (lock = BUG-538, event log = TASK-804)
    Status {
        /// Machine-readable JSON (`{drain:{running,pid,…}, in_flight:[…], log}`).
        #[clap(long)]
        json: bool,
    },
}
