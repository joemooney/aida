//! `aida review_cmd` clap types, owned by `review_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Persona / hat commands. A role is a persistent named context that you
/// can resume across shells — captures the working directory, last
/// Inspect / resume past Claude Code sessions for this project, enriched
/// with the AIDA role and most-recent spec from each session's .jsonl.
/// Review-prompt generation and related review-workflow helpers. v1:
/// only `prompt`; future subcommands (e.g. `summary`, `checks`) can
/// land here without bloating the top-level `aida` help.
// trace:STORY-67 | ai:claude
// trace:TASK-487 | ai:claude
#[derive(Subcommand, Debug)]
pub enum ReviewCommand {
    /// Generate a markdown review prompt from a set of linked
    /// requirements' acceptance criteria. Specs come either from an
    /// explicit `--specs` list (CSV) or from the commit-range of a
    /// PR/MR (parsed `(REQ-ID)` trailers in commit messages).
    ///
    /// For best results with --pr, install `gh` (https://cli.github.com)
    /// for GitHub or `glab` (https://gitlab.com/gitlab-org/cli) for
    /// GitLab. Without them, AIDA falls back to base=main + a local
    /// review branch named `pr-N` / `mr-N` — works when the PR was
    /// started via `aida session start --owns PR-N`, surprising
    /// otherwise.
    // trace:STORY-67, TASK-40 | ai:claude
    Prompt {
        /// Comma-separated list of spec IDs (e.g. "FR-N,STORY-M"). When
        /// given, --pr is ignored.
        #[clap(long, value_name = "SPEC-IDS")]
        specs: Option<String>,

        /// Pull spec IDs from the PR/MR's commit range. Forge auto-
        /// detected from origin URL; override with --forge github|gitlab.
        /// Resolves base/head via `gh pr view` / `glab mr view` when
        /// installed; otherwise falls back to base=main and a local
        /// review branch named `pr-N` / `mr-N`.
        // trace:TASK-40 | ai:claude
        #[clap(long, value_name = "N", conflicts_with = "specs")]
        pr: Option<u64>,

        /// Forge override for self-hosted GHE / self-hosted GitLab
        /// when --pr is used.
        #[clap(long, value_name = "FORGE")]
        forge: Option<String>,

        /// Write the prompt to PATH instead of stdout. Useful when
        /// piping into a freshly-launched Claude Code session
        /// (`aida review prompt --pr 2 --write .aida-review-prompt.md`).
        #[clap(long, value_name = "PATH")]
        write: Option<String>,
    },

    /// Assemble active review fragments into a root REVIEW.md file.
    // trace:SPIKE-35 | ai:antigravity
    Assemble {
        /// Output path for the assembled REVIEW.md file (default: REVIEW.md in project root).
        #[clap(long, short = 'o', value_name = "PATH")]
        output: Option<PathBuf>,
    },

    /// Record a review verdict for a spec as first-class state.
    ///
    /// A verdict written only as prose (a comment saying "changes
    /// requested at commit abc123") cannot be enforced. Recording it here
    /// stores the verdict, the commit the review examined, and the time —
    /// and `aida queue done` then REFUSES to mark the spec done while the
    /// branch still sits at that commit.
    ///
    /// Record `--verdict approved` after a follow-up review to clear it.
    // trace:BUG-775 | ai:claude
    Record {
        /// Spec the verdict applies to.
        spec: String,

        /// The verdict: approved, request-changes, or rejected.
        #[clap(long, value_name = "VERDICT")]
        verdict: String,

        /// Commit the review examined. Defaults to the branch tip
        /// (current HEAD) at the time of recording.
        #[clap(long, value_name = "SHA")]
        sha: Option<String>,

        /// Branch the review examined. Defaults to the checked-out branch.
        #[clap(long, value_name = "BRANCH")]
        branch: Option<String>,

        /// One-line rationale stored with the verdict and shown wherever
        /// the verdict surfaces.
        // trace:BUG-1294 | ai:claude
        #[clap(long, value_name = "TEXT", allow_hyphen_values = true)]
        summary: Option<String>,

        /// Concrete review finding to hand to the rework implementer. Repeat
        /// for multiple findings; omitted findings preserve any existing
        /// findings[] in the verdict record.
        // trace:TASK-1189 | ai:codex
        // Review findings routinely quote CLI flags. Treat the next argv as
        // prose even when it begins with `-`; later argv remain ordinary flags.
        // trace:TASK-182 | ai:codex
        #[clap(long, value_name = "TEXT", allow_hyphen_values = true)]
        finding: Vec<String>,

        /// Defect class for a finding, paired with `--finding` by position:
        /// the first `--finding-class` belongs to the first `--finding`, and
        /// so on. Use `-` to leave one finding unclassified. Classes:
        /// incomplete-fix, fail-open, absent-evidence-reads-as-good,
        /// untested-path, contract-drift, race, perf, portability,
        /// stale-base, scope-creep. An unknown class is warned about and the
        /// finding is recorded unclassified — it never blocks the verdict.
        // trace:STORY-1417 | ai:claude
        #[clap(long, value_name = "CLASS")]
        finding_class: Vec<String>,

        /// Also write the orchestrator's phase-3 handshake file
        /// (`.aida/review-verdicts/PR-<N>.json`) for this PR number, at the
        /// drive root. This is what lets a headless drain's phase 4 proceed.
        // trace:BUG-802 | ai:claude
        #[arg(long)]
        pr: Option<u64>,
    },

    /// Mark a pull request as under review, so nothing merges it before
    /// your verdict lands.
    ///
    /// While the claim is live, `aida pr ship` and a drain's merge phase
    /// refuse to merge the PR at the claimed head, and `aida awaiting`
    /// shows it as under review. `aida review record … --pr N` clears the
    /// claim; so does `--release`. It also expires on its own after
    /// `--ttl-mins`, so an abandoned review never holds a PR indefinitely.
    // trace:STORY-1405 | ai:claude
    Claim {
        /// Pull request number being reviewed.
        #[clap(long, value_name = "N")]
        pr: u64,

        /// Head commit under review. Defaults to the PR's current head.
        #[clap(long, value_name = "SHA")]
        sha: Option<String>,

        /// Spec the PR implements, shown alongside the claim.
        #[clap(long, value_name = "SPEC")]
        spec: Option<String>,

        /// Minutes until the claim expires on its own. Capped at 1440 (24h);
        /// a larger value is clamped, not refused.
        #[clap(long, value_name = "MINUTES", default_value_t = 30)]
        ttl_mins: u64,

        /// Remove the claim instead of placing it (an abandoned review).
        #[clap(long)]
        release: bool,
    },

    /// Show the recorded review verdict for a spec, if any.
    // trace:BUG-775 | ai:claude
    Verdict {
        /// Spec to look up.
        spec: String,

        /// Emit the raw record as JSON.
        #[clap(long)]
        json: bool,
    },

    /// List recorded review verdicts.
    // trace:TASK-1590 | ai:antigravity
    List {
        /// Emit the list as JSON.
        #[clap(long)]
        json: bool,
    },

    /// Count review findings per defect class across every recorded verdict,
    /// including archived rounds. Alias of `aida findings classes`.
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

    /// One-time repair: expand every abbreviated `reviewed_sha` on disk to
    /// its full commit sha where this repo can still resolve it, and mark
    /// the rest explicitly unresolvable. Never invents a value and never
    /// touches an already-full or sha-less record.
    // trace:BUG-1516 | ai:claude
    NormalizeShas {
        /// Report what would change without writing anything.
        #[clap(long)]
        dry_run: bool,
    },

    /// Report specs stranded by a refusal at the PR's current head: still
    /// Done, unheld, unqueued. Read-only by default; `--fix` applies the
    /// same protection a fresh refusal gets (merge hold + Needs Attention).
    Stranded {
        /// Emit the raw report as JSON.
        #[clap(long)]
        json: bool,

        /// Explicit opt-in remediation: hold the PR and park the spec in
        /// Needs Attention. Idempotent — a spec already protected is left
        /// alone, so a second run changes nothing.
        #[clap(long, conflicts_with = "age")]
        fix: bool,

        /// Offline staleness report instead of the (forge-backed) sweep
        /// above: every spec whose last review verdict still blocks done and
        /// whose branch — checked against LOCAL git only, no network call —
        /// shows no newer commit since the reviewed commit. Sorted oldest
        /// refusal first, each row carrying the measured stall time (or
        /// "unknown" when it can't be established).
        // trace:TASK-1423 | ai:claude
        #[clap(long, conflicts_with = "fix")]
        age: bool,
    },

    /// Turn a temporary review mode on or off, or show its state.
    ///
    /// `aida review mode mass-change on` lets reviewers defer acceptance
    /// gaps instead of blocking on them, for a fixed window (one week by
    /// default). Turning it on records when and by whom; it turns itself off
    /// when the window closes, and `aida status` shows it while it is on.
    // trace:STORY-1415 | ai:claude
    Mode {
        #[clap(subcommand)]
        mode: ReviewModeCommand,
    },
}

/// The temporary review modes `aida review mode` switches.
// trace:STORY-1415 | ai:claude
#[derive(Subcommand, Debug)]
pub enum ReviewModeCommand {
    /// Mass-change mode: acceptance gaps are deferred as tracked findings
    /// instead of blocking, for a fixed window. `on` starts the clock,
    /// `off` ends it early and reports the deferred findings left open,
    /// `status` shows the state, day and deferred-finding count.
    MassChange {
        /// on, off, or status.
        #[clap(value_enum)]
        action: MassChangeAction,

        /// Window length in days when turning the mode on (1-30).
        /// Defaults to 7.
        #[clap(long, value_name = "N")]
        days: Option<i64>,

        /// Emit the state as JSON.
        #[clap(long)]
        json: bool,
    },
}

/// What `aida review mode mass-change` does.
// trace:STORY-1415 | ai:claude
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MassChangeAction {
    /// Turn the mode on and start its clock.
    On,
    /// Turn the mode off now.
    Off,
    /// Show whether the mode is on and how long is left.
    Status,
}

impl MassChangeAction {
    pub fn as_str(self) -> &'static str {
        match self {
            MassChangeAction::On => "on",
            MassChangeAction::Off => "off",
            MassChangeAction::Status => "status",
        }
    }
}
