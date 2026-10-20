//! `aida comment_cmd` clap types, owned by `comment_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum CommentCommand {
    /// Add a comment to a requirement
    Add {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,

        /// Comment content. Prefer the positional `[CONTENT]`; this flag is a
        /// hidden backward-compat alias kept so existing `--content` scripts
        /// keep working.
        // trace:TASK-778 — de-duplicated from the positional [CONTENT];
        // hidden from --help so the two forms don't read as distinct args.
        // trace:BUG-1294 | ai:claude
        #[clap(long, hide = true, allow_hyphen_values = true, conflicts_with_all = ["content_positional", "body_file", "stdin", "interactive"])]
        content: Option<String>,

        /// Comment content (positional argument)
        #[clap(value_name = "CONTENT", conflicts_with_all = ["content", "body_file", "stdin", "interactive"])]
        content_positional: Option<String>,

        /// Read comment content from a file. Prefer this for text containing
        /// backticks or `$()` so the shell cannot perform command substitution.
        #[clap(long, value_name = "PATH", conflicts_with_all = ["content", "content_positional", "stdin", "interactive"])]
        body_file: Option<PathBuf>,

        /// Read comment content from stdin.
        #[clap(long, conflicts_with_all = ["content", "content_positional", "body_file", "interactive"])]
        stdin: bool,

        /// Author of the comment (defaults to AIDA_AUTHOR env var or system user)
        #[clap(long)]
        author: Option<String>,

        /// The seat whose claim this comment relays. Use it when recording
        /// another seat's measurement, finding or argument rather than
        /// something you verified; it is stored with the comment and shown
        /// as "via <author>, originally <seat>".
        // trace:BUG-1534 | ai:claude
        #[clap(long, value_name = "SEAT")]
        relayed_from: Option<String>,

        /// Parent comment ID (for replies)
        #[clap(long)]
        parent: Option<String>,

        /// Use interactive mode (prompts)
        #[clap(long)]
        interactive: bool,
    },

    /// List all comments for a requirement
    List {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
    },

    /// Edit a comment
    Edit {
        /// Requirement ID (UUID or SPEC-ID)
        #[clap(long)]
        req_id: String,

        /// Comment ID to edit
        #[clap(long)]
        comment_id: String,

        /// New content
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true, conflicts_with = "interactive")]
        content: Option<String>,

        /// Read replacement content from a file. Prefer this for text
        /// containing backticks or `$()`.
        #[clap(long, value_name = "PATH", conflicts_with_all = ["content", "stdin", "interactive"])]
        body_file: Option<PathBuf>,

        /// Read replacement content from stdin.
        #[clap(long, conflicts_with_all = ["content", "body_file", "interactive"])]
        stdin: bool,

        /// Use interactive mode (prompts)
        #[clap(long)]
        interactive: bool,
    },

    /// Delete a comment
    Delete {
        /// Requirement ID (UUID or SPEC-ID)
        #[clap(long)]
        req_id: String,

        /// Comment ID to delete
        #[clap(long)]
        comment_id: String,
    },
}
