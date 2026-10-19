//! `aida sandbox_cmd` clap types, owned by `sandbox_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Throwaway sandbox store for drain-testing and scenario play.
/// The sandbox is an ordinary git-canonical store living under a temp dir; it
/// is targeted via the `AIDA_STORE` env override, so it never touches the
/// project's real `aida-store` orphan branch.
// trace:SPIKE-48 | ai:claude
#[derive(Subcommand, Debug)]
pub enum SandboxCommand {
    /// Create a fresh throwaway store and print the `AIDA_STORE=...` export to
    /// activate it. Idempotent unless `--force` (refuses to clobber a populated
    /// existing sandbox without it).
    Create {
        /// Where to create the sandbox store. Default: a stable per-user temp
        /// dir (`$TMPDIR/aida-sandbox-<user>`), so re-running points at the
        /// same playground.
        #[clap(long)]
        path: Option<PathBuf>,

        /// Seed a few curated scenario specs (a lifecycle walk + a blocked-by
        /// chain) so there's something to play with immediately.
        #[clap(long)]
        seed: bool,

        /// Recreate even if the target already holds a populated sandbox store.
        #[clap(long)]
        force: bool,
    },

    /// Wipe the sandbox store's contents and re-initialize it empty (or seeded
    /// with `--seed`). The directory itself is reused.
    Reset {
        /// Sandbox store dir (default: the per-user temp sandbox).
        #[clap(long)]
        path: Option<PathBuf>,

        /// Re-seed curated scenario specs after the reset.
        #[clap(long)]
        seed: bool,
    },

    /// Delete the sandbox store directory entirely.
    Destroy {
        /// Sandbox store dir (default: the per-user temp sandbox).
        #[clap(long)]
        path: Option<PathBuf>,
    },

    /// Print the path of the (default or `--path`) sandbox store and whether it
    /// exists. With `--export`, print the `AIDA_STORE=...` line to eval.
    Path {
        /// Sandbox store dir (default: the per-user temp sandbox).
        #[clap(long)]
        path: Option<PathBuf>,

        /// Print a shell `export AIDA_STORE=...` line instead of the bare path.
        #[clap(long)]
        export: bool,
    },
}
