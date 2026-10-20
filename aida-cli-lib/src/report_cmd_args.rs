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

    /// Generate AI integration report
    AiIntegration {
        /// Output format: markdown or html
        #[clap(long, short = 'f', default_value = "markdown")]
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
