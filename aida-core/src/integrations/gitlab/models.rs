// trace:STORY-0321 | ai:claude
//! GitLab API data models.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// GitLab project information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitLabProject {
    pub id: u64,
    pub name: String,
    pub name_with_namespace: String,
    pub path: String,
    pub path_with_namespace: String,
    pub web_url: String,
    #[serde(default)]
    pub description: Option<String>,
    pub default_branch: Option<String>,
    pub visibility: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub last_activity_at: Option<DateTime<Utc>>,
}

/// GitLab issue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitLabIssue {
    pub id: u64,
    pub iid: u64,
    pub project_id: u64,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    pub state: IssueState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub closed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub closed_by: Option<GitLabUser>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub milestone: Option<GitLabMilestone>,
    #[serde(default)]
    pub assignees: Vec<GitLabUser>,
    #[serde(default)]
    pub assignee: Option<GitLabUser>,
    pub author: GitLabUser,
    #[serde(default)]
    pub user_notes_count: u32,
    #[serde(default)]
    pub upvotes: u32,
    #[serde(default)]
    pub downvotes: u32,
    #[serde(default)]
    pub due_date: Option<String>,
    #[serde(default)]
    pub confidential: bool,
    #[serde(default)]
    pub weight: Option<u32>,
    pub web_url: String,
    #[serde(default)]
    pub references: Option<IssueReferences>,
}

impl GitLabIssue {
    /// Get display ID (e.g., "GL-123")
    pub fn display_id(&self) -> String {
        format!("GL-{}", self.iid)
    }

    /// Check if the issue is open
    pub fn is_open(&self) -> bool {
        self.state == IssueState::Opened
    }

    /// Get the first assignee username
    pub fn assignee_username(&self) -> Option<&str> {
        self.assignee
            .as_ref()
            .or_else(|| self.assignees.first())
            .map(|u| u.username.as_str())
    }
}

/// Issue state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum IssueState {
    Opened,
    Closed,
    #[serde(other)]
    Unknown,
}

/// Issue references
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueReferences {
    pub short: String,
    pub relative: String,
    pub full: String,
}

/// GitLab user
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitLabUser {
    pub id: u64,
    pub username: String,
    pub name: String,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub web_url: Option<String>,
}

/// GitLab milestone
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitLabMilestone {
    pub id: u64,
    pub iid: u64,
    pub project_id: Option<u64>,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    pub state: MilestoneState,
    #[serde(default)]
    pub due_date: Option<String>,
    #[serde(default)]
    pub start_date: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub web_url: Option<String>,
}

/// Milestone state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MilestoneState {
    Active,
    Closed,
    #[serde(other)]
    Unknown,
}

/// GitLab label
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitLabLabel {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub description_html: Option<String>,
    pub color: String,
    #[serde(default)]
    pub text_color: Option<String>,
    #[serde(default)]
    pub open_issues_count: Option<u32>,
    #[serde(default)]
    pub closed_issues_count: Option<u32>,
    #[serde(default)]
    pub priority: Option<u32>,
}

/// GitLab note (comment)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitLabNote {
    pub id: u64,
    #[serde(rename = "type")]
    pub note_type: Option<String>,
    pub body: String,
    #[serde(default)]
    pub attachment: Option<String>,
    pub author: GitLabUser,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub system: bool,
    pub noteable_id: u64,
    pub noteable_type: String,
    #[serde(default)]
    pub resolvable: bool,
    #[serde(default)]
    pub confidential: bool,
}

/// Request to create an issue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateIssueRequest {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidential: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee_ids: Option<Vec<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub milestone_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<String>, // Comma-separated
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<u32>,
}

/// Request to update an issue
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateIssueRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_event: Option<StateEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee_ids: Option<Vec<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub milestone_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub add_labels: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remove_labels: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<u32>,
}

/// State event for updating issue state
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StateEvent {
    Close,
    Reopen,
}

/// Request to create a note
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateNoteRequest {
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidential: Option<bool>,
}

/// API error response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitLabError {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}

impl std::fmt::Display for GitLabError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(msg) = &self.message {
            write!(f, "{}", msg)
        } else if let Some(err) = &self.error {
            if let Some(desc) = &self.error_description {
                write!(f, "{}: {}", err, desc)
            } else {
                write!(f, "{}", err)
            }
        } else {
            write!(f, "Unknown error")
        }
    }
}

/// List issues filter
#[derive(Debug, Clone, Default)]
pub struct IssueFilter {
    pub state: Option<IssueState>,
    pub labels: Option<Vec<String>>,
    pub milestone: Option<String>,
    pub assignee_username: Option<String>,
    pub author_username: Option<String>,
    pub search: Option<String>,
    pub updated_after: Option<DateTime<Utc>>,
    pub updated_before: Option<DateTime<Utc>>,
    pub order_by: Option<String>,
    pub sort: Option<String>,
    pub per_page: Option<u32>,
    pub page: Option<u32>,
    /// Specific issue IIDs to fetch
    pub iids: Option<Vec<u64>>,
}

impl IssueFilter {
    /// Create a new filter for open issues
    pub fn open() -> Self {
        Self {
            state: Some(IssueState::Opened),
            ..Default::default()
        }
    }

    /// Create a filter for issues updated after a timestamp
    pub fn updated_since(timestamp: DateTime<Utc>) -> Self {
        Self {
            updated_after: Some(timestamp),
            ..Default::default()
        }
    }

    /// Add labels filter
    pub fn with_labels(mut self, labels: Vec<String>) -> Self {
        self.labels = Some(labels);
        self
    }

    /// Add specific IIDs
    pub fn with_iids(mut self, iids: Vec<u64>) -> Self {
        self.iids = Some(iids);
        self
    }

    /// Set pagination
    pub fn with_pagination(mut self, page: u32, per_page: u32) -> Self {
        self.page = Some(page);
        self.per_page = Some(per_page);
        self
    }

    /// Convert to query parameters
    pub fn to_query_params(&self) -> Vec<(String, String)> {
        let mut params = Vec::new();

        if let Some(state) = &self.state {
            let state_str = match state {
                IssueState::Opened => "opened",
                IssueState::Closed => "closed",
                IssueState::Unknown => "all",
            };
            params.push(("state".to_string(), state_str.to_string()));
        }

        if let Some(labels) = &self.labels {
            params.push(("labels".to_string(), labels.join(",")));
        }

        if let Some(milestone) = &self.milestone {
            params.push(("milestone".to_string(), milestone.clone()));
        }

        if let Some(assignee) = &self.assignee_username {
            params.push(("assignee_username".to_string(), assignee.clone()));
        }

        if let Some(author) = &self.author_username {
            params.push(("author_username".to_string(), author.clone()));
        }

        if let Some(search) = &self.search {
            params.push(("search".to_string(), search.clone()));
        }

        if let Some(updated_after) = &self.updated_after {
            params.push(("updated_after".to_string(), updated_after.to_rfc3339()));
        }

        if let Some(updated_before) = &self.updated_before {
            params.push(("updated_before".to_string(), updated_before.to_rfc3339()));
        }

        if let Some(order_by) = &self.order_by {
            params.push(("order_by".to_string(), order_by.clone()));
        }

        if let Some(sort) = &self.sort {
            params.push(("sort".to_string(), sort.clone()));
        }

        if let Some(per_page) = self.per_page {
            params.push(("per_page".to_string(), per_page.to_string()));
        }

        if let Some(page) = self.page {
            params.push(("page".to_string(), page.to_string()));
        }

        if let Some(iids) = &self.iids {
            for iid in iids {
                params.push(("iids[]".to_string(), iid.to_string()));
            }
        }

        params
    }
}
