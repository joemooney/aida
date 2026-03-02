// trace:STORY-0321 | ai:claude
//! GitLab integration module.
//!
//! This module provides integration with GitLab for issue tracking and synchronization.
//!
//! # Features
//!
//! - Connect to GitLab.com or self-hosted GitLab instances
//! - View GitLab issues within AIDA
//! - Link AIDA requirements to GitLab issues
//! - Create GitLab issues from AIDA requirements
//! - Sync changes between AIDA and GitLab
//!
//! # Configuration
//!
//! GitLab integration is configured via `~/.config/aida/gitlab.toml`:
//!
//! ```toml
//! [gitlab]
//! url = "https://gitlab.com"
//! project_id = 12345
//! # Token is read from AIDA_GITLAB_TOKEN environment variable
//! ```
//!
//! # Example
//!
//! ```ignore
//! use aida_core::integrations::gitlab::{GitLabClient, GitLabConfig};
//!
//! let config = GitLabConfig::new("https://gitlab.com", 12345)
//!     .with_token("your-token");
//!
//! let client = GitLabClient::new(config)?;
//! let project = client.test_connection().await?;
//! println!("Connected to: {}", project.name);
//! ```

mod client;
mod config;
mod models;

pub use client::{ClientError, GitLabClient};
pub use config::{
    ConfigError, ConflictStrategy, FieldSyncDirection, FieldSyncRules, GitLabConfig, LabelConfig,
    PollingConfig, SyncConfig, SyncMode,
};
pub use models::{
    CreateIssueRequest, CreateNoteRequest, GitLabError, GitLabIssue, GitLabLabel, GitLabMilestone,
    GitLabNote, GitLabProject, GitLabUser, IssueFilter, IssueReferences, IssueState,
    MilestoneState, StateEvent, UpdateIssueRequest,
};
