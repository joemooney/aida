//! `aida identity_gate` clap types, owned by `identity_gate.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Manage the shared person-alias registry (`registry/aliases.toml` on the
/// `aida-store` branch). One human routinely registers under several identity
/// strings across machines (`joe`, `joe.mooney`, `joe.mooney@gmail.com`); each
/// host mints its own ids, so the queue, the team roster, and the block list
/// otherwise show one person as several owners. Linking the strings collapses
/// them to one canonical person at every comparison/display boundary.
///
/// Composes with the case-fold (`Joe` vs `joe` already merge): the alias map is
/// the SECOND normalization layer, for the genuinely-different strings.
// trace:TASK-845 | ai:claude
#[derive(Subcommand, Debug)]
pub enum IdentityCommand {
    /// Link two identity strings as the same canonical person (bidirectional,
    /// idempotent). Writes `registry/aliases.toml` with a CAS push-wins loop.
    /// After linking, all the surfaces resolve both strings to one person.
    // trace:TASK-845 | ai:claude
    Link {
        /// The first identity string (e.g. `joe`).
        a: String,
        /// The second identity string (e.g. `joe.mooney@gmail.com`).
        b: String,
    },

    /// List the recorded person links: one block per canonical person with the
    /// aliases that resolve to them.
    // trace:TASK-845 | ai:claude
    List {
        /// Machine-readable JSON output.
        #[clap(long)]
        json: bool,
    },

    /// Show the canonical person an identity string resolves to, plus all the
    /// aliases that share it (after case-fold + alias-resolve).
    // trace:TASK-845 | ai:claude
    Show {
        /// The identity string to resolve (e.g. `Joe.Mooney@work.example`).
        id: String,
        /// Machine-readable JSON output.
        #[clap(long)]
        json: bool,
    },

    /// Check the identity a commit made here would carry (author + committer,
    /// honoring GIT_AUTHOR_EMAIL/GIT_COMMITTER_EMAIL overrides) against the
    /// project's `[identity] allowed_emails` allowlist in .aida/config.toml.
    /// Exits non-zero on a violation; a project with no allowlist passes with
    /// a hint on how to enable the gate.
    // trace:TASK-1330 | ai:claude
    Check,

    /// (plumbing) The fail-closed pre-push identity gate. Reads the ref lines
    /// git feeds a pre-push hook on stdin, inspects every commit the push
    /// would introduce on the named remote, and exits non-zero when any
    /// carries an author, committer, or Co-authored-by email outside
    /// `[identity] allowed_emails`. Silent no-op when no allowlist is
    /// configured.
    // trace:TASK-1330 | ai:claude
    #[clap(hide = true)]
    CheckPush {
        /// The remote the triggering push targets (hook argument $1).
        pushed_remote: String,
    },

    /// Install the identity gate into the repo's pre-push hook. Idempotent:
    /// refreshes AIDA's own hooks in place (including the mirror fan-out
    /// hook, which embeds the gate) and never clobbers a custom pre-push
    /// hook (prints the lines to add instead).
    // trace:TASK-1330 | ai:claude
    InstallHook,
}
