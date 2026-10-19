//! `aida db_cmd` clap types, owned by `db_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum DbCommand {
    /// Print the path to the active store (the orphan-branch worktree, in
    /// distributed mode). The legacy `--name` registry lookup was dropped
    /// in Phase 1 of the kernel/module audit.
    // trace:FR-1-076 | ai:claude
    Path,

    /// (archived) Migrate between legacy YAML/SQLite/PostgreSQL backends.
    /// Kept for one-shot migrations off the legacy pre-git-canonical path.
    // trace:FR-1-076 | ai:claude
    // trace:EPIC-1-001 | ai:claude
    // trace:TASK-487 | ai:claude
    #[clap(hide = true)]
    Migrate {
        /// Source format: "yaml", "sqlite", or "postgres"
        #[clap(long)]
        from: String,

        /// Target format: "yaml", "sqlite", or "postgres"
        #[clap(long)]
        to: String,

        /// Output path (for yaml/sqlite) or connection string (for postgres)
        #[clap(long, short = 'o')]
        output: Option<String>,

        /// Overwrite existing target file
        #[clap(long)]
        force: bool,
    },

    /// Show database statistics and info
    Info,

    /// Sync git-backed store with remote (push/pull)
    Sync {
        /// Pull from remote before pushing
        #[clap(long)]
        pull: bool,

        /// Push to remote after committing
        #[clap(long)]
        push: bool,

        /// Commit message for pending changes
        // trace:BUG-1294 | ai:claude
        #[clap(long, short = 'm', allow_hyphen_values = true)]
        message: Option<String>,
    },

    /// (archived) Export legacy DB to a git-backed store directory.
    // trace:FR-1-076 | ai:claude
    #[clap(hide = true)]
    ExportGit {
        /// Output directory for the git-backed store
        #[clap(long, short = 'o')]
        output: String,
    },

    /// Assign agreed IDs (short IDs) to all objects that don't have one.
    /// Run this at merge-to-trunk to collapse a node-aware distributed id
    /// (e.g. `FR-N-NNN`) down to its short form (`FR-NNN`).
    // trace:TASK-487 | ai:claude
    MergeGate,

    /// Collapse legacy origin spec_ids onto their agreed_ids. For each
    /// requirement where spec_id ≠ agreed_id (typically a legacy
    /// zero-padded origin id with a later-assigned short agreed id), set
    /// spec_id := agreed_id and clear agreed_id. The on-disk YAML moves
    /// to the new sharded path; relationships are unaffected (they use
    /// UUIDs). Run with --dry-run first to preview.
    // trace:FR-1-071 | ai:claude
    // trace:TASK-487 | ai:claude
    RetireLegacyIds {
        /// Show what would change without writing anything.
        #[clap(long)]
        dry_run: bool,
    },

    /// Initialize a multi-repo workspace (multiple code repos sharing one store)
    WorkspaceInit {
        /// Workspace name
        #[clap(long)]
        name: Option<String>,

        /// Git remote URL for the shared store
        #[clap(long)]
        remote: Option<String>,
    },

    /// Show status of the git-backed store (changes, sync state, conflicts)
    Status,

    /// Manage pre-allocated agreed ID blocks for offline-safe trace comments
    // trace:FR-2-005 | ai:claude
    // trace:TASK-487 | ai:claude
    Block {
        #[clap(subcommand)]
        subcommand: BlockCommand,
    },

    /// Replay the Done → Completed auto-bump scan over a wider commit
    /// range than `aida pull` saw, recovering specs that got stranded
    /// at Done. The pull-time auto-bump only scans commits the current
    /// pull brings in; if a spec's YAML was unreadable at pull time, or
    /// the user pulled before flipping the spec to Done, the bump
    /// silently misses and the spec is stuck without manual intervention.
    // trace:TASK-226 | ai:claude
    // trace:STORY-86 | ai:claude
    // trace:BUG-96 | ai:claude
    // trace:TASK-487 | ai:claude
    ReconcileStatus {
        /// Bound the scan range. Accepts a commit SHA, a tag, or any
        /// `git rev-parse`-able ref. Without --since, walks the most
        /// recent 200 commits on the default branch (an over-broad
        /// window is safe — only Done specs flip, so idempotent on
        /// already-Completed specs).
        #[clap(long, value_name = "REF")]
        since: Option<String>,

        /// Limit the replay to a single spec. Faster than the full
        /// scan when you know which spec is stuck. The same default-
        /// branch + status-Done guards apply.
        #[clap(long, value_name = "SPEC-ID")]
        spec: Option<String>,

        /// Show what would flip without writing anything. Pairs with
        /// `--since` / `--spec` for previewing a targeted replay.
        #[clap(long)]
        dry_run: bool,
    },

    /// Audit the store for consistency problems. Currently supports
    /// `--collisions` (two requirements claiming the same short id,
    /// which the gate-time check prevents going forward but doesn't
    /// retroactively detect).
    // trace:TASK-80 | ai:claude
    // trace:BUG-82 | ai:claude
    // trace:TASK-487 | ai:claude
    Check {
        /// Report requirements whose preferred display id (agreed_id
        /// when assigned, else spec_id) collides with another
        /// requirement's spec_id or agreed_id. Pre-existing collisions
        /// from before the gate-time check landed persist in the store
        /// and won't auto-clear.
        #[clap(long)]
        collisions: bool,

        /// When used with `--collisions`, re-gate the later (higher
        /// position-encoded) claimant's agreed_id to the next free
        /// short id. The earlier claimant keeps the contested id.
        /// Without this flag, the command only reports; the operator
        /// decides which to keep.
        #[clap(long)]
        repair: bool,
    },

    /// Repair custom edges that graph traversals do not follow.
    ///
    /// Any edge stored as a custom type whose spelling resolves to a
    /// standard relationship type (for example `related`, `depends-on`,
    /// `verified_by` or `replaced_by`) is rewritten: when the source already
    /// has an edge of that standard type to the same target the custom edge
    /// is deleted, otherwise it is converted in place. Other custom edge
    /// types (for example `implements` or a `sprint_*` label) are left
    /// alone. Each changed spec is written with its own store commit, and
    /// the command refuses to leave a spec holding two edges of the same
    /// type to one target. Running it again after a successful run changes
    /// nothing.
    // trace:TASK-1426 | ai:claude
    // trace:TASK-1488 | ai:claude
    MigrateRelatedEdges {
        /// Report the counts and the per-spec actions without writing
        /// anything.
        #[clap(long)]
        dry_run: bool,

        /// Emit the report as JSON.
        #[clap(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum BlockCommand {
    /// Claim a new block of agreed IDs for this node (requires network push)
    Claim {
        /// Type prefix to claim a block for (e.g., FR, BUG, EPIC). Defaults to FR.
        #[clap(long, default_value = "FR")]
        r#type: String,

        /// Number of IDs to reserve in the block
        #[clap(long, default_value = "100")]
        size: u32,
    },

    /// List all claimed blocks across all nodes
    List,

    /// Show blocks for the current node and their remaining capacity
    Status,

    /// Cross-check nodes.toml against blocks.yaml. Reports any block-
    /// owning node id missing from the registry and (under blocks-only
    /// policy) any registered node with no claimed block. Exits non-zero
    /// on inconsistency.
    // trace:FR-281 | ai:claude
    Verify,
}
