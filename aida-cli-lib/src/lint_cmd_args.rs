//! `aida lint` clap types, owned by `lint_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Source lints under `aida lint`. Report-only: they never edit the store.
// trace:TASK-1785 | ai:claude
#[derive(Subcommand, Debug)]
pub enum LintCommand {
    /// Report numeric literals inside traced Rust functions and constants
    /// that the owning spec never mentions. A literal is explained when its
    /// value appears in the description of a spec traced on (or directly
    /// above) the enclosing item, or in a decision linked to that spec.
    /// 0 and 1 are exempt; mark any other deliberate choice with a trailing
    /// `// arbitrary` (or `// arbitrary: <reason>`) comment. Report-only:
    /// exits non-zero only with `--fail-on unexplained`.
    Constants {
        /// File or directory to scan (defaults to the project root).
        #[clap(long, value_name = "PATH")]
        path: Option<PathBuf>,

        /// Emit the report as JSON for agents / scripts.
        #[clap(long)]
        json: bool,

        /// Exit non-zero when the report contains findings of this kind.
        #[clap(long, value_enum, value_name = "KIND")]
        fail_on: Option<LintConstantsFailOn>,
    },
}

/// What `aida lint constants --fail-on` turns into a non-zero exit.
// trace:TASK-1785 | ai:claude
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LintConstantsFailOn {
    /// Any literal the owning spec does not explain.
    Unexplained,
}
