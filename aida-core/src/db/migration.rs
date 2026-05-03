//! Migration utilities for converting between storage backends
//!
//! This module provides functions to migrate data between YAML and SQLite backends,
//! as well as import/export to JSON format for interoperability.

use anyhow::{Context, Result};
use std::path::Path;

use super::traits::DatabaseBackend;
use super::{SqliteBackend, YamlBackend};
use crate::models::RequirementsStore;

/// Migrates data from a YAML file to a SQLite database
///
/// After successful migration, the YAML file is updated with a `migrated_to` marker
/// pointing to the SQLite database. This prevents accidentally opening stale YAML data.
///
/// # Arguments
/// * `yaml_path` - Path to the source YAML file
/// * `sqlite_path` - Path to the destination SQLite database
///
/// # Returns
/// The number of requirements migrated
pub fn migrate_yaml_to_sqlite<P1: AsRef<Path>, P2: AsRef<Path>>(
    yaml_path: P1,
    sqlite_path: P2,
) -> Result<usize> {
    let yaml_path = yaml_path.as_ref();
    let sqlite_path = sqlite_path.as_ref();

    let yaml_backend = YamlBackend::new(yaml_path);
    let sqlite_backend = SqliteBackend::new(sqlite_path)?;

    // Load from YAML
    let store = yaml_backend
        .load()
        .context("Failed to load YAML database")?;

    let req_count = store.requirements.len();

    // Save to SQLite
    sqlite_backend
        .save(&store)
        .context("Failed to save to SQLite database")?;

    // Update YAML with migration marker
    // This prevents accidentally opening the stale YAML after migration
    let mut marked_store = store;
    marked_store.migrated_to = Some(sqlite_path.display().to_string());
    yaml_backend
        .save(&marked_store)
        .context("Failed to update YAML with migration marker")?;

    Ok(req_count)
}

/// Migrates data from a SQLite database to a YAML file
///
/// # Arguments
/// * `sqlite_path` - Path to the source SQLite database
/// * `yaml_path` - Path to the destination YAML file
///
/// # Returns
/// The number of requirements migrated
pub fn migrate_sqlite_to_yaml<P1: AsRef<Path>, P2: AsRef<Path>>(
    sqlite_path: P1,
    yaml_path: P2,
) -> Result<usize> {
    let sqlite_backend = SqliteBackend::new(sqlite_path)?;
    let yaml_backend = YamlBackend::new(yaml_path);

    // Load from SQLite
    let store = sqlite_backend
        .load()
        .context("Failed to load SQLite database")?;

    let req_count = store.requirements.len();

    // Save to YAML
    yaml_backend
        .save(&store)
        .context("Failed to save to YAML file")?;

    Ok(req_count)
}

/// Exports a RequirementsStore to a JSON file
///
/// JSON format is useful for:
/// - Interoperability with other systems
/// - API responses
/// - Backup/restore
///
/// # Arguments
/// * `store` - The requirements store to export
/// * `json_path` - Path to the destination JSON file
pub fn export_to_json<P: AsRef<Path>>(store: &RequirementsStore, json_path: P) -> Result<()> {
    let json = serde_json::to_string_pretty(store).context("Failed to serialize to JSON")?;

    std::fs::write(json_path, json).context("Failed to write JSON file")?;

    Ok(())
}

/// Imports a RequirementsStore from a JSON file
///
/// # Arguments
/// * `json_path` - Path to the source JSON file
///
/// # Returns
/// The imported RequirementsStore
pub fn import_from_json<P: AsRef<Path>>(json_path: P) -> Result<RequirementsStore> {
    let json = std::fs::read_to_string(json_path).context("Failed to read JSON file")?;

    let store: RequirementsStore = serde_json::from_str(&json).context("Failed to parse JSON")?;

    Ok(store)
}

/// Migrates data from any backend to PostgreSQL
///
/// # Arguments
/// * `source` - The source backend (YAML or SQLite)
/// * `connection_string` - PostgreSQL connection string
///
/// # Returns
/// The number of requirements migrated
#[cfg(feature = "postgres")]
pub fn migrate_to_postgres(source: &dyn DatabaseBackend, connection_string: &str) -> Result<usize> {
    use super::PostgresBackend;

    let postgres_backend = PostgresBackend::new(connection_string)?;

    // Load from source
    let store = source.load().context("Failed to load source database")?;

    let req_count = store.requirements.len();

    // Save to PostgreSQL
    postgres_backend
        .save(&store)
        .context("Failed to save to PostgreSQL database")?;

    Ok(req_count)
}

/// Migrates data from PostgreSQL to another backend
///
/// # Arguments
/// * `connection_string` - PostgreSQL connection string
/// * `target` - The target backend (YAML or SQLite)
///
/// # Returns
/// The number of requirements migrated
#[cfg(feature = "postgres")]
pub fn migrate_from_postgres(
    connection_string: &str,
    target: &dyn DatabaseBackend,
) -> Result<usize> {
    use super::PostgresBackend;

    let postgres_backend = PostgresBackend::new(connection_string)?;

    // Load from PostgreSQL
    let store = postgres_backend
        .load()
        .context("Failed to load PostgreSQL database")?;

    let req_count = store.requirements.len();

    // Save to target
    target
        .save(&store)
        .context("Failed to save to target database")?;

    Ok(req_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::{NamedTempFile, TempDir};

    #[test]
    fn test_yaml_to_sqlite_migration() {
        let yaml_file = NamedTempFile::with_suffix(".yaml").unwrap();
        let sqlite_file = NamedTempFile::with_suffix(".db").unwrap();

        // Create a YAML file with some data
        let yaml_backend = YamlBackend::new(yaml_file.path());
        let mut store = RequirementsStore::new();
        store.name = "Migration Test".to_string();
        store.title = "Test Migration".to_string();
        yaml_backend.save(&store).unwrap();

        // Migrate to SQLite
        let count = migrate_yaml_to_sqlite(yaml_file.path(), sqlite_file.path()).unwrap();
        assert_eq!(count, 0); // No requirements

        // Verify SQLite has the data
        let sqlite_backend = SqliteBackend::new(sqlite_file.path()).unwrap();
        let loaded = sqlite_backend.load().unwrap();
        assert_eq!(loaded.name, "Migration Test");
        assert_eq!(loaded.title, "Test Migration");
    }

    #[test]
    fn test_sqlite_to_yaml_migration() {
        let sqlite_file = NamedTempFile::with_suffix(".db").unwrap();
        let yaml_file = NamedTempFile::with_suffix(".yaml").unwrap();

        // Create a SQLite database with some data
        let sqlite_backend = SqliteBackend::new(sqlite_file.path()).unwrap();
        let mut store = RequirementsStore::new();
        store.name = "SQLite Test".to_string();
        store.title = "Test SQLite".to_string();
        sqlite_backend.save(&store).unwrap();

        // Migrate to YAML
        let count = migrate_sqlite_to_yaml(sqlite_file.path(), yaml_file.path()).unwrap();
        assert_eq!(count, 0);

        // Verify YAML has the data
        let yaml_backend = YamlBackend::new(yaml_file.path());
        let loaded = yaml_backend.load().unwrap();
        assert_eq!(loaded.name, "SQLite Test");
        assert_eq!(loaded.title, "Test SQLite");
    }

    #[test]
    fn test_json_export_import() {
        let temp_dir = TempDir::new().unwrap();
        let json_path = temp_dir.path().join("export.json");

        let mut store = RequirementsStore::new();
        store.name = "JSON Test".to_string();
        store.title = "Test JSON Export".to_string();

        // Export
        export_to_json(&store, &json_path).unwrap();

        // Import
        let loaded = import_from_json(&json_path).unwrap();
        assert_eq!(loaded.name, "JSON Test");
        assert_eq!(loaded.title, "Test JSON Export");
    }
}
