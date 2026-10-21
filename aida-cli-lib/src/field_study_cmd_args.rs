//! `aida field_study_cmd` clap types, owned by `field_study_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Observe-only rule-adherence field study.
///
/// The git log is the planted sensor: every commit records its message + diff.
/// `scan` recomputes the stated-rule verdicts (commit-format, trace-presence)
/// over recent commits into a local-only log; `report` aggregates it. Opt-in —
/// nothing is recorded until `AIDA_FIELD_STUDY=1` or `[field_study] enabled =
/// true`.
// trace:SPIKE-67 | ai:claude
#[derive(Subcommand, Debug)]
pub enum FieldStudyCommand {
    /// Harvest rule-adherence verdicts from recent commits into the local
    /// field-study log. Idempotent — a commit already recorded is skipped.
    // trace:SPIKE-67 | ai:claude
    Scan {
        /// Git revision range to scan (e.g. `HEAD~200`, a tag, `v1.0..HEAD`).
        /// A value starting with `-` is refused. Default: the most recent
        /// commits up to `--limit`.
        // trace:SPIKE-67 | ai:claude
        // trace:BUG-1622 | ai:claude
        #[clap(long, value_name = "REV")]
        since: Option<String>,
        /// Cap how many commits are inspected. Default 200.
        // trace:SPIKE-67 | ai:claude
        #[clap(long, default_value = "200")]
        limit: usize,
    },
    /// Report adherence rates from the local field-study log, bucketed by task
    /// span — the "does the would-block rate rise with span?" lens.
    // trace:SPIKE-67 | ai:claude
    Report {
        /// Emit a JSON object for machine consumers.
        // trace:SPIKE-67 | ai:claude
        #[clap(long)]
        json: bool,
    },
    /// Report the live drain-observed stated-rule violations — the real-time
    /// gate-vs-rule signal: which stated rules a real drain broke (CI red on
    /// fmt/clippy/provenance, a punt, a reviewer flag) with no gate to stop
    /// them.
    // trace:SPIKE-67 | ai:claude
    Violations {
        /// Emit a JSON object for machine consumers.
        // trace:SPIKE-67 | ai:claude
        #[clap(long)]
        json: bool,
    },
}
