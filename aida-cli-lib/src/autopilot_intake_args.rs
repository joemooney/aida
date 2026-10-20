//! `aida autopilot_intake` clap types, owned by `autopilot_intake.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Advisor-autopilot auditability + reversal surface.
///
/// The read-only + audit + reversal half of the bounded-authority envelope:
/// see what autopilot WOULD decide, review the recorded verdicts, and reverse
/// any of them. Autopilot cannot approve/reject/queue on its own — that
/// authority is deliberately withheld.
// trace:TASK-1147 | ai:claude
#[derive(Subcommand, Debug)]
pub enum AutopilotCommand {
    /// Dry-run the policy envelope over the current groom candidates and show,
    /// per spec, the four-gate verdict (auto-execute vs gated, and which gate
    /// stopped it). Writes NOTHING to any spec. Pass --record to append the
    /// projected verdicts to the local audit log for later review.
    Inspect {
        /// Only weigh specs riskier than this level out (low / medium / high /
        /// unknown). Mirrors `aida groom --risk`. Default: medium.
        #[clap(long, value_name = "MAX", default_value = "medium")]
        risk: String,
        /// Only consider specs carrying this tag.
        #[clap(long, value_name = "TAG")]
        only_tag: Option<String>,
        /// Never consider specs carrying this tag.
        #[clap(long, value_name = "TAG")]
        exclude_tag: Option<String>,
        /// Append the projected verdicts to the local audit log.
        #[clap(long)]
        record: bool,
        /// Emit a stable JSON shape instead of the table.
        #[clap(long)]
        json: bool,
    },
    /// List the recorded autopilot decisions (and any reversals) from the local
    /// audit log, newest last.
    Audit {
        /// Cap the number of rows shown (0 = all).
        #[clap(long, default_value = "50")]
        limit: usize,
        /// Only show decisions that have NOT been challenged.
        #[clap(long)]
        open: bool,
        /// Emit a stable JSON shape instead of the table.
        #[clap(long)]
        json: bool,
    },
    /// Mark a recorded autopilot decision as challenged / reversed. TARGET is a
    /// decision id (from `aida autopilot audit`) or a SPEC-ID (reverses that
    /// spec's most recent un-challenged decision).
    Challenge {
        /// Decision id or SPEC-ID to reverse.
        #[clap(value_name = "TARGET")]
        target: String,
        /// Why the decision is being reversed.
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        note: Option<String>,
    },
    /// List every action autopilot actually EXECUTED, with the prior state each
    /// one can be restored to. Distinct from `audit`, which lists the dry-run
    /// projection of what the envelope WOULD decide.
    // trace:TASK-1018 | ai:claude — plain `//` keeps the marker out of `--help`.
    Executions {
        /// Cap the number of rows shown (0 = all).
        #[clap(long, default_value = "50")]
        limit: usize,
        /// Only show executions that have NOT been reversed.
        #[clap(long)]
        open: bool,
        /// Only show executions whose evidence records a product handoff — the
        /// "is a product seat steering the queue?" review lens. A product
        /// recommendation is evidence only; it never carries approval authority.
        // trace:TASK-1013 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long)]
        from_product: bool,
        /// Only show executions taken under this composition mode — the level
        /// of supervision in effect when the action landed. Pass a whole token
        /// (`autopilot` for the bare envelope, `zen+autopilot`, …) to match it
        /// exactly, or a single layer (`zen`, `solo`) to match every
        /// composition that used it.
        // trace:TASK-1014 | ai:claude — plain `//` keeps the marker out of `--help`.
        #[clap(long, value_name = "MODE")]
        mode: Option<String>,
        /// Emit a stable JSON shape instead of the table.
        #[clap(long)]
        json: bool,
    },
    /// Undo one executed autopilot action, restoring the state recorded with it.
    ///
    /// TARGET is an execution id (from `aida autopilot executions`) or a
    /// SPEC-ID, which reverts that spec's most recent un-reverted action. The
    /// reversal is derived from the recorded prior state alone, applied through
    /// the same paths `aida edit` / `aida queue` use, and itself recorded.
    // trace:TASK-1018 | ai:claude
    Revert {
        /// Execution id or SPEC-ID to undo.
        #[clap(value_name = "TARGET")]
        target: String,
        /// Show the restore steps without applying them.
        #[clap(long)]
        dry_run: bool,
        /// Why the action is being undone.
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        note: Option<String>,
        /// Emit a stable JSON shape instead of the human summary.
        #[clap(long)]
        json: bool,
    },
    /// Rebuild the local execution index from the durable audit comments on the
    /// specs themselves.
    ///
    /// The index under `.aida/` is per-clone runtime state; the trail that
    /// survives a fresh clone, a different machine, or a different agent is the
    /// comment on each spec. This refills the index from it. Append-only and
    /// idempotent — nothing already indexed is rewritten.
    // trace:TASK-1018 | ai:claude
    Reindex {
        /// Report what would be recovered without writing the index.
        #[clap(long)]
        dry_run: bool,
    },
}
