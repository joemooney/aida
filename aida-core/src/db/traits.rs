//! Database abstraction traits
//!
//! This module defines the core trait that all storage backends must implement.

use anyhow::Result;
use std::path::PathBuf;
use uuid::Uuid;

use crate::models::{QueueEntry, Requirement, RequirementsStore, User};

/// Error type for version conflicts during optimistic locking
#[derive(Debug, Clone)]
pub struct VersionConflict {
    /// ID of the conflicting record
    pub id: Uuid,
    /// The version the client expected
    pub expected_version: i64,
    /// The current version in the database
    pub current_version: i64,
    /// Human-readable identifier (spec_id or name)
    pub display_id: String,
}

impl std::fmt::Display for VersionConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Version conflict for {}: expected version {}, but current version is {}. \
             Another user may have modified this record.",
            self.display_id, self.expected_version, self.current_version
        )
    }
}

impl std::error::Error for VersionConflict {}

/// Result type for optimistic locking operations
#[derive(Debug)]
pub enum UpdateResult {
    /// Update succeeded
    Success,
    /// Update failed due to version conflict
    Conflict(VersionConflict),
}

/// Types of database backends available
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendType {
    /// YAML file storage (single file)
    Yaml,
    /// SQLite database storage
    Sqlite,
    /// PostgreSQL database storage
    Postgres,
    /// Git-backed storage (sharded YAML files in a directory)
    Git,
}

impl std::fmt::Display for BackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendType::Yaml => write!(f, "YAML"),
            BackendType::Sqlite => write!(f, "SQLite"),
            BackendType::Postgres => write!(f, "PostgreSQL"),
            BackendType::Git => write!(f, "Git"),
        }
    }
}

/// Configuration for database backends
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    /// Path to the database file
    pub path: PathBuf,
    /// Backend type
    pub backend_type: BackendType,
    /// Whether to enable write-ahead logging (SQLite only)
    pub wal_mode: bool,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("requirements.yaml"),
            backend_type: BackendType::Yaml,
            wal_mode: true,
        }
    }
}

/// Core trait for database backends
///
/// This trait provides a unified interface for storing and retrieving
/// requirements data, regardless of the underlying storage mechanism.
///
/// The design philosophy is:
/// - `load()` and `save()` work with the full `RequirementsStore` for compatibility
/// - Individual CRUD operations are provided for more efficient database access
/// - Backends can choose to implement efficient versions or delegate to load/save
pub trait DatabaseBackend: Send + Sync {
    /// Returns the backend type
    fn backend_type(&self) -> BackendType;

    /// Returns the path to the database file
    fn path(&self) -> &std::path::Path;

    // =========================================================================
    // Full Store Operations (for compatibility with existing code)
    // =========================================================================

    /// Loads the entire requirements store from the database
    fn load(&self) -> Result<RequirementsStore>;

    /// Saves the entire requirements store to the database
    fn save(&self, store: &RequirementsStore) -> Result<()>;

    /// Performs an atomic update operation
    /// Default implementation loads, applies changes, and saves
    fn update_atomically<F>(&self, update_fn: F) -> Result<RequirementsStore>
    where
        F: FnOnce(&mut RequirementsStore),
        Self: Sized,
    {
        let mut store = self.load()?;
        update_fn(&mut store);
        self.save(&store)?;
        Ok(store)
    }

    // =========================================================================
    // Requirement CRUD Operations
    // =========================================================================

    /// Gets a requirement by its UUID
    fn get_requirement(&self, id: &Uuid) -> Result<Option<Requirement>> {
        let store = self.load()?;
        Ok(store.requirements.iter().find(|r| &r.id == id).cloned())
    }

    /// Gets a requirement by its spec_id (e.g., "FR-001"). Match is
    /// case-insensitive and also tries `agreed_id`, so callers may pass
    /// user input verbatim.
    fn get_requirement_by_spec_id(&self, spec_id: &str) -> Result<Option<Requirement>> {
        let store = self.load()?;
        Ok(store
            .requirements
            .iter()
            .find(|r| {
                r.spec_id
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(spec_id))
                    || r.agreed_id
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(spec_id))
            })
            .cloned())
    }

    /// Lists all requirements (non-archived by default)
    fn list_requirements(&self, include_archived: bool) -> Result<Vec<Requirement>> {
        let store = self.load()?;
        Ok(store
            .requirements
            .iter()
            .filter(|r| include_archived || !r.archived)
            .cloned()
            .collect())
    }

    /// Adds a new requirement
    /// Returns the requirement with assigned spec_id
    /// Note: This uses the simple SPEC-XXX format for ID generation.
    /// For more complex ID generation (with feature/type prefixes), use update_atomically
    fn add_requirement(&self, requirement: Requirement) -> Result<Requirement> {
        let mut store = self.load()?;
        let mut req = requirement;

        // Assign spec_id if not set using the simple format
        if req.spec_id.is_none() {
            req.spec_id = Some(format!("SPEC-{:03}", store.next_spec_number));
            store.next_spec_number += 1;
        }

        store.requirements.push(req.clone());
        self.save(&store)?;
        Ok(req)
    }

    /// Updates an existing requirement
    fn update_requirement(&self, requirement: &Requirement) -> Result<()> {
        let mut store = self.load()?;
        if let Some(pos) = store
            .requirements
            .iter()
            .position(|r| r.id == requirement.id)
        {
            store.requirements[pos] = requirement.clone();
            self.save(&store)?;
            Ok(())
        } else {
            anyhow::bail!("Requirement not found: {}", requirement.id)
        }
    }

    /// Updates a requirement with optimistic locking
    ///
    /// This method checks that the requirement's version matches the database version
    /// before updating. If another process modified the requirement, returns a conflict.
    ///
    /// The requirement's version field should contain the version that was loaded.
    /// On success, the version is incremented in the database.
    fn update_requirement_versioned(&self, requirement: &Requirement) -> Result<UpdateResult> {
        // Default implementation doesn't support versioning, just updates
        self.update_requirement(requirement)?;
        Ok(UpdateResult::Success)
    }

    /// Deletes a requirement by UUID
    fn delete_requirement(&self, id: &Uuid) -> Result<()> {
        let mut store = self.load()?;
        let original_len = store.requirements.len();
        store.requirements.retain(|r| &r.id != id);
        if store.requirements.len() == original_len {
            anyhow::bail!("Requirement not found: {}", id)
        }
        self.save(&store)
    }

    // =========================================================================
    // User CRUD Operations
    // =========================================================================

    /// Gets a user by UUID
    fn get_user(&self, id: &Uuid) -> Result<Option<User>> {
        let store = self.load()?;
        Ok(store.users.iter().find(|u| &u.id == id).cloned())
    }

    /// Gets a user by handle
    fn get_user_by_handle(&self, handle: &str) -> Result<Option<User>> {
        let store = self.load()?;
        Ok(store.users.iter().find(|u| u.handle == handle).cloned())
    }

    /// Lists all users
    fn list_users(&self, include_archived: bool) -> Result<Vec<User>> {
        let store = self.load()?;
        Ok(store
            .users
            .iter()
            .filter(|u| include_archived || !u.archived)
            .cloned()
            .collect())
    }

    /// Adds a new user
    fn add_user(&self, user: User) -> Result<User> {
        let mut store = self.load()?;
        let mut u = user;

        // Assign spec_id if not set
        if u.spec_id.is_none() {
            u.spec_id = Some(store.next_meta_id(crate::models::META_PREFIX_USER));
        }

        store.users.push(u.clone());
        self.save(&store)?;
        Ok(u)
    }

    /// Updates an existing user
    fn update_user(&self, user: &User) -> Result<()> {
        let mut store = self.load()?;
        if let Some(pos) = store.users.iter().position(|u| u.id == user.id) {
            store.users[pos] = user.clone();
            self.save(&store)?;
            Ok(())
        } else {
            anyhow::bail!("User not found: {}", user.id)
        }
    }

    /// Deletes a user by UUID
    fn delete_user(&self, id: &Uuid) -> Result<()> {
        let mut store = self.load()?;
        let original_len = store.users.len();
        store.users.retain(|u| &u.id != id);
        if store.users.len() == original_len {
            anyhow::bail!("User not found: {}", id)
        }
        self.save(&store)
    }

    // =========================================================================
    // Metadata Operations
    // =========================================================================

    /// Gets the database name
    fn get_name(&self) -> Result<String> {
        Ok(self.load()?.name)
    }

    /// Sets the database name
    fn set_name(&self, name: &str) -> Result<()> {
        let mut store = self.load()?;
        store.name = name.to_string();
        self.save(&store)
    }

    /// Gets the database title
    fn get_title(&self) -> Result<String> {
        Ok(self.load()?.title)
    }

    /// Sets the database title
    fn set_title(&self, title: &str) -> Result<()> {
        let mut store = self.load()?;
        store.title = title.to_string();
        self.save(&store)
    }

    /// Gets the database description
    fn get_description(&self) -> Result<String> {
        Ok(self.load()?.description)
    }

    /// Sets the database description
    fn set_description(&self, description: &str) -> Result<()> {
        let mut store = self.load()?;
        store.description = description.to_string();
        self.save(&store)
    }

    // =========================================================================
    // Baseline Operations
    // =========================================================================

    /// Creates a new baseline from current requirements
    /// For YAML backend, this also creates a git tag
    fn create_baseline(
        &self,
        name: String,
        description: Option<String>,
        created_by: String,
    ) -> Result<crate::models::Baseline> {
        let mut store = self.load()?;
        let baseline = store.create_baseline(name, description, created_by).clone();
        self.save(&store)?;
        Ok(baseline)
    }

    /// Lists all baselines
    fn list_baselines(&self) -> Result<Vec<crate::models::Baseline>> {
        let store = self.load()?;
        Ok(store.baselines.clone())
    }

    /// Gets a baseline by ID
    fn get_baseline(&self, id: &Uuid) -> Result<Option<crate::models::Baseline>> {
        let store = self.load()?;
        Ok(store.baselines.iter().find(|b| &b.id == id).cloned())
    }

    /// Deletes a baseline (if not locked)
    fn delete_baseline(&self, id: &Uuid) -> Result<bool> {
        let mut store = self.load()?;
        let deleted = store.delete_baseline(id);
        if deleted {
            self.save(&store)?;
        }
        Ok(deleted)
    }

    /// Compares current state against a baseline
    fn compare_with_baseline(
        &self,
        baseline_id: &Uuid,
    ) -> Result<Option<crate::models::BaselineComparison>> {
        let store = self.load()?;
        Ok(store.compare_with_baseline(baseline_id))
    }

    /// Compares two baselines
    fn compare_baselines(
        &self,
        source_id: &Uuid,
        target_id: &Uuid,
    ) -> Result<Option<crate::models::BaselineComparison>> {
        let store = self.load()?;
        Ok(store.compare_baselines(source_id, target_id))
    }

    // =========================================================================
    // Utility Operations
    // =========================================================================

    /// Gets the current store version (for detecting external modifications)
    ///
    /// This is used for polling to detect if the database has been modified
    /// by another process since we last loaded it.
    fn get_store_version(&self) -> Result<i64> {
        Ok(self.load()?.store_version)
    }

    /// Returns true if the database file exists
    fn exists(&self) -> bool {
        self.path().exists()
    }

    /// Creates the database with default/empty data if it doesn't exist
    fn create_if_not_exists(&self) -> Result<()> {
        if !self.exists() {
            self.save(&RequirementsStore::new())?;
        }
        Ok(())
    }

    // =========================================================================
    // Queue Operations (STORY-0366)
    // =========================================================================

    /// Lists queue entries for a user
    /// If include_completed is false, excludes entries whose requirement is Completed
    fn queue_list(&self, _user_id: &str, _include_completed: bool) -> Result<Vec<QueueEntry>> {
        anyhow::bail!("Queue not supported for this backend")
    }

    /// Lists every user id that has a stored queue (one per persisted queue
    /// file). Read-only enumeration powering the fleet-wide
    /// `aida queue list --all-users` view. The default impl returns an empty
    /// list (no fleet view); backends that persist per-user queues override it.
    // trace:STORY-672
    fn queue_users(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    /// Adds an entry to a user's queue
    fn queue_add(&self, _entry: QueueEntry) -> Result<()> {
        anyhow::bail!("Queue not supported for this backend")
    }

    /// Removes an entry from a user's queue
    fn queue_remove(&self, _user_id: &str, _requirement_id: &Uuid) -> Result<()> {
        anyhow::bail!("Queue not supported for this backend")
    }

    /// Removes an entry from a user's queue, optionally scoped to a single
    /// routing role. When `role` is `Some(r)`, only the entry whose
    /// `for_role` matches `r` (case-insensitive, after role-name
    /// canonicalization) is removed — so a spec queued for several roles can
    /// be surgically dropped from one queue without emptying the others.
    /// When `role` is `None`, removal is role-blind (every entry for the
    /// requirement is dropped), matching `queue_remove`. The default impl
    /// ignores the role filter and delegates to `queue_remove`; backends
    /// that store `for_role` override this.
    // trace:BUG-529 | ai:claude
    fn queue_remove_for_role(
        &self,
        user_id: &str,
        requirement_id: &Uuid,
        _role: Option<&str>,
    ) -> Result<()> {
        self.queue_remove(user_id, requirement_id)
    }

    /// Reorders queue entries by updating positions
    fn queue_reorder(&self, _user_id: &str, _items: &[(Uuid, i64)]) -> Result<()> {
        anyhow::bail!("Queue not supported for this backend")
    }

    /// Clears queue entries. If completed_only is true, only removes entries
    /// whose requirement has status Completed.
    fn queue_clear(&self, _user_id: &str, _completed_only: bool) -> Result<()> {
        anyhow::bail!("Queue not supported for this backend")
    }

    /// Bulk-remove every queue entry whose `requirement_id` is in `ids`, in a
    /// single write + commit. Returns the entries that were removed (for
    /// reporting). The caller decides which ids are dead — this is a dumb
    /// set-membership removal so the (cache-fast) terminal/archived
    /// determination stays out of the backend. Powers queue-GC (drop routed
    /// entries whose target spec is archived/Completed/Rejected) without the
    /// N-commits cost of looping `queue_remove`. Default no-op for backends
    /// without a queue.
    // trace:TASK-1052 | ai:claude
    fn queue_remove_many(&self, _user_id: &str, _ids: &[Uuid]) -> Result<Vec<QueueEntry>> {
        Ok(Vec::new())
    }

    // =========================================================================
    // Utility Operations
    // =========================================================================

    /// Returns statistics about the database
    fn stats(&self) -> Result<DatabaseStats> {
        let store = self.load()?;
        Ok(DatabaseStats {
            requirement_count: store.requirements.len(),
            user_count: store.users.len(),
            feature_count: store.features.len(),
            backend_type: self.backend_type(),
        })
    }
}

/// Statistics about a database
#[derive(Debug, Clone)]
pub struct DatabaseStats {
    pub requirement_count: usize,
    pub user_count: usize,
    pub feature_count: usize,
    pub backend_type: BackendType,
}
