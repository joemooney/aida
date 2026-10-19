//! `aida node_cmd` clap types, owned by `node_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use crate::cli::TeamCommand;
use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum NodeCommand {
    /// List all nodes registered in the shared registry. The current node
    /// (the one whose `.aida/node.toml` matches an entry) is marked with `*`.
    List,

    /// Show details for a single node. With no id, defaults to the current
    /// node (read from `.aida/node.toml`).
    Show {
        /// Node id to show (omit for current node). Accepted as a string.
        // trace:EPIC-9 | ai:claude
        // trace:TASK-487 | ai:claude
        id: Option<String>,
    },

    /// Acquire a node id for this clone. Performs a CAS push loop on the
    /// shared registry to claim the next sequential id, then writes the
    /// per-clone identity file at `.aida-store/.aida/node.toml`.
    /// Defaults pull `git config user.email` for the email stamp and the
    /// system hostname for the hostname stamp.
    // trace:EPIC-1-052 | ai:claude
    // trace:STORY-41 | ai:claude
    Acquire {
        /// Claim a specific node id (must be free in the registry).
        /// Accepted as a string — `"JM"` or `"1"` alike.
        // trace:EPIC-9 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long)]
        id: Option<String>,

        /// Override the hostname stamp (default: system hostname)
        #[clap(long)]
        hostname: Option<String>,

        /// Override the email stamp (default: `git config user.email`)
        #[clap(long)]
        email: Option<String>,

        /// Friendly name for this node, recorded alongside the node id in the
        /// shared roster (`aida team`). Defaults to `<host>-<user>-<seq>` (e.g.
        /// `imac-joe-1`). At a TTY with no flag you are prompted with the
        /// default pre-filled; non-interactive use takes the default silently.
        // trace:STORY-652 | ai:claude
        #[clap(long, value_name = "NAME")]
        node_name: Option<String>,

        /// Re-acquire even if `.aida/node.toml` already exists (default: refuse)
        #[clap(long)]
        force: bool,

        /// On collision, accept the suggested numeric-suffix fallback without
        /// prompting (e.g., `JM` taken → silently use `JM2`). Implies non-
        /// interactive mode.
        // trace:STORY-42 | ai:claude
        #[clap(long)]
        yes: bool,

        /// Re-claim an already-registered node id (the shared registry
        /// entry gets re-attributed to this clone). When the previous
        /// clone is reachable on this host, drops a HIJACKED.toml marker
        /// inside its `.aida-store/.aida/` so the user sees a warning the
        /// next time they run `aida` there. Mutually exclusive with
        /// `--id`.
        // trace:STORY-43 | ai:claude
        #[clap(long, value_name = "ID", conflicts_with = "id")]
        hijack: Option<String>,

        /// Backfill a node entry into the shared registry for some OTHER
        /// (typically legacy) clone, then push — WITHOUT touching this
        /// clone's own identity. The running clone keeps its `.aida/node.toml`
        /// untouched and no blocks are allocated. Requires explicit `--id`,
        /// `--hostname`, and `--email` (nothing is inferred from the local
        /// environment, since the entry is not about this clone). Mutually
        /// exclusive with `--hijack` and `--force`.
        // trace:FR-265 | ai:claude
        #[clap(long, conflicts_with_all = ["hijack", "force"])]
        remote_only: bool,
    },

    /// Backfill the owner ($USER string) on an existing node entry in the
    /// shared registry, then push (CAS). If the id is the CURRENT node, the
    /// local `.aida/node.toml` is updated too. Errors clearly if the id is
    /// absent. For legacy nodes that predate the owner/name identity fields.
    // trace:STORY-654 | ai:claude
    SetOwner {
        /// Node id whose owner to set.
        id: String,
        /// The owner's `current_user_id` string ($USER / $AIDA_USER).
        #[clap(long)]
        user: String,
    },

    /// Backfill the friendly name on an existing node entry in the shared
    /// registry, then push (CAS). If the id is the CURRENT node, the local
    /// `.aida/node.toml` is updated too. Errors clearly if the id is absent.
    // trace:STORY-654 | ai:claude
    SetName {
        /// Node id whose name to set.
        id: String,
        /// The friendly node name (e.g. `imac-joe-1`).
        #[clap(long)]
        name: String,
    },

    /// Remove a node entry from the shared registry. Does not invalidate
    /// any IDs already issued by that node. The node id is not reused.
    Release {
        /// Node id to release
        id: String,

        /// Skip the confirmation prompt
        #[clap(long, short = 'y')]
        yes: bool,
    },

    /// List the team: every registered node/clone sharing this store, with
    /// each member's role. Identity introspection under the `node` namespace.
    /// Equivalent to the top-level `aida team` (kept visible because the
    /// multi-clone harness invokes it); with a subcommand, manage per-user
    /// roles (`set-role`, `my-role`).
    // trace:TASK-851 | ai:claude
    Team {
        /// Machine-readable JSON output (bare roster view only).
        #[clap(long)]
        json: bool,
        #[clap(subcommand)]
        cmd: Option<TeamCommand>,
    },

    /// Print the caller identity AIDA resolved — role, agent-type, agent-name,
    /// user-id, headless flag, ai-tool, and active session/scope. Read-only.
    /// Identity introspection under the `node` namespace; equivalent to the
    /// hidden top-level `aida whoami`.
    // trace:TASK-851 | ai:claude
    Whoami,
}
