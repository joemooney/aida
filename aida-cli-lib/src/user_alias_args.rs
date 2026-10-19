//! `aida user_alias` clap types, owned by `user_alias.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// The `aida alias` verbs. Bare `aida alias` and `aida alias list` print the
/// registry (built-in shortcuts plus your personal/project aliases). `add` /
/// `remove` manage user-defined aliases.
// trace:STORY-667 | ai:claude
// trace:TASK-877 | ai:claude — user-defined alias CRUD verbs.
#[derive(Subcommand, Debug)]
pub enum AliasCommand {
    /// List built-in shortcuts plus your personal/project aliases.
    List {
        /// Emit the registry as JSON for machine consumers.
        #[clap(long)]
        json: bool,
    },
    /// Define a personal or project alias: `aida <name> ...` runs the command.
    ///
    /// Example: `aida alias add approved list --status approved` makes
    /// `aida approved` expand to `aida list --status approved`. Everything
    /// after the name is stored verbatim as the expansion, so put any scope
    /// flag BEFORE the name: `aida alias add --global approved list ...`. An
    /// alias may not shadow a real subcommand. Default scope is the project
    /// when inside an AIDA project, else personal; `--global` / `--project`
    /// force it.
    Add {
        /// The short name typed as `aida <name>`.
        name: String,
        /// The command (after `aida`) the alias expands to. Everything here is
        /// stored verbatim, e.g. `list --status approved`.
        #[clap(trailing_var_arg = true, required = true, num_args = 1..)]
        command: Vec<String>,
        /// Write to the personal store (`~/.aida/aliases.toml`).
        #[clap(long)]
        global: bool,
        /// Write to the project store (`.aida/aliases.toml`, git-trackable).
        #[clap(long)]
        project: bool,
    },
    /// Remove a personal or project alias by name.
    Remove {
        /// The alias name to remove.
        name: String,
        /// Remove from the personal store (`~/.aida/aliases.toml`).
        #[clap(long)]
        global: bool,
        /// Remove from the project store (`.aida/aliases.toml`).
        #[clap(long)]
        project: bool,
    },
}
