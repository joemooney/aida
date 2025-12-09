use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[clap(author, version, about = "A simple requirements management system")]
pub struct Cli {
    /// Path to the requirements file
    #[clap(long, default_value = "requirements.yaml")]
    pub file: String,

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

    /// Migrate database between formats (YAML <-> SQLite)
    Migrate {
        /// Source format: "yaml" or "sqlite"
        #[clap(long)]
        from: String,

        /// Target format: "yaml" or "sqlite"
        #[clap(long)]
        to: String,

        /// Output path (optional, defaults to same name with new extension)
        #[clap(long, short = 'o')]
        output: Option<PathBuf>,

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
        #[clap(long)]
        id: String,

        /// Comment content
        #[clap(long)]
        content: Option<String>,

        /// Author of the comment (defaults to system user)
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

        /// Type of the requirement (functional, non-functional, system, user)
        #[clap(long)]
        r#type: Option<String>,

        /// Owner of the requirement
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

        /// New type (functional, non-functional, system, user, etc.)
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
}
