// trace:ARCH-github-integration | ai:claude
//! GitHub integration for AIDA requirements management.
//!
//! Provides bidirectional sync between AIDA requirements and GitHub Issues:
//! - Push requirements as GitHub issues with mapped labels
//! - Pull issue updates back to AIDA
//! - Track sync state with content hashing
//! - Conflict detection when both sides change

pub mod client;
pub mod config;
pub mod models;

pub use client::{ClientError, GitHubClient};
pub use config::{ConfigError, GitHubConfig, LabelConfig};
pub use models::{
    CreateIssueRequest, GitHubComment, GitHubIssue, GitHubLabel, GitHubMilestone, GitHubRepo,
    GitHubUser, IssueFilter, UpdateIssueRequest,
};
