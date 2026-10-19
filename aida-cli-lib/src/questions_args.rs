//! `aida questions` clap types, owned by `questions.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// `aida questions` — the async decision inbox. The advisor distills a fork
/// it can't resolve into a structured DecisionRequest on the spec; the human
/// batch-answers it here, OUTSIDE any agent (plain CLI, no LLM session).
/// Slice 1 records the answer (pure data op); the loop-resume auto-applier
/// that applies the chosen resolution token is deferred.
// trace:STORY-522 | ai:claude
// trace:TASK-780 | ai:claude
#[derive(Subcommand, Debug)]
pub enum QuestionsCommand {
    /// Run by: anyone, when: you want to see what's pending (read-only).
    ///
    /// List the decision inbox — every spec with a recorded DecisionRequest,
    /// pending ones first. The pure read; never prompts (scripting/scanning).
    List,

    /// Run by: advisor / automation, when: detecting which specs need a human
    /// decision before they're pickable (the detection pass).
    ///
    /// Sweep the scoped backlog for specs that are likely to need a human
    /// decision before implementation, then attach DecisionRequests for the
    /// flagged candidates that do not already have an open request.
    Sweep {
        /// Scope to sweep. Defaults to `backlog` (approved/planned/in-progress
        /// near-term work). Supported: backlog, approved, planned,
        /// in-progress, all. Low-priority and archived specs are always
        /// excluded.
        scope: Option<String>,
        // trace:TASK-700 | ai:claude
        /// Attach the DecisionRequests. Without this the sweep only PREVIEWS
        /// which specs would get one (it mutates specs, so writing is opt-in).
        #[clap(long)]
        apply: bool,
    },

    /// Run by: the advisor, when: it hits a fork it can't resolve and needs to
    /// pose the question to the human (not a human-run command).
    ///
    /// Pose a structured DecisionRequest on a spec. The advisor distills a
    /// fork into a self-contained question + enumerated choices, each mapping
    /// to a deterministic resolution token. Refuses if the spec already has a
    /// pending request unless --force. Requires at least two choices.
    Ask {
        /// The spec (UUID or SPEC-ID) the question is about.
        spec: String,

        /// The self-contained question (no spec re-read needed to answer).
        #[clap(long, short = 'q', value_name = "TEXT")]
        question: String,

        /// A choice, as `label|consequence|resolution`. Repeatable; supply at
        /// least two. `resolution` is a deterministic action token (e.g.
        /// `status:rejected`, `tag:+deferred:post-stability`, `noop`) — never
        /// free-form prose.
        #[clap(long, short = 'c', value_name = "LABEL|CONSEQUENCE|RESOLUTION")]
        choice: Vec<String>,

        /// 1-based index of the recommended default choice (stored 0-based).
        #[clap(long, value_name = "N")]
        recommend: Option<usize>,

        /// Why the recommended default is recommended.
        // trace:BUG-1294 | ai:claude
        #[clap(long, value_name = "TEXT", allow_hyphen_values = true)]
        rationale: Option<String>,

        /// Overwrite an existing pending DecisionRequest on this spec.
        #[clap(long)]
        force: bool,
    },

    /// Run by: human + agent together (interactive), when: an under-specified
    /// spec needs acceptance criteria authored before it's pickable.
    ///
    /// Fire up an INTERACTIVE advisor that interrogates you to author
    /// acceptance criteria for under-specified specs — the human-side
    /// complement to `sweep` (sweep detects, clarify resolves). Launches
    /// `claude "/aida-clarify <specs>"` (interactive, not headless) seeded
    /// with the flagged specs and walks you through each until pickable.
    ///
    /// With no specs it defaults to the swept set (the same detection `sweep`
    /// uses), honouring the same exclusions: visions/folders/meta/principles/
    /// terms, already-built/held specs, and specs with an active lease.
    Clarify {
        /// The spec(s) to clarify. Omit to clarify the swept set.
        specs: Vec<String>,

        /// Show which specs would be clarified + the exact `claude` command,
        /// then exit without launching.
        #[clap(long)]
        dry_run: bool,
    },

    /// Run by: TUI / advisor automation, when: a drive hold is only missing
    /// acceptance criteria and should be remedied without a manual clarify hop.
    ///
    /// Launch a bounded headless advisor pass that reads the named spec and its
    /// graph context, authors a minimal grounded `## Acceptance` section when
    /// possible, and exits non-zero when the missing criteria cannot be safely
    /// inferred from the substrate.
    // trace:TASK-1075 | ai:codex
    Remedy {
        /// The under-specified spec to remedy.
        spec: String,

        /// Show the resolved headless prompt and log path, then exit without
        /// launching.
        #[clap(long)]
        dry_run: bool,
    },

    /// Run by: the human, when: draining pending decisions (no LLM session — a
    /// plain operator data op).
    ///
    /// Answer pending decisions — the HUMAN-decision drain, the symmetric
    /// complement of `aida burndown run` (which drains the decision-FREE ready
    /// set with autonomous agents). Answering APPLIES the chosen resolution:
    /// it binds a design decision into the spec's `## Acceptance`, clears a
    /// disposition gate, or rejects — then auto-queues the now decision-free
    /// spec onto the burndown ready set (advisor-gated; honest about any
    /// remaining blocker). No agent, no LLM session — a pure operator data op.
    ///
    /// `answer <spec> <choice>` records one answer non-interactively (choice
    /// is a 1-based number, or the word `default`/`recommended`).
    /// `answer` with no args enters the interactive loop over all pending
    /// (TTY) — the human-decision drain. `answer --all-defaults` confirms
    /// every recommended default at once.
    // trace:STORY-555 | ai:claude
    Answer {
        /// The spec (UUID or SPEC-ID) to answer. Omit for the interactive loop.
        spec: Option<String>,

        /// The chosen option: a 1-based number, or `default`/`recommended`.
        choice: Option<String>,

        /// Confirm the recommended default for every pending request that has
        /// one, in a single batch.
        #[clap(long)]
        all_defaults: bool,

        /// Attach a free-text counter-proposal recorded ALONGSIDE the chosen
        /// option (e.g. "name it list-claude-sessions"). A pure data op — no
        /// LLM at answer-time; the implementer reads choice + note later. The
        /// non-interactive form of the interactive prompt's "type something"
        /// escape; pair with <spec> <choice>.
        // trace:TASK-791 | ai:claude
        // trace:BUG-1294 | ai:claude
        #[clap(long, value_name = "TEXT", allow_hyphen_values = true)]
        note: Option<String>,
    },
}
