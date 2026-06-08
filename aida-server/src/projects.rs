// trace:FR-0227 | ai:claude:high
//! Multi-project management for AIDA server
//!
//! Manages multiple isolated SQLite databases, one per project.
//! Projects are stored in a data directory with a registry file.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};

use aida_core::db::create_backend;

use crate::service::ServerState;

/// Information about a project stored in the registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    pub description: String,
    pub created_at: DateTime<Utc>,
    #[serde(skip)]
    pub db_path: PathBuf,
}

/// Registry file format
#[derive(Debug, Default, Serialize, Deserialize)]
struct ProjectRegistry {
    projects: Vec<ProjectInfo>,
}

/// Manages multiple project databases
pub struct ProjectManager {
    data_dir: PathBuf,
    /// Project metadata from registry file
    projects: RwLock<HashMap<String, ProjectInfo>>,
    /// Lazily-loaded database backends per project
    backends: RwLock<HashMap<String, Arc<ServerState>>>,
}

impl ProjectManager {
    /// Create a new ProjectManager for the given data directory
    pub fn new(data_dir: PathBuf) -> Result<Self> {
        // Ensure data directory exists
        std::fs::create_dir_all(&data_dir)?;

        // Load existing registry synchronously during construction
        let registry_path = data_dir.join("projects.json");
        let projects = if registry_path.exists() {
            let content = std::fs::read_to_string(&registry_path)?;
            let registry: ProjectRegistry = serde_json::from_str(&content)?;

            let mut projects = HashMap::new();
            for mut project in registry.projects {
                project.db_path = data_dir.join(format!("{}.db", project.name));
                projects.insert(project.name.clone(), project);
            }
            projects
        } else {
            HashMap::new()
        };

        let manager = Self {
            data_dir: data_dir.clone(),
            projects: RwLock::new(projects),
            backends: RwLock::new(HashMap::new()),
        };

        info!("ProjectManager initialized with data_dir: {:?}", data_dir);
        Ok(manager)
    }

    /// Get the data directory path
    pub fn data_dir(&self) -> &PathBuf {
        &self.data_dir
    }

    /// List all projects
    pub async fn list_projects(&self) -> Vec<ProjectInfo> {
        let projects = self.projects.read().await;
        projects.values().cloned().collect()
    }

    /// Get a specific project's info
    pub async fn get_project(&self, name: &str) -> Option<ProjectInfo> {
        let projects = self.projects.read().await;
        projects.get(name).cloned()
    }

    /// Create a new project
    pub async fn create_project(&self, name: &str, description: &str) -> Result<ProjectInfo> {
        // Validate project name (alphanumeric, hyphens, underscores only)
        if !Self::is_valid_project_name(name) {
            return Err(anyhow!(
                "Invalid project name. Use only letters, numbers, hyphens, and underscores."
            ));
        }

        let mut projects = self.projects.write().await;

        // Check if project already exists
        if projects.contains_key(name) {
            return Err(anyhow!("Project '{}' already exists", name));
        }

        let db_path = self.data_dir.join(format!("{}.db", name));

        // Create the database file by initializing a backend
        let _state = tokio::task::spawn_blocking({
            let db_path = db_path.clone();
            move || {
                let backend = create_backend(&db_path, None)?;
                ServerState::new(backend)
            }
        })
        .await??;

        let project = ProjectInfo {
            name: name.to_string(),
            description: description.to_string(),
            created_at: Utc::now(),
            db_path,
        };

        projects.insert(name.to_string(), project.clone());

        // Save registry
        drop(projects);
        self.save_registry().await?;

        info!("Created new project: {}", name);
        Ok(project)
    }

    /// Delete a project and its database
    pub async fn delete_project(&self, name: &str) -> Result<()> {
        let mut projects = self.projects.write().await;
        let mut backends = self.backends.write().await;

        let project = projects
            .remove(name)
            .ok_or_else(|| anyhow!("Project '{}' not found", name))?;

        // Remove cached backend
        backends.remove(name);

        // Delete the database file
        if project.db_path.exists() {
            std::fs::remove_file(&project.db_path)?;
            info!("Deleted database file: {:?}", project.db_path);
        }

        // Save registry
        drop(projects);
        drop(backends);
        self.save_registry().await?;

        info!("Deleted project: {}", name);
        Ok(())
    }

    /// Get or create the ServerState for a project
    pub async fn get_backend(&self, project: &str) -> Result<Arc<ServerState>> {
        // First check if we have a cached backend
        {
            let backends = self.backends.read().await;
            if let Some(state) = backends.get(project) {
                return Ok(Arc::clone(state));
            }
        }

        // Check if project exists
        let project_info = {
            let projects = self.projects.read().await;
            projects.get(project).cloned()
        };

        let project_info =
            project_info.ok_or_else(|| anyhow!("Project '{}' not found", project))?;

        // Create backend (in blocking context for PostgreSQL compatibility)
        let db_path = project_info.db_path.clone();
        let state = tokio::task::spawn_blocking(move || {
            let backend = create_backend(&db_path, None)?;
            Ok::<_, anyhow::Error>(Arc::new(ServerState::new(backend)?))
        })
        .await??;

        // Cache the backend
        {
            let mut backends = self.backends.write().await;
            backends.insert(project.to_string(), Arc::clone(&state));
        }

        info!("Loaded backend for project: {}", project);
        Ok(state)
    }

    /// Validate project name
    fn is_valid_project_name(name: &str) -> bool {
        if name.is_empty() || name.len() > 64 {
            return false;
        }

        // Must start with a letter
        if !name
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic())
            .unwrap_or(false)
        {
            return false;
        }

        // Only alphanumeric, hyphens, underscores
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    }

    /// Save the project registry to disk
    async fn save_registry(&self) -> Result<()> {
        let projects = self.projects.read().await;
        let registry = ProjectRegistry {
            projects: projects.values().cloned().collect(),
        };

        let registry_path = self.data_dir.join("projects.json");
        let content = serde_json::to_string_pretty(&registry)?;
        std::fs::write(&registry_path, content)?;

        info!("Saved project registry to {:?}", registry_path);
        Ok(())
    }

    /// Migrate an existing single-database setup to multi-project
    ///
    /// If a requirements.db exists in the data directory without a registry,
    /// create a "default" project for it.
    pub async fn migrate_legacy_database(&self) -> Result<bool> {
        let legacy_db = self.data_dir.join("requirements.db");
        let registry_path = self.data_dir.join("projects.json");

        // Only migrate if legacy DB exists but no registry
        if legacy_db.exists() && !registry_path.exists() {
            info!("Found legacy database, migrating to multi-project setup");

            // Rename to default.db
            let new_path = self.data_dir.join("default.db");
            std::fs::rename(&legacy_db, &new_path)?;

            // Create project entry
            let project = ProjectInfo {
                name: "default".to_string(),
                description: "Default project (migrated from legacy database)".to_string(),
                created_at: Utc::now(),
                db_path: new_path,
            };

            let mut projects = self.projects.write().await;
            projects.insert("default".to_string(), project);
            drop(projects);

            self.save_registry().await?;

            warn!("Migrated legacy requirements.db to 'default' project");
            return Ok(true);
        }

        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_valid_project_names() {
        assert!(ProjectManager::is_valid_project_name("myproject"));
        assert!(ProjectManager::is_valid_project_name("my-project"));
        assert!(ProjectManager::is_valid_project_name("my_project"));
        assert!(ProjectManager::is_valid_project_name("MyProject123"));
        assert!(ProjectManager::is_valid_project_name("a"));

        assert!(!ProjectManager::is_valid_project_name(""));
        assert!(!ProjectManager::is_valid_project_name("123project")); // starts with number
        assert!(!ProjectManager::is_valid_project_name("-project")); // starts with hyphen
        assert!(!ProjectManager::is_valid_project_name("my project")); // has space
        assert!(!ProjectManager::is_valid_project_name("my.project")); // has dot
    }

    #[tokio::test]
    async fn test_create_and_list_projects() {
        let temp_dir = tempdir().unwrap();
        let manager = ProjectManager::new(temp_dir.path().to_path_buf()).unwrap();

        // Initially empty
        let projects = manager.list_projects().await;
        assert!(projects.is_empty());

        // Create a project
        let project = manager
            .create_project("test-project", "A test project")
            .await
            .unwrap();
        assert_eq!(project.name, "test-project");
        assert_eq!(project.description, "A test project");

        // Should be in list now
        let projects = manager.list_projects().await;
        assert_eq!(projects.len(), 1);

        // Can't create duplicate
        let result = manager.create_project("test-project", "Duplicate").await;
        assert!(result.is_err());
    }
}
