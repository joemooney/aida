use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[clap(author, version, about = "A simple requirements management system")]
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
    },

    /// Show details for a specific requirement
    Show {
        /// The ID of the requirement to show
        id: String,
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
    #[clap(subcommand)]
    Feature(FeatureCommand),

    /// Database management commands
    #[clap(subcommand)]
    Db(DbCommand),

    /// Relationship management commands
    #[clap(subcommand)]
    Rel(RelationshipCommand),

    /// Relationship definition management commands
    #[clap(subcommand)]
    RelDef(RelDefCommand),

    /// Manage comments on requirements
    #[clap(subcommand)]
    Comment(CommentCommand),

    /// ID configuration commands
    #[clap(subcommand)]
    Config(ConfigCommand),

    /// Requirement type management commands
    #[clap(subcommand, name = "type")]
    Type(TypeCommand),

    /// Export requirements to different formats
    Export {
        /// Output format (mapping, json)
        #[clap(long, short = 'f', default_value = "mapping")]
        format: String,

        /// Output file path
        #[clap(long, short = 'o')]
        output: Option<PathBuf>,
    },

    /// Open the user guide in the default browser
    UserGuide {
        /// Open in dark mode
        #[clap(long)]
        dark: bool,
    },

    /// Server management commands (requires --server or AIDA_SERVER)
    #[clap(subcommand)]
    Server(ServerCommand),

    /// Code-to-requirement traceability commands
    #[clap(subcommand)]
    Trace(TraceCommand),

    /// Report generation commands
    #[clap(subcommand)]
    Report(ReportCommand),

    /// Scaffolding management commands
    #[clap(subcommand)]
    Scaffold(ScaffoldCommand),

    /// GitLab integration commands
    #[clap(subcommand)]
    Gitlab(GitLabCommand),

    /// Search requirements for a pattern (like grep)
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
    },
}
