//! `aida findings_gate` clap types, owned by `findings_gate.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// `aida findings` — triage findings filed by headless drain phases (the
/// reviewer's `from-review:` tag, the implementer's `from-implementer:`
/// tag) and the advisor seat's `from-advisor:` observations.
// trace:STORY-278, STORY-285 | ai:claude
// trace:TASK-487 | ai:claude
// trace:STORY-467 | ai:claude
#[derive(Subcommand, Debug)]
pub enum FindingsCommand {
    /// File an advisor observation as a finding awaiting triage. Capture a
    /// pattern you've spotted from a live session before the context decays;
    /// promote, dismiss, or recur it later. The note becomes the finding's
    /// description; the title is the first line of the note unless --title
    /// is given.
    // trace:STORY-467 | ai:claude
    Add {
        /// The observation body. Required — without a note the finding has
        /// nothing to triage. Use `-` to read from stdin.
        // trace:BUG-1294 | ai:claude
        #[clap(long, value_name = "TEXT", allow_hyphen_values = true)]
        note: String,

        /// What kind of finding this is. Free-form so future kinds (e.g.
        /// `principle-candidate`, `architectural-concern`) ship without
        /// a code change; `observation` is the default and the only
        /// advisor kind today.
        #[clap(long, value_name = "KIND", default_value = "observation")]
        kind: String,

        /// One-line title. Defaults to the first line of `--note`,
        /// truncated to 80 characters.
        // trace:BUG-1294 | ai:claude
        #[clap(long, value_name = "TEXT", allow_hyphen_values = true)]
        title: Option<String>,

        /// Severity — `major`, `minor`, or `cosmetic`. Absent → unknown
        /// (sorted last in the triage view).
        #[clap(long, value_name = "LEVEL")]
        severity: Option<String>,

        /// Comma-separated SPEC-IDs the observation is about. The first
        /// becomes the origin (`from-advisor:<SPEC>`); the rest get
        /// `linked:<SPEC>` tags. Omit to file as `from-advisor:general`.
        #[clap(long, value_name = "IDS", value_delimiter = ',')]
        linked_specs: Vec<String>,

        /// Extra tags, comma-separated. Added verbatim — use the same
        /// `aida:<subcommand>` colon-namespaced convention as elsewhere.
        #[clap(long, value_name = "TAGS")]
        tags: Option<String>,
    },

    /// Increment a finding's recurrence counter. Use when you spot the same
    /// pattern again — the counter survives in the `recurrence:N` tag and
    /// the optional `--note` appends to the audit trail. Recurrence ≥ 3 is
    /// the promote-it signal (see `.aida/discipline/observation-discipline.md`).
    // trace:STORY-467 | ai:claude
    Recur {
        /// The finding's ID (UUID or SPEC-ID).
        id: String,

        /// What you saw this time. Appended as a timestamped audit comment.
        // trace:BUG-1294 | ai:claude
        #[clap(long, value_name = "TEXT", allow_hyphen_values = true)]
        note: Option<String>,
    },

    /// Count review findings per defect class across every recorded verdict,
    /// including archived rounds. Only findings recorded with a class
    /// (`aida review record --finding-class`) are counted; free text is never
    /// classified after the fact. `aida review classes` is the same report.
    // trace:STORY-1417 | ai:claude
    Classes {
        /// Only count rounds recorded since this point: a relative window
        /// (`45m`, `12h`, `30d`, `2w`, or `24 hours ago`), an ISO date
        /// (`2026-05-01`, local midnight), a zone-less ISO datetime
        /// (`2026-05-01T10:00`, local time), or an RFC3339 timestamp.
        #[clap(long, value_name = "WHEN")]
        since: Option<String>,

        /// Emit the report as JSON.
        #[clap(long)]
        json: bool,
    },

    /// List draft findings awaiting triage, grouped by source then origin and
    /// severity-sorted.
    List {
        /// Narrow to review findings raised against one PR.
        #[clap(long, value_name = "N")]
        pr: Option<u32>,

        /// Narrow to findings from one source: `review`, `implementer`, or
        /// `advisor`.
        #[clap(long, value_enum)]
        source: Option<crate::findings::FindingSource>,

        /// Narrow to findings carrying `kind:<value>` — implementer kinds
        /// (`deviation`, `design-choice`, `bug-spotted`,
        /// `followup-suggestion`) or advisor kinds (`observation`).
        #[clap(long, value_name = "KIND")]
        kind: Option<String>,

        /// Print just the pending-finding count (for session-start surfacing).
        #[clap(long)]
        count: bool,

        /// Machine-readable JSON: `{findings: [...], punts: [...],
        /// shelved: [...], total}`. Same document as `--format json`.
        // trace:BUG-1289 | ai:claude
        #[clap(long)]
        json: bool,
    },

    /// Dismiss a finding — sets status Rejected and records an audit comment.
    Dismiss {
        /// The finding's ID (UUID or SPEC-ID).
        id: String,

        /// Rationale for the dismissal. When provided, the audit comment
        /// includes the text verbatim (e.g. *"Dismissed by joe 2026-05-20:
        /// duplicates an earlier filing"*) instead of the bare "Dismissed"
        /// marker — so the *why* lands in one command instead of two.
        // trace:TASK-404 | ai:claude
        // trace:TASK-420 | ai:claude
        // trace:BUG-1294 | ai:claude
        #[clap(long, value_name = "TEXT", allow_hyphen_values = true)]
        reason: Option<String>,
    },

    /// Promote a finding — sets status Approved and adds it to a work queue.
    Promote {
        /// The finding's ID (UUID or SPEC-ID).
        id: String,

        /// Route the promoted finding to this role's work queue. Defaults
        /// to `implementer` — a promoted finding is implementation work.
        #[clap(long = "for", value_name = "ROLE")]
        r#for: Option<String>,

        /// Rationale for the promotion ("more important than its priority
        /// suggests", "blocks another open PR", etc.). When provided, the
        /// rationale is appended as an audit comment so the *why* survives
        /// alongside the queue note.
        // trace:TASK-404 | ai:claude
        // trace:TASK-420 | ai:claude
        // trace:BUG-1294 | ai:claude
        #[clap(long, value_name = "TEXT", allow_hyphen_values = true)]
        reason: Option<String>,

        /// When the finding's origin-ID fix has already merged to the default
        /// branch, bump the promoted spec straight to Completed instead of
        /// queueing it as fresh work. Without this flag, an already-merged
        /// origin-ID only prints a warning and the finding still queues.
        // trace:TASK-579 | ai:claude
        #[clap(long = "auto-complete")]
        auto_complete: bool,

        /// Promote (and queue) even when the origin-ID fix already merged —
        /// for reopening or extending the finding. Suppresses the
        /// already-merged warning / auto-complete entirely.
        // trace:TASK-579 | ai:claude
        #[clap(long)]
        force: bool,

        /// Destination: `work` (the default — fix this instance) or `gate`
        /// (gate the whole class). The gate destination is offered once the
        /// finding's recurrence count reaches the promote threshold.
        // trace:STORY-1428 | ai:claude
        #[clap(long = "to", value_name = "DEST", default_value = "work")]
        to: String,

        /// Gate screening answer, required with `--to gate`: `mechanical`
        /// (a violation is recognisable without judgement), `agent` (only an
        /// agent could recognise it; its verdict is advisory), or `none`
        /// (stays prose — recorded so the question is not re-opened).
        // trace:STORY-1428 | ai:claude
        #[clap(long, value_name = "ANSWER")]
        detectable: Option<String>,
    },

    /// Calibration review surface — list cold-boot vs fork-from-live
    /// advisor verdicts side-by-side. Default view shows disagreements
    /// (the rows that name a substrate gap); `--all` widens to agreements
    /// too, and `--stats` swaps the table for the rolling metric.
    /// Annotate one row with the `annotate` sub-action.
    // trace:STORY-347 | ai:claude
    Calibration {
        #[clap(subcommand)]
        action: Option<CalibrationAction>,

        /// Restrict to records since this point: a relative window (`7d`,
        /// `12h`, `2w`, `30m`, or `24 hours ago`), an ISO date (`2026-05-01`,
        /// local midnight), a zone-less ISO datetime (local time), or RFC3339.
        // trace:TASK-1509 | ai:claude
        #[clap(long, value_name = "WINDOW")]
        since: Option<String>,

        /// Show only records where the two advisors agreed.
        #[clap(long, conflicts_with_all = ["disagreement", "all"])]
        agreement: bool,

        /// Show only records where the two advisors disagreed. The default
        /// (no flag) is equivalent — disagreements are the triage signal.
        #[clap(long, conflicts_with_all = ["agreement", "all"])]
        disagreement: bool,

        /// Show every record (agreements + disagreements + no-fork rows).
        #[clap(long, conflicts_with_all = ["agreement", "disagreement"])]
        all: bool,

        /// Print the rolling-metric summary (agreement rate over the last N
        /// records, 4-week trend, annotation-category histogram) instead
        /// of the per-record table.
        #[clap(long)]
        stats: bool,

        /// Number of records the `--stats` agreement rate considers.
        /// Default 50.
        #[clap(long, value_name = "N", default_value_t = 50)]
        last: usize,

        /// Emit a machine-readable JSON view instead of the human table.
        #[clap(long)]
        json: bool,
    },
}

// `aida findings calibration <ACTION>` — sub-action of the calibration
// review surface. The base verb (`aida findings calibration` with no
// action) lists records; the `annotate` action attaches a one-line note
// to a specific punt-id. trace:STORY-347 | ai:claude
#[derive(Subcommand, Debug)]
pub enum CalibrationAction {
    /// Annotate a calibration record. The note is a one-line triage hint;
    /// the prefix categories `gap → wrote memory <name>`, `inherently
    /// in-flight, accept`, and `cold-boot was actually correct` feed the
    /// `--stats` histogram, but any text is accepted.
    Annotate {
        /// The punt-id (the `<SPEC>-<unix-seconds>` directory name under
        /// `.aida/punts/`).
        punt_id: String,
        /// The annotation text. Trimmed; stored verbatim.
        note: String,
    },
}
