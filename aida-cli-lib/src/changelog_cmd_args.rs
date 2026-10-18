//! `aida changelog_cmd` clap types, owned by `changelog_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use crate::cli::PlanCommand;
use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Auto-generated changelog tooling. Mirrors `PlanCommand`'s shape.
// trace:TASK-299 | ai:claude
#[derive(Subcommand, Debug)]
pub enum ChangelogCommand {
    /// Print the full changelog (or a bounded slice) to stdout, or write
    /// it to `--out`. The default scope is every `v*` tag plus an
    /// `[Unreleased]` section for commits since the most recent tag.
    // trace:TASK-956 | ai:claude — folded into the `changelog` surface: hidden
    // from `changelog --help` (the canonical idempotent rewrite is `changelog
    // refresh`, used by release.sh), but the `generate` name still runs its own
    // print-to-stdout logic. It is NOT a true alias of `refresh` — `generate`
    // defaults to stdout over the full history with `--since/--until` range
    // flags, whereas `refresh` writes CHANGELOG.md with `--released-as`, so
    // collapsing it would change the sink and drop the range flags. Hiding
    // preserves the name + behavior, off `--help`.
    #[clap(hide = true)]
    Generate {
        /// Only include releases at or after this tag (inclusive). May
        /// be combined with `--until` to bound the span.
        #[clap(long, value_name = "TAG")]
        since: Option<String>,

        /// Only include releases at or before this tag (inclusive).
        #[clap(long, value_name = "TAG")]
        until: Option<String>,

        /// Write to this file instead of stdout.
        #[clap(long, value_name = "PATH")]
        out: Option<PathBuf>,
    },

    /// Regenerate `CHANGELOG.md` at the repo root with every release.
    /// Idempotent — same git state + spec store → byte-identical output.
    /// Used by `release.sh` to land an up-to-date changelog with the
    /// version-bump commit.
    Refresh {
        /// Render commits-since-the-last-tag under `[<version>] —
        /// <today>` instead of `[Unreleased]`. `release.sh` passes the
        /// about-to-be-tagged version here so the changelog commits
        /// with the bump (the new tag does not exist yet).
        #[clap(long, value_name = "VERSION")]
        released_as: Option<String>,

        /// Write to this file instead of `<repo-root>/CHANGELOG.md`.
        #[clap(long, value_name = "PATH")]
        out: Option<PathBuf>,
    },

    /// Print only the `[Unreleased]` section (commits since the most
    /// recent tag) — a quick "what would land in the next release" view.
    Preview,
}
