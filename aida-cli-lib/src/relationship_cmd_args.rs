//! `aida relationship_cmd` clap types, owned by `relationship_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum RelationshipCommand {
    /// Add a relationship between local requirements
    ///
    /// Both positional and flag forms work:
    ///   aida rel add <from> <to> --type child
    ///   aida rel add --from <from> --to <to> --type child
    ///
    /// Cross-store references such as `other-project#<SPEC-ID>` are not
    /// local graph edges; record them in comments until AIDA has a resolver.
    // trace:TASK-487 | ai:claude
    // trace:BUG-790 | ai:codex
    Add {
        /// Source requirement ID (UUID or SPEC-ID). Positional or via --from.
        #[clap(value_name = "FROM")]
        from_pos: Option<String>,

        /// Target requirement ID (UUID or SPEC-ID). Positional or via --to.
        #[clap(value_name = "TO")]
        to_pos: Option<String>,

        /// Source requirement ID (UUID or SPEC-ID). Alias for the FROM positional.
        #[clap(long = "from", value_name = "FROM", hide = true)]
        from_flag: Option<String>,

        /// Target requirement ID (UUID or SPEC-ID). Alias for the TO positional.
        #[clap(long = "to", value_name = "TO", hide = true)]
        to_flag: Option<String>,

        /// Relationship type (parent, child, duplicate, verifies, verified-by, references, or custom)
        #[clap(long = "type", short = 't')]
        r#type: String,

        /// Create bidirectional relationship (adds inverse relationship automatically)
        #[clap(long, short = 'b')]
        bidirectional: bool,

        /// Override the guard that refuses `--type child` when the
        /// target (the parent) is in a terminal status
        /// (Completed/Rejected). Use when intentionally backfilling a
        /// forgotten child onto a closed epic.
        // trace:BUG-64 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long)]
        force_parent: bool,
    },

    /// Remove a relationship between requirements
    ///
    /// Both positional and flag forms work:
    ///   aida rel remove <from> <to> --type child
    ///   aida rel remove --from <from> --to <to> --type child
    // trace:TASK-487 | ai:claude
    Remove {
        /// Source requirement ID (UUID or SPEC-ID). Positional or via --from.
        #[clap(value_name = "FROM")]
        from_pos: Option<String>,

        /// Target requirement ID (UUID or SPEC-ID). Positional or via --to.
        #[clap(value_name = "TO")]
        to_pos: Option<String>,

        /// Source requirement ID (UUID or SPEC-ID). Alias for the FROM positional.
        #[clap(long = "from", value_name = "FROM", hide = true)]
        from_flag: Option<String>,

        /// Target requirement ID (UUID or SPEC-ID). Alias for the TO positional.
        #[clap(long = "to", value_name = "TO", hide = true)]
        to_flag: Option<String>,

        /// Relationship type
        #[clap(long = "type", short = 't')]
        r#type: String,

        /// Remove bidirectional relationship (removes inverse relationship too)
        #[clap(long, short = 'b')]
        bidirectional: bool,
    },

    /// List relationships — global by default, or filtered to a specific
    /// source/target requirement.
    ///
    /// Examples:
    ///   aida rel list                      # all edges in the graph
    ///   aida rel list <spec-id>            # outgoing edges from a spec
    ///   aida rel list --source <spec-id>   # same as positional form
    ///   aida rel list --spec <spec-id>     # alias of --source
    ///   aida rel list --id <spec-id>       # alias of --source
    ///   aida rel list --target <spec-id>   # incoming edges (what points AT it)
    ///   aida rel list --type child         # filter by edge type
    ///   aida rel list --dangling           # only edges with unresolved targets
    ///   aida rel list --type child --dangling   # composable
    // trace:TASK-65 | ai:claude
    // trace:TASK-487 | ai:claude
    #[clap(visible_alias = "show")]
    List {
        /// Source requirement ID (UUID or SPEC-ID). Positional alias of
        /// --source. When omitted (and --target/--source unset), lists every
        /// edge in the graph.
        #[clap(value_name = "ID")]
        id: Option<String>,

        /// Source requirement — same as the positional. Explicit form for
        /// scripts that want to be unambiguous. `--spec` and `--id` are
        /// accepted as aliases so the dominant ID-flag convention (used by
        /// `db reconcile-status --spec`, etc.) Just Works here too.
        // trace:BUG-577 | ai:claude
        #[clap(long, visible_alias = "spec", visible_alias = "id")]
        source: Option<String>,

        /// Target requirement — inverts the query to "what edges point AT
        /// this requirement?". Useful for "who depends on this epic?".
        // trace:TASK-487 | ai:claude
        #[clap(long)]
        target: Option<String>,

        /// Filter by relationship type (parent, child, duplicate, verifies,
        /// verified-by, references, or a custom name).
        #[clap(long = "type", short = 't')]
        r#type: Option<String>,

        /// Only show edges whose target UUID doesn't resolve (deleted-target
        /// tombstones from removed reqs). Pairs with `aida doctor
        /// verify-relationships --repair` to clean them up.
        // trace:BUG-53 | ai:claude
        // trace:TASK-487 | ai:claude
        #[clap(long)]
        dangling: bool,

        /// Include edges between terminal-status (Completed/Rejected)
        /// requirements. By default the global listing hides them so the
        /// view focuses on actionable work, matching `aida list`.
        #[clap(long)]
        all: bool,

        /// Cap the number of edges shown (0 = no cap). When unset, the
        /// unfiltered global listing auto-caps to avoid a firehose on large
        /// stores and prints a note; a source/target/type/dangling filter
        /// lifts the auto-cap. Pass an explicit `--limit` to override either way.
        // trace:TASK-778
        #[clap(long)]
        limit: Option<usize>,
    },
}
