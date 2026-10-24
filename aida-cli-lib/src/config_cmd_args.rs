//! `aida config_cmd` clap types, owned by `config_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Configure this clone's store-only transport profile (never shared).
    // trace:STORY-1742 | ai:codex
    #[clap(subcommand)]
    Transport(crate::transport_profile::TransportCommand),

    /// Show current ID configuration
    Show {
        /// Optional section to show, e.g. `store.sync`.
        // trace:STORY-284 | ai:codex
        section: Option<String>,
    },

    /// Set the ID format (single-level or two-level)
    Format {
        /// Format: "single" for PREFIX-NNN, "two" for FEATURE-TYPE-NNN
        // Distinct from the global --format id. trace:BUG-2189 | ai:claude
        #[clap(id = "id_format", value_name = "FORMAT")]
        format: String,
    },

    /// Set the numbering strategy
    Numbering {
        /// Strategy: "global", "per-prefix", or "per-feature-type"
        strategy: String,
    },

    /// Set the number of digits in IDs
    Digits {
        /// Number of digits (1-6)
        digits: u8,
    },

    /// Migrate existing SPEC-XXX IDs to new format
    Migrate {
        /// Skip confirmation prompt
        #[clap(long, short = 'y')]
        yes: bool,
    },

    /// Show or update user-level preferences at `~/.aida/preferences.toml`.
    /// Currently stores: preferred node id (used as default by `aida init`)
    /// and a fallback email (used when `git config user.email` is unset).
    // trace:STORY-44 | ai:claude
    User {
        /// Set the preferred node id (validated as `[A-Za-z0-9][A-Za-z0-9_-]*`,
        /// 1-32 chars). Pass empty string to clear.
        #[clap(long)]
        node_id: Option<String>,

        /// Set the fallback email. Pass empty string to clear.
        #[clap(long)]
        email: Option<String>,

        /// Print current preferences as TOML to stdout (machine-readable).
        #[clap(long)]
        toml: bool,
    },

    /// Show or toggle workflow hints printed inline at state-transition
    /// moments (queue drained → open PR, session end → start review).
    /// No-arg form prints the current setting; pass `true` / `false` to
    /// persist into `.aida/config.toml [hints] workflow_hints`. The env
    /// var `AIDA_HINTS=false` overrides the config for the current shell
    /// without writing to disk.
    // trace:STORY-106 | ai:claude
    Hints {
        /// `true` to enable, `false` to disable. Omit to show the
        /// current effective value (env-aware).
        enabled: Option<String>,
    },

    /// Inspect and customize the CLI glyph set — list the symbols, apply a
    /// canned theme, or override an individual glyph. Writes to
    /// `.aida/config.toml` (or `~/.aida/config.toml` with `--user`),
    /// preserving the rest of the file.
    // trace:STORY-633 | ai:claude
    #[clap(subcommand)]
    Glyph(GlyphCommand),

    /// Inspect the effective launch permission posture for managed agents.
    // trace:STORY-1127 | ai:codex
    #[clap(subcommand)]
    Permissions(ConfigPermissionsCommand),

    /// Browse every configurable item in a navigable TUI: per row the knob's
    /// name, current value, built-in default, where it was set (scope), and a
    /// one-line explanation. The visual companion to `config show` — same
    /// resolved surface, one screen, arrow-key navigation. Safe knobs can be
    /// edited in place; integrity-floor knobs stay read-only or subcommand-only.
    /// Needs a TTY; without one it points you at `config show`.
    // trace:STORY-661 trace:STORY-1131 | ai:claude+codex
    Menu,
}

// trace:STORY-1127 | ai:codex
#[derive(Subcommand, Debug)]
pub enum ConfigPermissionsCommand {
    /// Show the resolved per-agent posture and diagnostics. Read-only.
    Show {
        /// Emit machine-readable JSON.
        #[clap(long)]
        json: bool,
    },

    /// Apply a named agent permission posture to local or user config files.
    /// `bypass` must be confirmed by a person at an interactive terminal; it is
    /// refused when stdin or stdout is not a terminal.
    // trace:STORY-1128 | ai:codex
    // trace:BUG-1667 | ai:claude
    Set {
        /// Permission posture to write.
        tier: ConfigPermissionTier,

        /// Write user-global files under `~/.aida` and `~/.codex`.
        #[clap(long, conflicts_with = "local")]
        user: bool,

        /// Write project-local files under `.aida` and `.codex` (default).
        #[clap(long)]
        local: bool,
    },
}

// trace:STORY-1128 | ai:codex
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum ConfigPermissionTier {
    Contained,
    Native,
    Bypass,
}

/// `aida config glyph ...` — CLI surface over the glyph registry, themes, and
/// the per-symbol override table.
// trace:STORY-633 | ai:claude (EPIC-45 phase 4)
#[derive(Subcommand, Debug)]
pub enum GlyphCommand {
    /// List every glyph: name, currently-resolved rendering (honoring the
    /// active profile + theme + overrides), and the unicode registry default.
    List,

    /// Inspect or apply glyph themes. With no NAME (or `theme list`), lists the
    /// embedded themes each with a one-line preview row. With a NAME, applies
    /// that theme — writes `[ui] theme = "<name>"` (a clean reference).
    Theme {
        /// Theme name to apply. Omit (or pass `list`) to list available themes.
        name: Option<String>,
        /// Materialize the theme's bundle into `[glyphs]` for hand-tweaking
        /// instead of writing the named reference.
        #[clap(long)]
        expand: bool,
        /// Write to `~/.aida/config.toml` instead of the project config.
        #[clap(long)]
        user: bool,
    },

    /// Override an individual glyph: writes `[glyphs] <name> = "<value>"`.
    Set {
        /// Glyph name (see `aida config glyph list`).
        name: String,
        /// Replacement string to render for this glyph.
        value: String,
        /// Write to `~/.aida/config.toml` instead of the project config.
        #[clap(long)]
        user: bool,
    },

    /// Drop a single per-symbol override.
    Unset {
        /// Glyph name whose override to remove.
        name: String,
        /// Operate on `~/.aida/config.toml` instead of the project config.
        #[clap(long)]
        user: bool,
    },

    /// Clear ALL per-symbol overrides (the whole `[glyphs]` table).
    Reset {
        /// Operate on `~/.aida/config.toml` instead of the project config.
        #[clap(long)]
        user: bool,
    },
}
