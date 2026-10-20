//! `aida brief_cmd` clap types, owned by `brief_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum BriefCommand {
    /// List pending brief files.
    List {
        /// Only list briefs routed to this agent.
        #[clap(long = "for-agent")]
        for_agent: Option<String>,

        /// Include acknowledged briefs.
        #[clap(long)]
        include_acked: bool,
    },

    /// Mark a brief acknowledged so it stops appearing in default lists.
    Ack {
        /// Brief file path, either absolute or relative to the current directory.
        brief_file: PathBuf,
    },

    /// Read a brief file and print its contents to stdout.
    Read {
        /// Brief file path, shortcut (<agent>/<filename>), or agent name.
        brief_file: String,

        /// Read the most recent unacked brief for the given agent.
        #[clap(long)]
        latest: bool,
    },
}
