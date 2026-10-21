//! `aida merge_hold_cmd` clap types, owned by `merge_hold_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum MergeHoldAction {
    /// Release eligible Rework-only holds using a fresh recorded approval.
    /// Sensitive or composed holds still require a human clear.
    // trace:STORY-1687 | ai:codex
    AutoRelease {
        /// PR to inspect; omit to inspect all local hold markers.
        pr: Option<u64>,
    },
    /// Inspect or provision the three repository-level merge-hold label definitions.
    // trace:BUG-1747 | ai:codex
    Labels {
        /// Create any definitions that are missing from this repository.
        #[clap(long)]
        create_missing: bool,
        /// Emit one JSON document.
        #[clap(long)]
        json: bool,
    },
    /// List every active merge-hold: marker-backed holds AND holds that exist
    /// only as the `aida:merge-hold` label (shown as `label-only (no marker)`).
    /// Each row is the held PR + the hold reason. A marker whose PR merged or
    /// closed without merging is flagged stale; a marker whose stated premise
    /// has moved (the PR head passed the commit it cites, or a rework hold's
    /// verdict is now approved or closed) is flagged `premise stale` for a
    /// human to judge — nothing is cleared automatically.
    // trace:TASK-161 trace:BUG-1469 trace:BUG-1562 | ai:claude
    List {
        /// Machine-readable JSON output.
        #[clap(long)]
        json: bool,
        /// Re-sync the `aida:merge-hold` label on every live hold whose
        /// recorded label state is not `synced` (repairs a hold whose label
        /// never landed, so the required merge-hold-gate check enforces it),
        /// and re-route every recusal hold to a live independent reader at
        /// the PR's current head.
        // trace:BUG-1236 | ai:claude
        #[clap(long)]
        fix: bool,
    },
    /// Place a supervised merge-hold by hand: write the marker file AND mirror
    /// the `aida:merge-hold` label, so a PR you hold outside a drain releases
    /// through the same `aida merge-hold clear <pr>` path (a label applied by
    /// hand alone leaves the required check red with nothing to release).
    /// Refuses when the PR already has a marker unless `--replace` is given;
    /// `aida merge-hold list --fix` re-applies a missing label without
    /// touching the marker.
    // trace:BUG-1236 | ai:claude
    Add {
        /// PR number to hold.
        pr: u64,
        /// Hold reason recorded on the marker (default: "held by hand").
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        reason: Option<String>,
        /// Typed reason: supervision, recusal, rework, or decision.
        // trace:STORY-1397 | ai:codex
        #[clap(long, default_value = "supervision")]
        reason_kind: String,
        /// Stable principal identity excluded from review/merge. Repeatable;
        /// required for a recusal hold.
        // trace:STORY-1397 | ai:codex
        #[clap(long = "recused-principal")]
        recused_principals: Vec<String>,
        /// Stable independent reader identity already routed this exact hold.
        // trace:STORY-1397 | ai:codex
        #[clap(long = "route-to")]
        routed_to: Vec<String>,
        /// Exact PR head the recusal/review route applies to.
        #[clap(long)]
        head: Option<String>,
        /// What must be true for this hold to be released — numbered checks
        /// evaluated when read, rather than a reason describing the past.
        /// Shown by `aida merge-hold list`.
        // trace:BUG-1562 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        release_condition: Option<String>,
        /// For a rework hold: the review verdict it stands on (a spec id or
        /// `PR-<n>`, as stored under `.aida/review-verdicts/`). The marker
        /// references that record instead of quoting it, and `aida pr ship`
        /// releases the hold only once an approving verdict is recorded at
        /// the PR's current head.
        // trace:BUG-1532 | ai:claude
        #[clap(long)]
        verdict: Option<String>,
        /// Replace an existing marker for this PR. Without it, `add` refuses
        /// rather than silently overwrite the marker's body.
        // trace:BUG-1562 | ai:claude
        #[clap(long)]
        replace: bool,
    },
    /// Clear a merge-hold: remove the marker file and drop the
    /// `aida:merge-hold` label, releasing the PR for merge. A hold that exists
    /// only as the label is cleared the same way and the clearance recorded.
    /// Give a PR number, or `--stale` to sweep every marker whose PR can never
    /// merge (merged, or closed without merging).
    // trace:TASK-161 trace:BUG-1499 trace:BUG-1541 | ai:claude
    Clear {
        /// PR number whose hold to clear.
        pr: Option<u64>,
        /// Clear every marker whose PR has merged or was closed without
        /// merging (phantom holds). A PR whose state cannot be read is kept.
        #[clap(long)]
        stale: bool,
    },
}
