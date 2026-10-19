//! `aida plan_cmd` clap types, owned by `plan_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Implementation-plan tooling. Plans live in `docs/plans/` and follow
/// the structured template at `docs/plans/_TEMPLATE.md`.
// trace:TASK-92 | ai:claude
// trace:TASK-487 | ai:claude
#[derive(Subcommand, Debug)]
pub enum PlanCommand {
    /// Verify a plan file against the structured template: report
    /// drifted `path:line` refs (with suggested corrections), missing
    /// files, and absent required sections. Exits non-zero on any
    /// failure so it can run as a pre-commit hook on `docs/plans/`.
    // trace:TASK-93 | ai:claude
    Verify {
        /// Path to the plan markdown file (e.g.
        /// `docs/plans/2026-05-13-story-86-done-status.md`).
        file: PathBuf,

        /// Rewrite drifted `path:line` refs in place to corrected line
        /// numbers (or symbol form where a symbol is named). Without
        /// this flag, `verify` only reports.
        #[clap(long)]
        fix: bool,

        /// Suppress the per-check OK lines; print only warnings, errors,
        /// and the final verdict. Useful for pre-commit hooks.
        #[clap(long, short = 'q')]
        quiet: bool,
    },

    /// Derive a `## Reusable helpers` section for a spec from the trace
    /// graph: walk sibling specs (same parent), same-feature specs, and
    /// tag-sharing specs, harvest their AIDA trace annotations, and report
    /// the files + helpers they already touch so the implementer reuses
    /// rather than re-invents.
    // trace:TASK-94 | ai:claude
    Helpers {
        /// SPEC-ID of the requirement to derive reusable helpers for.
        spec: String,

        /// Append the generated section to this plan file instead of
        /// printing it to stdout.
        #[clap(long)]
        append: Option<PathBuf>,
    },

    /// Promote Approved spec(s) to Planned when a plan file exists for them
    /// under `docs/plans/` — a file whose `Specs:` header line lists the
    /// SPEC-ID. Formalizes the Approved -> Planned lifecycle step so
    /// plan-ahead work (plan now, implement later — the parallel-pipelining
    /// workflow) is visible in the queue. Pass a SPEC-ID to promote one, or
    /// `--all` to sweep every Approved spec that has a matching plan.
    // trace:STORY-265 | ai:claude
    Promote {
        /// SPEC-ID to promote. Omit and pass `--all` to sweep every
        /// Approved spec that has a plan file.
        spec: Option<String>,

        /// Promote every Approved spec that has a matching plan file.
        #[clap(long, conflicts_with = "spec")]
        all: bool,

        /// Report what would be promoted without writing.
        #[clap(long)]
        dry_run: bool,
    },

    /// Plan-only fan-out over a set of Approved specs: run the plan step
    /// for each spec in the set (sequentially) and promote each
    /// Approved -> Planned once its plan file lands. The set is selected
    /// by `--batch NAME` (every spec tagged `batch:NAME`), `--epic ID`
    /// (every spec tagged `parent:ID`), or an explicit list of SPEC-IDs.
    /// The workable-set discipline drops the low-priority tail by default
    /// (avoids speculative plan-slop on specs that may never be built) —
    /// pass `--include-low` to plan those too. True parallelism is the
    /// harness's job: this driver hands the set to the orchestrating agent
    /// by running each `aida queue work <spec> --plan-only` in turn;
    /// promotion is contention-free pre-work, never a merge.
    // trace:STORY-519 | ai:claude
    FanOut {
        /// Explicit SPEC-IDs to fan out over. Mutually exclusive with
        /// --batch / --epic; pick exactly one selection mode.
        specs: Vec<String>,

        /// Select every Approved spec tagged `batch:NAME`.
        #[clap(long, conflicts_with_all = ["epic", "specs"])]
        batch: Option<String>,

        /// Select every Approved spec tagged `parent:ID` (the epic/story
        /// rollup convention).
        #[clap(long, conflicts_with_all = ["batch", "specs"])]
        epic: Option<String>,

        /// Include low-priority specs in the fan-out. Off by default so
        /// the workable set excludes the low-priority tail.
        #[clap(long)]
        include_low: bool,

        /// Report the resolved set without launching any plan session or
        /// promoting anything.
        #[clap(long)]
        dry_run: bool,

        /// Skip the per-spec plan session launch and only run the
        /// Approved -> Planned promotion for specs that already have a
        /// plan file. Useful after the harness has fanned the plan
        /// sessions out itself and just needs the lifecycle bumps.
        #[clap(long)]
        promote_only: bool,
    },

    /// Synthesize a `docs/plans/` file from a merged/open PR's description
    /// and commit log. For externally authored planning flows that land a
    /// PR directly without ever writing a local plan file —
    /// this reconciles them back into AIDA's plan-archival convention.
    /// Reads `gh pr view <PR> --json title,body,commits,number` plus
    /// `gh pr diff <PR> --name-only`, fills the 11-section template, and
    /// writes `docs/plans/<date>-<slug>-from-pr-<N>.md`. Idempotent —
    /// re-running overwrites the same file deterministically. The output
    /// is shaped to pass `aida plan verify`.
    // trace:TASK-305 | ai:claude
    Capture {
        /// PR number to capture (e.g. `65`). Accepts a bare number or a
        /// `PR-65` / `#65` form.
        pr: String,

        /// Print the synthesized plan to stdout instead of writing a file
        /// under `docs/plans/`.
        #[clap(long)]
        stdout: bool,
    },

    /// Context-grounding pre-plan scan for a spec. Before you generate a
    /// plan (or import a Spec-Kit / OpenSpec-style artifact), this gathers a
    /// read-only snapshot of the code the work will touch: the files +
    /// symbols related specs already trace to (current APIs / architectural
    /// constraints) and a list of likely-stale assumptions — code paths the
    /// spec text names that no longer exist in the tree. Read-only by
    /// default; the result is provenance you can attach to the spec or a
    /// plan file so the grounding travels with the work.
    ///
    /// Compose it with external artifact generators: run the scan first,
    /// hand its summary to Spec Kit / OpenSpec as grounding context, then
    /// `--attach` the provenance so the imported spec records what the tree
    /// actually looked like at plan time.
    // trace:TASK-0418
    Scan {
        /// SPEC-ID (or agreed id / UUID) of the requirement to scan for.
        spec: String,

        /// Attach the scan summary to the spec as a provenance comment.
        /// This is the only write the command performs; without it the
        /// scan is strictly read-only and only prints.
        #[clap(long)]
        attach: bool,

        /// Append the scan summary as a `## Pre-plan scan` section to this
        /// plan file instead of (or in addition to) printing it.
        #[clap(long, value_name = "PATH")]
        append: Option<PathBuf>,

        /// Emit the scan as JSON for machine consumption (e.g. piping into
        /// an external plan/spec generator) instead of the markdown report.
        #[clap(long)]
        json: bool,
    },
}
