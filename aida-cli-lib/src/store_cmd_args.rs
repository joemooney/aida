//! `aida store_cmd` clap types, owned by `store_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Operations on the orphan-store SHA pin (the `Aida-Store: <sha>`
/// trailer in every code commit).
// trace:EPIC-21 | ai:claude
#[derive(Subcommand, Debug)]
pub enum StoreCommand {
    /// Show the relationship between the current code commit and the
    /// orphan store: which store SHA the code was paired with at commit
    /// time, what the store HEAD is now, drift in commits between them.
    Status,

    /// Install the prepare-commit-msg hook (`aida-store-pair.sh`) into
    /// `.git/hooks/`. Idempotent — safe to re-run. New projects get
    /// this hook from `aida init` automatically; this command exists
    /// for retrofitting an existing project.
    InstallHook {
        /// Overwrite an existing prepare-commit-msg hook (default: refuse).
        #[clap(long)]
        force: bool,
    },

    /// Deep-repack the orphan store git repo to relieve the substrate tax of a
    /// long, never-compacted history. The bare command runs an aggressive
    /// `git gc` — safe + non-destructive: no history is rewritten, `aida
    /// history` is unaffected, no force-push needed. `gc` is an alias.
    // trace:STORY-733 | ai:claude
    #[clap(alias = "gc")]
    Compact {
        /// DESTRUCTIVE opt-in: rewrite the orphan-store history to a single
        /// snapshot commit. Rewrites the branch (breaks the full `aida
        /// history events` timeline — preserved only via the backup ref) and
        /// needs a coordinated force-push every clone must re-sync. Never runs
        /// automatically. Without `--yes` it only PRINTS the plan.
        #[clap(long)]
        squash: bool,

        /// Required to actually perform a `--squash` rewrite. Without it,
        /// `--squash` prints exactly what it would do and exits without
        /// touching the store.
        #[clap(long)]
        yes: bool,
    },

    /// Preview the identity this machine will WRITE to the shared store —
    /// hostname + git email, and how the configured redaction (`[node]
    /// public_hostname` / `public_email` in `~/.aida/config.toml`) rewrites
    /// them. Read-only; verify up front, before your first push, that no raw
    /// corporate identity will land in a public-mirrored store.
    // trace:TASK-1122 | ai:claude
    #[clap(alias = "identity")]
    ScrubPreview,
}
