//! Database abstraction layer for AIDA requirements management
//!
//! This module provides a trait-based abstraction for storage backends,
//! allowing the system to use different databases (YAML files, SQLite, PostgreSQL)
//! while maintaining a consistent interface.

#[cfg(feature = "native")]
mod cache;
#[cfg(feature = "native")]
mod cached_git_backend;
#[cfg(feature = "native")]
mod git_backend;
#[cfg(feature = "native")]
mod migration;
#[cfg(feature = "postgres")]
mod postgres_backend;
#[cfg(feature = "native")]
mod sqlite_backend;
mod traits;
#[cfg(feature = "native")]
mod yaml_backend;

#[cfg(feature = "native")]
pub use cache::{
    cache_lock_info_path, compute_blocked, compute_degrees, edge_weight, fast_fail_cache_enabled,
    read_cache_lock_info, set_fast_fail_cache, ArchiveFilter, Cache, CacheLockInfo, DeferFilter,
    Degrees, ListFilter, RequirementSummary, SortOrder,
};
#[cfg(feature = "native")]
pub use cached_git_backend::CachedGitBackend;
#[cfg(feature = "native")]
pub use git_backend::GitBackend;
#[cfg(feature = "native")]
pub use migration::{
    export_to_json, import_from_json, migrate_sqlite_to_yaml, migrate_yaml_to_sqlite,
};
#[cfg(all(feature = "native", feature = "postgres"))]
pub use migration::{migrate_from_postgres, migrate_to_postgres};
#[cfg(feature = "postgres")]
pub use postgres_backend::PostgresBackend;
#[cfg(feature = "native")]
pub use sqlite_backend::SqliteBackend;
pub use traits::{BackendType, DatabaseBackend, DatabaseConfig, UpdateResult, VersionConflict};
#[cfg(feature = "native")]
pub use yaml_backend::YamlBackend;

#[cfg(feature = "native")]
use anyhow::Result;
#[cfg(feature = "native")]
use std::path::Path;

/// Creates a database backend based on the file extension or explicit type
///
/// For PostgreSQL, pass a connection string as the path:
/// `postgres://user:password@host:port/database`
#[cfg(feature = "native")]
pub fn create_backend(
    path: &Path,
    backend_type: Option<BackendType>,
) -> Result<Box<dyn DatabaseBackend>> {
    let path_str = path.to_string_lossy();

    let bt = backend_type.unwrap_or_else(|| {
        // Check for PostgreSQL connection string first
        if path_str.starts_with("postgres://") || path_str.starts_with("postgresql://") {
            return BackendType::Postgres;
        }
        // Check if path is a directory — use git backend
        // This covers both existing stores (with metadata.yaml) and fresh directories
        if path.is_dir() || (path.extension().is_none() && !path.exists()) {
            return BackendType::Git;
        }
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
        BackendType::Git => {
            // Git paths get the cache wrapper automatically (EPIC-1-001).
            // Cache lives at the project's .aida/cache.db; if probe fails to
            // locate one, falls back to a sibling file next to the store.
            let cache_path = CachedGitBackend::default_cache_path(path);
            Ok(Box::new(CachedGitBackend::open(path, &cache_path)?))
        }
        #[cfg(feature = "postgres")]
        BackendType::Postgres => Ok(Box::new(PostgresBackend::new(&path_str)?)),
        #[cfg(not(feature = "postgres"))]
        BackendType::Postgres => {
            anyhow::bail!("PostgreSQL support not compiled in. Enable the 'postgres' feature.")
        }
    }
}

/// Opens an existing database or creates a new one
#[cfg(feature = "native")]
pub fn open_or_create(
    path: &Path,
    backend_type: Option<BackendType>,
) -> Result<Box<dyn DatabaseBackend>> {
    create_backend(path, backend_type)
}
