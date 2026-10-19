//! `aida statusline_cmd` clap types, owned by `statusline_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Opt-in statusline bootstrap actions. AIDA's bootstrap goal is to make
/// other projects agent-ready without forcing a house style, so the
/// AIDA-aware statusline is a convenience the user enables deliberately —
/// the bare `aida statusline` render stays the quiet default.
// trace:TASK-0414
#[derive(Subcommand, Debug)]
pub enum StatuslineAction {
    /// Emit the one-liner as an OSC terminal-title escape sequence.
    // trace:STORY-1028 | ai:codex
    Title,

    /// Print (or install) client-appropriate statusline configuration so a
    /// user can enable the AIDA-aware statusline segment. Prints by default;
    /// pass `--install` to write the supported client's `settings.json` entry.
    // trace:TASK-0414
    Setup {
        /// Which client to emit setup for: `claude` (command-backed
        /// statusLine in settings.json), `codex` (built-in TUI footer
        /// fields — Codex does not run arbitrary commands in its footer),
        /// `antigravity` (command-backed statusLine plus title command),
        /// or `all` (default — print guidance for every supported client).
        #[clap(long, value_parser = ["claude", "codex", "antigravity", "all"], default_value = "all")]
        client: String,

        /// Install the configuration instead of only printing it. Only the
        /// `claude` and `antigravity` clients support install (they
        /// write/merge the `statusLine` block into settings.json); Codex
        /// stays print-only because its footer config is hand-edited.
        #[clap(long)]
        install: bool,

        /// Antigravity settings path. Defaults to
        /// `$AIDA_ANTIGRAVITY_SETTINGS`, then
        /// `~/.gemini/antigravity-cli/settings.json`.
        #[clap(long)]
        settings_path: Option<std::path::PathBuf>,

        /// For Antigravity, replace its default status line instead of
        /// stacking AIDA with it (`stack_with_default: false`).
        #[clap(long)]
        replace_default: bool,
    },
}
