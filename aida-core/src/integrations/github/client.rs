// trace:ARCH-github-integration | ai:claude
//! GitHub API client.

use anyhow::Result;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};

use super::config::GitHubConfig;
use super::models::*;

/// Errors from the GitHub API client.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("GitHub API error ({status}): {message}")]
    Api { status: u16, message: String },
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Configuration error: {0}")]
    Config(String),
}

impl ClientError {
    pub fn is_rate_limited(&self) -> bool {
        matches!(
            self,
            Self::Api { status: 403, .. } | Self::Api { status: 429, .. }
        )
    }

    pub fn is_auth_error(&self) -> bool {
        matches!(self, Self::Api { status: 401, .. })
    }
}

/// GitHub API client.
pub struct GitHubClient {
    client: reqwest::Client,
    config: GitHubConfig,
}

impl GitHubClient {
    /// Create a new GitHub client.
    pub fn new(config: GitHubConfig) -> Result<Self> {
        config.validate()?;

        let token = config.effective_token()?;
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token))
                .map_err(|e| ClientError::Config(e.to_string()))?,
        );
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static("aida-requirements-manager"),
        );
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static("2022-11-28"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|e| ClientError::Http(e.to_string()))?;

        Ok(Self { client, config })
    }

    /// Test the connection by fetching repo info.
    pub async fn test_connection(&self) -> Result<GitHubRepo> {
        let url = self.config.api_endpoint("");
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        self.handle_response(resp).await
    }

    /// List issues with optional filtering.
    pub async fn list_issues(&self, filter: Option<IssueFilter>) -> Result<Vec<GitHubIssue>> {
        let url = self.config.api_endpoint("/issues");
        let params = filter.unwrap_or_default().to_query_params();

        let resp = self
            .client
            .get(&url)
            .query(&params)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;

        let issues: Vec<GitHubIssue> = self.handle_response(resp).await?;
        // Filter out pull requests (GitHub returns PRs as issues)
        Ok(issues
            .into_iter()
            .filter(|i| !i.is_pull_request())
            .collect())
    }

    /// Get a single issue by number.
    pub async fn get_issue(&self, number: u64) -> Result<GitHubIssue> {
        let url = self.config.api_endpoint(&format!("/issues/{}", number));
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        self.handle_response(resp).await
    }

    /// Create a new issue.
    pub async fn create_issue(&self, request: &CreateIssueRequest) -> Result<GitHubIssue> {
        let url = self.config.api_endpoint("/issues");
        let resp = self
            .client
            .post(&url)
            .json(request)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        self.handle_response(resp).await
    }

    /// Update an existing issue.
    pub async fn update_issue(
        &self,
        number: u64,
        request: &UpdateIssueRequest,
    ) -> Result<GitHubIssue> {
        let url = self.config.api_endpoint(&format!("/issues/{}", number));
        let resp = self
            .client
            .patch(&url)
            .json(request)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        self.handle_response(resp).await
    }

    /// Add a comment to an issue.
    pub async fn add_comment(&self, number: u64, body: &str) -> Result<GitHubComment> {
        let url = self
            .config
            .api_endpoint(&format!("/issues/{}/comments", number));
        let resp = self
            .client
            .post(&url)
            .json(&serde_json::json!({ "body": body }))
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        self.handle_response(resp).await
    }

    /// List comments on an issue.
    pub async fn list_comments(&self, number: u64) -> Result<Vec<GitHubComment>> {
        let url = self
            .config
            .api_endpoint(&format!("/issues/{}/comments", number));
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        self.handle_response(resp).await
    }

    /// List labels in the repository.
    pub async fn list_labels(&self) -> Result<Vec<GitHubLabel>> {
        let url = self.config.api_endpoint("/labels");
        let resp = self
            .client
            .get(&url)
            .query(&[("per_page", "100")])
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        self.handle_response(resp).await
    }

    /// Create a label.
    pub async fn create_label(
        &self,
        name: &str,
        color: &str,
        description: Option<&str>,
    ) -> Result<GitHubLabel> {
        let url = self.config.api_endpoint("/labels");
        let mut body = serde_json::json!({
            "name": name,
            "color": color.trim_start_matches('#'),
        });
        if let Some(desc) = description {
            body["description"] = serde_json::Value::String(desc.to_string());
        }
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        self.handle_response(resp).await
    }

    /// List milestones.
    pub async fn list_milestones(&self) -> Result<Vec<GitHubMilestone>> {
        let url = self.config.api_endpoint("/milestones");
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        self.handle_response(resp).await
    }

    /// Get config reference.
    pub fn config(&self) -> &GitHubConfig {
        &self.config
    }

    /// Handle API response — parse JSON or return error.
    async fn handle_response<T: serde::de::DeserializeOwned>(
        &self,
        resp: reqwest::Response,
    ) -> Result<T> {
        let status = resp.status();
        if status.is_success() {
            let body = resp
                .text()
                .await
                .map_err(|e| ClientError::Http(e.to_string()))?;
            serde_json::from_str(&body).map_err(|e| {
                ClientError::Parse(format!("{}: {}", e, &body[..body.len().min(200)])).into()
            })
        } else {
            let status_code = status.as_u16();
            let body = resp.text().await.unwrap_or_default();

            if status_code == 404 {
                Err(ClientError::NotFound(body).into())
            } else {
                // Try to extract GitHub's error message
                let message = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(String::from))
                    .unwrap_or(body);
                Err(ClientError::Api {
                    status: status_code,
                    message,
                }
                .into())
            }
        }
    }
}
