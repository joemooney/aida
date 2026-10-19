//! `aida type_cmd` clap types, owned by `type_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum TypeCommand {
    /// List all requirement types
    List,

    /// Add a new requirement type
    Add {
        /// Name of the type (e.g., "Business")
        #[clap(long)]
        name: String,

        /// Prefix for the type (e.g., "BR")
        #[clap(long)]
        prefix: String,

        /// Description of the type
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        description: Option<String>,
    },

    /// Remove a requirement type
    Remove {
        /// Name or prefix of the type to remove
        name: String,

        /// Skip confirmation prompt
        #[clap(long, short = 'y')]
        yes: bool,
    },
}
