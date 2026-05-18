// trace:ARCH-jira-integration | ai:claude
//! Jira integration configuration with field mapping spec.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Jira integration configuration.
/// The mapping spec drives how AIDA fields map to Jira fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraConfig {
    /// Jira Cloud instance URL (e.g., "https://myorg.atlassian.net")
    pub instance_url: String,
    /// Jira project key (e.g., "AIDA", "PROJ")
    pub project_key: String,
    /// User email for API authentication
    pub user_email: String,
    /// API token (NOT stored in config — use AIDA_JIRA_TOKEN env var)
    #[serde(skip)]
    pub api_token: Option<String>,
    /// Whether the integration is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Field mapping specification
    #[serde(default)]
    pub mapping: FieldMapping,
}

fn default_true() -> bool {
    true
}

/// Bidirectional field mapping between AIDA and Jira.
/// This is the core of the mapping spec — it drives how fields translate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldMapping {
    /// AIDA requirement type → Jira issue type name
    #[serde(default = "default_type_mapping")]
    pub types: HashMap<String, String>,
    /// AIDA status → Jira status name (for transitions)
    #[serde(default = "default_status_mapping")]
    pub statuses: HashMap<String, String>,
    /// AIDA priority → Jira priority name
    #[serde(default = "default_priority_mapping")]
    pub priorities: HashMap<String, String>,
    /// Jira issue type → AIDA requirement type (reverse mapping for pull)
    #[serde(default = "default_reverse_type_mapping")]
    pub reverse_types: HashMap<String, String>,
    /// Jira status → AIDA status (reverse mapping for pull)
    #[serde(default = "default_reverse_status_mapping")]
    pub reverse_statuses: HashMap<String, String>,
    /// AIDA label prefix for tags synced to Jira
    #[serde(default = "default_label_prefix")]
    pub label_prefix: String,
}

fn default_label_prefix() -> String {
    "aida:".into()
}

fn default_type_mapping() -> HashMap<String, String> {
    let mut m = HashMap::new();
    // Default to Task for most types — safe for all Jira project types.
    // Override in ~/.config/aida/jira.toml for projects with Story/Bug types.
    m.insert("Functional".into(), "Task".into());
    m.insert("NonFunctional".into(), "Task".into());
    m.insert("Bug".into(), "Task".into());
    m.insert("Story".into(), "Task".into());
    m.insert("Task".into(), "Task".into());
    m.insert("Epic".into(), "Epic".into());
    m.insert("Spike".into(), "Task".into());
    m
}

fn default_status_mapping() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("Draft".into(), "To Do".into());
    m.insert("Approved".into(), "To Do".into());
    m.insert("Planned".into(), "To Do".into());
    m.insert("In Progress".into(), "In Progress".into());
    m.insert("Completed".into(), "Done".into());
    m.insert("Rejected".into(), "Done".into());
    m
}

fn default_priority_mapping() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("High".into(), "High".into());
    m.insert("Medium".into(), "Medium".into());
    m.insert("Low".into(), "Low".into());
    m
}

fn default_reverse_type_mapping() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("Bug".into(), "Bug".into());
    m.insert("Story".into(), "Story".into());
    m.insert("Task".into(), "Task".into());
    m.insert("Epic".into(), "Epic".into());
    m.insert("Sub-task".into(), "Task".into());
    m
}

fn default_reverse_status_mapping() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("To Do".into(), "Draft".into());
    m.insert("In Progress".into(), "In Progress".into());
    m.insert("Done".into(), "Completed".into());
    m.insert("Backlog".into(), "Draft".into());
    m
}

impl Default for FieldMapping {
    fn default() -> Self {
        Self {
            types: default_type_mapping(),
            statuses: default_status_mapping(),
            priorities: default_priority_mapping(),
            reverse_types: default_reverse_type_mapping(),
            reverse_statuses: default_reverse_status_mapping(),
            label_prefix: default_label_prefix(),
        }
    }
}

impl Default for JiraConfig {
    fn default() -> Self {
        Self {
            instance_url: String::new(),
            project_key: String::new(),
            user_email: String::new(),
            api_token: None,
            enabled: true,
            mapping: FieldMapping::default(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error(
        "Instance URL not configured. Set with: aida jira config --url https://myorg.atlassian.net"
    )]
    NoUrl,
    #[error("Project key not configured. Set with: aida jira config --project PROJ")]
    NoProject,
    #[error("Email not configured. Set with: aida jira config --email you@example.com")]
    NoEmail,
    #[error("API token not set. Set AIDA_JIRA_TOKEN environment variable")]
    NoToken,
}

impl JiraConfig {
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)?;
        let mut config: Self = toml::from_str(&content)?;
        config.api_token = std::env::var("AIDA_JIRA_TOKEN").ok();
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        // Atomic write — uniform with the concurrent-writer paths. trace:TASK-331 | ai:claude
        crate::write_atomic(&path, content)?;
        Ok(())
    }

    pub fn config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot determine config directory"))?;
        Ok(config_dir.join("aida").join("jira.toml"))
    }

    pub fn effective_token(&self) -> Result<String> {
        self.api_token
            .clone()
            .or_else(|| std::env::var("AIDA_JIRA_TOKEN").ok())
            .or_else(|| std::env::var("JIRA_API_KEY").ok())
            .or_else(|| std::env::var("JIRA_API_TOKEN").ok())
            .ok_or_else(|| ConfigError::NoToken.into())
    }

    pub fn validate(&self) -> Result<()> {
        if self.instance_url.is_empty() {
            return Err(ConfigError::NoUrl.into());
        }
        if self.project_key.is_empty() {
            return Err(ConfigError::NoProject.into());
        }
        if self.user_email.is_empty() {
            return Err(ConfigError::NoEmail.into());
        }
        self.effective_token()?;
        Ok(())
    }

    /// Map an AIDA requirement type to a Jira issue type name.
    pub fn map_type(&self, aida_type: &str) -> String {
        self.mapping
            .types
            .get(aida_type)
            .cloned()
            .unwrap_or_else(|| "Task".into())
    }

    /// Map a Jira issue type to an AIDA requirement type.
    pub fn reverse_map_type(&self, jira_type: &str) -> String {
        self.mapping
            .reverse_types
            .get(jira_type)
            .cloned()
            .unwrap_or_else(|| "Task".into())
    }

    /// Map an AIDA status to a Jira status name.
    pub fn map_status(&self, aida_status: &str) -> String {
        self.mapping
            .statuses
            .get(aida_status)
            .cloned()
            .unwrap_or_else(|| "To Do".into())
    }

    /// Map a Jira status to an AIDA status.
    pub fn reverse_map_status(&self, jira_status: &str) -> String {
        self.mapping
            .reverse_statuses
            .get(jira_status)
            .cloned()
            .unwrap_or_else(|| "Draft".into())
    }

    /// Map an AIDA priority to a Jira priority name.
    pub fn map_priority(&self, aida_priority: &str) -> String {
        self.mapping
            .priorities
            .get(aida_priority)
            .cloned()
            .unwrap_or_else(|| "Medium".into())
    }

    /// API base URL.
    pub fn api_url(&self, path: &str) -> String {
        format!(
            "{}/rest/api/3{}",
            self.instance_url.trim_end_matches('/'),
            path
        )
    }
}
