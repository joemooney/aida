// trace:ARCH-jira-integration | ai:claude
//! Jira Cloud integration for AIDA requirements management.
//!
//! Provides bidirectional sync between AIDA requirements and Jira issues
//! using a configurable field mapping spec that drives how AIDA types,
//! statuses, and priorities translate to Jira's model.

pub mod client;
pub mod config;
pub mod models;

pub use client::{ClientError, JiraClient};
pub use config::{ConfigError, FieldMapping, JiraConfig};
pub use models::{
    CreateIssueFields, CreateIssueRequest, CreatedIssue, IssueTypeRef, JiraIssue, JiraProject,
    JiraSearchResults, PriorityRef, ProjectRef, text_to_adf,
};
