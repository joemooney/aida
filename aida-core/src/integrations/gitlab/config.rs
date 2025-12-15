// trace:STORY-0321 | ai:claude
//! GitLab integration configuration types and handling.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// GitLab connection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitLabConfig {
    /// GitLab instance URL (e.g., "https://gitlab.com" or self-hosted)
    pub url: String,
    /// GitLab project ID (numeric)
    pub project_id: u64,
    /// Personal Access Token (stored separately for security)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Whether the integration is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Label configuration
    #[serde(default)]
    pub labels: LabelConfig,
    /// Polling configuration
    #[serde(default)]
    pub polling: PollingConfig,
    /// Sync configuration
    #[serde(default)]
    pub sync: SyncConfig,
}

fn default_true() -> bool {
    true
}

impl Default for GitLabConfig {
    fn default() -> Self {
        Self {
            url: "https://gitlab.com".to_string(),
            project_id: 0,
            token: None,
            enabled: true,
            labels: LabelConfig::default(),
            polling: PollingConfig::default(),
            sync: SyncConfig::default(),
        }
    }
}

impl GitLabConfig {
    /// Create a new GitLab configuration
    pub fn new(url: impl Into<String>, project_id: u64) -> Self {
        Self {
            url: url.into(),
            project_id,
            ..Default::default()
        }
    }

    /// Set the access token
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    /// Get the API base URL
    pub fn api_url(&self) -> String {
        format!("{}/api/v4", self.url.trim_end_matches('/'))
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.url.is_empty() {
            return Err(ConfigError::MissingField("url".to_string()));
        }
        if self.project_id == 0 {
            return Err(ConfigError::MissingField("project_id".to_string()));
        }
        if self.token.is_none() {
            return Err(ConfigError::MissingField("token".to_string()));
        }
        // Validate URL format
        if !self.url.starts_with("http://") && !self.url.starts_with("https://") {
            return Err(ConfigError::InvalidUrl(self.url.clone()));
        }
        Ok(())
    }

    /// Get the configuration file path
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("aida").join("gitlab.toml"))
    }

    /// Load configuration from file
    pub fn load() -> Result<Option<Self>, ConfigError> {
        let path = Self::config_path().ok_or(ConfigError::NoConfigDir)?;
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| ConfigError::IoError(e.to_string()))?;
        let config: Self = toml::from_str(&content)
            .map_err(|e| ConfigError::ParseError(e.to_string()))?;
        Ok(Some(config))
    }

    /// Save configuration to file
    pub fn save(&self) -> Result<(), ConfigError> {
        let path = Self::config_path().ok_or(ConfigError::NoConfigDir)?;

        // Create directory if it doesn't exist
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ConfigError::IoError(e.to_string()))?;
        }

        // Don't save token to file - it should be in keyring or env var
        let mut config_to_save = self.clone();
        config_to_save.token = None;

        let content = toml::to_string_pretty(&config_to_save)
            .map_err(|e| ConfigError::SerializeError(e.to_string()))?;
        std::fs::write(&path, content)
            .map_err(|e| ConfigError::IoError(e.to_string()))?;
        Ok(())
    }

    /// Get token from environment variable
    pub fn token_from_env() -> Option<String> {
        std::env::var("AIDA_GITLAB_TOKEN").ok()
    }

    /// Get effective token (from config, env, or keyring)
    pub fn effective_token(&self) -> Option<String> {
        // Priority: config > env var
        self.token.clone().or_else(Self::token_from_env)
    }
}

/// Label mapping configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LabelConfig {
    /// Prefix for AIDA-managed labels (e.g., "aida::")
    #[serde(default = "default_label_prefix")]
    pub prefix: String,
    /// Type to label mappings
    #[serde(default)]
    pub types: HashMap<String, String>,
    /// Priority to label mappings
    #[serde(default)]
    pub priorities: HashMap<String, String>,
    /// Status to label mappings
    #[serde(default)]
    pub statuses: HashMap<String, String>,
    /// Auto-create missing labels in GitLab
    #[serde(default)]
    pub auto_create_labels: bool,
}

fn default_label_prefix() -> String {
    "aida::".to_string()
}

impl LabelConfig {
    /// Get default type mappings
    pub fn default_type_mappings() -> HashMap<String, String> {
        let mut map = HashMap::new();
        map.insert("Bug".to_string(), "type::bug".to_string());
        map.insert("Story".to_string(), "type::story".to_string());
        map.insert("Task".to_string(), "type::task".to_string());
        map.insert("Epic".to_string(), "type::epic".to_string());
        map.insert("Functional".to_string(), "type::requirement".to_string());
        map.insert("NonFunctional".to_string(), "type::nfr".to_string());
        map
    }

    /// Get default priority mappings
    pub fn default_priority_mappings() -> HashMap<String, String> {
        let mut map = HashMap::new();
        map.insert("High".to_string(), "priority::high".to_string());
        map.insert("Medium".to_string(), "priority::medium".to_string());
        map.insert("Low".to_string(), "priority::low".to_string());
        map
    }

    /// Get default status mappings
    pub fn default_status_mappings() -> HashMap<String, String> {
        let mut map = HashMap::new();
        map.insert("Draft".to_string(), "status::draft".to_string());
        map.insert("Approved".to_string(), "status::approved".to_string());
        map.insert("InProgress".to_string(), "status::in-progress".to_string());
        map.insert("Completed".to_string(), "status::done".to_string());
        map.insert("Rejected".to_string(), "status::rejected".to_string());
        map
    }
}

/// Polling configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollingConfig {
    /// Whether polling is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Poll interval in seconds
    #[serde(default = "default_poll_interval")]
    pub interval_seconds: u64,
    /// Number of issues to fetch per API call
    #[serde(default = "default_batch_size")]
    pub batch_size: u32,
    /// Maximum concurrent API calls
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: u32,
}

fn default_poll_interval() -> u64 {
    300 // 5 minutes
}

fn default_batch_size() -> u32 {
    100
}

fn default_max_concurrent() -> u32 {
    3
}

impl Default for PollingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_seconds: default_poll_interval(),
            batch_size: default_batch_size(),
            max_concurrent: default_max_concurrent(),
        }
    }
}

/// Sync configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    /// Sync mode
    #[serde(default)]
    pub mode: SyncMode,
    /// Default conflict resolution strategy
    #[serde(default)]
    pub conflict_strategy: ConflictStrategy,
    /// Field-level sync rules
    #[serde(default)]
    pub fields: FieldSyncRules,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            mode: SyncMode::Manual,
            conflict_strategy: ConflictStrategy::Manual,
            fields: FieldSyncRules::default(),
        }
    }
}

/// Sync mode
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SyncMode {
    /// Only push changes from AIDA to GitLab
    PushOnly,
    /// Only pull changes from GitLab to AIDA
    PullOnly,
    /// Sync in both directions
    Bidirectional,
    /// Manual sync only
    #[default]
    Manual,
}

/// Conflict resolution strategy
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ConflictStrategy {
    /// AIDA changes always win
    AidaWins,
    /// GitLab changes always win
    GitLabWins,
    /// Require manual resolution
    #[default]
    Manual,
}

/// Field-level sync rules
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldSyncRules {
    pub title: FieldSyncDirection,
    pub description: FieldSyncDirection,
    pub status: FieldSyncDirection,
    pub priority: FieldSyncDirection,
    pub assignee: FieldSyncDirection,
}

impl Default for FieldSyncRules {
    fn default() -> Self {
        Self {
            title: FieldSyncDirection::Bidirectional,
            description: FieldSyncDirection::Bidirectional,
            status: FieldSyncDirection::PushOnly, // AIDA is source of truth for status
            priority: FieldSyncDirection::PushOnly, // AIDA is source of truth for priority
            assignee: FieldSyncDirection::PullOnly, // GitLab assignment flows to AIDA
        }
    }
}

/// Field sync direction
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum FieldSyncDirection {
    /// Push from AIDA to GitLab only
    PushOnly,
    /// Pull from GitLab to AIDA only
    PullOnly,
    /// Sync in both directions
    #[default]
    Bidirectional,
    /// Don't sync this field
    None,
}

/// Configuration errors
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Missing required field: {0}")]
    MissingField(String),
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),
    #[error("No config directory available")]
    NoConfigDir,
    #[error("IO error: {0}")]
    IoError(String),
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Serialize error: {0}")]
    SerializeError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validation() {
        let config = GitLabConfig::default();
        assert!(config.validate().is_err());

        let config = GitLabConfig::new("https://gitlab.com", 12345)
            .with_token("test-token");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_api_url() {
        let config = GitLabConfig::new("https://gitlab.com/", 123);
        assert_eq!(config.api_url(), "https://gitlab.com/api/v4");

        let config = GitLabConfig::new("https://gitlab.example.com", 123);
        assert_eq!(config.api_url(), "https://gitlab.example.com/api/v4");
    }
}
