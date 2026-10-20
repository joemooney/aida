//! `aida plan_intent` clap types, owned by `plan_intent.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Spec-quality tooling — checks you run ON a spec before work begins.
// trace:STORY-656 | ai:claude
#[derive(Subcommand, Debug)]
pub enum SpecCommand {
    /// Implementer-readiness pre-check: report whether a spec is ready to be
    /// picked up BEFORE it is queued, WITHOUT implementing anything. Always
    /// runs a deterministic 0-100 readiness score across weighted dimensions
    /// (description depth, an acceptance section, implementable type, priority,
    /// a linked parent, a not-too-vague heuristic), each with a pass/fail and a
    /// one-line reason. With `--ai`, also shells an AI pass (gated like
    /// `aida intent`, needs a TTY) that lists the questions an implementer
    /// would ask, the assumptions they'd make, and the ambiguities / missing
    /// acceptance it found.
    Dryrun {
        /// The SPEC-ID to pre-check (any story/task/bug/feature id).
        id: String,

        /// Also run the optional AI gap-report pass (shells `claude -p`; needs
        /// an interactive terminal). The deterministic pre-check runs either
        /// way — this only appends the AI report.
        #[clap(long)]
        ai: bool,

        /// Machine-readable JSON
        /// (`{spec, readiness_score, dimensions:[{name,pass,reason,weight}], ai_report?}`).
        #[clap(long)]
        json: bool,
    },

    /// Interview mode: resolve a spec's `dryrun` readiness gaps INTO the spec by
    /// asking clarifying questions. Runs the same deterministic readiness check
    /// `dryrun` does, turns each failing dimension into a question (missing
    /// acceptance -> "what does done look like?"; no parent -> "which spec is
    /// this a child of?"; no priority -> "high/medium/low?"), then folds the
    /// answers back into the spec as binding `## Acceptance` criteria and
    /// structured fields — never into comments.
    ///
    /// With a terminal it prompts for each gap in turn. Without one (the
    /// advisor/agent seat) it emits the structured question list and exits
    /// without blocking on stdin; feed answers back with `--answers <file>`.
    /// Propose-by-default: the spec is only written with `--apply`.
    // trace:STORY-657 | ai:claude
    Interview {
        /// The SPEC-ID to interview (any story/task/bug/feature id).
        id: String,

        /// Write the resolved answers into the spec (acceptance criteria,
        /// parent link, priority) and re-score. Without it, the command only
        /// proposes the edits / emits the question list — it never mutates the
        /// spec by surprise.
        #[clap(long)]
        apply: bool,

        /// Also derive questions from the optional AI gap report (shells
        /// `claude -p`, needs a TTY) — its ambiguities and implementer
        /// questions become extra acceptance-shaped questions.
        #[clap(long)]
        ai: bool,

        /// Answer the questions non-interactively from a JSON file (an array of
        /// `{"dimension":..,"answer":..}`, or `{"answers":[..]}`). Pair with
        /// `--apply` to fold them in. This is the agent/headless feedback path.
        #[clap(long, value_name = "FILE")]
        answers: Option<String>,

        /// Machine-readable JSON. Without answers, emits the question list
        /// (`{spec, readiness_score, questions:[{dimension,kind,prompt}]}`) for
        /// an agent to answer. With `--answers`/`--apply`, emits the applied
        /// edit summary.
        #[clap(long)]
        json: bool,
    },
}
