//! `aida scaffold_cmd` clap types, owned by `scaffold_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// What `aida scaffold upgrade --only <SCOPE>` limits the upgrade to.
// trace:BUG-1977 | ai:claude
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScaffoldUpgradeOnly {
    /// Only the AIDA git hooks git would not run as this build ships them:
    /// a stale AIDA-generated body is backed up beside itself and replaced,
    /// and a hook without an executable bit is set to 0755. Custom hooks
    /// and every other file are left alone.
    // trace:BUG-2009 | ai:claude
    GitHooks,
}

/// Commands for scaffolding management
#[derive(Subcommand, Debug)]
pub enum ScaffoldCommand {
    /// Check scaffolding status against actual project files
    Status {
        /// Project root directory (defaults to current directory)
        #[clap(long)]
        project_root: Option<PathBuf>,

        /// Show detailed file comparisons
        #[clap(long, short = 'v')]
        verbose: bool,

        /// Generate HTML report with diffs
        #[clap(long)]
        report: bool,

        /// Output file for report (defaults to stdout)
        #[clap(long, short = 'o')]
        output: Option<PathBuf>,
    },

    /// Preview scaffolding artifacts without applying
    Preview {
        /// Project root directory (defaults to current directory)
        #[clap(long)]
        project_root: Option<PathBuf>,
    },

    /// Apply scaffolding to project
    Apply {
        /// Project root directory (defaults to current directory)
        #[clap(long)]
        project_root: Option<PathBuf>,

        /// Overwrite existing files
        #[clap(long)]
        force: bool,

        /// Show what would be done without making changes
        #[clap(long)]
        dry_run: bool,

        /// Remove obsolete `aida-*` skills/commands/hooks that this AIDA
        /// version no longer ships (left over from an older version). Without
        /// this, they are only reported. Symlinks and non-`aida-` files are
        /// never touched.
        // trace:BUG-298 | ai:claude
        #[clap(long)]
        prune: bool,
    },

    /// Extract embedded templates to disk for customization
    Extract {
        /// Directory to extract templates to (defaults to ~/.config/aida/templates)
        #[clap(long)]
        output: Option<PathBuf>,

        /// Overwrite existing files
        #[clap(long)]
        force: bool,
    },

    /// Write legacy Codex prompt bodies to ~/.codex/prompts.
    ///
    /// Current Codex CLI releases do not discover these files as `/aida-*`
    /// slash commands. For interactive Codex sessions, use the project's
    /// `.agents/skills/` via `/skills` or `$aida-*`, or run the matching
    /// `aida ...` CLI verb directly. On Codex >=0.142 this command prints that
    /// warning and skips writing the dead prompt surface.
    // trace:BUG-1095 | ai:codex
    // trace:BUG-1118 | ai:codex
    CodexPrompts {
        /// Destination directory (defaults to ~/.codex/prompts)
        #[clap(long)]
        dest: Option<PathBuf>,

        /// Overwrite existing prompt files
        #[clap(long)]
        force: bool,
    },

    /// Install the user-global AIDA awareness snippet for Claude and Codex.
    ///
    /// Writes marker-delimited, edit-preserving blocks to `~/.claude/CLAUDE.md`
    /// and `~/.codex/AGENTS.md`. Existing content outside the markers is left
    /// byte-for-byte intact; `--refresh` updates only an unedited AIDA block.
    // trace:STORY-831 | ai:codex
    UserInstructions {
        /// Refresh unedited AIDA blocks to this binary's current snippet.
        #[clap(long)]
        refresh: bool,
    },

    /// Bring every installed agent pack level with this binary's templates,
    /// preserving your edits.
    ///
    /// Covers Claude skills and commands, Codex skills, and Antigravity skills.
    /// A pack file whose body still matches the scaffold checksum it was
    /// written with is overlaid with the current template; one you have
    /// edited, one with no scaffold marker, and one that is a symlink are
    /// left exactly as they are. Packs you never installed are not created.
    /// If a legacy `~/.codex/prompts` pack exists, refresh reports it with a
    /// prune hint and leaves it untouched.
    ///
    /// This is the no-`--force` delivery path for a template fix — the same
    /// contract `aida init --with-memories --refresh` has for memories.
    // trace:TASK-1170 | ai:claude
    Refresh {
        /// Project root directory (defaults to current directory)
        #[clap(long)]
        project_root: Option<PathBuf>,

        /// Codex custom-prompt directory to check for a deprecation notice
        /// (defaults to ~/.codex/prompts). Matches `scaffold codex-prompts --dest`.
        #[clap(long)]
        dest: Option<PathBuf>,
    },

    /// Apply category-aware upgrades to scaffolded files. The right
    /// follow-up to `scaffold status` reporting drift — does the right
    /// thing per file category instead of `apply --force`'s blast-radius.
    ///
    /// Per category (see `docs/plans/2026-05-04-scaffold-categorization.md`):
    ///   - **template** (`.claude/skills/*`, `.claude/commands/*`,
    ///     `.claude/hooks/*`, `.claude/AIDA.md`, `.agents/skills/aida-*/**`,
    ///     `.git/hooks/commit-msg`) — AIDA-owned; drifted files are
    ///     overwritten with the embedded template.
    ///   - **seed** (CLAUDE.md, AGENTS.md) — user-owned post-init;
    ///     drifted files are LEFT ALONE. Missing files are created.
    ///     AGENTS.md's delimited AIDA-AUTOGEN block IS upgraded.
    ///   - **managed-merge** (`.claude/settings.json`, `.mcp.json`) —
    ///     v1 leaves existing files alone (slot-merge deferred). Missing
    ///     files are created.
    ///
    /// Output groups by category and only mentions files that need
    /// attention or actually changed — no per-file noise for the
    /// happy-path matching files.
    // trace:FR-1-028 | ai:claude
    Upgrade {
        /// Project root directory (defaults to current directory)
        #[clap(long)]
        project_root: Option<PathBuf>,

        /// Show what would change without writing anything.
        #[clap(long)]
        dry_run: bool,

        /// Override the category strategy and overwrite every drifted
        /// file regardless of category. Equivalent to today's
        /// `apply --force` but with the cleaner per-category output.
        /// Use sparingly — overwrites your CLAUDE.md / AGENTS.md.
        #[clap(long)]
        force: bool,

        /// Remove obsolete `aida-*` skills/commands/hooks this AIDA
        /// version no longer ships (e.g. a skill deleted upstream but
        /// left behind — or resurrected — by an older binary). Without
        /// this flag they are only reported. Symlinks and non-`aida-`
        /// files are never touched.
        #[clap(long)]
        prune: bool,

        /// Limit the upgrade to one scope. `git-hooks` replaces only the
        /// stale AIDA-generated git hooks a store write refuses (backing
        /// each up beside itself), makes non-executable AIDA hooks 0755,
        /// and touches no other file.
        // trace:BUG-1977 trace:BUG-2009 | ai:claude
        #[clap(long, value_enum, conflicts_with_all = ["force", "prune"])]
        only: Option<ScaffoldUpgradeOnly>,
    },

    /// Show a unified diff between embedded templates and on-disk files
    /// for any drifted scaffold artifact. Exits 1 if any drift is found,
    /// 0 if clean. Pairs with `aida scaffold status` — when that reports
    /// "N modified" this is how you actually see what changed.
    ///
    /// CLAUDE.md and AGENTS.md are diffed only on the AIDA-managed portion:
    /// CLAUDE.md is presence-only (drift = `@.claude/AIDA.md` import line
    /// missing); AGENTS.md compares just the content between
    /// `<!-- AIDA-AUTOGEN-BEGIN -->` / `<!-- AIDA-AUTOGEN-END -->` markers.
    /// Everything else gets a full-content diff.
    // trace:FR-1-027 | ai:claude
    Diff {
        /// Specific path (relative to project root) to diff. When omitted,
        /// every modified file is diffed in turn with file headers between.
        path: Option<PathBuf>,

        /// Project root directory (defaults to current directory)
        #[clap(long)]
        project_root: Option<PathBuf>,

        /// Force plain output even on a TTY (the colored crate normally
        /// auto-disables when piped). Equivalent to NO_COLOR=1.
        #[clap(long)]
        no_color: bool,

        /// Number of context lines around each hunk
        #[clap(long, short = 'U', default_value = "3")]
        context: usize,

        /// Print only the relative paths of drifted files (one per line),
        /// no diff body. Useful for piping into other tools.
        #[clap(long)]
        list: bool,
    },
}
