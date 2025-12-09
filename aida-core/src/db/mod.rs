//! Database abstraction layer for AIDA requirements management
//!
//! This module provides a trait-based abstraction for storage backends,
//! allowing the system to use different databases (YAML files, SQLite, etc.)
//! while maintaining a consistent interface.

mod traits;
mod yaml_backend;
mod sqlite_backend;
mod migration;

pub use traits::{DatabaseBackend, BackendType, DatabaseConfig, UpdateResult, VersionConflict};
pub use yaml_backend::YamlBackend;
pub use sqlite_backend::SqliteBackend;
pub use migration::{migrate_yaml_to_sqlite, migrate_sqlite_to_yaml, export_to_json, import_from_json};

use anyhow::Result;
use std::path::Path;

/// Creates a database backend based on the file extension or explicit type
pub fn create_backend(path: &Path, backend_type: Option<BackendType>) -> Result<Box<dyn DatabaseBackend>> {
    let bt = backend_type.unwrap_or_else(|| {
        // Infer from file extension
        match path.extension().and_then(|e| e.to_str()) {
            Some("yaml") | Some("yml") => BackendType::Yaml,
            Some("db") | Some("sqlite") | Some("sqlite3") => BackendType::Sqlite,
            _ => BackendType::Yaml, // Default to YAML
        }
    });

    match bt {
        BackendType::Yaml => Ok(Box::new(YamlBackend::new(path))),
        BackendType::Sqlite => Ok(Box::new(SqliteBackend::new(path)?)),
    }
}

/// Opens an existing database or creates a new one
pub fn open_or_create(path: &Path, backend_type: Option<BackendType>) -> Result<Box<dyn DatabaseBackend>> {
    create_backend(path, backend_type)
}
