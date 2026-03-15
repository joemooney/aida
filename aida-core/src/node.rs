// trace:ARCH-distributed-node | ai:claude
//! Node identity and workspace configuration for distributed AIDA.
//!
//! A **node** is a single clone/installation of AIDA. Each node gets a unique
//! sequential integer ID via the git CAS push loop at `aida init`. After
//! registration, the node can generate globally unique object IDs offline
//! indefinitely.
//!
//! A **workspace** groups multiple code repos that share a single AIDA database.
//! The workspace config is discovered by walking up the directory tree.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::dispenser::IdMode;

// ---------------------------------------------------------------------------
// Node Identity
// ---------------------------------------------------------------------------

/// Information about a registered node (persisted locally, gitignored).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    /// The assigned node ID (unique within the workspace)
    pub node_id: u32,
    /// The user ID who owns this node
    pub user_id: u32,
    /// Hostname at registration time (informational)
    pub hostname: String,
    /// When this node was registered
    pub registered_at: DateTime<Utc>,
}

impl NodeConfig {
    /// Load node config from a TOML file.
    #[cfg(feature = "native")]
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: NodeConfig = toml::from_str(&content)?;
        Ok(config)
    }

    /// Save node config to a TOML file.
    #[cfg(feature = "native")]
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Get the IdMode for this node.
    pub fn id_mode(&self) -> IdMode {
        IdMode::Distributed {
            node_id: self.node_id,
        }
    }
}

/// A node registration entry in the shared registry (committed to git).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRegistryEntry {
    /// The assigned node ID
    pub id: u32,
    /// The user ID who owns this node
    pub user_id: u32,
    /// Hostname at registration time
    pub hostname: String,
    /// Registration timestamp
    pub registered: DateTime<Utc>,
}

/// The shared node registry (committed to git, append-only).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NodeRegistry {
    /// All registered nodes
    #[serde(default)]
    pub nodes: Vec<NodeRegistryEntry>,
}

impl NodeRegistry {
    /// Get the next available node ID.
    pub fn next_node_id(&self) -> u32 {
        self.nodes.iter().map(|n| n.id).max().unwrap_or(0) + 1
    }

    /// Check if a node ID is already registered.
    pub fn is_registered(&self, node_id: u32) -> bool {
        self.nodes.iter().any(|n| n.id == node_id)
    }

    /// Get a node entry by ID.
    pub fn get(&self, node_id: u32) -> Option<&NodeRegistryEntry> {
        self.nodes.iter().find(|n| n.id == node_id)
    }

    /// Register a new node. Returns the assigned node ID.
    pub fn register(&mut self, user_id: u32, hostname: String) -> u32 {
        let id = self.next_node_id();
        self.nodes.push(NodeRegistryEntry {
            id,
            user_id,
            hostname,
            registered: Utc::now(),
        });
        id
    }

    /// Load from a TOML file.
    #[cfg(feature = "native")]
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        let registry: NodeRegistry = toml::from_str(&content)?;
        Ok(registry)
    }

    /// Save to a TOML file.
    #[cfg(feature = "native")]
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// User Identity
// ---------------------------------------------------------------------------

/// A user registration entry in the shared registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRegistryEntry {
    /// The assigned user ID
    pub id: u32,
    /// Display name
    pub name: String,
    /// Email address (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Registration timestamp
    pub registered: DateTime<Utc>,
}

/// The shared user registry (committed to git, append-only).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserRegistry {
    #[serde(default)]
    pub users: Vec<UserRegistryEntry>,
}

impl UserRegistry {
    /// Get the next available user ID.
    pub fn next_user_id(&self) -> u32 {
        self.users.iter().map(|u| u.id).max().unwrap_or(0) + 1
    }

    /// Register a new user. Returns the assigned user ID.
    pub fn register(&mut self, name: String, email: Option<String>) -> u32 {
        let id = self.next_user_id();
        self.users.push(UserRegistryEntry {
            id,
            name,
            email,
            registered: Utc::now(),
        });
        id
    }

    /// Find a user by name.
    pub fn find_by_name(&self, name: &str) -> Option<&UserRegistryEntry> {
        self.users
            .iter()
            .find(|u| u.name.eq_ignore_ascii_case(name))
    }
}

// ---------------------------------------------------------------------------
// Agreed ID Counters
// ---------------------------------------------------------------------------

/// Per-type counters for agreed IDs assigned at merge-to-trunk.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgreedCounters {
    /// Maps type prefix to the last assigned agreed sequence number.
    /// e.g., {"FR": 422, "FEAT": 89}
    #[serde(flatten)]
    pub counters: std::collections::HashMap<String, u32>,
}

impl AgreedCounters {
    /// Get the next agreed ID for a type and increment the counter.
    pub fn next(&mut self, type_prefix: &str) -> u32 {
        let counter = self
            .counters
            .entry(type_prefix.to_uppercase())
            .or_insert(0);
        *counter += 1;
        *counter
    }

    /// Peek at the next agreed ID without incrementing.
    pub fn peek(&self, type_prefix: &str) -> u32 {
        self.counters
            .get(&type_prefix.to_uppercase())
            .copied()
            .unwrap_or(0)
            + 1
    }

    /// Format an agreed ID: `FR-423`
    pub fn format_agreed_id(type_prefix: &str, seq: u32) -> String {
        format!("{}-{}", type_prefix.to_uppercase(), seq)
    }
}

// ---------------------------------------------------------------------------
// Workspace Configuration
// ---------------------------------------------------------------------------

/// Workspace configuration — discovered by walking up the directory tree.
///
/// A workspace groups multiple code repos that share a single AIDA database
/// (the `aida_path` repo). All nodes within a workspace share the same
/// node registry and agreed ID counters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    /// Workspace name (e.g., "gdms-disruptive")
    pub workspace: String,
    /// Path to the aida repo (relative to workspace root)
    #[serde(default = "default_aida_path")]
    pub aida_path: String,
    /// Code repos in this workspace
    #[serde(default)]
    pub repos: Vec<String>,
}

fn default_aida_path() -> String {
    "./aida".to_string()
}

impl WorkspaceConfig {
    /// Discover a workspace config by walking up the directory tree
    /// looking for `.aida-workspace`.
    #[cfg(feature = "native")]
    pub fn discover(from: &Path) -> Option<(PathBuf, Self)> {
        let mut current = from.to_path_buf();
        loop {
            let candidate = current.join(".aida-workspace");
            if candidate.exists() {
                if let Ok(content) = std::fs::read_to_string(&candidate) {
                    if let Ok(config) = toml::from_str::<WorkspaceConfig>(&content) {
                        return Some((current, config));
                    }
                }
            }
            if !current.pop() {
                return None;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Deployment Mode
// ---------------------------------------------------------------------------

/// The deployment mode for an AIDA instance.
///
/// This is the top-level configuration that determines how IDs are generated,
/// how storage works, and whether distributed features are enabled.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode")]
pub enum DeploymentMode {
    /// Centralized: single PostgreSQL or SQLite database, simple sequential IDs.
    /// This is the default for teams with always-available connectivity.
    /// IDs: `FR-001`, `FEAT-042`
    #[serde(rename = "centralized")]
    Centralized,

    /// Distributed: git-based event log, node-namespaced IDs, offline-capable.
    /// IDs: `FR-7-001`, `FEAT-3-042` (with optional agreed IDs on trunk)
    #[serde(rename = "distributed")]
    Distributed {
        /// Path to the shared aida git repo (absolute or relative to workspace)
        #[serde(default)]
        aida_repo_path: Option<String>,
    },
}

impl Default for DeploymentMode {
    fn default() -> Self {
        Self::Centralized
    }
}

impl DeploymentMode {
    /// Whether this mode uses node-namespaced IDs.
    pub fn is_distributed(&self) -> bool {
        matches!(self, Self::Distributed { .. })
    }

    /// Get the IdMode for the dispenser.
    /// In centralized mode, returns `IdMode::Centralized`.
    /// In distributed mode, requires a node_id (from NodeConfig).
    pub fn id_mode(&self, node_id: Option<u32>) -> IdMode {
        match self {
            Self::Centralized => IdMode::Centralized,
            Self::Distributed { .. } => IdMode::Distributed {
                node_id: node_id.expect("distributed mode requires a node_id"),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_registry_sequential_ids() {
        let mut registry = NodeRegistry::default();
        let id1 = registry.register(1, "laptop".into());
        let id2 = registry.register(1, "workstation".into());
        let id3 = registry.register(2, "alice-dev".into());
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
        assert!(registry.is_registered(1));
        assert!(!registry.is_registered(99));
    }

    #[test]
    fn test_user_registry() {
        let mut registry = UserRegistry::default();
        let id1 = registry.register("Joe".into(), Some("joe@example.com".into()));
        let id2 = registry.register("Alice".into(), None);
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert!(registry.find_by_name("joe").is_some());
        assert!(registry.find_by_name("JOE").is_some()); // case-insensitive
        assert!(registry.find_by_name("bob").is_none());
    }

    #[test]
    fn test_agreed_counters() {
        let mut counters = AgreedCounters::default();
        assert_eq!(counters.peek("FR"), 1);
        assert_eq!(counters.next("FR"), 1);
        assert_eq!(counters.next("FR"), 2);
        assert_eq!(counters.next("FEAT"), 1);
        assert_eq!(counters.peek("FR"), 3);

        assert_eq!(
            AgreedCounters::format_agreed_id("FR", 423),
            "FR-423"
        );
    }

    #[test]
    fn test_deployment_mode_centralized() {
        let mode = DeploymentMode::Centralized;
        assert!(!mode.is_distributed());
        assert_eq!(mode.id_mode(None), IdMode::Centralized);
    }

    #[test]
    fn test_deployment_mode_distributed() {
        let mode = DeploymentMode::Distributed {
            aida_repo_path: Some("./aida".into()),
        };
        assert!(mode.is_distributed());
        assert_eq!(
            mode.id_mode(Some(7)),
            IdMode::Distributed { node_id: 7 }
        );
    }

    #[test]
    fn test_deployment_mode_serde_roundtrip() {
        let centralized = DeploymentMode::Centralized;
        let json = serde_json::to_string(&centralized).unwrap();
        assert_eq!(json, r#"{"mode":"centralized"}"#);

        let distributed = DeploymentMode::Distributed {
            aida_repo_path: Some("./aida".into()),
        };
        let json = serde_json::to_string(&distributed).unwrap();
        let back: DeploymentMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, distributed);
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_node_config_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.toml");

        let config = NodeConfig {
            node_id: 7,
            user_id: 102,
            hostname: "joe-laptop".into(),
            registered_at: Utc::now(),
        };
        config.save(&path).unwrap();

        let loaded = NodeConfig::load(&path).unwrap();
        assert_eq!(loaded.node_id, 7);
        assert_eq!(loaded.user_id, 102);
        assert_eq!(loaded.hostname, "joe-laptop");
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_node_registry_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nodes.toml");

        let mut registry = NodeRegistry::default();
        registry.register(102, "joe-laptop".into());
        registry.register(102, "joe-workstation".into());
        registry.save(&path).unwrap();

        let loaded = NodeRegistry::load(&path).unwrap();
        assert_eq!(loaded.nodes.len(), 2);
        assert_eq!(loaded.nodes[0].hostname, "joe-laptop");
        assert_eq!(loaded.nodes[1].hostname, "joe-workstation");
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_workspace_config_serde() {
        let config = WorkspaceConfig {
            workspace: "gdms-disruptive".into(),
            aida_path: "./aida".into(),
            repos: vec!["pacgate".into(), "pacinet".into()],
        };
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let back: WorkspaceConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(back.workspace, "gdms-disruptive");
        assert_eq!(back.repos.len(), 2);
    }
}
