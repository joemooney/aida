//! `aida tracker_cmd` clap types, owned by `tracker_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

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
