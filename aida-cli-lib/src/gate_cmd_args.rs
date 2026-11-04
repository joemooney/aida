//! `aida gate_cmd` clap types, owned by `gate_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// The shipped gate library. Read-only.
// trace:STORY-1427 | ai:claude
#[derive(Subcommand, Debug)]
pub enum GateCommand {
    /// List the shipped gates, their versions, and the lifecycle moment each
    /// one is a default for.
    List {
        /// Only gates that are a default at this moment (e.g. `groom`).
        #[clap(long, value_name = "MOMENT")]
        moment: Option<String>,

        /// Emit JSON for agents / scripts.
        #[clap(long)]
        json: bool,
    },

    /// Print one gate's checklist for an agent to follow.
    Show {
        /// Gate name, e.g. `well-formed`.
        name: String,
    },

    /// Run a gate over a spec: the deterministic tier first, then the
    /// checklist for what it cannot decide. Advisory; never edits the spec.
    Run {
        /// Gate name, e.g. `well-formed`.
        name: String,

        /// SPEC-ID to judge.
        #[clap(value_parser = crate::spec_arg::spec_id)]
        spec: String,

        /// Emit the verdict as JSON.
        #[clap(long)]
        json: bool,
    },

    /// Diagnostic: one pull request's effective gate state (verdict, merge
    /// hold, execution mode, decision) computed from the requirements it
    /// ships, identified once against the committed store, and its gate
    /// ledger. Read-only: it decides nothing and changes nothing.
    // trace:STORY-2324 | ai:claude
    State {
        /// The pull request: `PR-<N>` or `<N>`.
        change: String,

        /// Also show what today's merge gates decide, column by column, and
        /// count where the two differ.
        #[clap(long)]
        compare: bool,

        /// Emit JSON.
        #[clap(long)]
        json: bool,
    },

    /// One-time operator step: give every gate-ledger event the canonical
    /// requirement key its spec label names in the committed store. Lines
    /// keep their order and bytes; nothing is added or removed. The legacy
    /// verdict and hold files are reported and left in place.
    // trace:STORY-2324 | ai:claude
    #[clap(group(
        ArgGroup::new("migrate_mode")
            .required(true)
            .args(["dry_run", "execute", "rollback"]),
    ))]
    Migrate {
        /// Plan and report; change nothing.
        #[clap(long)]
        dry_run: bool,

        /// Rewrite the ledger, keeping the old one as an archive. Needs an
        /// interactive terminal in the host shell, outside any sandbox, with
        /// no drain running, and no open pull request carrying a label that
        /// names no single requirement.
        #[clap(long)]
        execute: bool,

        /// Restore the ledger the last `--execute` archived. Refused when the
        /// ledger was written since. Same terminal and host-shell rules.
        #[clap(long)]
        rollback: bool,

        /// Emit JSON.
        #[clap(long)]
        json: bool,
    },
}
