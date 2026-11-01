//! `aida doc_cmd` clap types, owned by `doc_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Project the graph into a layered docs tree.
// trace:FR-1-077 | ai:claude
#[derive(Subcommand, Debug)]
pub enum DocsCommand {
    /// Render (or re-render) the docs tree under `<output>/aida/`.
    /// Default output is `docs/aida/`.
    Build {
        /// Output directory (default: `docs/aida/`)
        #[clap(long, short = 'o')]
        output: Option<PathBuf>,

        /// Print what would be written, but don't write
        #[clap(long)]
        dry_run: bool,
    },

    /// Verify the docs tree on disk matches what the graph would render.
    /// Exits non-zero on drift. Suitable for a CI / pre-commit check.
    Check {
        /// Output directory (default: `docs/aida/`)
        #[clap(long, short = 'o')]
        output: Option<PathBuf>,
    },

    /// Print AIDA's machinery + lifecycle glossary from the binary's
    /// embedded copy.
    ///
    /// Surfaces the canonical vocabulary (orchestrator, phase, drain, lease,
    /// role, scope, worktree, …) and the spec-lifecycle verbs (committed,
    /// pushed, merged, completed, released) without hunting for the file under
    /// .aida/discipline/ — and stays correct even if the project's
    /// scaffolded copy is missing or stale. Default prints both sections.
    Glossary {
        /// Print only the machinery glossary (orchestrator, drain, lease, …).
        #[clap(long)]
        machinery: bool,
        /// Print only the lifecycle vocabulary (the spec-state verbs).
        #[clap(long)]
        lifecycle: bool,
    },

    /// Review documentation for drift that has built up: command text this
    /// build no longer has, and terms the website defines with no glossary
    /// entry. Reviews the CLI docs, the website pages, the glossary and
    /// READMEs touched since `--since` (default: the last recorded sweep;
    /// every one on the first sweep). A normal run follows `[docs_sweep]
    /// mode`: `report` mails the findings to the product seat, `pr` batches
    /// mechanical fixes into one documentation-only PR. The scheduled
    /// `docs-sweep` job runs it daily while the queue is idle.
    // trace:STORY-2220 | ai:claude
    Sweep {
        /// Review changes since this git revision instead of the last
        /// recorded sweep.
        #[clap(long, value_name = "REF")]
        since: Option<String>,

        /// Print the findings only: send no mail, open no PR, file no specs,
        /// and leave the last-swept revision where it is.
        #[clap(long)]
        report_only: bool,

        /// Emit the findings as JSON.
        #[clap(long)]
        json: bool,
    },
}

/// Living-documentation entry surface.
///
/// `Doc` requirements are narrative captured *during* work — rationale,
/// scenarios, recipes, gotchas — linked back to the specs they explain via
/// `References` relationships. They flow through the same orphan-store /
/// cache pipeline as every other requirement; this subcommand exists to
/// make capture and lookup ergonomic so the docs land while the context
/// is still fresh.
// trace:EPIC-24, STORY-104 | ai:claude
// trace:TASK-487 | ai:claude
#[derive(Subcommand, Debug)]
pub enum DocCommand {
    /// Capture a documentation entry — narrative tied to one or more specs.
    Add {
        /// Title of the doc entry (e.g., "When to use `aida queue work --steal`").
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        title: String,

        /// Specs this entry is about. Repeat or comma-separate. Each id is
        /// resolved against the store and linked as a `References`
        /// relationship — that's what `aida doc show <SPEC>` walks to find
        /// docs about a given spec.
        // trace:STORY-104 | ai:claude
        #[clap(long, value_delimiter = ',')]
        about: Vec<String>,

        /// Scenario tag — short label for the situation this doc covers
        /// (e.g., "muddle recovery", "first-time setup"). Stored in
        /// `custom_fields["scenario"]`; used as a filter axis by
        /// `aida doc list --scenario <name>`.
        // trace:BUG-1294 | ai:claude
        #[clap(long, allow_hyphen_values = true)]
        scenario: Option<String>,

        /// Audience tags (repeat or comma-separate) — who this doc is for:
        /// `user`, `agent`, `developer`. Stored as a comma-joined string in
        /// `custom_fields["audience"]`.
        #[clap(long, value_delimiter = ',')]
        audience: Vec<String>,

        /// Description body. For long-form prose use --description-from-file
        /// or --description-stdin (mirrors `aida add`).
        // trace:BUG-1294 | ai:claude
        #[clap(long, conflicts_with_all = ["description_from_file", "description_stdin"], allow_hyphen_values = true)]
        description: Option<String>,

        /// Read the description body from a file.
        #[clap(long, conflicts_with_all = ["description", "description_stdin"])]
        description_from_file: Option<PathBuf>,

        /// Read the description body from stdin.
        #[clap(long, conflicts_with_all = ["description", "description_from_file"])]
        description_stdin: bool,

        /// Tags for the entry (comma-separated).
        #[clap(long)]
        tags: Option<String>,
    },

    /// List documentation entries.
    List {
        /// Show only docs that reference <id> via `--about`. Resolves <id>
        /// against the store and walks the `References` edges back.
        #[clap(long, value_name = "ID")]
        about: Option<String>,

        /// Show only docs whose scenario tag matches exactly.
        #[clap(long)]
        scenario: Option<String>,

        /// Show only docs whose audience tag set contains <name>.
        #[clap(long)]
        audience: Option<String>,
    },

    /// Show docs. If <id> is a Doc-type spec id, print the entry's full
    /// detail. Otherwise treat <id> as a referenced spec and print every
    /// Doc that mentions it via `--about`.
    Show {
        /// Doc spec id, or any other spec id to walk `--about` references.
        id: String,
    },

    /// Release-time doc-coverage gate. List specs that reached Completed
    /// since the previous git tag but have no `aida doc` entry about them
    /// (no Doc that References them). Warn-only — exits 0 even when gaps
    /// exist — so it can be wired into the release flow without blocking.
    // trace:TASK-680 | ai:claude
    Coverage {
        /// Treat everything after this git ref/tag as "this release". When
        /// absent, the most recent `v*` tag is used; if there is no tag, the
        /// full history is scanned.
        #[clap(long, value_name = "REF")]
        since: Option<String>,

        /// Emit the gap list as JSON instead of a human warning block.
        #[clap(long)]
        json: bool,
    },

    /// Diff-driven doc nudge for a PR. Scan the changes a branch adds for NEW
    /// public surface (a CLI flag, a CLI subcommand, or an MCP tool) and, when
    /// the spec that surface traces to has no `aida doc` entry about it, nudge
    /// the author to capture one. Warn-only — exits 0 — so `/aida-pr` can call
    /// it without ever blocking the PR.
    // trace:TASK-939 | ai:claude
    Suggest {
        /// Git range to scan (e.g. `origin/main..HEAD`). When absent, the
        /// commits this branch adds over the default branch are scanned
        /// (`<default-branch>..HEAD`).
        #[clap(long, value_name = "RANGE")]
        range: Option<String>,

        /// Emit the nudge as JSON instead of a human block.
        #[clap(long)]
        json: bool,
    },
}
