// trace:STORY-0321 | ai:claude
//! External integrations module.
//!
//! This module contains integrations with external systems such as:
//! - GitLab (issue tracking and synchronization)
//! - Future: GitHub, Jira, Azure DevOps, etc.

#[cfg(feature = "gitlab")]
pub mod gitlab;
