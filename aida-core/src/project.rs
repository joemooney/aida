use anyhow::Result;
use std::env;
use std::path::PathBuf;

use crate::registry::{get_registry_path, Registry};

/// Result of checking for migration status
#[derive(Debug)]
pub enum MigrationCheck {
    /// No migration detected, use this path
    NoMigration(PathBuf),
    /// YAML was migrated to SQLite, use the SQLite path instead
    MigratedToSqlite {
        yaml_path: PathBuf,
        sqlite_path: PathBuf,
    },
    /// SQLite exists alongside YAML but no marker - potential stale data
    PossibleStaleYaml {
        yaml_path: PathBuf,
        sqlite_path: PathBuf,
    },
}

/// Check if a YAML file has been migrated to SQLite
pub fn check_migration_status(yaml_path: &PathBuf) -> MigrationCheck {
    // Check if corresponding SQLite file exists
    let sqlite_path = yaml_path.with_extension("db");

    if yaml_path.exists() && sqlite_path.exists() {
        // Both files exist - check for migration marker at the START of YAML file
        // The marker should be at the beginning, not embedded in nested content
        if let Ok(content) = std::fs::read_to_string(yaml_path) {
            // Check first few lines for the migration marker (comment + field)
            let first_lines: String = content.lines().take(5).collect::<Vec<_>>().join("\n");
            if first_lines.contains("migrated_to:") {
                return MigrationCheck::MigratedToSqlite {
                    yaml_path: yaml_path.clone(),
                    sqlite_path,
                };
            }
        }
        // Both exist but no marker at start - potential stale data situation
        return MigrationCheck::PossibleStaleYaml {
            yaml_path: yaml_path.clone(),
            sqlite_path,
        };
    }

    // Only SQLite exists - use it
    if sqlite_path.exists() && !yaml_path.exists() {
        return MigrationCheck::NoMigration(sqlite_path);
    }

    // Default: use the YAML path (or it doesn't exist yet)
    MigrationCheck::NoMigration(yaml_path.clone())
}

/// Determines the requirements file path to use based on the available information.
/// This version does NOT check for migration - use `determine_requirements_path_with_migration_check`
/// for migration-aware path resolution.
pub fn determine_requirements_path(project_option: Option<&str>) -> Result<PathBuf> {
    // Check if requirements.yaml exists in current directory - but only if we're not explicitly
    // specifying a project via command line option or environment variable
    let use_local_file = project_option.is_none() && env::var("REQ_DB_NAME").is_err();
    let current_dir_path = PathBuf::from("requirements.yaml");

    if use_local_file && current_dir_path.exists() {
        return Ok(current_dir_path);
    }

    // Get the registry path and ensure it exists
    let registry_path = get_registry_path()?;
    if !registry_path.exists() {
        Registry::create_default(&registry_path)?;
    }

    // Load the registry
    let registry = Registry::load(&registry_path)?;

    // Priority 1: Use the command line project option if provided
    if let Some(project_name) = project_option {
        if let Some(project) = registry.get_project(project_name) {
            return Ok(PathBuf::from(&project.path));
        } else {
            anyhow::bail!("Project '{}' not found in registry", project_name);
        }
    }

    // Priority 2: Use the REQ_DB_NAME environment variable if set
    if let Ok(env_project) = env::var("REQ_DB_NAME") {
        if let Some(project) = registry.get_project(&env_project) {
            return Ok(PathBuf::from(&project.path));
        } else {
            anyhow::bail!(
                "Project '{}' from REQ_DB_NAME not found in registry",
                env_project
            );
        }
    }

    // Priority 3: Check if there's only one project in the registry
    if registry.projects.len() == 1 {
        let (_, project) = registry.projects.iter().next().unwrap();
        return Ok(PathBuf::from(&project.path));
    }

    // Priority 4: Use the default project if configured in registry
    if let Some((_, default_project)) = registry.get_default_project() {
        return Ok(PathBuf::from(&default_project.path));
    }

    // If we got here, there's no clear project to use
    anyhow::bail!(
        "Could not determine requirements file. Please specify a project with -p, \
         set REQ_DB_NAME environment variable, or ensure requirements.yaml exists in current directory"
    )
}

/// Lists available projects from the registry
pub fn list_available_projects() -> Result<Vec<(String, String)>> {
    let registry_path = get_registry_path()?;
    if !registry_path.exists() {
        Registry::create_default(&registry_path)?;
    }

    let registry = Registry::load(&registry_path)?;
    let mut projects = Vec::new();

    for (name, project) in &registry.projects {
        projects.push((name.clone(), project.description.clone()));
    }

    Ok(projects)
}
