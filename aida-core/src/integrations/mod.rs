// trace:STORY-0321 | ai:claude
//! External integrations module.
//!
//! This module contains integrations with external systems such as:
//! - GitLab (issue tracking and synchronization)
//! - GitHub (issue tracking and synchronization)

#[cfg(feature = "github")]
pub mod github;
#[cfg(feature = "gitlab")]
pub mod gitlab;
#[cfg(feature = "jira")]
pub mod jira;
