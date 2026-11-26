//! `aida report_cmd` clap types, owned by `report_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Commands for generating reports
#[derive(Subcommand, Debug)]
pub enum ReportCommand {
    /// Compose and optionally file an upstream AIDA bug report.
    Bug {
        /// Short report title. The pasted subject becomes `aida: <title>`.
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        title: String,

        /// Report description. If omitted, reads stdin when piped or opens
        /// $EDITOR at a TTY.
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        description: Option<String>,

        /// Copy the rendered report to the system clipboard as well as stdout.
        #[clap(long)]
        copy: bool,

        /// Do not file a local upstream:aida spec after printing.
        #[clap(long)]
        no_file: bool,
    },

    /// Compose and optionally file an upstream AIDA idea report.
    Idea {
        /// Short report title. The pasted subject becomes `aida: <title>`.
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        title: String,

        /// Report description. If omitted, reads stdin when piped or opens
        /// $EDITOR at a TTY.
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        description: Option<String>,

        /// Copy the rendered report to the system clipboard as well as stdout.
        #[clap(long)]
        copy: bool,

        /// Do not file a local upstream:aida spec after printing.
        #[clap(long)]
        no_file: bool,
    },

    /// Classify open specs and rank areas by hotness (read-only)
    ///
    /// Each open spec gets an area, a size (S/M/L/XL), its priority and status,
    /// and a noise flag for review-finding specs. Areas are ranked by a weighted
    /// blend of open and high-priority specs, change requests in the last 7
    /// days, open PRs and 14-day file churn. A flow section shows landings per
    /// day, open-to-landed time, change-request rounds, rework sinks and stale
    /// claims; recommendations list commands to run, and the report runs none.
    /// The area map, weights and `stale_days` live under `[report.backlog]` in
    /// `.aida/config.toml`.
    // trace:STORY-3143 | ai:claude
    Backlog {
        /// Emit JSON (same as the global `--format json`).
        #[clap(long)]
        json: bool,
    },

    /// Generate AI integration report
    AiIntegration {
        /// Report format: markdown or html
        // Not `--format`: that is the global output-format flag, and a
        // same-named local arg made clap panic at parse. trace:BUG-2189 | ai:claude
        #[clap(
            id = "report_format",
            long = "report-format",
            short = 'f',
            value_name = "FORMAT",
            default_value = "markdown"
        )]
        format: String,

        /// Output file path (defaults to stdout)
        #[clap(long, short = 'o')]
        output: Option<PathBuf>,

        /// Project root directory for scaffolding status check
        #[clap(long)]
        project_root: Option<PathBuf>,

        /// Include scaffolding status in report
        #[clap(long)]
        include_scaffold: bool,
    },
}
