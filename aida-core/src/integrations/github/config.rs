// trace:ARCH-github-integration | ai:claude
//! GitHub integration configuration.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// GitHub integration configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubConfig {
    /// GitHub API base URL (default: https://api.github.com)
    #[serde(default = "default_api_url")]
    pub api_url: String,
    /// Repository in "owner/repo" format
    pub repo: String,
    /// Personal access token (not saved to config file — use env var)
    #[serde(skip)]
    pub token: Option<String>,
    /// Whether the integration is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Label mappings
    #[serde(default)]
    pub labels: LabelConfig,
}

fn default_api_url() -> String {
    "https://api.github.com".into()
}

fn default_true() -> bool {
    true
}

/// Label mapping configuration — maps AIDA types/priorities/statuses to GitHub labels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelConfig {
    /// Label prefix (default: "aida:")
    #[serde(default = "default_prefix")]
    pub prefix: String,
    /// Requirement type → label name
    #[serde(default = "default_type_labels")]
    pub types: HashMap<String, String>,
    /// Priority → label name
    #[serde(default = "default_priority_labels")]
    pub priorities: HashMap<String, String>,
    /// Status → label name
    #[serde(default = "default_status_labels")]
    pub statuses: HashMap<String, String>,
}

fn default_prefix() -> String {
    "aida:".into()
}

fn default_type_labels() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("Bug".into(), "bug".into());
    m.insert("Story".into(), "enhancement".into());
    m.insert("Task".into(), "task".into());
    m.insert("Epic".into(), "epic".into());
    m.insert("Functional".into(), "requirement".into());
    m.insert("NonFunctional".into(), "non-functional".into());
    m
}

fn default_priority_labels() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("High".into(), "priority:high".into());
    m.insert("Medium".into(), "priority:medium".into());
    m.insert("Low".into(), "priority:low".into());
    m
}

fn default_status_labels() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("Draft".into(), "status:draft".into());
    m.insert("Approved".into(), "status:approved".into());
    m.insert("InProgress".into(), "status:in-progress".into());
    m.insert("Completed".into(), "status:completed".into());
    m.insert("Rejected".into(), "status:rejected".into());
    m
}

impl Default for LabelConfig {
    fn default() -> Self {
        Self {
            prefix: default_prefix(),
            types: default_type_labels(),
            priorities: default_priority_labels(),
            statuses: default_status_labels(),
        }
    }
}

impl Default for GitHubConfig {
    fn default() -> Self {
        Self {
            api_url: default_api_url(),
            repo: String::new(),
            token: None,
            enabled: true,
            labels: LabelConfig::default(),
        }
    }
}

/// Configuration errors.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Repository not configured. Set with: aida github config --repo owner/repo")]
    NoRepo,
    #[error("Token not configured. Set AIDA_GITHUB_TOKEN env var or use --token")]
    NoToken,
    #[error("Invalid repo format: {0}. Expected 'owner/repo'")]
    InvalidRepo(String),
    #[error("Config file error: {0}")]
    FileError(String),
}

impl GitHubConfig {
    /// Load config from the standard config file path.
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)?;
        let mut config: Self = toml::from_str(&content)?;
        config.token = std::env::var("AIDA_GITHUB_TOKEN").ok();
        Ok(config)
    }

    /// Save config to the standard config file path (token excluded).
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Standard config file path.
    pub fn config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot determine config directory"))?;
        Ok(config_dir.join("aida").join("github.toml"))
    }

    /// Get the effective token (env var or runtime).
    pub fn effective_token(&self) -> Result<String> {
        self.token
            .clone()
            .or_else(|| std::env::var("AIDA_GITHUB_TOKEN").ok())
            .ok_or_else(|| ConfigError::NoToken.into())
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<()> {
        if self.repo.is_empty() {
            return Err(ConfigError::NoRepo.into());
        }
        if !self.repo.contains('/') || self.repo.split('/').count() != 2 {
            return Err(ConfigError::InvalidRepo(self.repo.clone()).into());
        }
        self.effective_token()?;
        Ok(())
    }

    /// Get the owner and repo name separately.
    pub fn owner_repo(&self) -> Result<(&str, &str)> {
        let parts: Vec<&str> = self.repo.split('/').collect();
        if parts.len() != 2 {
            return Err(ConfigError::InvalidRepo(self.repo.clone()).into());
        }
        Ok((parts[0], parts[1]))
    }

    /// Build the full API URL for a path.
    pub fn api_endpoint(&self, path: &str) -> String {
        format!(
            "{}/repos/{}{}",
            self.api_url.trim_end_matches('/'),
            self.repo,
            path
        )
    }
}
