// trace:ARCH-github-integration | ai:claude
//! GitHub API data models.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A GitHub repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubRepo {
    pub id: u64,
    pub name: String,
    pub full_name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub html_url: String,
    pub default_branch: String,
    #[serde(rename = "private")]
    pub is_private: bool,
}

/// A GitHub issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubIssue {
    pub id: u64,
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub state: String, // "open" or "closed"
    pub html_url: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub closed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub labels: Vec<GitHubLabel>,
    #[serde(default)]
    pub assignees: Vec<GitHubUser>,
    #[serde(default)]
    pub assignee: Option<GitHubUser>,
    pub user: GitHubUser,
    #[serde(default)]
    pub milestone: Option<GitHubMilestone>,
    #[serde(default)]
    pub comments: u64,
    /// Pull requests show up as issues — this field distinguishes them
    #[serde(default)]
    pub pull_request: Option<serde_json::Value>,
}

impl GitHubIssue {
    pub fn is_open(&self) -> bool {
        self.state == "open"
    }

    pub fn is_pull_request(&self) -> bool {
        self.pull_request.is_some()
    }

    pub fn assignee_login(&self) -> Option<&str> {
        self.assignee.as_ref().map(|u| u.login.as_str())
    }

    pub fn label_names(&self) -> Vec<&str> {
        self.labels.iter().map(|l| l.name.as_str()).collect()
    }
}

/// A GitHub user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubUser {
    pub id: u64,
    pub login: String,
    #[serde(default)]
    pub name: Option<String>,
    pub html_url: String,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

/// A GitHub label.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubLabel {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub color: String,
}

/// A GitHub milestone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubMilestone {
    pub id: u64,
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    pub state: String,
    #[serde(default)]
    pub due_on: Option<DateTime<Utc>>,
}

/// A GitHub comment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubComment {
    pub id: u64,
    pub body: String,
    pub user: GitHubUser,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub html_url: String,
}

/// Request to create a GitHub issue.
#[derive(Debug, Clone, Serialize)]
pub struct CreateIssueRequest {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub assignees: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub milestone: Option<u64>,
}

/// Request to update a GitHub issue.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateIssueRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>, // "open" or "closed"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignees: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub milestone: Option<u64>,
}

/// Filter for listing issues.
#[derive(Debug, Clone, Default)]
pub struct IssueFilter {
    pub state: Option<String>, // "open", "closed", "all"
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    pub creator: Option<String>,
    pub milestone: Option<String>,
    pub sort: Option<String>,      // "created", "updated", "comments"
    pub direction: Option<String>, // "asc", "desc"
    pub since: Option<DateTime<Utc>>,
    pub per_page: Option<u32>,
    pub page: Option<u32>,
}

impl IssueFilter {
    pub fn open() -> Self {
        Self {
            state: Some("open".into()),
            ..Default::default()
        }
    }

    pub fn to_query_params(&self) -> Vec<(String, String)> {
        let mut params = Vec::new();
        if let Some(ref s) = self.state {
            params.push(("state".into(), s.clone()));
        }
        if !self.labels.is_empty() {
            params.push(("labels".into(), self.labels.join(",")));
        }
        if let Some(ref a) = self.assignee {
            params.push(("assignee".into(), a.clone()));
        }
        if let Some(ref c) = self.creator {
            params.push(("creator".into(), c.clone()));
        }
        if let Some(ref s) = self.sort {
            params.push(("sort".into(), s.clone()));
        }
        if let Some(ref d) = self.direction {
            params.push(("direction".into(), d.clone()));
        }
        if let Some(ref s) = self.since {
            params.push(("since".into(), s.to_rfc3339()));
        }
        params.push(("per_page".into(), self.per_page.unwrap_or(30).to_string()));
        if let Some(p) = self.page {
            params.push(("page".into(), p.to_string()));
        }
        params
    }
}
