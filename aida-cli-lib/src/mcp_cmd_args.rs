//! `aida mcp_cmd` clap types, owned by `mcp_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use crate::cli::SkillCommand;
use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// MCP server management.
// trace:STORY-361 | ai:claude
#[derive(Subcommand, Debug)]
pub enum McpCommand {
    /// Register the AIDA MCP server in `.mcp.json` so MCP-speaking agents
    /// (Codex, Cursor, …) can call its coordination tools.
    // trace:STORY-361 | ai:claude
    // trace:TASK-487 | ai:claude
    ///
    /// For local single-machine use the entry runs `aida mcp-serve` over
    /// stdio. Cross-machine MCP is deferred to a follow-up SPIKE; the
    /// printed URL is `stdio://aida` so configs can switch transport later.
    RegisterAgent {
        /// Name to register the AIDA server under (default: "aida").
        #[clap(long, default_value = "aida")]
        name: String,
        /// Print the rendered config and tool surface to stdout instead of
        /// writing `.mcp.json`.
        #[clap(long)]
        print: bool,
        /// Overwrite an existing entry with the same name.
        #[clap(long)]
        force: bool,
    },

    /// Translate this project's `.mcp.json` (Claude Code's MCP server
    /// registration) into the equivalent Codex CLI (`.codex/config.toml`)
    /// and Gemini CLI (`.gemini/settings.json`) MCP configs, preserving the
    /// registered server's command/args/env — so a headless codex/gemini
    /// drain agent gets AIDA's MCP tools with no hand-editing.
    ///
    /// `aida init` already scaffolds `.codex/config.toml` for a fresh
    /// project; this command is for the reverse direction — deriving both
    /// vendor configs from whatever `.mcp.json` actually says today
    /// (including a hand-edited command/args/env, or a renamed server), and
    /// it's the only path that also covers Gemini CLI.
    ///
    /// Each target file is merged, not clobbered: an existing file without
    /// an aida-shaped entry gets one added; an existing file whose entry
    /// already matches is left alone (reported up to date); an existing
    /// file whose entry differs is skipped unless `--force`. When
    /// `.mcp.json` is absent, or present with no aida server registered,
    /// this reports "nothing to translate" rather than erroring.
    // trace:TASK-1046 | ai:claude
    // trace:TASK-0424 | ai:claude
    Translate {
        /// Project root directory (defaults to current directory).
        #[clap(long)]
        project_root: Option<PathBuf>,

        /// Overwrite a target file's aida entry when it already exists but
        /// differs from what `.mcp.json` would produce.
        #[clap(long)]
        force: bool,

        /// Show what would be written without writing anything.
        #[clap(long)]
        dry_run: bool,
    },

    /// Manage and render agent skills
    #[clap(subcommand)]
    Skill(SkillCommand),
}
