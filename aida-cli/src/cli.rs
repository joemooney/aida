use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[clap(
    author,
    version,
    about = "AI-native requirements management — durable, agent-readable specs",
    after_help = "Less-common commands (db, cache, gitlab, mcp-serve, etc.) are hidden \
                  by default. Run `aida help-all` for the full inventory grouped by topic."
)]
pub struct Cli {
    /// Path to the requirements file (overrides auto-detection)
    #[clap(long)]
    pub file: Option<String>,

    /// Project name to use from central registry
    #[clap(long, short = 'p')]
    pub project: Option<String>,

    /// Connect to a remote AIDA server (e.g., "localhost:50051" or "grpc://host:port")
    /// Can also be set via AIDA_SERVER environment variable
    #[clap(long, short = 's')]
    pub server: Option<String>,

    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum ServerCommand {
    /// Check server status
    Status,

    /// List requirements from server
    List {
        /// Filter by status
        #[clap(long)]
        status: Option<String>,

        /// Filter by feature
        #[clap(long)]
        feature: Option<String>,

        /// Limit results
        #[clap(long, default_value = "100")]
        limit: i32,
    },

    /// Get a requirement from server
    Get {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
    },

    /// Ping server to check connectivity
    Ping,
}

/// Commands for code-to-requirement traceability
#[derive(Subcommand, Debug)]
pub enum TraceCommand {
    /// Add a trace link from code to a requirement
    Add {
        /// Requirement ID (UUID or SPEC-ID)
        #[clap(long)]
        req: String,

        /// Path to the source file
        #[clap(long)]
        file: String,

        /// Symbol name (function, struct, module, etc.)
        #[clap(long)]
        symbol: Option<String>,

        /// Starting line number
        #[clap(long)]
        line_start: Option<u32>,

        /// Ending line number
        #[clap(long)]
        line_end: Option<u32>,

        /// Artifact type: source, test, config, doc
        #[clap(long, short = 't', default_value = "source")]
        r#type: String,

        /// Notes about this trace link
        #[clap(long)]
        notes: Option<String>,

        /// Git commit hash where this was implemented
        #[clap(long)]
        commit: Option<String>,
    },

    /// List trace links for a requirement or file
    List {
        /// Requirement ID (UUID or SPEC-ID) - lists all trace links for this requirement
        #[clap(long)]
        req: Option<String>,

        /// File path - lists all trace links for this file
        #[clap(long)]
        file: Option<String>,
    },

    /// Remove a trace link
    Remove {
        /// Requirement ID (UUID or SPEC-ID)
        #[clap(long)]
        req: String,

        /// Trace link ID to remove
        #[clap(long)]
        link_id: String,
    },

    /// Scan source files for trace comments (// trace:REQ-ID format)
    Scan {
        /// Path to scan (file or directory, defaults to current directory)
        path: Option<String>,

        /// File extensions to scan (comma-separated, e.g., "rs,py,ts")
        #[clap(long, default_value = "rs")]
        extensions: String,

        /// Add discovered trace links to requirements database
        #[clap(long)]
        update: bool,

        /// Show verbose output
        #[clap(long, short = 'v')]
        verbose: bool,
    },

    /// Sweep git commits for requirement references
    Sweep {
        /// Number of commits to scan (default: all)
        #[clap(long)]
        limit: Option<u32>,

        /// Branch to scan (default: current)
        #[clap(long)]
        branch: Option<String>,

        /// Only show commits, don't update database
        #[clap(long)]
        dry_run: bool,

        /// Show verbose output
        #[clap(long, short = 'v')]
        verbose: bool,
    },
}

/// Commands for generating reports
#[derive(Subcommand, Debug)]
pub enum ReportCommand {
    /// Generate AI integration report
    AiIntegration {
        /// Output format: markdown or html
        #[clap(long, short = 'f', default_value = "markdown")]
        format: String,

        /// Output file path (defaults to stdout)
        #[clap(long, short = 'o')]
        output: Option<PathBuf>,

        /// Project root directory for scaffolding status check
        #[clap(long)]
        project_root: Option<PathBuf>,

        /// Include scaffolding status in report
        #[clap(long)]
        include_scaffold: bool,
    },
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

    /// Apply category-aware upgrades to scaffolded files. The right
    /// follow-up to `scaffold status` reporting drift — does the right
    /// thing per file category instead of `apply --force`'s blast-radius.
    ///
    /// Per category (see `docs/plans/2026-05-04-scaffold-categorization.md`):
    ///   - **template** (`.claude/skills/*`, `.claude/commands/*`,
    ///     `.claude/hooks/*`, `.claude/AIDA.md`, `.codex/skills/**`,
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
    /// trace:FR-1-028 | ai:claude
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
    },

    /// Show a unified diff between embedded templates and on-disk files
    /// for any drifted scaffold artifact. Exits 1 if any drift is found,
    /// 0 if clean. Pairs with `aida scaffold status` — when that reports
    /// "N modified" this is how you actually see what changed.
    /// trace:FR-1-027 | ai:claude
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
    },
}

/// Persona / hat commands. A role is a persistent named context that you
/// can resume across shells — captures the working directory, last
/// Inspect / resume past Claude Code sessions for this project, enriched
/// with the AIDA role and most-recent spec from each session's .jsonl.
/// trace:FR-1-043 | ai:claude
#[derive(Subcommand, Debug)]
pub enum SessionCommand {
    /// List recent Claude Code sessions for this project (cwd) with
    /// role + spec context extracted from each session's .jsonl. By
    /// default shows the 20 most recent.
    List {
        /// Show at most N sessions (default 20).
        #[clap(long, short = 'n', default_value = "20")]
        limit: usize,

        /// Plain output (no color), suitable for piping.
        #[clap(long)]
        no_color: bool,
    },

    /// Pick a session interactively and resume it via `claude --resume`.
    /// When `id` is given, resume that session directly without the
    /// picker. The picker shows the same enriched columns as `list`.
    Resume {
        /// Specific session id (or 8-char prefix) to resume. When
        /// omitted, opens an interactive picker.
        id: Option<String>,

        /// Show at most N sessions in the picker (default 20).
        #[clap(long, short = 'n', default_value = "20")]
        limit: usize,
    },

    /// Launch a new Claude Code session, recording the active role + a
    /// user-chosen title so `aida session list` can show them
    /// reliably (instead of greping the auto-generated subject). Execs
    /// `claude --permission-mode <mode>` once the launch metadata is
    /// recorded; permission-mode defaults to `bypassPermissions`
    /// (matching the user's typical "auto" workflow).
    /// trace:FR-1-044 | ai:claude
    New {
        /// Title for the session (shown in `aida session list`). When
        /// omitted, you'll be prompted interactively.
        #[clap(long, short = 't')]
        title: Option<String>,

        /// Claude Code permission mode. Most common values:
        /// `bypassPermissions` (default — no prompts), `acceptEdits`
        /// (auto-accept edits, prompt other tools), `default` (prompt
        /// for everything), `plan`.
        #[clap(long, default_value = "bypassPermissions")]
        permission_mode: String,

        /// Override the role recorded for this session (defaults to
        /// $AIDA_SESSION_ROLE).
        #[clap(long)]
        role: Option<String>,
    },
}

/// activity, optional purpose, and acts as a label in the statusline.
/// State lives at `<project>/.aida/roles/<name>.toml`.
/// trace:EPIC-1-001 | ai:claude
#[derive(Subcommand, Debug)]
pub enum RoleCommand {
    /// Enter (resume) an existing role. Errors if the role doesn't exist —
    /// use `aida role add` to create a new one. Outputs shell code:
    ///   `eval "$(aida role enter architect)"`
    /// or via the `aida-role` shell helper.
    Enter {
        /// Role name — must already exist
        name: String,

        /// Restore the role's last working directory (cd in the eval output)
        #[clap(long)]
        cd: bool,
    },

    /// Add a new role, then enter it. Errors if the name already exists
    /// (use `aida role enter` to resume an existing one).
    Add {
        /// Role name — e.g. architect, reviewer, implementer
        name: String,

        /// Set the role's stated purpose
        #[clap(long)]
        purpose: Option<String>,

        /// Store this role globally (~/.aida/roles/) instead of per-project.
        /// Useful for personas you carry across projects (triage, code-review).
        #[clap(long)]
        global: bool,
    },

    /// List all roles for this project (and any global roles), sorted
    /// by last activity. Active role marked with `*`.
    List,

    /// Show details for one role (defaults to the active role if any).
    Show {
        name: Option<String>,
    },

    /// Deactivate the current role (state preserved). Outputs shell code:
    ///   `eval "$(aida role end)"`
    End,

    /// Delete a role's state file permanently. Confirms before deleting
    /// unless --yes is passed.
    Delete {
        name: String,

        /// Skip confirmation prompt
        #[clap(long, short = 'y')]
        yes: bool,
    },

    /// Install a starter set of global roles (triage, architect,
    /// implementer, reviewer) at ~/.aida/roles/. Idempotent — skips any
    /// that already exist; safe to re-run.
    Scaffold,

    /// Manage per-role scope filters. Scope filters are auto-applied to
    /// `aida list` and `aida queue list/next` while the role is active —
    /// e.g. set `--tags inbox --status draft` on the `triage` role to
    /// always see just the inbox-tagged drafts when wearing that hat.
    /// Override on a single command with explicit --tags/--status flags
    /// or --no-scope.
    /// trace:TASK-1-021 | ai:claude
    #[clap(subcommand)]
    Scope(RoleScopeCommand),

    /// Manage the per-role Claude Code system-prompt addendum. The text
    /// is injected into Claude's context at session start via the
    /// aida-role-context.sh hook when this role is active.
    /// trace:TASK-1-022 | ai:claude
    #[clap(subcommand)]
    Prompt(RolePromptCommand),
}

/// System-prompt addendum management. Text persists to the role's TOML
/// alongside scope filters; a SessionStart hook reads it and emits it
/// to the model as additionalContext.
/// trace:TASK-1-022 | ai:claude
#[derive(Subcommand, Debug)]
pub enum RolePromptCommand {
    /// Set the system-prompt addendum. Defaults to the active role; pass
    /// --name to target a different one. Pass content as the positional
    /// argument or via --content; --stdin reads from stdin (handy for
    /// multi-line text via `aida role prompt set --stdin < addendum.md`).
    Set {
        /// Role name (defaults to active role from AIDA_SESSION_ROLE)
        #[clap(long)]
        name: Option<String>,

        /// Addendum text (positional)
        content: Option<String>,

        /// Addendum text (alternative to positional, useful for shells
        /// that mangle special characters in arguments)
        #[clap(long, conflicts_with = "content")]
        content_flag: Option<String>,

        /// Read addendum from stdin
        #[clap(long, conflicts_with_all = ["content", "content_flag"])]
        stdin: bool,
    },

    /// Print the current system-prompt addendum (defaults to active role).
    Show {
        /// Role name (defaults to active role from AIDA_SESSION_ROLE)
        #[clap(long)]
        name: Option<String>,
    },

    /// Remove the system-prompt addendum (defaults to active role).
    Clear {
        /// Role name (defaults to active role from AIDA_SESSION_ROLE)
        #[clap(long)]
        name: Option<String>,
    },
}

/// Scope-filter management for a role. State persists to the role's TOML
/// file, so the filter follows the role into every shell that enters it.
/// trace:TASK-1-021 | ai:claude
#[derive(Subcommand, Debug)]
pub enum RoleScopeCommand {
    /// Set scope filters on a role. Defaults to the active role; pass --name
    /// to target a different one. At least one of --tags/--status required.
    Set {
        /// Role name (defaults to active role from AIDA_SESSION_ROLE)
        #[clap(long)]
        name: Option<String>,

        /// Comma-separated tags to AND into the default filter
        #[clap(long)]
        tags: Option<String>,

        /// Status to auto-apply (e.g. draft, approved, in-progress)
        #[clap(long)]
        status: Option<String>,
    },

    /// Show the scope filters for a role (defaults to active role).
    Show {
        /// Role name (defaults to active role from AIDA_SESSION_ROLE)
        #[clap(long)]
        name: Option<String>,
    },

    /// Clear scope filters. Without flags, clears all scope; with --tags
    /// or --status, clears just that field.
    Clear {
        /// Role name (defaults to active role from AIDA_SESSION_ROLE)
        #[clap(long)]
        name: Option<String>,

        /// Clear only the tag scope
        #[clap(long)]
        tags: bool,

        /// Clear only the status scope
        #[clap(long)]
        status: bool,
    },
}

/// Developer commands for working *on* AIDA itself (pyenv-style activation
/// of an in-repo build, running dev servers, installing shell helpers).
/// trace:EPIC-1-001 | ai:claude
#[derive(Subcommand, Debug)]
pub enum DevCommand {
    /// Emit shell code that prepends the in-repo build dir to PATH.
    /// Use as: `eval "$(aida dev activate)"`. The `target/release` build
    /// is preferred over `target/debug`; whichever has been built more
    /// recently wins.
    Activate {
        /// Override the AIDA repo path (defaults to current directory if it
        /// looks like the aida repo, or $AIDA_DEV_REPO if set).
        #[clap(long)]
        repo: Option<String>,
    },

    /// Emit shell code that undoes a previous `aida dev activate`.
    /// Use as: `eval "$(aida dev deactivate)"`.
    Deactivate,

    /// Run aida-server (REST/gRPC) and the React dev server (vite) in the
    /// foreground. Ctrl+C stops both. Defaults to serving the current
    /// project's store; React dev server only starts when run from the
    /// AIDA repo (since aida-web-react/ lives there).
    Serve {
        /// Override the REST/HTTP port for aida-server (default: 8080)
        #[clap(long)]
        rest_port: Option<u16>,

        /// Override the gRPC port for aida-server (default: 50051)
        #[clap(long)]
        grpc_port: Option<u16>,

        /// Override the vite dev server port (default: 5173)
        #[clap(long)]
        web_port: Option<u16>,

        /// Skip starting the React dev server even when aida-web-react/ exists
        #[clap(long)]
        no_web: bool,
    },

    /// Show current dev-activation state: is AIDA_DEV_ACTIVE set, what
    /// binary is being used, when was it built, what's the AIDA repo path.
    Status,

    /// Print shell helper functions (aida-on, aida-off) suitable for
    /// pasting into ~/.bashrc or ~/.zshrc. Pass --install to append them
    /// directly.
    ShellInit {
        /// Detect the user's shell and append the helpers to its rc file
        #[clap(long)]
        install: bool,
    },

    /// One-command release: bump version + tag + push (via scripts/release.sh),
    /// wait for the GitHub Actions workflow to publish binary tarballs, then
    /// upgrade your sibling installs to the new version. Default bump: patch.
    Release {
        /// Version bump kind: major, minor, patch, or an explicit version like "0.5.0"
        #[clap(default_value = "patch")]
        bump: String,
    },

    /// Alias for `aida dev release patch` — the most common case.
    Patch,
}

/// Commands for the SQLite read-cache that projects the git-canonical store
/// for fast list/filter/search queries (EPIC-1-001).
#[derive(Subcommand, Debug)]
pub enum CacheCommand {
    /// Force a full rebuild of the cache from the git store
    Rebuild,

    /// Show cache state (HEAD comparison, requirement count, last build time)
    Status,
}

#[derive(Subcommand, Debug)]
pub enum DbCommand {
    /// Register a project in the registry
    Register {
        /// Name of the project
        #[clap(long)]
        name: Option<String>,

        /// Path to the requirements file
        #[clap(long)]
        path: Option<PathBuf>,

        /// Description of the project
        #[clap(long)]
        description: Option<String>,

        /// Set this project as the default
        #[clap(long)]
        default: bool,

        /// Use interactive mode (prompts)
        #[clap(long)]
        interactive: bool,
    },

    /// Print the path to the database YAML file
    Path {
        /// The name of the database to lookup
        #[clap(long)]
        name: Option<String>,
    },

    /// Migrate database between formats (YAML <-> SQLite <-> PostgreSQL)
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
        #[clap(long, short = 'm')]
        message: Option<String>,
    },

    /// Export current database to a git-backed store directory
    ExportGit {
        /// Output directory for the git-backed store
        #[clap(long, short = 'o')]
        output: String,
    },

    /// Assign agreed IDs (short IDs) to all objects that don't have one.
    /// Run this at merge-to-trunk to give distributed IDs (FR-7-048) their
    /// short form (FR-423).
    MergeGate,

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
}

#[derive(Subcommand, Debug)]
pub enum FeatureCommand {
    /// Add a new feature with a prefix for IDs
    Add {
        /// Name of the feature (e.g., "Authentication")
        #[clap(long)]
        name: Option<String>,

        /// Prefix for requirement IDs (e.g., "AUTH")
        #[clap(long)]
        prefix: Option<String>,

        /// Use interactive mode (prompts)
        #[clap(long)]
        interactive: bool,
    },

    /// List all features
    List,

    /// Show details for a specific feature
    Show {
        /// The name or prefix of the feature to show
        name: String,
    },

    /// Edit an existing feature
    Edit {
        /// The name or prefix of the feature to edit
        name: String,

        /// New name for the feature
        #[clap(long)]
        new_name: Option<String>,

        /// New prefix for the feature
        #[clap(long)]
        new_prefix: Option<String>,

        /// Use interactive mode (prompts)
        #[clap(long)]
        interactive: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Show current ID configuration
    Show,

    /// Set the ID format (single-level or two-level)
    Format {
        /// Format: "single" for PREFIX-NNN, "two" for FEATURE-TYPE-NNN
        format: String,
    },

    /// Set the numbering strategy
    Numbering {
        /// Strategy: "global", "per-prefix", or "per-feature-type"
        strategy: String,
    },

    /// Set the number of digits in IDs
    Digits {
        /// Number of digits (1-6)
        digits: u8,
    },

    /// Migrate existing SPEC-XXX IDs to new format
    Migrate {
        /// Skip confirmation prompt
        #[clap(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum TypeCommand {
    /// List all requirement types
    List,

    /// Add a new requirement type
    Add {
        /// Name of the type (e.g., "Business")
        #[clap(long)]
        name: String,

        /// Prefix for the type (e.g., "BR")
        #[clap(long)]
        prefix: String,

        /// Description of the type
        #[clap(long)]
        description: Option<String>,
    },

    /// Remove a requirement type
    Remove {
        /// Name or prefix of the type to remove
        name: String,

        /// Skip confirmation prompt
        #[clap(long, short = 'y')]
        yes: bool,
    },
}

/// Commands for managing relationship type definitions
#[derive(Subcommand, Debug)]
pub enum RelDefCommand {
    /// List all relationship definitions
    List,

    /// Show details for a specific relationship definition
    Show {
        /// Name of the relationship definition
        name: String,
    },

    /// Add a new relationship definition
    Add {
        /// Unique name for the relationship (lowercase, no spaces)
        #[clap(long)]
        name: String,

        /// Human-readable display name
        #[clap(long)]
        display_name: Option<String>,

        /// Description of what this relationship means
        #[clap(long)]
        description: Option<String>,

        /// Name of the inverse relationship (e.g., "child" for "parent")
        #[clap(long)]
        inverse: Option<String>,

        /// Whether this relationship is symmetric (A->B implies B->A)
        #[clap(long)]
        symmetric: bool,

        /// Cardinality: 1:1, 1:n, n:1, n:n (default: n:n)
        #[clap(long, default_value = "n:n")]
        cardinality: String,

        /// Allowed source requirement types (comma-separated, empty = all)
        #[clap(long)]
        source_types: Option<String>,

        /// Allowed target requirement types (comma-separated, empty = all)
        #[clap(long)]
        target_types: Option<String>,

        /// Color for visualization (hex format, e.g., #ff6b6b)
        #[clap(long)]
        color: Option<String>,
    },

    /// Edit an existing relationship definition
    Edit {
        /// Name of the relationship definition to edit
        name: String,

        /// New display name
        #[clap(long)]
        display_name: Option<String>,

        /// New description
        #[clap(long)]
        description: Option<String>,

        /// New allowed source types (comma-separated)
        #[clap(long)]
        source_types: Option<String>,

        /// New allowed target types (comma-separated)
        #[clap(long)]
        target_types: Option<String>,

        /// New color
        #[clap(long)]
        color: Option<String>,
    },

    /// Remove a relationship definition (only custom ones)
    Remove {
        /// Name of the relationship definition to remove
        name: String,

        /// Skip confirmation prompt
        #[clap(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum RelationshipCommand {
    /// Add a relationship between requirements
    Add {
        /// Source requirement ID (UUID or SPEC-ID)
        #[clap(long)]
        from: String,

        /// Target requirement ID (UUID or SPEC-ID)
        #[clap(long)]
        to: String,

        /// Relationship type (parent, child, duplicate, verifies, verified-by, references, or custom)
        #[clap(long)]
        r#type: String,

        /// Create bidirectional relationship (adds inverse relationship automatically)
        #[clap(long, short = 'b')]
        bidirectional: bool,
    },

    /// Remove a relationship between requirements
    Remove {
        /// Source requirement ID (UUID or SPEC-ID)
        #[clap(long)]
        from: String,

        /// Target requirement ID (UUID or SPEC-ID)
        #[clap(long)]
        to: String,

        /// Relationship type
        #[clap(long)]
        r#type: String,

        /// Remove bidirectional relationship (removes inverse relationship too)
        #[clap(long, short = 'b')]
        bidirectional: bool,
    },

    /// List all relationships for a requirement
    #[clap(visible_alias = "show")]
    List {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum CommentCommand {
    /// Add a comment to a requirement
    Add {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,

        /// Comment content (positional or --content)
        #[clap(long)]
        content: Option<String>,

        /// Comment content (positional argument)
        #[clap(name = "CONTENT")]
        content_positional: Option<String>,

        /// Author of the comment (defaults to AIDA_AUTHOR env var or system user)
        #[clap(long)]
        author: Option<String>,

        /// Parent comment ID (for replies)
        #[clap(long)]
        parent: Option<String>,

        /// Use interactive mode (prompts)
        #[clap(long)]
        interactive: bool,
    },

    /// List all comments for a requirement
    List {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
    },

    /// Edit a comment
    Edit {
        /// Requirement ID (UUID or SPEC-ID)
        #[clap(long)]
        req_id: String,

        /// Comment ID to edit
        #[clap(long)]
        comment_id: String,

        /// New content
        #[clap(long)]
        content: Option<String>,

        /// Use interactive mode (prompts)
        #[clap(long)]
        interactive: bool,
    },

    /// Delete a comment
    Delete {
        /// Requirement ID (UUID or SPEC-ID)
        #[clap(long)]
        req_id: String,

        /// Comment ID to delete
        #[clap(long)]
        comment_id: String,
    },
}

/// GitLab integration commands
#[derive(Subcommand, Debug)]
pub enum GitLabCommand {
    /// Configure GitLab connection
    Config {
        /// GitLab instance URL (e.g., https://gitlab.com or self-hosted)
        #[clap(long)]
        url: Option<String>,

        /// GitLab project ID (numeric)
        #[clap(long)]
        project: Option<u64>,

        /// Personal Access Token (will be stored securely)
        #[clap(long)]
        token: Option<String>,

        /// Show current configuration
        #[clap(long)]
        show: bool,
    },

    /// Test connection to GitLab
    Test,

    /// List issues from GitLab
    List {
        /// Filter by state (opened, closed, all)
        #[clap(long, default_value = "opened")]
        state: String,

        /// Filter by labels (comma-separated)
        #[clap(long)]
        labels: Option<String>,

        /// Search query
        #[clap(long)]
        search: Option<String>,

        /// Maximum number of issues to show
        #[clap(long, default_value = "20")]
        limit: u32,
    },

    /// Show a specific GitLab issue
    Show {
        /// Issue IID (e.g., 123 or GL-123)
        iid: String,
    },

    /// Show sync status for linked items
    Status {
        /// Requirement ID to check (optional, shows all if not specified)
        id: Option<String>,

        /// Only show diverged items
        #[clap(long)]
        diverged: bool,
    },

    /// Manage GitLab label mappings
    Labels {
        /// Validate that mapped labels exist in GitLab project
        #[clap(long)]
        validate: bool,

        /// Create missing labels in GitLab project
        #[clap(long)]
        create_missing: bool,

        /// Initialize label mappings with defaults
        #[clap(long)]
        init: bool,
    },

    /// Refresh sync state by checking GitLab for changes
    Refresh {
        /// Specific requirement ID to refresh (optional, refreshes all if not specified)
        id: Option<String>,

        /// Force refresh even if recently checked
        #[clap(long)]
        force: bool,
    },

    /// Control background polling for GitLab changes
    Poll {
        /// Action: status, start, stop
        #[clap(default_value = "status")]
        action: String,

        /// Poll interval in seconds (for start)
        #[clap(long, default_value = "300")]
        interval: u64,
    },
}

/// Personal work queue commands (STORY-0368)
#[derive(Subcommand, Debug)]
pub enum QueueCommand {
    /// List items in your queue. When a role is active and --role is not
    /// passed explicitly, defaults to filtering on that role.
    List {
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Include completed requirements
        #[clap(long)]
        include_completed: bool,
        /// Filter to items routed to a specific role (e.g., "implementer").
        /// Pass --role any to show all items including unrouted.
        #[clap(long)]
        role: Option<String>,
        /// Show all items regardless of any active-role default filter
        #[clap(long)]
        all: bool,
        /// Bypass the active role's scope_tags / scope_status filters.
        /// trace:TASK-1-021 | ai:claude
        #[clap(long)]
        no_scope: bool,
    },
    /// Add a requirement to your queue
    Add {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
        /// Add to top of queue
        #[clap(long)]
        top: bool,
        /// Add to bottom of queue (default)
        #[clap(long)]
        bottom: bool,
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Note explaining why this was queued
        #[clap(long)]
        note: Option<String>,
        /// Route this item to a specific role queue (e.g., "implementer",
        /// "architect"). The other role wearer can then `aida queue list`
        /// while in that role to see incoming work.
        #[clap(long)]
        r#for: Option<String>,
    },
    /// Remove a requirement from your queue
    Remove {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
    },
    /// Move a queue item to a new position
    Move {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
        /// Move to top of queue
        #[clap(long)]
        top: bool,
        /// Move to bottom of queue
        #[clap(long)]
        bottom: bool,
        /// Move before this requirement ID
        #[clap(long)]
        before: Option<String>,
    },
    /// Clear queue entries
    Clear {
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Only clear completed requirements
        #[clap(long)]
        completed: bool,
    },
    /// Peek at the top item in your queue without removing it. When a role
    /// is active and --role is not passed, defaults to filtering on it.
    /// Use this between work items to see what's next.
    Next {
        /// Filter to items routed to a specific role
        #[clap(long)]
        role: Option<String>,
        /// Show the top item from the full queue regardless of role
        #[clap(long)]
        all: bool,
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Bypass the active role's scope_tags / scope_status filters.
        /// trace:TASK-1-021 | ai:claude
        #[clap(long)]
        no_scope: bool,
    },
    /// Mark a requirement as completed AND remove it from your queue in one
    /// atomic step. Convenience for the implementer's done-then-pickup-next
    /// loop. Equivalent to: `aida edit <id> --status completed && aida queue remove <id>`.
    Done {
        /// Requirement ID (UUID or SPEC-ID)
        id: String,
        /// User ID (defaults to AIDA_USER or system user)
        #[clap(long)]
        user: Option<String>,
        /// Skip confirmation
        #[clap(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Add a new requirement
    Add {
        /// Title of the requirement
        #[clap(long)]
        title: Option<String>,

        /// Description of the requirement
        #[clap(long)]
        description: Option<String>,

        /// Status of the requirement (draft, approved, completed, rejected)
        #[clap(long)]
        status: Option<String>,

        /// Priority of the requirement (high, medium, low)
        #[clap(long)]
        priority: Option<String>,

        /// Type of requirement (functional, non-functional, system, user, bug, epic, story, task, spike, sprint, folder)
        #[clap(long)]
        r#type: Option<String>,

        /// Owner of the requirement (defaults to AIDA_AUTHOR env var or system user)
        #[clap(long)]
        owner: Option<String>,

        /// Feature the requirement belongs to (defaults to REQ_FEATURE env var or "Uncategorized")
        #[clap(long)]
        feature: Option<String>,

        /// Tags for the requirement (comma-separated)
        #[clap(long)]
        tags: Option<String>,

        /// Custom ID prefix override (uppercase letters only, e.g., SEC, PERF)
        #[clap(long)]
        prefix: Option<String>,

        /// Parent requirement ID (UUID or SPEC-ID) to link as child
        #[clap(long)]
        parent: Option<String>,

        /// Use interactive mode (prompts)
        #[clap(long)]
        interactive: bool,
    },

    /// List all requirements
    List {
        /// Filter by status
        #[clap(long)]
        status: Option<String>,

        /// Filter by priority
        #[clap(long)]
        priority: Option<String>,

        /// Filter by type
        #[clap(long)]
        r#type: Option<String>,

        /// Filter by feature
        #[clap(long)]
        feature: Option<String>,

        /// Filter by tags (comma separated)
        #[clap(long)]
        tags: Option<String>,

        /// Bypass the active role's scope filters for this command.
        /// trace:TASK-1-021 | ai:claude
        #[clap(long)]
        no_scope: bool,
    },

    /// Show details for a specific requirement
    Show {
        /// The ID of the requirement to show
        id: String,

        /// Print comment bodies inline after the description (instead of just
        /// the count). Equivalent to following up with `aida comment list <ID>`.
        #[clap(long, short = 'c')]
        comments: bool,
    },

    /// Edit an existing requirement
    Edit {
        /// The ID of the requirement to edit
        id: String,

        /// New title for the requirement
        #[clap(long)]
        title: Option<String>,

        /// New description for the requirement
        #[clap(long)]
        description: Option<String>,

        /// New status (draft, approved, completed, rejected)
        #[clap(long)]
        status: Option<String>,

        /// New priority (high, medium, low)
        #[clap(long)]
        priority: Option<String>,

        /// New type (functional, non-functional, system, user, bug, epic, story, task, spike, sprint, folder)
        #[clap(long)]
        r#type: Option<String>,

        /// New owner
        #[clap(long)]
        owner: Option<String>,

        /// New feature
        #[clap(long)]
        feature: Option<String>,

        /// New tags (comma-separated, replaces existing)
        #[clap(long)]
        tags: Option<String>,

        /// Use interactive mode (launches editor)
        #[clap(long, short = 'i')]
        interactive: bool,
    },

    /// Delete a requirement
    Del {
        /// The ID (UUID or SPEC-ID) of the requirement to delete
        id: String,

        /// Skip confirmation prompt
        #[clap(long, short = 'y')]
        yes: bool,
    },

    /// Feature management commands
    #[clap(subcommand, hide = true)]
    Feature(FeatureCommand),

    /// Database management commands
    #[clap(subcommand, hide = true)]
    Db(DbCommand),

    /// SQLite cache view commands (git-canonical mode only)
    #[clap(subcommand, hide = true)]
    Cache(CacheCommand),

    /// Show this project's status — storage mode, requirement counts,
    /// cache state, sync state, recent activity. When run inside the AIDA
    /// repo itself, also shows release-readiness diagnostics.
    Status {
        /// Suppress the AIDA-development-context section even when running
        /// inside the aida repo
        #[clap(long)]
        no_dev_context: bool,
    },

    /// AIDA-developer-only commands: activate the in-repo dev binary,
    /// run dev servers, install shell helpers. End users don't need these.
    #[clap(subcommand, hide = true)]
    Dev(DevCommand),

    /// Manage personas / hats — persistent named contexts that resume
    /// across shells. `aida role enter <name>` switches; `aida role list`
    /// shows what's defined for this project. State at `.aida/roles/`.
    #[clap(subcommand)]
    Role(RoleCommand),

    /// Inspect or resume past Claude Code sessions in this project,
    /// enriched with the AIDA role + most-recent spec for each session
    /// (extracted from the SessionStart hook output stored in each
    /// session's `.jsonl`). Wrapper around `claude --resume` since
    /// Claude Code's auto-generated subject can't be customized.
    /// trace:FR-1-043 | ai:claude
    #[clap(subcommand)]
    Session(SessionCommand),

    /// One-line project + role summary suitable for shell prompts and
    /// the `statusLine.command` setting in ~/.claude/settings.json.
    /// Sub-50ms (reads the cache + the orphan-store queue YAML). Format:
    ///   aida · <project> · role:<name> · @SPEC · q:N · cache:fresh|stale
    /// Where `q:N` is the depth of the queue routed to the active role
    /// (omitted when zero). trace:FR-1-041 | ai:claude
    Statusline {
        /// When to emit ANSI color: `auto` (default — color iff stdout
        /// is a TTY and `NO_COLOR` is unset), `always`, `never`.
        #[clap(long, value_parser = ["auto", "always", "never"], default_value = "auto")]
        color: String,
    },

    /// Upgrade aida to the latest release (or a specified version).
    /// Detects how aida was installed (cargo / pre-built binary) and uses
    /// the matching upgrade strategy. From a developer build with no
    /// --target, scans common install locations and offers to upgrade
    /// any stale sibling installs.
    Upgrade {
        /// Compare current/sibling versions to latest release; report only, don't install.
        #[clap(long)]
        check: bool,

        /// Install a specific version instead of latest. Format: `v0.4.0`.
        #[clap(long)]
        version: Option<String>,

        /// Skip confirmation prompt
        #[clap(long, short = 'y')]
        yes: bool,

        /// Upgrade a specific binary path instead of the currently-running one.
        /// Useful from a developer shell to upgrade `~/.local/bin/aida` etc.
        #[clap(long)]
        target: Option<String>,
    },

    /// Relationship management commands
    #[clap(subcommand, hide = true)]
    Rel(RelationshipCommand),

    /// Relationship definition management commands
    #[clap(subcommand, hide = true)]
    RelDef(RelDefCommand),

    /// Manage comments on requirements
    #[clap(subcommand)]
    Comment(CommentCommand),

    /// ID configuration commands
    #[clap(subcommand, hide = true)]
    Config(ConfigCommand),

    /// Requirement type management commands
    #[clap(subcommand, name = "type", hide = true)]
    Type(TypeCommand),

    /// Export requirements to different formats
    #[clap(hide = true)]
    Export {
        /// Output format (mapping, json, tree)
        #[clap(long, short = 'f', default_value = "mapping")]
        format: String,

        /// Output file path
        #[clap(long, short = 'o')]
        output: Option<PathBuf>,

        /// Requirement ID (UUID or SPEC-ID) for tree export - exports this requirement and all descendants
        #[clap(long)]
        id: Option<String>,
    },

    /// Import requirements from a tree JSON file
    #[clap(hide = true)]
    Import {
        /// Path to the tree JSON file to import
        file: PathBuf,

        /// Parent requirement ID (UUID or SPEC-ID) to attach imported tree under
        #[clap(long)]
        parent: Option<String>,

        /// Conflict strategy: skip, rename, replace (default: skip)
        #[clap(long, default_value = "skip")]
        on_conflict: String,
    },

    /// Open the user guide in the default browser
    #[clap(hide = true)]
    UserGuide {
        /// Open in dark mode
        #[clap(long)]
        dark: bool,
    },

    /// Server management commands (requires --server or AIDA_SERVER)
    #[clap(subcommand, hide = true)]
    Server(ServerCommand),

    /// Code-to-requirement traceability commands
    #[clap(subcommand, hide = true)]
    Trace(TraceCommand),

    /// Report generation commands
    #[clap(subcommand, hide = true)]
    Report(ReportCommand),

    /// Initialize AIDA in the current project
    Init {
        /// Skip generating agent skills and commands (.claude/* and .codex/skills/*)
        #[clap(long)]
        no_skills: bool,

        /// Agent profile to scaffold
        #[clap(long, default_value = "both", value_parser = ["claude", "codex", "both"])]
        agent: String,

        /// Skip generating commit validation hooks
        #[clap(long)]
        no_hooks: bool,

        /// Overwrite existing files if already initialized
        #[clap(long)]
        force: bool,

        /// (deprecated, accepted for backwards compat) Initialize in
        /// distributed mode. As of EPIC-1-001 distributed is the default,
        /// so this flag is a no-op. Use `--centralized` to opt out.
        #[clap(long, hide = true)]
        distributed: bool,

        /// Initialize in legacy centralized mode (SQLite-canonical, single
        /// requirements.db). Deprecated — git-canonical is now the default.
        #[clap(long)]
        centralized: bool,

        /// Use a sibling repo instead of an orphan branch for the store.
        /// Recommended for multi-repo workspaces where multiple code repos
        /// share one store. Implies distributed mode.
        #[clap(long)]
        sibling: bool,

        /// Git remote URL for the shared aida registry (used with --sibling).
        /// Example: git@github.com:org/aida-registry.git
        #[clap(long)]
        registry_remote: Option<String>,
    },

    /// Scaffolding management commands
    #[clap(subcommand, hide = true)]
    Scaffold(ScaffoldCommand),

    /// GitLab integration commands
    #[clap(subcommand, hide = true)]
    Gitlab(GitLabCommand),

    /// GitHub integration commands
    #[clap(subcommand, hide = true)]
    Github(GitHubCommand),

    /// Jira integration commands
    #[clap(subcommand, hide = true)]
    Jira(JiraCommand),

    /// Start MCP (Model Context Protocol) server over stdio
    ///
    /// Exposes AIDA requirements as MCP tools for Claude Code integration.
    /// Reads JSON-RPC 2.0 requests from stdin, writes responses to stdout.
    #[clap(hide = true)]
    McpServe,

    /// Search requirements for a pattern (like grep)
    #[clap(hide = true)]
    Grep {
        /// Pattern to search for (regex supported with -E)
        pattern: String,

        /// Case insensitive search
        #[clap(long, short = 'i')]
        ignore_case: bool,

        /// Use extended regex (ERE)
        #[clap(long, short = 'E')]
        extended_regex: bool,

        /// Show N lines of context after match
        #[clap(long, short = 'A', default_value = "0")]
        after_context: usize,

        /// Show N lines of context before match
        #[clap(long, short = 'B', default_value = "0")]
        before_context: usize,

        /// Show N lines of context before and after match
        #[clap(long, short = 'C')]
        context: Option<usize>,

        /// Search only in specific field(s): title, description, comments, tags, owner, feature
        #[clap(long, short = 'f')]
        field: Option<String>,

        /// Filter by status
        #[clap(long)]
        status: Option<String>,

        /// Filter by type
        #[clap(long)]
        r#type: Option<String>,

        /// Filter by feature
        #[clap(long)]
        feature: Option<String>,

        /// Only show matching SPEC-IDs (like grep -l)
        #[clap(long, short = 'l')]
        files_with_matches: bool,

        /// Show match count per requirement (like grep -c)
        #[clap(long, short = 'c')]
        count: bool,

        /// Invert match (show non-matching)
        #[clap(long, short = 'v')]
        invert_match: bool,
    },

    /// Personal work queue commands
    #[clap(subcommand, hide = true)]
    Queue(QueueCommand),

    /// Simple search for requirements (case-insensitive by default)
    Search {
        /// Search query (searches title, description, and comments)
        query: String,

        /// Make search case-sensitive (default is case-insensitive)
        #[clap(long, short = 's')]
        case_sensitive: bool,

        /// Filter by status
        #[clap(long)]
        status: Option<String>,

        /// Filter by feature
        #[clap(long)]
        feature: Option<String>,

        /// Maximum number of matches to return (default: 200)
        #[clap(long, short = 'n', default_value = "200")]
        limit: usize,
    },

    /// Project activity — what's been touched and how it stands now.
    /// Default mode is a per-requirement digest sorted by last-touch
    /// time, intended for "what was I up to last session?" Pass
    /// `--events` to switch to a chronological per-event feed (slower;
    /// decodes each commit's YAML diff into status changes, comments
    /// added, etc.).
    /// trace:FR-1-037 | ai:claude
    History {
        /// Number of items to show. In digest mode (default) this caps
        /// the number of distinct requirements; in --events mode it
        /// caps the number of decoded events.
        #[clap(long, short = 'n', default_value = "20")]
        limit: usize,

        /// Walk at most N commits on the orphan branch. Default 250 in
        /// digest mode (cheap to scan) and 5x --limit in --events mode.
        #[clap(long)]
        max_commits: Option<usize>,

        /// Switch to per-event chronological mode — each commit's YAML
        /// diff is decoded into one event line per change (status
        /// transitions, comments added, tags edited, etc.). Slower than
        /// digest because it shells out to `git show` per file per
        /// commit; useful for inspecting one requirement closely with
        /// --id, less useful as a general overview.
        #[clap(long)]
        events: bool,

        /// Only show entries for this requirement (SPEC-ID match).
        #[clap(long)]
        id: Option<String>,

        /// Only show entries for requirements of this type (functional,
        /// bug, …).
        #[clap(long)]
        r#type: Option<String>,

        /// Only show entries authored by this user (matches against the
        /// last_modified_by HLC field if present, else the git committer
        /// email).
        #[clap(long)]
        author: Option<String>,

        /// Only show events after this date (ISO 8601, e.g. 2026-05-01).
        #[clap(long)]
        since: Option<String>,

        /// Only show events before this date (ISO 8601).
        #[clap(long)]
        until: Option<String>,

        /// (--events only) filter to status transitions.
        #[clap(long)]
        status_changes: bool,

        /// (--events only) filter to comment events.
        #[clap(long)]
        comments: bool,

        /// (--events only) terse one-line-per-event format.
        #[clap(long)]
        oneline: bool,
    },

    /// List all commands (including the less-common ones hidden from
    /// `aida --help`), grouped by topic.
    HelpAll,
}

/// GitHub integration commands
#[derive(Subcommand, Debug)]
pub enum GitHubCommand {
    /// Configure GitHub connection
    Config {
        /// Repository in owner/repo format (e.g., "myorg/myproject")
        #[clap(long)]
        repo: Option<String>,

        /// Personal Access Token (or set AIDA_GITHUB_TOKEN env var)
        #[clap(long)]
        token: Option<String>,

        /// GitHub API URL (default: https://api.github.com)
        #[clap(long)]
        api_url: Option<String>,

        /// Show current configuration
        #[clap(long)]
        show: bool,
    },

    /// Test connection to GitHub
    Test,

    /// List issues from GitHub
    List {
        /// Filter by state (open, closed, all)
        #[clap(long, default_value = "open")]
        state: String,

        /// Filter by labels (comma-separated)
        #[clap(long)]
        labels: Option<String>,

        /// Maximum number of issues to show
        #[clap(long, default_value = "20")]
        limit: u32,
    },

    /// Show a specific GitHub issue
    Show {
        /// Issue number (e.g., 42 or GH-42)
        number: String,
    },

    /// Push a requirement to GitHub as an issue
    Push {
        /// Requirement ID (spec_id or agreed_id)
        id: String,
    },

    /// Pull GitHub issues into AIDA as requirements
    Pull {
        /// Only pull issues with these labels (comma-separated)
        #[clap(long)]
        labels: Option<String>,

        /// Only pull open issues (default: true)
        #[clap(long, default_value = "true")]
        open_only: bool,

        /// Maximum number of issues to pull
        #[clap(long, default_value = "50")]
        limit: u32,

        /// Dry run — show what would be imported without creating requirements
        #[clap(long)]
        dry_run: bool,
    },

    /// Sync: detect drift between AIDA requirements and GitHub issues
    Sync {
        /// Only check linked items (those with [GH-N] prefix or github URL)
        #[clap(long)]
        linked_only: bool,

        /// Apply changes (default: dry-run showing what would change)
        #[clap(long)]
        apply: bool,
    },

    /// List labels in the GitHub repository
    Labels {
        /// Create default AIDA labels if missing
        #[clap(long)]
        create_missing: bool,
    },
}

/// Jira integration commands
#[derive(Subcommand, Debug)]
pub enum JiraCommand {
    /// Configure Jira connection and field mapping
    Config {
        /// Jira Cloud instance URL (e.g., https://myorg.atlassian.net)
        #[clap(long)]
        url: Option<String>,

        /// Jira project key (e.g., AIDA)
        #[clap(long)]
        project: Option<String>,

        /// Email for API authentication
        #[clap(long)]
        email: Option<String>,

        /// Show current configuration
        #[clap(long)]
        show: bool,

        /// Show the field mapping spec
        #[clap(long)]
        show_mapping: bool,
    },

    /// Test connection to Jira
    Test,

    /// List issues from Jira
    List {
        /// JQL query (default: all issues in configured project)
        #[clap(long)]
        jql: Option<String>,

        /// Maximum results
        #[clap(long, default_value = "20")]
        limit: u32,
    },

    /// Show a specific Jira issue
    Show {
        /// Issue key (e.g., PROJ-123)
        key: String,
    },

    /// Push a requirement to Jira as an issue
    Push {
        /// Requirement ID (spec_id or agreed_id)
        id: String,
    },

    /// Sync: detect drift between AIDA requirements and linked Jira issues
    Sync {
        /// Apply changes — push AIDA state to Jira for drifted items
        #[clap(long)]
        apply: bool,
    },

    /// Pull Jira issues into AIDA as requirements
    Pull {
        /// JQL filter (default: all open issues in project)
        #[clap(long)]
        jql: Option<String>,

        /// Maximum issues to pull
        #[clap(long, default_value = "50")]
        limit: u32,

        /// Dry run — show what would be imported
        #[clap(long)]
        dry_run: bool,
    },
}
