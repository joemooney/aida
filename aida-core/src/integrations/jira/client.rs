// trace:ARCH-jira-integration | ai:claude
//! Jira Cloud REST API v3 client.

use anyhow::Result;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, CONTENT_TYPE};

use super::config::JiraConfig;
use super::models::*;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("Jira API error ({status}): {message}")]
    Api { status: u16, message: String },
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Configuration error: {0}")]
    Config(String),
}

/// Jira Cloud REST API client.
pub struct JiraClient {
    client: reqwest::Client,
    config: JiraConfig,
}

impl JiraClient {
    /// Create a new Jira client.
    /// Authentication: Basic auth with email:api_token (Jira Cloud standard).
    pub fn new(config: JiraConfig) -> Result<Self> {
        config.validate()?;

        let token = config.effective_token()?;
        let auth = base64::encode(format!("{}:{}", config.user_email, token));

        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Basic {}", auth))
                .map_err(|e| ClientError::Config(e.to_string()))?,
        );
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|e| ClientError::Http(e.to_string()))?;

        Ok(Self { client, config })
    }

    /// Test connection by fetching the project.
    pub async fn test_connection(&self) -> Result<JiraProject> {
        let url = self
            .config
            .api_url(&format!("/project/{}", self.config.project_key));
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        self.handle_response(resp).await
    }

    /// Search issues using JQL.
    pub async fn search(&self, jql: &str, max_results: u32) -> Result<JiraSearchResults> {
        let url = self.config.api_url("/search/jql");
        let body = serde_json::json!({
            "jql": jql,
            "maxResults": max_results,
            "fields": ["summary", "description", "status", "issuetype", "priority", "assignee", "reporter", "labels", "created", "updated", "resolution"]
        });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        self.handle_response(resp).await
    }

    /// List issues in the configured project.
    pub async fn list_issues(&self, max_results: u32) -> Result<JiraSearchResults> {
        let jql = format!(
            "project = {} ORDER BY updated DESC",
            self.config.project_key
        );
        self.search(&jql, max_results).await
    }

    /// Get a single issue by key (e.g., "PROJ-123").
    pub async fn get_issue(&self, key: &str) -> Result<JiraIssue> {
        let url = self.config.api_url(&format!("/issue/{}", key));
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        self.handle_response(resp).await
    }

    /// Create a new issue.
    pub async fn create_issue(&self, request: &CreateIssueRequest) -> Result<CreatedIssue> {
        let url = self.config.api_url("/issue");
        let resp = self
            .client
            .post(&url)
            .json(request)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        self.handle_response(resp).await
    }

    /// Update an issue's fields.
    pub async fn update_issue(&self, key: &str, fields: &serde_json::Value) -> Result<()> {
        let url = self.config.api_url(&format!("/issue/{}", key));
        let body = serde_json::json!({ "fields": fields });
        let resp = self
            .client
            .put(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Err(ClientError::Api {
                status,
                message: body,
            }
            .into())
        }
    }

    /// Add a comment to an issue.
    pub async fn add_comment(&self, key: &str, body_text: &str) -> Result<()> {
        let url = self.config.api_url(&format!("/issue/{}/comment", key));
        let body = serde_json::json!({
            "body": text_to_adf(body_text)
        });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Err(ClientError::Api {
                status,
                message: body,
            }
            .into())
        }
    }

    /// Get available transitions for an issue (for status changes).
    pub async fn get_transitions(&self, key: &str) -> Result<Vec<serde_json::Value>> {
        let url = self.config.api_url(&format!("/issue/{}/transitions", key));
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;
        let result: serde_json::Value = self.handle_response(resp).await?;
        Ok(result
            .get("transitions")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// Transition an issue to a new status.
    pub async fn transition_issue(&self, key: &str, transition_id: &str) -> Result<()> {
        let url = self.config.api_url(&format!("/issue/{}/transitions", key));
        let body = serde_json::json!({
            "transition": { "id": transition_id }
        });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClientError::Http(e.to_string()))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Err(ClientError::Api {
                status,
                message: body,
            }
            .into())
        }
    }

    pub fn config(&self) -> &JiraConfig {
        &self.config
    }

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
                ClientError::Api {
                    status: status.as_u16(),
                    message: format!("{}: {}", e, &body[..body.len().min(200)]),
                }
                .into()
            })
        } else {
            let status_code = status.as_u16();
            let body = resp.text().await.unwrap_or_default();
            if status_code == 404 {
                Err(ClientError::NotFound(body).into())
            } else {
                let message = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| {
                        v.get("errorMessages")
                            .and_then(|m| m.as_array())
                            .and_then(|a| a.first())
                            .and_then(|m| m.as_str())
                            .map(String::from)
                    })
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

/// Simple base64 encoding (avoid adding a dependency for this).
mod base64 {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(input: impl AsRef<str>) -> String {
        let bytes = input.as_ref().as_bytes();
        let mut result = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
            let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
            let triple = (b0 << 16) | (b1 << 8) | b2;
            result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
            result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
            if chunk.len() > 1 {
                result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
            } else {
                result.push('=');
            }
            if chunk.len() > 2 {
                result.push(CHARS[(triple & 0x3F) as usize] as char);
            } else {
                result.push('=');
            }
        }
        result
    }
}
