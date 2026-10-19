//! `aida human_cmd` clap types, owned by `human_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// The `aida human` role-vector: the human-tier attention verbs. `unblock` is
/// the deterministic prompt-assembler that ends the recurring "how do I get
/// open items into the burndown?" question by GENERATING the grooming question
/// for the advisor.
/// open items into the burndown?" question by GENERATING the grooming question
/// for the advisor.
// trace:STORY-563 | ai:claude — the role-vector design is SPIKE-57.
#[derive(Subcommand, Debug)]
pub enum HumanCommand {
    /// Mark yourself away from the keyboard. Alias for top-level `aida away`.
    // trace:TASK-770 | ai:codex
    Away,

    /// Mark yourself back at the keyboard. Alias for top-level `aida home`.
    // trace:TASK-770 | ai:codex
    Home,

    /// Show current effective presence. Alias for top-level `aida presence`.
    // trace:TASK-770 | ai:codex
    Presence,

    /// Show current effective presence. Alias for `aida human presence`.
    // trace:TASK-770 | ai:codex
    Status,

    /// Emit a paste-ready advisor prompt to groom the open items that are
    /// keeping themselves out of the burndown ready set. Read-only +
    /// deterministic — no LLM, no writes; it classifies each open spec by
    /// WHAT keeps it out (needs-approval, approved-unqueued, under-specified,
    /// build-supervised, decision-pending, deferred, blocked-by) and assembles
    /// the prompt that tells the advisor to queue the autonomous-able, clarify
    /// the under-specified first, and leave the rest parked. The advisor (the
    /// grooming skill / live session) is the actor the prompt drives.
    // trace:STORY-563 | ai:claude
    Unblock {
        /// Copy the assembled prompt to the system clipboard (like
        /// `aida goal --copy` / `aida ultraplan`). Default prints framed
        /// output to the terminal.
        // trace:STORY-563 | ai:claude
        #[clap(long, conflicts_with_all = ["stdout", "json"])]
        copy: bool,

        /// Print ONLY the bare assembled prompt to stdout, no framing — for
        /// piping / command substitution.
        // trace:STORY-563 | ai:claude
        #[clap(long, conflicts_with = "json")]
        stdout: bool,

        /// Emit the classification as JSON (`[{spec,class,action,reason}]`)
        /// instead of the prompt — for machine consumers / the TUI.
        // trace:STORY-563 | ai:claude
        #[clap(long)]
        json: bool,

        /// Walk the human-blocked specs one at a time and resolve each inline
        /// (approve/queue, capture a decision, clarify, surface a blocker),
        /// instead of emitting the paste-ready advisor prompt. Needs a
        /// terminal; resolutions self-invoke the existing `aida` verbs so the
        /// write path is the same one `aida edit` / `aida queue add` use.
        // trace:STORY-750 | ai:claude
        #[clap(long, conflicts_with_all = ["copy", "stdout", "json"])]
        interactive: bool,

        /// After an interactive sweep, re-classify and report how many specs are
        /// now drive-ready, naming the drain command to pick them up. Only
        /// meaningful with `--interactive`.
        // trace:STORY-750 | ai:claude
        #[clap(long, requires = "interactive")]
        then_drain: bool,
    },

    /// Answer a pending decision inline. Thin alias for `aida questions answer
    /// <spec> <choice>` — resolve a decision listed by `aida human` without
    /// switching commands. Pure pass-through; the canonical verb owns the logic.
    // trace:STORY-611 | ai:claude
    Answer {
        /// The spec carrying the pending decision (SPEC-ID or UUID).
        spec: String,
        /// The choice to record (1-based index, or the choice label).
        choice: String,
        /// Attach a counter-proposal note to the answer.
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        note: Option<String>,
    },

    /// Answer a pending decision inline. Alias of `aida human answer` (reads
    /// better for some) — delegates to `aida questions answer`.
    // trace:STORY-611 | ai:claude
    Decide {
        /// The spec carrying the pending decision (SPEC-ID or UUID).
        spec: String,
        /// The choice to record (1-based index, or the choice label).
        choice: String,
        /// Attach a counter-proposal note to the answer.
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        note: Option<String>,
    },

    /// Review a spec inline. Thin alias for `aida review <spec>` — drain a
    /// review listed by `aida human` without switching commands. Pure
    /// pass-through; the canonical verb owns the surface-detection + reviewer.
    // trace:STORY-611 | ai:claude
    Review {
        /// The spec to review (SPEC-ID or UUID).
        spec: String,
        /// Skip launching the reviewer agent — just report the surface.
        #[clap(long)]
        no_agent: bool,
        /// Proceed even if the PR's base branch is stale.
        #[clap(long)]
        allow_stale_base: bool,
        /// Launch an interactive advisor shell for a guided review instead of
        /// running the inline review prompt.
        // trace:STORY-818 | ai:codex
        #[clap(long, short = 'g', conflicts_with_all = ["no_agent", "allow_stale_base"])]
        guided: bool,
    },

    /// Trigger the human-audit reconcile pass at the advisor session.
    ///
    /// By default this ENQUEUES a durable request: it posts a directive onto
    /// the worker-directive channel (`.aida/worker.cmd`) that the polling
    /// advisor picks up on its next cycle (via `aida worker directives` / its
    /// per-turn check) and runs the `/aida-human-audit` skill. Works headless
    /// and across vendors; the only cost is up to one poll-cycle of latency.
    ///
    /// Pass `--inject` for an immediate nudge: it `tmux send-keys` the slash
    /// command straight into the advisor's registered pane, so it fires even
    /// when the advisor is idle between turns. `--inject` requires tmux — the
    /// advisor's pane is recorded at session start. When you are not in tmux or
    /// no pane is registered, `--inject` prints a note and falls back to the
    /// enqueue path (it never fails).
    // trace:STORY-768 | ai:claude
    Audit {
        /// Also inject the slash command straight into the advisor's tmux pane
        /// for an immediate run (requires tmux; falls back to enqueue when no
        /// pane is registered).
        // trace:STORY-768 | ai:claude
        #[clap(long)]
        inject: bool,
    },
}
