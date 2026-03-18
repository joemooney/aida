// trace:ARCH-jira-integration | ai:claude
//! Jira REST API data models.

use serde::{Deserialize, Serialize};

/// A Jira issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraIssue {
    pub id: String,
    pub key: String, // e.g., "PROJ-123"
    #[serde(rename = "self")]
    pub self_url: String,
    pub fields: JiraIssueFields,
}

impl JiraIssue {
    pub fn summary(&self) -> &str {
        &self.fields.summary
    }
    pub fn description_text(&self) -> String {
        // Jira Cloud uses ADF (Atlassian Document Format) for description
        // Extract plain text from the content array if present
        self.fields.description.as_ref()
            .and_then(|d| d.get("content"))
            .and_then(|c| c.as_array())
            .map(|paragraphs| {
                paragraphs.iter()
                    .filter_map(|p| p.get("content"))
                    .filter_map(|c| c.as_array())
                    .flat_map(|texts| texts.iter())
                    .filter_map(|t| t.get("text"))
                    .filter_map(|t| t.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    }
    pub fn status_name(&self) -> &str {
        self.fields.status.as_ref()
            .and_then(|s| s.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("Unknown")
    }
    pub fn issue_type_name(&self) -> &str {
        self.fields.issuetype.as_ref()
            .and_then(|t| t.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("Task")
    }
    pub fn priority_name(&self) -> &str {
        self.fields.priority.as_ref()
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("Medium")
    }
    pub fn assignee_name(&self) -> Option<&str> {
        self.fields.assignee.as_ref()
            .and_then(|a| a.get("displayName"))
            .and_then(|n| n.as_str())
    }
    pub fn labels(&self) -> &[String] {
        &self.fields.labels
    }
}

/// Jira issue fields (the nested fields object).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraIssueFields {
    pub summary: String,
    #[serde(default)]
    pub description: Option<serde_json::Value>, // ADF format
    #[serde(default)]
    pub status: Option<serde_json::Value>,
    #[serde(default)]
    pub issuetype: Option<serde_json::Value>,
    #[serde(default)]
    pub priority: Option<serde_json::Value>,
    #[serde(default)]
    pub assignee: Option<serde_json::Value>,
    #[serde(default)]
    pub reporter: Option<serde_json::Value>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub created: Option<String>,
    #[serde(default)]
    pub updated: Option<String>,
    #[serde(default)]
    pub resolution: Option<serde_json::Value>,
}

/// Jira search results (from /rest/api/3/search/jql — new API).
#[derive(Debug, Clone, Deserialize)]
pub struct JiraSearchResults {
    pub issues: Vec<JiraIssue>,
    #[serde(default, rename = "nextPageToken")]
    pub next_page_token: Option<String>,
    #[serde(default, rename = "isLast")]
    pub is_last: Option<bool>,
    // Legacy fields (from old /search endpoint)
    #[serde(default)]
    pub total: u64,
}

/// Jira project info.
#[derive(Debug, Clone, Deserialize)]
pub struct JiraProject {
    pub id: String,
    pub key: String,
    pub name: String,
    #[serde(rename = "self")]
    pub self_url: String,
}

/// Request to create a Jira issue.
#[derive(Debug, Clone, Serialize)]
pub struct CreateIssueRequest {
    pub fields: CreateIssueFields,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateIssueFields {
    pub project: ProjectRef,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<serde_json::Value>, // ADF format
    pub issuetype: IssueTypeRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<PriorityRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<AssigneeRef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectRef {
    pub key: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IssueTypeRef {
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PriorityRef {
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssigneeRef {
    #[serde(rename = "accountId")]
    pub account_id: String,
}

/// Created issue response.
#[derive(Debug, Clone, Deserialize)]
pub struct CreatedIssue {
    pub id: String,
    pub key: String,
    #[serde(rename = "self")]
    pub self_url: String,
}

/// Build an ADF (Atlassian Document Format) paragraph from plain text.
pub fn text_to_adf(text: &str) -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "type": "doc",
        "content": text.lines().map(|line| {
            serde_json::json!({
                "type": "paragraph",
                "content": [{
                    "type": "text",
                    "text": line
                }]
            })
        }).collect::<Vec<_>>()
    })
}
