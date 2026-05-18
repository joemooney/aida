use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Represents a project in the registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    /// Path to the requirements file
    pub path: String,
    /// Description of the project
    pub description: String,
}

/// Registry of all projects
#[derive(Debug, Serialize, Deserialize)]
pub struct Registry {
    pub projects: HashMap<String, Project>,
    /// Optional default project name
    pub default_project: Option<String>,
}

impl Registry {
    /// Loads the registry from the provided path
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read registry file: {:?}", path.as_ref()))?;

        serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse registry file: {:?}", path.as_ref()))
    }

    /// Gets a project by name
    pub fn get_project(&self, name: &str) -> Option<&Project> {
        self.projects.get(name)
    }

    /// Lists all project names
    pub fn list_projects(&self) -> Vec<&str> {
        self.projects.keys().map(|k| k.as_str()).collect()
    }

    /// Registers a new project or updates an existing one
    pub fn register_project(&mut self, name: String, path: String, description: String) {
        let project = Project { path, description };

        self.projects.insert(name, project);
    }

    /// Sets a project as the default
    pub fn set_default_project(&mut self, name: &str) -> Result<()> {
        if !self.projects.contains_key(name) {
            anyhow::bail!("Project '{}' not found in registry", name);
        }

        // Update the default project name
        self.default_project = Some(name.to_string());

        Ok(())
    }

    /// Clears the default project setting
    pub fn clear_default_project(&mut self) {
        self.default_project = None;
    }

    /// Gets the default project if set
    pub fn get_default_project(&self) -> Option<(&str, &Project)> {
        if let Some(default_name) = &self.default_project {
            if let Some(project) = self.projects.get(default_name) {
                return Some((default_name, project));
            }
        }
        None
    }

    /// Save the registry to the specified path
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let content = serde_yaml::to_string(&self)?;

        // Ensure parent directories exist
        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent)?;
        }

        // Atomic write: multi-node registry updates can race. trace:TASK-331 | ai:claude
        crate::write_atomic(path.as_ref(), content)
            .with_context(|| format!("Failed to write registry to {:?}", path.as_ref()))?;

        Ok(())
    }

    /// Creates a default registry file if it doesn't exist
    pub fn create_default<P: AsRef<Path>>(path: P) -> Result<()> {
        if path.as_ref().exists() {
            return Ok(());
        }

        let mut projects = HashMap::new();
        projects.insert(
            "default".to_string(),
            Project {
                path: "requirements.yaml".to_string(),
                description: "Default project".to_string(),
            },
        );

        let registry = Registry {
            projects,
            default_project: None,
        };
        let content = serde_yaml::to_string(&registry)?;

        // Ensure parent directories exist
        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent)?;
        }

        // Atomic write: multi-node registry updates can race. trace:TASK-331 | ai:claude
        crate::write_atomic(path.as_ref(), content)
            .with_context(|| format!("Failed to write default registry to {:?}", path.as_ref()))?;

        Ok(())
    }
}

/// Gets the path to the registry file
pub fn get_registry_path() -> Result<PathBuf> {
    // Check if AIDA_REGISTRY_PATH environment variable is set (also support legacy REQ_REGISTRY_PATH)
    if let Ok(path) = std::env::var("AIDA_REGISTRY_PATH") {
        return Ok(PathBuf::from(path));
    }
    if let Ok(path) = std::env::var("REQ_REGISTRY_PATH") {
        return Ok(PathBuf::from(path));
    }

    let home_dir = dirs::home_dir().context("Failed to determine home directory")?;

    // Check for new config file first, fall back to legacy if it exists
    let new_config = home_dir.join(".aida.config");
    let legacy_config = home_dir.join(".requirements.config");

    if new_config.exists() {
        Ok(new_config)
    } else if legacy_config.exists() {
        // Use legacy config if it exists and new one doesn't
        Ok(legacy_config)
    } else {
        // Default to new config location for new installations
        Ok(new_config)
    }
}

/// Gets the path to the AIDA config directory (~/.config/aida/)
pub fn get_config_dir() -> Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .context("Failed to determine config directory")?
        .join("aida");
    Ok(config_dir)
}

/// Gets the path to the templates directory (~/.config/aida/templates/)
pub fn get_templates_dir() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("templates"))
}
