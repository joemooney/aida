// trace:STORY-0321 | ai:claude
//! GitLab API client implementation.

use super::config::GitLabConfig;
use super::models::*;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use std::time::Duration;

/// GitLab API client
#[derive(Clone)]
pub struct GitLabClient {
    client: reqwest::Client,
    config: GitLabConfig,
}

impl GitLabClient {
    /// Create a new GitLab client
    pub fn new(config: GitLabConfig) -> Result<Self, ClientError> {
        config
            .validate()
            .map_err(|e| ClientError::Config(e.to_string()))?;

        let token = config
            .effective_token()
            .ok_or_else(|| ClientError::Config("No token available".to_string()))?;

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token))
                .map_err(|e| ClientError::Config(e.to_string()))?,
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| ClientError::Http(e.to_string()))?;

        Ok(Self { client, config })
    }

    /// Get the API base URL
    fn api_url(&self) -> String {
        self.config.api_url()
    }

    /// Get the project URL prefix
    fn project_url(&self) -> String {
        format!("{}/projects/{}", self.api_url(), self.config.project_id)
    }

    /// Test the connection to GitLab
    pub async fn test_connection(&self) -> Result<GitLabProject, ClientError> {
        let url = self.project_url();
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(ClientError::Api {
                status: status.as_u16(),
                message: error_text,
            });
        }

        response
            .json::<GitLabProject>()
            .await
            .map_err(|e| ClientError::Parse(e.to_string()))
    }

    /// List issues with optional filter
    pub async fn list_issues(
        &self,
        filter: Option<IssueFilter>,
    ) -> Result<Vec<GitLabIssue>, ClientError> {
        let url = format!("{}/issues", self.project_url());
        let mut request = self.client.get(&url);

        if let Some(filter) = filter {
            request = request.query(&filter.to_query_params());
        }

        let response = request
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(ClientError::Api {
                status: status.as_u16(),
                message: error_text,
            });
        }

        response
            .json::<Vec<GitLabIssue>>()
            .await
            .map_err(|e| ClientError::Parse(e.to_string()))
    }

    /// Get a single issue by IID
    pub async fn get_issue(&self, iid: u64) -> Result<GitLabIssue, ClientError> {
        let url = format!("{}/issues/{}", self.project_url(), iid);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            if status.as_u16() == 404 {
                return Err(ClientError::NotFound(format!("Issue #{} not found", iid)));
            }
            let error_text = response.text().await.unwrap_or_default();
            return Err(ClientError::Api {
                status: status.as_u16(),
                message: error_text,
            });
        }

        response
            .json::<GitLabIssue>()
            .await
            .map_err(|e| ClientError::Parse(e.to_string()))
    }

    /// Create a new issue
    pub async fn create_issue(
        &self,
        request: CreateIssueRequest,
    ) -> Result<GitLabIssue, ClientError> {
        let url = format!("{}/issues", self.project_url());
        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(ClientError::Api {
                status: status.as_u16(),
                message: error_text,
            });
        }

        response
            .json::<GitLabIssue>()
            .await
            .map_err(|e| ClientError::Parse(e.to_string()))
    }

    /// Update an existing issue
    pub async fn update_issue(
        &self,
        iid: u64,
        request: UpdateIssueRequest,
    ) -> Result<GitLabIssue, ClientError> {
        let url = format!("{}/issues/{}", self.project_url(), iid);
        let response = self
            .client
            .put(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            if status.as_u16() == 404 {
                return Err(ClientError::NotFound(format!("Issue #{} not found", iid)));
            }
            let error_text = response.text().await.unwrap_or_default();
            return Err(ClientError::Api {
                status: status.as_u16(),
                message: error_text,
            });
        }

        response
            .json::<GitLabIssue>()
            .await
            .map_err(|e| ClientError::Parse(e.to_string()))
    }

    /// Delete an issue (requires admin privileges)
    pub async fn delete_issue(&self, iid: u64) -> Result<(), ClientError> {
        let url = format!("{}/issues/{}", self.project_url(), iid);
        let response = self
            .client
            .delete(&url)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            if status.as_u16() == 404 {
                return Err(ClientError::NotFound(format!("Issue #{} not found", iid)));
            }
            let error_text = response.text().await.unwrap_or_default();
            return Err(ClientError::Api {
                status: status.as_u16(),
                message: error_text,
            });
        }

        Ok(())
    }

    /// Add a note (comment) to an issue
    pub async fn add_note(
        &self,
        iid: u64,
        request: CreateNoteRequest,
    ) -> Result<GitLabNote, ClientError> {
        let url = format!("{}/issues/{}/notes", self.project_url(), iid);
        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(ClientError::Api {
                status: status.as_u16(),
                message: error_text,
            });
        }

        response
            .json::<GitLabNote>()
            .await
            .map_err(|e| ClientError::Parse(e.to_string()))
    }

    /// List labels in the project
    pub async fn list_labels(&self) -> Result<Vec<GitLabLabel>, ClientError> {
        let url = format!("{}/labels", self.project_url());
        let response = self
            .client
            .get(&url)
            .query(&[("per_page", "100")])
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(ClientError::Api {
                status: status.as_u16(),
                message: error_text,
            });
        }

        response
            .json::<Vec<GitLabLabel>>()
            .await
            .map_err(|e| ClientError::Parse(e.to_string()))
    }

    // trace:STORY-0326 | ai:claude
    /// Create a new label in the project
    pub async fn create_label(
        &self,
        name: &str,
        color: &str,
        description: Option<&str>,
    ) -> Result<GitLabLabel, ClientError> {
        let url = format!("{}/labels", self.project_url());

        #[derive(serde::Serialize)]
        struct CreateLabelRequest<'a> {
            name: &'a str,
            color: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            description: Option<&'a str>,
        }

        let request = CreateLabelRequest {
            name,
            color,
            description,
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(ClientError::Api {
                status: status.as_u16(),
                message: error_text,
            });
        }

        response
            .json::<GitLabLabel>()
            .await
            .map_err(|e| ClientError::Parse(e.to_string()))
    }

    /// List milestones in the project
    pub async fn list_milestones(&self) -> Result<Vec<GitLabMilestone>, ClientError> {
        let url = format!("{}/milestones", self.project_url());
        let response = self
            .client
            .get(&url)
            .query(&[("per_page", "100"), ("state", "active")])
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(ClientError::Api {
                status: status.as_u16(),
                message: error_text,
            });
        }

        response
            .json::<Vec<GitLabMilestone>>()
            .await
            .map_err(|e| ClientError::Parse(e.to_string()))
    }

    /// List project members
    pub async fn list_members(&self) -> Result<Vec<GitLabUser>, ClientError> {
        let url = format!("{}/members/all", self.project_url());
        let response = self
            .client
            .get(&url)
            .query(&[("per_page", "100")])
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(ClientError::Api {
                status: status.as_u16(),
                message: error_text,
            });
        }

        response
            .json::<Vec<GitLabUser>>()
            .await
            .map_err(|e| ClientError::Parse(e.to_string()))
    }

    /// Get the current configuration
    pub fn config(&self) -> &GitLabConfig {
        &self.config
    }
}

/// Client errors
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("API error (status {status}): {message}")]
    Api { status: u16, message: String },
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Not found: {0}")]
    NotFound(String),
}

impl ClientError {
    /// Check if this is a rate limit error
    pub fn is_rate_limited(&self) -> bool {
        matches!(self, ClientError::Api { status: 429, .. })
    }

    /// Check if this is an authentication error
    pub fn is_auth_error(&self) -> bool {
        matches!(self, ClientError::Api { status: 401, .. })
    }

    /// Check if this is a permission error
    pub fn is_forbidden(&self) -> bool {
        matches!(self, ClientError::Api { status: 403, .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation_without_token() {
        let config = GitLabConfig::new("https://gitlab.com", 12345);
        let result = GitLabClient::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_issue_filter_query_params() {
        let filter = IssueFilter::open()
            .with_labels(vec!["bug".to_string()])
            .with_pagination(1, 20);

        let params = filter.to_query_params();
        assert!(params.iter().any(|(k, v)| k == "state" && v == "opened"));
        assert!(params.iter().any(|(k, v)| k == "labels" && v == "bug"));
        assert!(params.iter().any(|(k, v)| k == "per_page" && v == "20"));
        assert!(params.iter().any(|(k, v)| k == "page" && v == "1"));
    }
}
