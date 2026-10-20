//! `aida backlog` clap types, owned by `backlog.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Curate Approved-but-not-queued work into the queue with risk + conflict
/// heuristics. The "backlog" is the Approved pile that nobody has yet
/// committed to working — `aida backlog list` filters it, `analyze` reports
/// pairwise file-overlap, `groom` moves a curated selection onto the queue
/// (optionally under a single `batch:NAME` tag for `aida queue work
/// --batch NAME`).
// trace:STORY-444 | ai:claude
#[derive(Subcommand, Debug)]
pub enum BacklogCommand {
    /// Show Approved items that are not currently in any user's queue.
    /// Risk chips (low / medium / high / unknown) are advisory only.
    List {
        /// Filter to a single risk level (low, medium, high, unknown).
        #[clap(long, value_name = "LEVEL")]
        risk: Option<String>,
        /// Filter by requirement type (e.g. task, story, bug, doc).
        #[clap(long, value_name = "TYPE")]
        r#type: Option<String>,
        /// Filter by priority (high, medium, low).
        #[clap(long, value_name = "PRIORITY")]
        priority: Option<String>,
        /// Require this exact tag (case-insensitive).
        #[clap(long, value_name = "TAG")]
        tag: Option<String>,
        /// Require any tag with this prefix (case-insensitive).
        #[clap(long, value_name = "PREFIX")]
        tag_prefix: Option<String>,
        /// Cap the number of rows shown.
        #[clap(long, default_value = "50")]
        limit: usize,
        /// Emit a stable JSON shape instead of the table.
        #[clap(long)]
        json: bool,
        /// User ID for queue-membership scan (defaults to AIDA_USER /
        /// system user — same resolution as `aida queue list`).
        #[clap(long)]
        user: Option<String>,
    },
    /// Report pairwise file-overlap between selected backlog candidates.
    /// File sets come from inline spec-id trace markers in source plus the
    /// `## Critical Files` section of any owning plan in `docs/plans/`.
    Analyze {
        /// Comma-separated list of spec IDs to analyze (≥ 2 required).
        #[clap(long, value_name = "CSV")]
        specs: Option<String>,
        /// Shorthand for `--specs A,B` — analyzes exactly one pair.
        #[clap(long, value_names = ["A", "B"], num_args = 2)]
        pair: Option<Vec<String>>,
        /// Emit a stable JSON shape instead of the table.
        #[clap(long)]
        json: bool,
    },
    /// Move selected backlog items onto the queue. Optionally tags every
    /// groomed item with `batch:NAME` so `aida queue work --batch NAME`
    /// can drain them together — by default one full lifecycle PER MEMBER;
    /// add `--single-branch` to ship the batch as one branch / one PR.
    Groom {
        /// Comma-separated list of spec IDs to groom into the queue.
        #[clap(long, value_name = "CSV")]
        specs: Option<String>,
        /// Read newline-separated spec IDs from stdin (mutually exclusive
        /// with --specs).
        #[clap(long, conflicts_with = "specs")]
        from_stdin: bool,
        /// Auto-select every decision-free backlog item using the same
        /// pickability gate the burndown uses (bounded/not-epic, unblocked,
        /// decision-free, not parking-tagged), then groom the survivors.
        /// Mutually exclusive with --specs / --from-stdin. DRY-RUN BY DEFAULT:
        /// prints the would-groom and would-park sets; pass --apply to write.
        ///
        /// Caveat: "decision-free" here means "no attached DecisionRequest",
        /// which is coarser than true design-latitude — a design-heavy spec
        /// with no formal question still passes. Run `aida questions` before
        /// `--pickable --apply`, and keep the dry-run review load-bearing.
        #[clap(
            long,
            visible_alias = "auto",
            conflicts_with = "specs",
            conflicts_with = "from_stdin"
        )]
        pickable: bool,
        /// With --pickable, exclude candidates riskier than this level from
        /// auto-selection (low / medium / high / unknown). Uses the same risk
        /// chip `aida backlog list` shows. `--risk high` allows all;
        /// `--risk low` admits only low-risk. Default: medium.
        #[clap(long, value_name = "MAX", default_value = "medium")]
        risk: String,
        /// With --pickable, actually write the survivors to the queue.
        /// Without it, --pickable is a dry run (queueing = advisor sign-off).
        #[clap(long, visible_alias = "yes")]
        apply: bool,
        /// Tag every groomed item with `batch:NAME` (composes with
        /// `aida queue work --batch NAME`).
        #[clap(long, value_name = "NAME")]
        batch: Option<String>,
        /// Print what would happen without writing.
        #[clap(long)]
        dry_run: bool,
        /// Optional note recorded on every produced queue entry.
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        note: Option<String>,
        /// Override the queue user (defaults to AIDA_USER / system user).
        #[clap(long)]
        user: Option<String>,
    },
    /// Sum latest effort estimates for approved/planned/draft backlog items
    /// that are not currently queued.
    // trace:STORY-451 | ai:codex
    Load,
}
