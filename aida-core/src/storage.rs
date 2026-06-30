// trace:FR-0153 | ai:claude:high
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

use crate::models::{Requirement, RequirementsStore};

/// Error type for storage operations
#[derive(Debug)]
pub enum StorageError {
    /// File is locked by another process
    FileLocked,
    /// Other IO error
    IoError(std::io::Error),
    /// Parse error
    ParseError(String),
    /// Conflict detected during save
    Conflict(ConflictInfo),
}

/// Information about a conflict detected during save
#[derive(Debug, Clone)]
pub struct ConflictInfo {
    /// ID of the requirement with conflict
    pub requirement_id: Uuid,
    /// SPEC-ID for display
    pub spec_id: String,
    /// Fields that have conflicting changes
    pub conflicting_fields: Vec<FieldConflict>,
    /// The version from disk (external changes)
    pub disk_version: Box<Requirement>,
    /// The version we're trying to save (local changes)
    pub local_version: Box<Requirement>,
}

/// Describes a conflict in a specific field
#[derive(Debug, Clone)]
pub struct FieldConflict {
    /// Name of the field
    pub field_name: String,
    /// Original value when we last loaded
    pub original_value: String,
    /// Value on disk (external change)
    pub disk_value: String,
    /// Value we're trying to save (local change)
    pub local_value: String,
}

/// Result of attempting a save with conflict detection
#[derive(Debug)]
pub enum SaveResult {
    /// Save succeeded without conflicts
    Success,
    /// Save succeeded after auto-merging non-conflicting changes
    Merged {
        /// Number of requirements that were merged
        merged_count: usize,
    },
    /// Conflict detected - user action required
    Conflict(ConflictInfo),
}

/// Resolution strategy when a conflict is detected
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictResolution {
    /// Keep the local version, overwrite disk
    ForceLocal,
    /// Keep the disk version, discard local changes
    KeepDisk,
    /// Merge field by field (take local changes for modified fields)
    Merge,
}

/// Result of adding a new requirement atomically
#[derive(Debug)]
pub struct AddResult {
    /// The updated store with all changes (including external)
    pub store: RequirementsStore,
    /// Number of external requirements that were merged in
    pub external_changes_merged: usize,
    /// The SPEC-ID assigned to the new requirement
    pub spec_id: String,
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::FileLocked => write!(f, "File is locked by another user/process"),
            StorageError::IoError(e) => write!(f, "IO error: {}", e),
            StorageError::ParseError(s) => write!(f, "Parse error: {}", s),
            StorageError::Conflict(info) => write!(
                f,
                "Conflict detected for {} ({}): {} field(s) changed externally",
                info.spec_id,
                info.requirement_id,
                info.conflicting_fields.len()
            ),
        }
    }
}

impl std::error::Error for StorageError {}

/// Information about a user session with an open requirements file
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionInfo {
    /// Unique session ID (process-specific)
    pub session_id: String,
    /// User name/handle
    pub user_name: String,
    /// Hostname or machine identifier
    pub hostname: String,
    /// Process ID
    pub pid: u32,
    /// When this session started
    pub started_at: DateTime<Utc>,
    /// Last heartbeat timestamp
    pub last_heartbeat: DateTime<Utc>,
    /// Requirement currently being edited (if any)
    pub editing_requirement: Option<EditLock>,
}

/// Information about a requirement being edited
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EditLock {
    /// ID of the requirement being edited
    pub requirement_id: Uuid,
    /// SPEC-ID for display
    pub spec_id: String,
    /// When edit mode started
    pub started_at: DateTime<Utc>,
}

/// Lock file contents tracking all active sessions
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LockFileInfo {
    /// All active sessions (keyed by session_id)
    pub sessions: HashMap<String, SessionInfo>,
}

impl LockFileInfo {
    /// Create a new empty lock file info
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    /// Remove stale sessions (no heartbeat in the last N seconds)
    pub fn remove_stale_sessions(&mut self, stale_threshold_secs: i64) {
        let now = Utc::now();
        self.sessions.retain(|_, session| {
            let elapsed = now.signed_duration_since(session.last_heartbeat);
            elapsed.num_seconds() < stale_threshold_secs
        });
    }

    /// Get sessions that have a specific requirement open for editing
    pub fn get_editors(&self, requirement_id: Uuid) -> Vec<&SessionInfo> {
        self.sessions
            .values()
            .filter(|s| {
                s.editing_requirement
                    .as_ref()
                    .map(|e| e.requirement_id == requirement_id)
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Get all active sessions except the current one
    pub fn get_other_sessions(&self, current_session_id: &str) -> Vec<&SessionInfo> {
        self.sessions
            .values()
            .filter(|s| s.session_id != current_session_id)
            .collect()
    }
}

impl SessionInfo {
    /// Check if this session's heartbeat is stale
    pub fn is_stale(&self, threshold_secs: i64) -> bool {
        let elapsed = Utc::now().signed_duration_since(self.last_heartbeat);
        elapsed.num_seconds() > threshold_secs
    }
}

/// Handles saving and loading requirements from disk with file locking
/// for rudimentary multi-user support
pub struct Storage {
    file_path: PathBuf,
    lock_file_path: PathBuf,
}

impl Storage {
    /// Creates a new Storage instance
    pub fn new<P: AsRef<Path>>(file_path: P) -> Self {
        let file_path = file_path.as_ref().to_path_buf();
        let lock_file_path = file_path.with_extension("yaml.lock");
        Self {
            file_path,
            lock_file_path,
        }
    }

    /// Returns the path to the storage file
    pub fn path(&self) -> &Path {
        &self.file_path
    }

    /// Returns the path to the lock file
    pub fn lock_file_path(&self) -> &Path {
        &self.lock_file_path
    }

    /// Returns true if the storage file is a SQLite database (based on extension)
    pub fn is_sqlite(&self) -> bool {
        matches!(
            self.file_path.extension().and_then(|e| e.to_str()),
            Some("db") | Some("sqlite") | Some("sqlite3")
        )
    }

    /// Read the current lock file info (session tracking)
    pub fn read_lock_info(&self) -> Result<LockFileInfo> {
        if !self.lock_file_path.exists() {
            return Ok(LockFileInfo::new());
        }

        let content = fs::read_to_string(&self.lock_file_path)
            .with_context(|| format!("Failed to read lock file: {:?}", self.lock_file_path))?;

        // Try to parse as YAML, fall back to empty if invalid
        Ok(serde_yaml::from_str(&content).unwrap_or_else(|_| LockFileInfo::new()))
    }

    /// Write lock file info (session tracking)
    /// This atomically updates the lock file with current session info
    pub fn write_lock_info(&self, info: &LockFileInfo) -> Result<()> {
        // Create parent directories if needed
        if let Some(parent) = self.lock_file_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let yaml = serde_yaml::to_string(info).context("Failed to serialize lock info")?;

        fs::write(&self.lock_file_path, yaml)
            .with_context(|| format!("Failed to write lock file: {:?}", self.lock_file_path))?;

        Ok(())
    }

    /// Register a new session in the lock file
    pub fn register_session(&self, session: SessionInfo) -> Result<LockFileInfo> {
        let mut info = self.read_lock_info().unwrap_or_default();

        // Clean up stale sessions (no heartbeat in 30 seconds)
        info.remove_stale_sessions(30);

        // Add/update our session
        info.sessions.insert(session.session_id.clone(), session);

        self.write_lock_info(&info)?;
        Ok(info)
    }

    /// Update heartbeat for a session
    pub fn update_heartbeat(
        &self,
        session_id: &str,
        editing: Option<EditLock>,
    ) -> Result<LockFileInfo> {
        let mut info = self.read_lock_info().unwrap_or_default();

        // Clean up stale sessions
        info.remove_stale_sessions(30);

        // Update our session's heartbeat
        if let Some(session) = info.sessions.get_mut(session_id) {
            session.last_heartbeat = Utc::now();
            session.editing_requirement = editing;
        }

        self.write_lock_info(&info)?;
        Ok(info)
    }

    /// Unregister a session from the lock file
    pub fn unregister_session(&self, session_id: &str) -> Result<()> {
        let mut info = self.read_lock_info().unwrap_or_default();
        info.sessions.remove(session_id);

        // If no sessions left, we can delete the lock file
        if info.sessions.is_empty() {
            let _ = fs::remove_file(&self.lock_file_path);
        } else {
            self.write_lock_info(&info)?;
        }

        Ok(())
    }

    /// Get current active sessions (for displaying warnings)
    pub fn get_active_sessions(&self) -> Result<LockFileInfo> {
        let mut info = self.read_lock_info().unwrap_or_default();
        info.remove_stale_sessions(30);
        Ok(info)
    }

    /// Acquire an exclusive lock on the file for writing
    /// Returns the lock file handle which must be held during the operation
    fn acquire_write_lock(&self) -> Result<File> {
        // Create parent directories if needed
        if let Some(parent) = self.lock_file_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let lock_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.lock_file_path)
            .with_context(|| format!("Failed to create lock file: {:?}", self.lock_file_path))?;

        // Try to acquire exclusive lock with timeout
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(5);

        loop {
            match lock_file.try_lock_exclusive() {
                Ok(()) => return Ok(lock_file),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if start.elapsed() > timeout {
                        anyhow::bail!(
                            "Timeout waiting for file lock - another user may be editing: {:?}",
                            self.file_path
                        );
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("Failed to acquire lock on {:?}", self.lock_file_path)
                    })
                }
            }
        }
    }

    /// Acquire a shared lock on the file for reading
    fn acquire_read_lock(&self) -> Result<Option<File>> {
        if !self.lock_file_path.exists() {
            return Ok(None);
        }

        let lock_file = OpenOptions::new()
            .read(true)
            .open(&self.lock_file_path)
            .with_context(|| format!("Failed to open lock file: {:?}", self.lock_file_path))?;

        // Try to acquire shared lock with timeout
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(5);

        loop {
            // Use fs2's try_lock_shared explicitly to avoid conflict with std::fs::File method
            match fs2::FileExt::try_lock_shared(&lock_file) {
                Ok(()) => return Ok(Some(lock_file)),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if start.elapsed() > timeout {
                        anyhow::bail!(
                            "Timeout waiting for file lock - another user may be editing: {:?}",
                            self.file_path
                        );
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("Failed to acquire lock on {:?}", self.lock_file_path)
                    })
                }
            }
        }
    }

    /// Loads requirements from file with file locking
    /// Automatically detects file type from extension (.db/.sqlite for SQLite, otherwise YAML)
    pub fn load(&self) -> Result<RequirementsStore> {
        // Check if this is a SQLite database by extension
        let is_sqlite = matches!(
            self.file_path.extension().and_then(|e| e.to_str()),
            Some("db") | Some("sqlite") | Some("sqlite3")
        );

        if is_sqlite {
            // Use SQLite backend for .db files
            return self.load_sqlite();
        }

        // Directory paths → git-canonical store (EPIC-1-001).
        // Lets the Storage façade work transparently for `aida queue` and
        // any other handler that takes a Storage but the storage path is
        // pointing at .aida-store/.
        if self.file_path.is_dir() {
            use crate::db::DatabaseBackend;
            let backend = crate::db::GitBackend::new(&self.file_path)?;
            return backend.load();
        }

        // Create the file if it doesn't exist
        if !self.file_path.exists() {
            let parent = self
                .file_path
                .parent()
                .context("Failed to get parent directory")?;
            fs::create_dir_all(parent)?;
            let default_store = RequirementsStore::new();
            self.save(&default_store)?;
            return Ok(default_store);
        }

        // Acquire shared lock for reading
        let _lock = self.acquire_read_lock()?;

        // Open and read the file
        let file = File::open(&self.file_path)
            .with_context(|| format!("Failed to open file: {:?}", self.file_path))?;
        let reader = BufReader::new(file);

        // Parse the YAML content
        let mut store: crate::models::RequirementsStore = serde_yaml::from_reader(reader)
            .map_err(|e| {
                eprintln!("YAML parse error details: {}", e);
                e
            })
            .with_context(|| format!("Failed to parse YAML from {:?}", self.file_path))?;

        // Migrate existing features to numbered format if needed
        store.migrate_features();

        // Assign SPEC-IDs to requirements that don't have them
        let had_missing_spec_ids = store.requirements.iter().any(|r| r.spec_id.is_none());
        store.assign_spec_ids();

        // Migrate existing users to have $USER-XXX spec_ids
        let had_missing_user_spec_ids = store.users.iter().any(|u| u.spec_id.is_none());
        store.migrate_users_to_spec_ids();

        // Add any missing built-in type definitions
        let had_missing_types = store.migrate_type_definitions();

        // Add any missing built-in types to id_config.requirement_types
        // trace:FR-0309 | ai:claude:high
        let had_missing_id_config_types = store.migrate_id_config_types();

        // Repair any duplicate SPEC-IDs (auto-fix corruption)
        let repaired_duplicates = store.repair_duplicate_spec_ids();

        // Drop read lock before acquiring write lock for migration save
        drop(_lock);

        // Save back if we assigned any SPEC-IDs, added missing types, or repaired duplicates
        if had_missing_spec_ids
            || had_missing_user_spec_ids
            || had_missing_types
            || had_missing_id_config_types
            || repaired_duplicates > 0
        {
            self.save(&store)?;
        }

        // Validate SPEC-ID uniqueness (should pass after repair, but let's be safe)
        store.validate_unique_spec_ids()?;

        Ok(store)
    }

    /// Loads requirements from a SQLite database file
    fn load_sqlite(&self) -> Result<RequirementsStore> {
        use crate::db::{DatabaseBackend, SqliteBackend};

        let backend = SqliteBackend::new(&self.file_path)?;
        let mut store = backend.load()?;

        // Run migrations just like YAML loading
        store.migrate_features();
        let had_missing_spec_ids = store.requirements.iter().any(|r| r.spec_id.is_none());
        store.assign_spec_ids();
        let had_missing_user_spec_ids = store.users.iter().any(|u| u.spec_id.is_none());
        store.migrate_users_to_spec_ids();
        let had_missing_types = store.migrate_type_definitions();
        // trace:FR-0309 | ai:claude:high
        let had_missing_id_config_types = store.migrate_id_config_types();
        let repaired_duplicates = store.repair_duplicate_spec_ids();

        // Save back if we made any changes
        if had_missing_spec_ids
            || had_missing_user_spec_ids
            || had_missing_types
            || had_missing_id_config_types
            || repaired_duplicates > 0
        {
            backend.save(&store)?;
        }

        store.validate_unique_spec_ids()?;
        Ok(store)
    }

    /// Saves requirements to file with file locking
    /// Automatically detects file type from extension (.db/.sqlite for SQLite, otherwise YAML)
    pub fn save(&self, store: &RequirementsStore) -> Result<()> {
        // Check if this is a SQLite database by extension
        let is_sqlite = matches!(
            self.file_path.extension().and_then(|e| e.to_str()),
            Some("db") | Some("sqlite") | Some("sqlite3")
        );

        if is_sqlite {
            return self.save_sqlite(store);
        }

        // Directory paths → git-canonical store (EPIC-1-001). Symmetric with
        // `load()` and `update_atomically()`. Without this, an MCP server
        // handed a `Storage::new(.aida-store/)` would silently overwrite the
        // directory entry as a YAML file (or fail), and the canonical store
        // would never receive the write.
        // trace:BUG-310 | ai:claude
        if self.file_path.is_dir() {
            use crate::db::DatabaseBackend;
            let backend = crate::db::GitBackend::new(&self.file_path)?;
            return backend.save(store);
        }

        // Create parent directories if they don't exist
        if let Some(parent) = self.file_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Acquire exclusive lock for writing
        let mut lock_file = self.acquire_write_lock()?;

        // Write lock holder info (optional, for debugging)
        let _ = writeln!(
            lock_file,
            "Locked by PID {} at {}",
            std::process::id(),
            chrono::Utc::now().to_rfc3339()
        );

        // Serialize and write to file
        let yaml = serde_yaml::to_string(store)?;
        fs::write(&self.file_path, yaml)?;

        // Lock is automatically released when lock_file is dropped
        Ok(())
    }

    /// Saves requirements to a SQLite database file
    fn save_sqlite(&self, store: &RequirementsStore) -> Result<()> {
        use crate::db::{DatabaseBackend, SqliteBackend};

        let backend = SqliteBackend::new(&self.file_path)?;
        backend.save(store)?;
        Ok(())
    }

    /// Reload file from disk, detecting external changes
    /// Returns (store, changed) where changed indicates if the file was modified externally
    pub fn reload_if_changed(
        &self,
        current_store: &RequirementsStore,
    ) -> Result<(RequirementsStore, bool)> {
        let new_store = self.load()?;

        // Simple check: compare requirement counts and last modification
        // For more sophisticated detection, we could compare hashes
        let changed = new_store.requirements.len() != current_store.requirements.len()
            || new_store.users.len() != current_store.users.len()
            || new_store.features.len() != current_store.features.len();

        Ok((new_store, changed))
    }

    /// Perform an atomic update operation with proper locking
    /// This reloads the file, applies changes, and saves atomically
    pub fn update_atomically<F>(&self, update_fn: F) -> Result<RequirementsStore>
    where
        F: FnOnce(&mut RequirementsStore),
    {
        // Check if this is a SQLite database by extension
        let is_sqlite = matches!(
            self.file_path.extension().and_then(|e| e.to_str()),
            Some("db") | Some("sqlite") | Some("sqlite3")
        );

        // For SQLite, use the existing load/save methods which handle SQLite properly
        if is_sqlite {
            let mut store = self.load()?;
            update_fn(&mut store);
            self.save(&store)?;
            return Ok(store);
        }

        // Directory paths → git-canonical store (EPIC-1-001). Same pattern
        // as Storage::load() — delegate to GitBackend so any handler that
        // takes a Storage façade can mutate the canonical store.
        // trace:EPIC-1-001 | ai:claude
        if self.file_path.is_dir() {
            use crate::db::DatabaseBackend;
            let backend = crate::db::GitBackend::new(&self.file_path)?;
            let mut store = backend.load()?;
            update_fn(&mut store);
            backend.save(&store)?;
            return Ok(store);
        }

        // YAML path: Acquire exclusive lock
        let mut lock_file = self.acquire_write_lock()?;

        // Write lock holder info
        let _ = writeln!(
            lock_file,
            "Locked by PID {} at {}",
            std::process::id(),
            chrono::Utc::now().to_rfc3339()
        );

        // Load latest version from disk (YAML)
        let file = File::open(&self.file_path)
            .with_context(|| format!("Failed to open file: {:?}", self.file_path))?;
        let reader = BufReader::new(file);
        let mut store: RequirementsStore = serde_yaml::from_reader(reader)
            .with_context(|| format!("Failed to parse YAML from {:?}", self.file_path))?;

        // Apply the update
        update_fn(&mut store);

        // Save back
        let yaml = serde_yaml::to_string(&store)?;
        fs::write(&self.file_path, yaml)?;

        // Lock is released when lock_file is dropped
        Ok(store)
    }

    // trace:FR-0153 | ai:claude:high
    /// Save with conflict detection for a specific requirement
    ///
    /// This method:
    /// 1. Reloads the file from disk
    /// 2. Checks if the requirement was modified externally (based on modified_at timestamp)
    /// 3. If no external changes, applies the update
    /// 4. If external changes exist, performs field-level conflict detection
    /// 5. Auto-merges non-conflicting changes, returns conflict info for conflicts
    ///
    /// # Arguments
    /// * `local_store` - The local copy of the store with pending changes
    /// * `original_timestamps` - Map of requirement IDs to their modified_at timestamps when loaded
    /// * `modified_requirement_ids` - Set of requirement IDs that were modified locally
    ///
    /// # Returns
    /// * `SaveResult::Success` - No conflicts, save completed
    /// * `SaveResult::Merged` - External changes merged, save completed
    /// * `SaveResult::Conflict` - Conflict detected, user action required
    pub fn save_with_conflict_detection(
        &self,
        local_store: &RequirementsStore,
        original_timestamps: &HashMap<Uuid, DateTime<Utc>>,
        modified_requirement_ids: &[Uuid],
    ) -> Result<SaveResult> {
        // Check if this is a SQLite database by extension
        let is_sqlite = matches!(
            self.file_path.extension().and_then(|e| e.to_str()),
            Some("db") | Some("sqlite") | Some("sqlite3")
        );

        // For SQLite, use standard save - SQLite handles concurrency natively
        if is_sqlite {
            self.save(local_store)?;
            return Ok(SaveResult::Success);
        }

        // Acquire exclusive lock
        let mut lock_file = self.acquire_write_lock()?;

        // Write lock holder info
        let _ = writeln!(
            lock_file,
            "Locked by PID {} at {}",
            std::process::id(),
            chrono::Utc::now().to_rfc3339()
        );

        // Load latest version from disk (YAML path)
        let disk_store = if self.file_path.exists() {
            let file = File::open(&self.file_path)
                .with_context(|| format!("Failed to open file: {:?}", self.file_path))?;
            let reader = BufReader::new(file);
            serde_yaml::from_reader(reader)
                .with_context(|| format!("Failed to parse YAML from {:?}", self.file_path))?
        } else {
            RequirementsStore::new()
        };

        // Check for conflicts in modified requirements
        let mut merged_count = 0;
        let mut final_store = disk_store.clone();

        for &req_id in modified_requirement_ids {
            let local_req = local_store.requirements.iter().find(|r| r.id == req_id);
            let disk_req = disk_store.requirements.iter().find(|r| r.id == req_id);
            let original_timestamp = original_timestamps.get(&req_id);

            match (local_req, disk_req, original_timestamp) {
                // New requirement (not on disk yet)
                (Some(local), None, _) => {
                    final_store.requirements.push(local.clone());
                }

                // Requirement exists on disk - check for conflicts
                (Some(local), Some(disk), Some(&orig_ts)) => {
                    // Check if disk version was modified after we loaded
                    if disk.modified_at > orig_ts {
                        // External modification detected - check for field conflicts
                        let conflicts = Self::detect_field_conflicts(local, disk, &orig_ts);

                        if !conflicts.is_empty() {
                            // Real conflict - fields we want to modify were also modified externally
                            return Ok(SaveResult::Conflict(ConflictInfo {
                                requirement_id: req_id,
                                spec_id: disk.spec_id.clone().unwrap_or_else(|| req_id.to_string()),
                                conflicting_fields: conflicts,
                                disk_version: Box::new(disk.clone()),
                                local_version: Box::new(local.clone()),
                            }));
                        }

                        // No real conflicts - merge: take our changes + disk's other changes
                        let merged = Self::merge_requirement(local, disk);
                        if let Some(idx) =
                            final_store.requirements.iter().position(|r| r.id == req_id)
                        {
                            final_store.requirements[idx] = merged;
                        }
                        merged_count += 1;
                    } else {
                        // No external changes - just use our version
                        if let Some(idx) =
                            final_store.requirements.iter().position(|r| r.id == req_id)
                        {
                            final_store.requirements[idx] = local.clone();
                        }
                    }
                }

                // Requirement exists on disk but we don't have original timestamp
                // This is a fallback - just overwrite (legacy behavior)
                (Some(local), Some(_disk), None) => {
                    if let Some(idx) = final_store.requirements.iter().position(|r| r.id == req_id)
                    {
                        final_store.requirements[idx] = local.clone();
                    }
                }

                // Local deletion (requirement exists on disk but not in local)
                (None, Some(_), _) => {
                    // Keep deletion - remove from final store
                    final_store.requirements.retain(|r| r.id != req_id);
                }

                // Already deleted on both sides
                (None, None, _) => {}
            }
        }

        // Copy over non-requirement changes from local_store
        // (users, features, id_config, etc. - these don't have per-item conflict detection yet)
        final_store.name = local_store.name.clone();
        final_store.title = local_store.title.clone();
        final_store.description = local_store.description.clone();
        final_store.users = local_store.users.clone();
        final_store.id_config = local_store.id_config.clone();
        final_store.features = local_store.features.clone();
        final_store.relationship_definitions = local_store.relationship_definitions.clone();
        final_store.reaction_definitions = local_store.reaction_definitions.clone();
        final_store.type_definitions = local_store.type_definitions.clone();
        final_store.ai_prompts = local_store.ai_prompts.clone();
        final_store.allowed_prefixes = local_store.allowed_prefixes.clone();
        final_store.restrict_prefixes = local_store.restrict_prefixes;

        // Save the merged/updated store
        let yaml = serde_yaml::to_string(&final_store)?;
        fs::write(&self.file_path, yaml)?;

        if merged_count > 0 {
            Ok(SaveResult::Merged { merged_count })
        } else {
            Ok(SaveResult::Success)
        }
    }

    /// Detect field-level conflicts between local and disk versions
    /// Returns list of fields that were modified both locally and externally
    fn detect_field_conflicts(
        local: &Requirement,
        disk: &Requirement,
        _original_timestamp: &DateTime<Utc>,
    ) -> Vec<FieldConflict> {
        let mut conflicts = Vec::new();

        // We consider a conflict if:
        // 1. Local changed a field from its original value
        // 2. Disk also changed that same field from its original value
        // 3. The final values are different
        //
        // Since we don't have the original values stored separately,
        // we compare local vs disk and flag as conflict if different
        // This is a simpler but slightly more conservative approach

        // Compare key fields
        if local.title != disk.title {
            conflicts.push(FieldConflict {
                field_name: "title".to_string(),
                original_value: String::new(), // Unknown without snapshot
                disk_value: disk.title.clone(),
                local_value: local.title.clone(),
            });
        }

        if local.description != disk.description {
            conflicts.push(FieldConflict {
                field_name: "description".to_string(),
                original_value: String::new(),
                disk_value: disk.description.clone(),
                local_value: local.description.clone(),
            });
        }

        if local.status != disk.status {
            conflicts.push(FieldConflict {
                field_name: "status".to_string(),
                original_value: String::new(),
                disk_value: disk.status.to_string(),
                local_value: local.status.to_string(),
            });
        }

        if local.priority != disk.priority {
            conflicts.push(FieldConflict {
                field_name: "priority".to_string(),
                original_value: String::new(),
                disk_value: disk.priority.to_string(),
                local_value: local.priority.to_string(),
            });
        }

        if local.owner != disk.owner {
            conflicts.push(FieldConflict {
                field_name: "owner".to_string(),
                original_value: String::new(),
                disk_value: disk.owner.clone(),
                local_value: local.owner.clone(),
            });
        }

        if local.feature != disk.feature {
            conflicts.push(FieldConflict {
                field_name: "feature".to_string(),
                original_value: String::new(),
                disk_value: disk.feature.clone(),
                local_value: local.feature.clone(),
            });
        }

        if local.req_type != disk.req_type {
            conflicts.push(FieldConflict {
                field_name: "type".to_string(),
                original_value: String::new(),
                disk_value: disk.req_type.to_string(),
                local_value: local.req_type.to_string(),
            });
        }

        if local.tags != disk.tags {
            conflicts.push(FieldConflict {
                field_name: "tags".to_string(),
                original_value: String::new(),
                disk_value: disk.tags.iter().cloned().collect::<Vec<_>>().join(", "),
                local_value: local.tags.iter().cloned().collect::<Vec<_>>().join(", "),
            });
        }

        conflicts
    }

    /// Merge two versions of a requirement
    /// Takes local changes and merges with disk version
    fn merge_requirement(local: &Requirement, disk: &Requirement) -> Requirement {
        // Start with local (our changes)
        let mut merged = local.clone();

        // Merge comments: include all comments from both versions
        // Use a set to deduplicate by comment ID
        let mut comment_ids: std::collections::HashSet<Uuid> =
            merged.comments.iter().map(|c| c.id).collect();
        for comment in &disk.comments {
            if comment_ids.insert(comment.id) {
                merged.comments.push(comment.clone());
            }
        }
        // Sort comments by created_at
        merged.comments.sort_by_key(|c| c.created_at);

        // Merge history: include all history entries from both versions
        let mut history_ids: std::collections::HashSet<Uuid> =
            merged.history.iter().map(|h| h.id).collect();
        for entry in &disk.history {
            if history_ids.insert(entry.id) {
                merged.history.push(entry.clone());
            }
        }
        // Sort history by timestamp
        merged.history.sort_by_key(|h| h.timestamp);

        // Merge relationships: include all from both (dedupe by target_id + rel_type)
        let existing_rels: std::collections::HashSet<_> = merged
            .relationships
            .iter()
            .map(|r| (r.target_id, r.rel_type.clone()))
            .collect();
        for rel in &disk.relationships {
            if !existing_rels.contains(&(rel.target_id, rel.rel_type.clone())) {
                merged.relationships.push(rel.clone());
            }
        }

        // Merge URLs: include all from both (dedupe by URL)
        let existing_urls: std::collections::HashSet<_> =
            merged.urls.iter().map(|u| u.url.clone()).collect();
        for url in &disk.urls {
            if !existing_urls.contains(&url.url) {
                merged.urls.push(url.clone());
            }
        }

        // Keep the later modified_at timestamp
        if disk.modified_at > merged.modified_at {
            merged.modified_at = disk.modified_at;
        }

        // If disk has AI evaluation and we don't, take it
        if merged.ai_evaluation.is_none() && disk.ai_evaluation.is_some() {
            merged.ai_evaluation = disk.ai_evaluation.clone();
        }

        merged
    }

    /// Force save with a specific conflict resolution strategy
    pub fn save_with_resolution(
        &self,
        local_store: &RequirementsStore,
        requirement_id: Uuid,
        resolution: ConflictResolution,
    ) -> Result<RequirementsStore> {
        // Acquire exclusive lock
        let mut lock_file = self.acquire_write_lock()?;

        let _ = writeln!(
            lock_file,
            "Locked by PID {} at {}",
            std::process::id(),
            chrono::Utc::now().to_rfc3339()
        );

        // Load disk version
        let mut disk_store: RequirementsStore = if self.file_path.exists() {
            let file = File::open(&self.file_path)?;
            let reader = BufReader::new(file);
            serde_yaml::from_reader(reader)?
        } else {
            RequirementsStore::new()
        };

        match resolution {
            ConflictResolution::ForceLocal => {
                // Replace disk requirement with local version
                if let Some(local_req) = local_store
                    .requirements
                    .iter()
                    .find(|r| r.id == requirement_id)
                {
                    if let Some(idx) = disk_store
                        .requirements
                        .iter()
                        .position(|r| r.id == requirement_id)
                    {
                        disk_store.requirements[idx] = local_req.clone();
                    } else {
                        disk_store.requirements.push(local_req.clone());
                    }
                }
            }
            ConflictResolution::KeepDisk => {
                // Do nothing - disk version is already in disk_store
            }
            ConflictResolution::Merge => {
                // Merge local changes into disk version
                if let Some(local_req) = local_store
                    .requirements
                    .iter()
                    .find(|r| r.id == requirement_id)
                {
                    if let Some(disk_req) = disk_store
                        .requirements
                        .iter()
                        .find(|r| r.id == requirement_id)
                    {
                        let merged = Self::merge_requirement(local_req, disk_req);
                        if let Some(idx) = disk_store
                            .requirements
                            .iter()
                            .position(|r| r.id == requirement_id)
                        {
                            disk_store.requirements[idx] = merged;
                        }
                    } else {
                        disk_store.requirements.push(local_req.clone());
                    }
                }
            }
        }

        // Save the updated store
        let yaml = serde_yaml::to_string(&disk_store)?;
        fs::write(&self.file_path, yaml)?;

        Ok(disk_store)
    }

    /// Get a snapshot of requirement timestamps for conflict detection
    pub fn get_requirement_timestamps(store: &RequirementsStore) -> HashMap<Uuid, DateTime<Utc>> {
        store
            .requirements
            .iter()
            .map(|r| (r.id, r.modified_at))
            .collect()
    }

    /// Atomically add a new requirement with reload-before-save
    ///
    /// This method:
    /// 1. Acquires exclusive lock
    /// 2. Reloads fresh data from disk
    /// 3. Merges local non-requirement changes (features, users, config, etc.)
    /// 4. Generates a new SPEC-ID based on fresh state
    /// 5. Adds the new requirement
    /// 6. Saves to disk
    /// 7. Returns updated store with all changes
    ///
    /// This ensures:
    /// - No duplicate SPEC-IDs even with concurrent modifications
    /// - External requirement additions are preserved
    /// - Local config/feature/user changes are applied
    ///
    /// # Arguments
    /// * `local_store` - The local store with pending config/feature changes
    /// * `new_req` - The new requirement to add (without SPEC-ID yet)
    /// * `feature_prefix` - Optional feature prefix for ID generation
    /// * `type_prefix` - Optional type prefix for ID generation
    ///
    /// # Returns
    /// * `Ok(AddResult)` - Contains updated store, merge count, and assigned SPEC-ID
    // trace:FR-0183 | ai:claude:high
    pub fn add_requirement_atomic(
        &self,
        local_store: &RequirementsStore,
        new_req: Requirement,
        feature_prefix: Option<&str>,
        type_prefix: Option<&str>,
    ) -> Result<AddResult> {
        // Use appropriate backend based on file extension
        if self.is_sqlite() {
            return self.add_requirement_atomic_sqlite(
                local_store,
                new_req,
                feature_prefix,
                type_prefix,
            );
        }

        // YAML implementation follows
        // Acquire exclusive lock
        let mut lock_file = self.acquire_write_lock()?;

        let _ = writeln!(
            lock_file,
            "Locked by PID {} at {}",
            std::process::id(),
            chrono::Utc::now().to_rfc3339()
        );

        // Load fresh data from disk
        let mut disk_store: RequirementsStore = if self.file_path.exists() {
            let file = File::open(&self.file_path)?;
            let reader = BufReader::new(file);
            serde_yaml::from_reader(reader)?
        } else {
            RequirementsStore::new()
        };

        // Count external requirement additions (requirements in disk but not in local)
        let local_req_ids: std::collections::HashSet<Uuid> =
            local_store.requirements.iter().map(|r| r.id).collect();
        let external_changes_merged = disk_store
            .requirements
            .iter()
            .filter(|r| !local_req_ids.contains(&r.id))
            .count();

        // Apply local non-requirement changes to the fresh disk store
        // (users, features, config, etc. - these are simpler to just overwrite)
        disk_store.name = local_store.name.clone();
        disk_store.title = local_store.title.clone();
        disk_store.description = local_store.description.clone();
        disk_store.users = local_store.users.clone();
        disk_store.id_config = local_store.id_config.clone();
        disk_store.features = local_store.features.clone();
        disk_store.relationship_definitions = local_store.relationship_definitions.clone();
        disk_store.reaction_definitions = local_store.reaction_definitions.clone();
        disk_store.type_definitions = local_store.type_definitions.clone();
        disk_store.ai_prompts = local_store.ai_prompts.clone();
        disk_store.allowed_prefixes = local_store.allowed_prefixes.clone();
        disk_store.restrict_prefixes = local_store.restrict_prefixes;

        // Generate SPEC-ID using the fresh disk store state (which has all existing IDs)
        // This ensures we don't create duplicates
        disk_store.add_requirement_with_id(new_req, feature_prefix, type_prefix);

        // Get the SPEC-ID from the added requirement (last one in store)
        let spec_id = disk_store
            .requirements
            .last()
            .and_then(|r| r.spec_id.clone())
            .unwrap_or_default();

        // Save the updated store
        let yaml = serde_yaml::to_string(&disk_store).inspect_err(|_e| {
            // Log which field might contain control characters for debugging
            let check_ctrl = |s: &str, name: &str| {
                for (i, c) in s.chars().enumerate() {
                    if c.is_control() && c != '\n' && c != '\t' && c != '\r' {
                        eprintln!(
                            "Control char in {}: position {}, char code {}",
                            name, i, c as u32
                        );
                    }
                }
            };
            check_ctrl(&disk_store.name, "store.name");
            check_ctrl(&disk_store.title, "store.title");
            check_ctrl(&disk_store.description, "store.description");
            if let Some(req) = disk_store.requirements.last() {
                check_ctrl(&req.title, "new_req.title");
                check_ctrl(&req.description, "new_req.description");
                check_ctrl(&req.owner, "new_req.owner");
                check_ctrl(&req.feature, "new_req.feature");
                if let Some(ref created_by) = req.created_by {
                    check_ctrl(created_by, "new_req.created_by");
                }
            }
        })?;
        fs::write(&self.file_path, yaml)?;

        Ok(AddResult {
            store: disk_store,
            external_changes_merged,
            spec_id,
        })
    }

    /// SQLite implementation of add_requirement_atomic
    /// Uses JSON serialization (via SQLite backend) instead of YAML
    fn add_requirement_atomic_sqlite(
        &self,
        local_store: &RequirementsStore,
        new_req: Requirement,
        feature_prefix: Option<&str>,
        type_prefix: Option<&str>,
    ) -> Result<AddResult> {
        use crate::db::{DatabaseBackend, SqliteBackend};

        let backend = SqliteBackend::new(&self.file_path)?;

        // Load fresh data from database
        let mut disk_store = backend.load()?;

        // Count external requirement additions (requirements in disk but not in local)
        let local_req_ids: std::collections::HashSet<Uuid> =
            local_store.requirements.iter().map(|r| r.id).collect();
        let external_changes_merged = disk_store
            .requirements
            .iter()
            .filter(|r| !local_req_ids.contains(&r.id))
            .count();

        // Apply local non-requirement changes to the fresh disk store
        // (users, features, config, etc. - these are simpler to just overwrite)
        disk_store.name = local_store.name.clone();
        disk_store.title = local_store.title.clone();
        disk_store.description = local_store.description.clone();
        disk_store.users = local_store.users.clone();
        disk_store.id_config = local_store.id_config.clone();
        disk_store.features = local_store.features.clone();
        disk_store.relationship_definitions = local_store.relationship_definitions.clone();
        disk_store.reaction_definitions = local_store.reaction_definitions.clone();
        disk_store.type_definitions = local_store.type_definitions.clone();
        disk_store.ai_prompts = local_store.ai_prompts.clone();
        disk_store.allowed_prefixes = local_store.allowed_prefixes.clone();
        disk_store.restrict_prefixes = local_store.restrict_prefixes;

        // Generate SPEC-ID using the fresh disk store state (which has all existing IDs)
        // This ensures we don't create duplicates
        disk_store.add_requirement_with_id(new_req, feature_prefix, type_prefix);

        // Get the SPEC-ID from the added requirement (last one in store)
        let spec_id = disk_store
            .requirements
            .last()
            .and_then(|r| r.spec_id.clone())
            .unwrap_or_default();

        // Save the updated store to SQLite
        backend.save(&disk_store)?;

        Ok(AddResult {
            store: disk_store,
            external_changes_merged,
            spec_id,
        })
    }

    /// Returns the directory for storing attachments for a given spec_id
    /// Creates the directory if it doesn't exist
    pub fn get_attachments_dir(&self, spec_id: &str) -> Result<PathBuf> {
        let parent = self.file_path.parent().unwrap_or(Path::new("."));
        let attachments_dir = parent.join("attachments").join(spec_id);

        if !attachments_dir.exists() {
            fs::create_dir_all(&attachments_dir).with_context(|| {
                format!(
                    "Failed to create attachments directory: {:?}",
                    attachments_dir
                )
            })?;
        }

        Ok(attachments_dir)
    }

    /// Copies a file to the attachments directory for a requirement
    /// Returns the relative path to the stored file and the file size
    pub fn store_attachment_file(
        &self,
        spec_id: &str,
        source_path: &Path,
    ) -> Result<(String, u64)> {
        let attachments_dir = self.get_attachments_dir(spec_id)?;

        // Get the filename from the source path
        let filename = source_path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("Invalid source path: no filename"))?
            .to_string_lossy()
            .to_string();

        // Handle potential filename conflicts by adding a suffix
        let dest_path = {
            let initial_path = attachments_dir.join(&filename);
            if !initial_path.exists() {
                initial_path
            } else {
                // Find a unique filename
                let stem = source_path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "file".to_string());
                let ext = source_path
                    .extension()
                    .map(|s| format!(".{}", s.to_string_lossy()))
                    .unwrap_or_default();

                let mut counter = 1;
                loop {
                    let new_name = format!("{}_{}{}", stem, counter, ext);
                    let new_path = attachments_dir.join(&new_name);
                    if !new_path.exists() {
                        break new_path;
                    }
                    counter += 1;
                }
            }
        };

        // Copy the file
        fs::copy(source_path, &dest_path)
            .with_context(|| format!("Failed to copy file to {:?}", dest_path))?;

        // Get file size
        let metadata = fs::metadata(&dest_path)
            .with_context(|| format!("Failed to read file metadata: {:?}", dest_path))?;
        let size = metadata.len();

        // Build relative path
        let rel_path = format!(
            "attachments/{}/{}",
            spec_id,
            dest_path.file_name().unwrap().to_string_lossy()
        );

        Ok((rel_path, size))
    }

    /// Stores attachment data from bytes (for drag-drop where path isn't available)
    pub fn store_attachment_bytes(
        &self,
        spec_id: &str,
        filename: &str,
        data: &[u8],
    ) -> Result<(String, u64)> {
        let attachments_dir = self.get_attachments_dir(spec_id)?;

        // Handle potential filename conflicts
        let dest_path = {
            let initial_path = attachments_dir.join(filename);
            if !initial_path.exists() {
                initial_path
            } else {
                // Find a unique filename
                let path = Path::new(filename);
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "file".to_string());
                let ext = path
                    .extension()
                    .map(|s| format!(".{}", s.to_string_lossy()))
                    .unwrap_or_default();

                let mut counter = 1;
                loop {
                    let new_name = format!("{}_{}{}", stem, counter, ext);
                    let new_path = attachments_dir.join(&new_name);
                    if !new_path.exists() {
                        break new_path;
                    }
                    counter += 1;
                }
            }
        };

        // Write the file
        fs::write(&dest_path, data)
            .with_context(|| format!("Failed to write attachment file: {:?}", dest_path))?;

        // Build relative path
        let rel_path = format!(
            "attachments/{}/{}",
            spec_id,
            dest_path.file_name().unwrap().to_string_lossy()
        );

        Ok((rel_path, data.len() as u64))
    }

    /// Removes an attachment file from disk
    pub fn remove_attachment_file(&self, spec_id: &str, stored_path: &str) -> Result<()> {
        let parent = self.file_path.parent().unwrap_or(Path::new("."));
        let full_path = parent.join(stored_path);

        if full_path.exists() {
            fs::remove_file(&full_path)
                .with_context(|| format!("Failed to remove attachment file: {:?}", full_path))?;
        }

        // Try to clean up empty directories
        let attachments_dir = parent.join("attachments").join(spec_id);
        if attachments_dir.exists() {
            // Only remove if empty
            if let Ok(mut entries) = fs::read_dir(&attachments_dir) {
                if entries.next().is_none() {
                    let _ = fs::remove_dir(&attachments_dir);
                }
            }
        }

        Ok(())
    }

    /// Gets the full path to an attachment file
    pub fn get_attachment_full_path(&self, stored_path: &str) -> PathBuf {
        let parent = self.file_path.parent().unwrap_or(Path::new("."));
        parent.join(stored_path)
    }

    /// Checks if an attachment file exists
    pub fn attachment_exists(&self, stored_path: &str) -> bool {
        self.get_attachment_full_path(stored_path).exists()
    }

    // --- GitLab Sync State Methods (STORY-0325) ---

    /// Save a GitLab sync state record
    /// trace:STORY-0325 | ai:claude
    pub fn save_sync_state(&self, state: &crate::models::GitLabSyncState) -> Result<()> {
        if !self.is_sqlite() {
            anyhow::bail!("GitLab sync state is only supported for SQLite databases");
        }

        use crate::db::SqliteBackend;
        let backend = SqliteBackend::new(&self.file_path)?;
        backend.save_sync_state(state)
    }

    /// Load a GitLab sync state by requirement ID and issue IID
    /// trace:STORY-0325 | ai:claude
    pub fn load_sync_state(
        &self,
        requirement_id: uuid::Uuid,
        issue_iid: u64,
    ) -> Result<Option<crate::models::GitLabSyncState>> {
        if !self.is_sqlite() {
            anyhow::bail!("GitLab sync state is only supported for SQLite databases");
        }

        use crate::db::SqliteBackend;
        let backend = SqliteBackend::new(&self.file_path)?;
        backend.load_sync_state(requirement_id, issue_iid)
    }

    /// Load all GitLab sync states for a requirement
    /// trace:STORY-0325 | ai:claude
    pub fn load_sync_states_for_requirement(
        &self,
        requirement_id: uuid::Uuid,
    ) -> Result<Vec<crate::models::GitLabSyncState>> {
        if !self.is_sqlite() {
            anyhow::bail!("GitLab sync state is only supported for SQLite databases");
        }

        use crate::db::SqliteBackend;
        let backend = SqliteBackend::new(&self.file_path)?;
        backend.load_sync_states_for_requirement(requirement_id)
    }

    /// Load all GitLab sync states
    /// trace:STORY-0325 | ai:claude
    pub fn load_all_sync_states(&self) -> Result<Vec<crate::models::GitLabSyncState>> {
        if !self.is_sqlite() {
            anyhow::bail!("GitLab sync state is only supported for SQLite databases");
        }

        use crate::db::SqliteBackend;
        let backend = SqliteBackend::new(&self.file_path)?;
        backend.load_all_sync_states()
    }

    /// Load GitLab sync states by status
    /// trace:STORY-0325 | ai:claude
    pub fn load_sync_states_by_status(
        &self,
        status: crate::models::SyncStatus,
    ) -> Result<Vec<crate::models::GitLabSyncState>> {
        if !self.is_sqlite() {
            anyhow::bail!("GitLab sync state is only supported for SQLite databases");
        }

        use crate::db::SqliteBackend;
        let backend = SqliteBackend::new(&self.file_path)?;
        backend.load_sync_states_by_status(status)
    }

    /// Delete a GitLab sync state
    /// trace:STORY-0325 | ai:claude
    pub fn delete_sync_state(&self, requirement_id: uuid::Uuid, issue_iid: u64) -> Result<bool> {
        if !self.is_sqlite() {
            anyhow::bail!("GitLab sync state is only supported for SQLite databases");
        }

        use crate::db::SqliteBackend;
        let backend = SqliteBackend::new(&self.file_path)?;
        backend.delete_sync_state(requirement_id, issue_iid)
    }

    // =========================================================================
    // Queue Operations (STORY-0366)
    // =========================================================================
    // trace:STORY-0366 | ai:claude

    /// Pick the right backend for queue operations: SQLite for .db paths,
    /// GitBackend for directory paths. Returns an error for YAML files (no
    /// queue support there). Per EPIC-1-001 the canonical path is git.
    fn queue_backend(&self) -> Result<Box<dyn crate::db::DatabaseBackend>> {
        if self.is_sqlite() {
            return Ok(Box::new(crate::db::SqliteBackend::new(&self.file_path)?));
        }
        if self.file_path.is_dir() {
            return Ok(Box::new(crate::db::GitBackend::new(&self.file_path)?));
        }
        anyhow::bail!(
            "Queue is only supported for SQLite or git-canonical (directory) backends — \
             got {:?}",
            self.file_path
        )
    }

    /// List queue entries for a user
    pub fn queue_list(
        &self,
        user_id: &str,
        include_completed: bool,
    ) -> Result<Vec<crate::models::QueueEntry>> {
        self.queue_backend()?.queue_list(user_id, include_completed)
    }

    /// List every user id with a persisted queue (read-only). Powers the
    /// fleet-wide `aida queue list --all-users` aggregate view.
    // trace:STORY-672
    pub fn queue_users(&self) -> Result<Vec<String>> {
        self.queue_backend()?.queue_users()
    }

    /// Add an entry to a user's queue
    pub fn queue_add(&self, entry: crate::models::QueueEntry) -> Result<()> {
        self.queue_backend()?.queue_add(entry)
    }

    /// Remove an entry from a user's queue
    pub fn queue_remove(&self, user_id: &str, requirement_id: &uuid::Uuid) -> Result<()> {
        self.queue_backend()?.queue_remove(user_id, requirement_id)
    }

    /// Remove an entry from a user's queue, optionally scoped to a single
    /// routing role. `role == None` is role-blind (same as `queue_remove`);
    /// `role == Some(r)` removes only the entry whose `for_role` matches `r`.
    /// trace:BUG-529 | ai:claude
    pub fn queue_remove_for_role(
        &self,
        user_id: &str,
        requirement_id: &uuid::Uuid,
        role: Option<&str>,
    ) -> Result<()> {
        self.queue_backend()?
            .queue_remove_for_role(user_id, requirement_id, role)
    }

    /// Reorder queue entries
    pub fn queue_reorder(&self, user_id: &str, items: &[(uuid::Uuid, i64)]) -> Result<()> {
        self.queue_backend()?.queue_reorder(user_id, items)
    }

    /// Clear queue entries
    pub fn queue_clear(&self, user_id: &str, completed_only: bool) -> Result<()> {
        self.queue_backend()?.queue_clear(user_id, completed_only)
    }

    /// Bulk-remove the given requirement ids from a user's queue in one
    /// write + commit; returns the removed entries. Powers queue-GC.
    // trace:TASK-1052 | ai:claude
    pub fn queue_remove_many(
        &self,
        user_id: &str,
        ids: &[uuid::Uuid],
    ) -> Result<Vec<crate::models::QueueEntry>> {
        self.queue_backend()?.queue_remove_many(user_id, ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_store() -> RequirementsStore {
        let mut store = RequirementsStore::new();
        store.name = "test".to_string();
        store
    }

    fn create_test_requirement(title: &str) -> Requirement {
        Requirement::new(title.to_string(), format!("Description for {}", title))
    }

    #[test]
    fn test_save_and_load() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.yaml");
        let storage = Storage::new(&file_path);

        let mut store = create_test_store();
        store.requirements.push(create_test_requirement("Test Req"));

        // Save
        storage.save(&store).unwrap();

        // Load
        let loaded = storage.load().unwrap();
        assert_eq!(loaded.requirements.len(), 1);
        assert_eq!(loaded.requirements[0].title, "Test Req");
    }

    #[test]
    fn test_conflict_detection_no_conflict() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.yaml");
        let storage = Storage::new(&file_path);

        // Create initial store with one requirement
        let mut store = create_test_store();
        let mut req = create_test_requirement("Test Req");
        req.spec_id = Some("FR-0001".to_string());
        let req_id = req.id;
        store.requirements.push(req);

        // Save initial version
        storage.save(&store).unwrap();

        // Capture timestamps
        let timestamps = Storage::get_requirement_timestamps(&store);

        // Modify the requirement
        store.requirements[0].title = "Modified Title".to_string();
        store.requirements[0].modified_at = Utc::now();

        // Save with conflict detection - should succeed
        let result = storage
            .save_with_conflict_detection(&store, &timestamps, &[req_id])
            .unwrap();

        match result {
            SaveResult::Success => {} // Expected
            _ => panic!("Expected SaveResult::Success"),
        }
    }

    #[test]
    fn test_conflict_detection_with_external_change() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.yaml");
        let storage = Storage::new(&file_path);

        // Create initial store with one requirement
        let mut store = create_test_store();
        let mut req = create_test_requirement("Test Req");
        req.spec_id = Some("FR-0001".to_string());
        let req_id = req.id;
        store.requirements.push(req);

        // Save initial version
        storage.save(&store).unwrap();

        // Capture timestamps (simulating GUI loading the store)
        let timestamps = Storage::get_requirement_timestamps(&store);
        let mut local_store = store.clone();

        // Simulate external change (another tool modifies the file)
        store.requirements[0].title = "External Change".to_string();
        store.requirements[0].modified_at = Utc::now();
        storage.save(&store).unwrap();

        // Local change (different field modified by same field in external)
        local_store.requirements[0].title = "Local Change".to_string();
        local_store.requirements[0].modified_at = Utc::now();

        // Save with conflict detection - should detect conflict
        let result = storage
            .save_with_conflict_detection(&local_store, &timestamps, &[req_id])
            .unwrap();

        match result {
            SaveResult::Conflict(info) => {
                assert_eq!(info.requirement_id, req_id);
                assert!(!info.conflicting_fields.is_empty());
                assert!(info
                    .conflicting_fields
                    .iter()
                    .any(|f| f.field_name == "title"));
            }
            _ => panic!("Expected SaveResult::Conflict"),
        }
    }

    #[test]
    fn test_conflict_resolution_force_local() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.yaml");
        let storage = Storage::new(&file_path);

        // Create initial store
        let mut store = create_test_store();
        let mut req = create_test_requirement("Test Req");
        req.spec_id = Some("FR-0001".to_string());
        let req_id = req.id;
        store.requirements.push(req);
        storage.save(&store).unwrap();

        // External change
        store.requirements[0].title = "External".to_string();
        storage.save(&store).unwrap();

        // Local version with different title
        let mut local_store = store.clone();
        local_store.requirements[0].title = "Local".to_string();

        // Resolve with ForceLocal
        let result = storage
            .save_with_resolution(&local_store, req_id, ConflictResolution::ForceLocal)
            .unwrap();

        assert_eq!(result.requirements[0].title, "Local");
    }

    #[test]
    fn test_conflict_resolution_keep_disk() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.yaml");
        let storage = Storage::new(&file_path);

        // Create initial store
        let mut store = create_test_store();
        let mut req = create_test_requirement("Test Req");
        req.spec_id = Some("FR-0001".to_string());
        let req_id = req.id;
        store.requirements.push(req);
        storage.save(&store).unwrap();

        // External change
        store.requirements[0].title = "External".to_string();
        storage.save(&store).unwrap();

        // Local version with different title
        let mut local_store = store.clone();
        local_store.requirements[0].title = "Local".to_string();

        // Resolve with KeepDisk
        let result = storage
            .save_with_resolution(&local_store, req_id, ConflictResolution::KeepDisk)
            .unwrap();

        assert_eq!(result.requirements[0].title, "External");
    }

    #[test]
    fn test_get_requirement_timestamps() {
        let mut store = create_test_store();
        let req1 = create_test_requirement("Req1");
        let req2 = create_test_requirement("Req2");
        let id1 = req1.id;
        let id2 = req2.id;
        let ts1 = req1.modified_at;
        let ts2 = req2.modified_at;

        store.requirements.push(req1);
        store.requirements.push(req2);

        let timestamps = Storage::get_requirement_timestamps(&store);
        assert_eq!(timestamps.len(), 2);
        assert_eq!(timestamps.get(&id1), Some(&ts1));
        assert_eq!(timestamps.get(&id2), Some(&ts2));
    }

    #[test]
    fn test_new_requirement_preserves_external_additions() {
        // Scenario: Instance A adds R1, Instance B adds R2 (not knowing about R1)
        // Both should be preserved
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.yaml");
        let storage = Storage::new(&file_path);

        // Instance A: Create initial store with R1
        let mut store_a = create_test_store();
        let req1 = create_test_requirement("Req1 from Instance A");
        let req1_id = req1.id;
        store_a.requirements.push(req1);
        storage.save(&store_a).unwrap();

        // Instance B: Started with empty store (before R1 was added)
        // Now adds R2 without knowing about R1
        let mut store_b = create_test_store();
        let req2 = create_test_requirement("Req2 from Instance B");
        let req2_id = req2.id;
        store_b.requirements.push(req2);

        // Instance B saves with conflict detection
        // modified_requirement_ids contains only R2 (the new one)
        let original_timestamps: HashMap<Uuid, DateTime<Utc>> = HashMap::new();
        let modified_ids = vec![req2_id];

        let result = storage
            .save_with_conflict_detection(&store_b, &original_timestamps, &modified_ids)
            .unwrap();

        // Should succeed (no conflict - R2 is new)
        match result {
            SaveResult::Success => {}
            SaveResult::Merged { .. } => {}
            SaveResult::Conflict(_) => panic!("Should not have conflict"),
        }

        // Verify both requirements are preserved
        let final_store = storage.load().unwrap();
        assert_eq!(final_store.requirements.len(), 2);
        assert!(final_store.requirements.iter().any(|r| r.id == req1_id));
        assert!(final_store.requirements.iter().any(|r| r.id == req2_id));
    }

    #[test]
    fn test_deletion_preserves_external_additions() {
        // Scenario: Instance A adds R1, Instance B deletes R2 (not knowing about R1)
        // R1 should be preserved, R2 should be deleted
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.yaml");
        let storage = Storage::new(&file_path);

        // Initial store with R2
        let mut initial_store = create_test_store();
        let req2 = create_test_requirement("Req2");
        let req2_id = req2.id;
        initial_store.requirements.push(req2);
        storage.save(&initial_store).unwrap();

        // Instance A: Adds R1 externally
        let req1 = create_test_requirement("Req1 from Instance A");
        let req1_id = req1.id;
        let mut store_a = initial_store.clone();
        store_a.requirements.push(req1);
        storage.save(&store_a).unwrap();

        // Instance B: Started before R1, now deletes R2
        let store_b = create_test_store(); // Empty - R2 was deleted
        let original_timestamps: HashMap<Uuid, DateTime<Utc>> = HashMap::new();
        let modified_ids = vec![req2_id]; // Marking R2 as modified (deleted)

        let result = storage
            .save_with_conflict_detection(&store_b, &original_timestamps, &modified_ids)
            .unwrap();

        match result {
            SaveResult::Success => {}
            SaveResult::Merged { .. } => {}
            SaveResult::Conflict(_) => panic!("Should not have conflict"),
        }

        // Verify R1 is preserved, R2 is deleted
        let final_store = storage.load().unwrap();
        assert_eq!(final_store.requirements.len(), 1);
        assert!(final_store.requirements.iter().any(|r| r.id == req1_id));
        assert!(!final_store.requirements.iter().any(|r| r.id == req2_id));
    }

    // trace:BUG-310 | ai:claude
    //
    // Regression: pointing Storage at a git-canonical store directory must
    // round-trip writes through GitBackend so that a *different* GitBackend
    // (e.g. the CLI's CachedGitBackend) sees the new requirement on the next
    // load. Before BUG-310 was fixed, `Storage::save` ignored the directory
    // case and wrote a YAML file at the directory path, which either failed
    // or corrupted the store — so MCP-served writes never reached the
    // canonical store the CLI reads.
    #[test]
    fn storage_save_on_directory_writes_through_git_backend() {
        use crate::db::DatabaseBackend;
        let temp_dir = TempDir::new().unwrap();
        let store_dir = temp_dir.path().join("store");
        std::fs::create_dir_all(&store_dir).unwrap();

        // Seed the directory as a real GitBackend (sets up `objects/`).
        let backend = crate::db::GitBackend::new(&store_dir).unwrap();
        backend.save(&create_test_store()).unwrap();

        // Use the Storage façade — the same surface the MCP server uses.
        let storage = Storage::new(&store_dir);
        let mut store = storage.load().unwrap();
        let mut req = create_test_requirement("MCP-written req");
        req.spec_id = Some("SPEC-001".to_string());
        store.requirements.push(req);
        storage.save(&store).unwrap();

        // A fresh GitBackend (mirroring an independent CLI invocation) must
        // see the requirement that Storage wrote.
        let cli_view = crate::db::GitBackend::new(&store_dir).unwrap();
        let loaded = cli_view.load().unwrap();
        assert!(
            loaded
                .requirements
                .iter()
                .any(|r| r.title == "MCP-written req"),
            "GitBackend reload did not see the requirement Storage wrote — \
             Storage::save likely fell through to the YAML path on a directory"
        );
    }
}
