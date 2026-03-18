// trace:ARCH-distributed-git-backend | ai:claude
//! Git-backed storage backend for distributed AIDA.
//!
//! Stores requirements as individual YAML files in a sharded directory layout
//! within a git repository. Metadata (store name, features, ID config, etc.)
//! is stored in a separate `metadata.yaml` file.
//!
//! This backend implements the `DatabaseBackend` trait, allowing it to be
//! used as a drop-in replacement for the YAML/SQLite/PostgreSQL backends.
//!
//! Git operations (commit, push, pull) are NOT automatic — the caller decides
//! when to sync. This backend handles only local file I/O.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::traits::{BackendType, DatabaseBackend, UpdateResult, VersionConflict};
use crate::models::{
    DispenserHandle, QueueEntry, Requirement, RequirementsStore,
};
use crate::object_store;

/// Git-backed storage backend.
///
/// Directory layout:
/// ```text
/// {root}/
///   metadata.yaml          — store name, features, ID config, etc.
///   objects/
///     FR/000/FR-001.yaml   — individual requirement files (sharded)
///     BUG/000/BUG-001.yaml
///   relations/             — (future: append-only relation log)
///   registry/              — (future: node/user registries)
/// ```
pub struct GitBackend {
    /// Root directory of the git-backed store
    root: PathBuf,
    /// Path to the objects directory
    objects_root: PathBuf,
    /// Path to the metadata file
    metadata_path: PathBuf,
    /// Optional dispenser for ID generation
    dispenser: Option<DispenserHandle>,
    /// Whether to auto-commit changes to git after writes
    auto_commit: bool,
    /// Whether to record operations in the append-only oplog
    oplog_enabled: bool,
}

/// Metadata stored separately from requirements (the "store" fields).
/// This is everything in RequirementsStore except the requirements themselves.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoreMetadata {
    #[serde(default)]
    name: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    users: Vec<crate::models::User>,
    #[serde(default)]
    teams: Vec<crate::models::Team>,
    #[serde(default)]
    id_config: crate::models::IdConfiguration,
    #[serde(default)]
    features: Vec<crate::models::FeatureDefinition>,
    #[serde(default = "default_one")]
    next_feature_number: u32,
    #[serde(default = "default_one")]
    next_spec_number: u32,
    #[serde(default)]
    prefix_counters: std::collections::HashMap<String, u32>,
    #[serde(default)]
    relationship_definitions: Vec<crate::models::RelationshipDefinition>,
    #[serde(default)]
    reaction_definitions: Vec<crate::models::ReactionDefinition>,
    #[serde(default)]
    meta_counters: std::collections::HashMap<String, u32>,
    #[serde(default)]
    type_definitions: Vec<crate::models::CustomTypeDefinition>,
    #[serde(default)]
    allowed_prefixes: Vec<String>,
    #[serde(default)]
    restrict_prefixes: bool,
    #[serde(default)]
    ai_prompts: crate::models::AiPromptConfig,
    #[serde(default)]
    baselines: Vec<crate::models::Baseline>,
}

fn default_one() -> u32 {
    1
}

impl Default for StoreMetadata {
    fn default() -> Self {
        Self {
            name: String::new(),
            title: String::new(),
            description: String::new(),
            users: Vec::new(),
            teams: Vec::new(),
            id_config: crate::models::IdConfiguration::default(),
            features: Vec::new(),
            next_feature_number: 1,
            next_spec_number: 1,
            prefix_counters: std::collections::HashMap::new(),
            relationship_definitions: crate::models::RelationshipDefinition::defaults(),
            reaction_definitions: crate::models::default_reaction_definitions(),
            meta_counters: std::collections::HashMap::new(),
            type_definitions: crate::models::default_type_definitions(),
            allowed_prefixes: Vec::new(),
            restrict_prefixes: false,
            ai_prompts: crate::models::AiPromptConfig::default(),
            baselines: Vec::new(),
        }
    }
}

impl GitBackend {
    /// Create a new git backend rooted at the given directory.
    /// Creates the directory structure if it doesn't exist.
    pub fn new(root: &Path) -> Result<Self> {
        let objects_root = root.join("objects");
        let metadata_path = root.join("metadata.yaml");

        std::fs::create_dir_all(&objects_root)
            .with_context(|| format!("Failed to create objects dir: {}", objects_root.display()))?;

        Ok(Self {
            root: root.to_path_buf(),
            objects_root,
            metadata_path,
            dispenser: None,
            auto_commit: true,
            oplog_enabled: true,
        })
    }

    /// Set the dispenser for ID generation (distributed mode).
    pub fn with_dispenser(mut self, dispenser: DispenserHandle) -> Self {
        self.dispenser = Some(dispenser);
        self
    }

    /// Disable auto-commit (useful for batch operations or testing).
    pub fn with_auto_commit(mut self, enabled: bool) -> Self {
        self.auto_commit = enabled;
        self
    }

    /// Enable or disable the operation log.
    pub fn with_oplog(mut self, enabled: bool) -> Self {
        self.oplog_enabled = enabled;
        self
    }

    /// Record an operation in the append-only oplog.
    fn record_op(&self, target_id: uuid::Uuid, kind: crate::oplog::OpKind) {
        if !self.oplog_enabled {
            return;
        }
        let oplog_path = self.root.join("oplog.yaml");
        let node_id = self.dispenser.as_ref()
            .and_then(|d| d.state().ok())
            .map(|s| match s.mode {
                crate::dispenser::IdMode::Distributed { node_id } => node_id,
                _ => 0,
            })
            .unwrap_or(0);

        if let Ok(mut log) = crate::oplog::OpLog::load(&oplog_path) {
            if log.node_id == 0 && node_id > 0 {
                log.node_id = node_id;
            }
            log.append(target_id, "aida".into(), kind);
            let _ = log.save(&oplog_path);
        }
    }

    /// Stage all changes and commit to git if auto_commit is enabled.
    /// The commit message describes what changed.
    fn auto_commit(&self, message: &str) {
        if !self.auto_commit || !crate::git_ops::is_git_repo(&self.root) {
            return;
        }
        // Stage objects, metadata, and oplog
        let _ = crate::git_ops::add_all(&self.root, "objects");
        if self.metadata_path.exists() {
            let _ = crate::git_ops::add(&self.root, &["metadata.yaml"]);
        }
        if self.root.join("oplog.yaml").exists() {
            let _ = crate::git_ops::add(&self.root, &["oplog.yaml"]);
        }
        // Commit (silently ignore errors — git ops are best-effort)
        let _ = crate::git_ops::commit(&self.root, message);
    }

    /// Load metadata from the metadata.yaml file.
    fn load_metadata(&self) -> Result<StoreMetadata> {
        if !self.metadata_path.exists() {
            return Ok(StoreMetadata::default());
        }
        let content = std::fs::read_to_string(&self.metadata_path)
            .with_context(|| format!("Failed to read {}", self.metadata_path.display()))?;
        let meta: StoreMetadata = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse {}", self.metadata_path.display()))?;
        Ok(meta)
    }

    /// Save metadata to the metadata.yaml file.
    fn save_metadata(&self, meta: &StoreMetadata) -> Result<()> {
        let content = serde_yaml::to_string(meta)?;
        std::fs::write(&self.metadata_path, content)
            .with_context(|| format!("Failed to write {}", self.metadata_path.display()))?;
        Ok(())
    }

    /// Convert metadata + requirements into a full RequirementsStore.
    fn assemble_store(
        &self,
        meta: StoreMetadata,
        requirements: Vec<Requirement>,
    ) -> RequirementsStore {
        RequirementsStore {
            name: meta.name,
            title: meta.title,
            description: meta.description,
            requirements,
            users: meta.users,
            teams: meta.teams,
            id_config: meta.id_config,
            features: meta.features,
            next_feature_number: meta.next_feature_number,
            next_spec_number: meta.next_spec_number,
            prefix_counters: meta.prefix_counters,
            relationship_definitions: meta.relationship_definitions,
            reaction_definitions: meta.reaction_definitions,
            meta_counters: meta.meta_counters,
            type_definitions: meta.type_definitions,
            allowed_prefixes: meta.allowed_prefixes,
            restrict_prefixes: meta.restrict_prefixes,
            ai_prompts: meta.ai_prompts,
            baselines: meta.baselines,
            store_version: 0,
            migrated_to: None,
            dispenser: self.dispenser.clone(),
        }
    }

    /// Extract metadata from a RequirementsStore (everything except requirements).
    fn extract_metadata(store: &RequirementsStore) -> StoreMetadata {
        StoreMetadata {
            name: store.name.clone(),
            title: store.title.clone(),
            description: store.description.clone(),
            users: store.users.clone(),
            teams: store.teams.clone(),
            id_config: store.id_config.clone(),
            features: store.features.clone(),
            next_feature_number: store.next_feature_number,
            next_spec_number: store.next_spec_number,
            prefix_counters: store.prefix_counters.clone(),
            relationship_definitions: store.relationship_definitions.clone(),
            reaction_definitions: store.reaction_definitions.clone(),
            meta_counters: store.meta_counters.clone(),
            type_definitions: store.type_definitions.clone(),
            allowed_prefixes: store.allowed_prefixes.clone(),
            restrict_prefixes: store.restrict_prefixes,
            ai_prompts: store.ai_prompts.clone(),
            baselines: store.baselines.clone(),
        }
    }
}

impl DatabaseBackend for GitBackend {
    fn backend_type(&self) -> BackendType {
        BackendType::Git
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn load(&self) -> Result<RequirementsStore> {
        let meta = self.load_metadata()?;
        let requirements = object_store::load_all_objects(&self.objects_root)?;
        Ok(self.assemble_store(meta, requirements))
    }

    fn save(&self, store: &RequirementsStore) -> Result<()> {
        // Save metadata
        let meta = Self::extract_metadata(store);
        self.save_metadata(&meta)?;

        // Collect existing object files for deletion tracking
        let existing = object_store::list_objects(&self.objects_root)?;
        let existing_specs: std::collections::HashSet<String> =
            existing.iter().map(|(s, _)| s.clone()).collect();

        // Track which specs are in the current store
        let mut current_specs = std::collections::HashSet::new();

        // Write each requirement
        for req in &store.requirements {
            if let Some(ref spec_id) = req.spec_id {
                current_specs.insert(spec_id.clone());
                object_store::write_object(&self.objects_root, req)?;
            }
        }

        // Delete object files that are no longer in the store
        for spec_id in &existing_specs {
            if !current_specs.contains(spec_id) {
                let _ = object_store::delete_object(&self.objects_root, spec_id);
            }
        }

        self.auto_commit("chore: update requirements store");
        Ok(())
    }

    // Override individual CRUD for efficiency — don't reload everything each time

    fn get_requirement_by_spec_id(&self, spec_id: &str) -> Result<Option<Requirement>> {
        // Try direct lookup by spec_id (file name)
        if let Ok(req) = object_store::read_object(&self.objects_root, spec_id) {
            return Ok(Some(req));
        }
        // Fall back to scanning for agreed_id match
        let files = object_store::list_objects(&self.objects_root)?;
        for (_name, path) in &files {
            if let Ok(req) = object_store::read_object_from_path(path) {
                if req.agreed_id.as_deref() == Some(spec_id) {
                    return Ok(Some(req));
                }
            }
        }
        Ok(None)
    }

    fn get_requirement(&self, id: &uuid::Uuid) -> Result<Option<Requirement>> {
        object_store::find_by_uuid(&self.objects_root, id)
    }

    fn update_requirement(&self, requirement: &Requirement) -> Result<()> {
        let spec_id = requirement.spec_id.as_deref()
            .ok_or_else(|| anyhow::anyhow!("Cannot update requirement without spec_id in git backend"))?;

        // Record ops for changed fields (compare with existing if possible)
        if let Ok(old) = object_store::read_object(&self.objects_root, spec_id) {
            if old.title != requirement.title {
                self.record_op(requirement.id, crate::oplog::OpKind::SetTitle {
                    title: requirement.title.clone(),
                });
            }
            if old.effective_status() != requirement.effective_status() {
                self.record_op(requirement.id, crate::oplog::OpKind::SetStatus {
                    status: requirement.effective_status(),
                });
            }
            if old.effective_priority() != requirement.effective_priority() {
                self.record_op(requirement.id, crate::oplog::OpKind::SetPriority {
                    priority: requirement.effective_priority(),
                });
            }
            if old.owner != requirement.owner {
                self.record_op(requirement.id, crate::oplog::OpKind::SetOwner {
                    owner: requirement.owner.clone(),
                });
            }
            if old.description != requirement.description {
                self.record_op(requirement.id, crate::oplog::OpKind::SetDescription {
                    description: requirement.description.clone(),
                });
            }
        }

        object_store::write_object(&self.objects_root, requirement)?;
        self.auto_commit(&format!("update {}", spec_id));
        Ok(())
    }

    fn delete_requirement(&self, id: &uuid::Uuid) -> Result<()> {
        if let Some(req) = object_store::find_by_uuid(&self.objects_root, id)? {
            if let Some(ref spec_id) = req.spec_id {
                self.record_op(*id, crate::oplog::OpKind::Archive);
                object_store::delete_object(&self.objects_root, spec_id)?;
                self.auto_commit(&format!("delete {}", spec_id));
                return Ok(());
            }
        }
        anyhow::bail!("Requirement not found: {}", id)
    }

    fn add_requirement(&self, requirement: Requirement) -> Result<Requirement> {
        let mut req = requirement;

        if req.spec_id.is_none() {
            // Load metadata to get counters, assign ID, save metadata back
            let meta = self.load_metadata()?;
            let mut temp_store = self.assemble_store(meta, Vec::new());
            // Generate ID using the store's configured strategy
            let req_clone = req.clone();
            temp_store.add_requirement_with_id(req_clone, None, None);
            // The pushed req has the assigned spec_id
            if let Some(last) = temp_store.requirements.last() {
                req.spec_id = last.spec_id.clone();
            }
            // Persist the updated counters back to metadata
            let updated_meta = Self::extract_metadata(&temp_store);
            self.save_metadata(&updated_meta)?;
        }

        // Record create operation
        self.record_op(req.id, crate::oplog::OpKind::Create {
            title: req.title.clone(),
            description: req.description.clone(),
            req_type: format!("{:?}", req.req_type),
            status: req.effective_status(),
            priority: req.effective_priority(),
        });

        object_store::write_object(&self.objects_root, &req)?;
        let spec_id = req.spec_id.as_deref().unwrap_or("unknown");
        self.auto_commit(&format!("add {} — {}", spec_id, req.title));
        Ok(req)
    }

    fn exists(&self) -> bool {
        self.root.exists()
    }

    // Queue operations — stored as registry/queues/{user_id}.yaml

    fn queue_list(&self, user_id: &str, _include_completed: bool) -> Result<Vec<QueueEntry>> {
        let path = self.root.join("registry/queues").join(format!("{}.yaml", user_id));
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&path)?;
        let entries: Vec<QueueEntry> = serde_yaml::from_str(&content).unwrap_or_default();
        Ok(entries)
    }

    fn queue_add(&self, entry: QueueEntry) -> Result<()> {
        let dir = self.root.join("registry/queues");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.yaml", entry.user_id));
        let mut entries = if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            serde_yaml::from_str::<Vec<QueueEntry>>(&content).unwrap_or_default()
        } else {
            Vec::new()
        };
        // Upsert: replace if same requirement_id exists
        entries.retain(|e| e.requirement_id != entry.requirement_id);
        entries.push(entry);
        entries.sort_by_key(|e| e.position);
        let yaml = serde_yaml::to_string(&entries)?;
        std::fs::write(&path, yaml)?;
        self.auto_commit("update queue");
        Ok(())
    }

    fn queue_remove(&self, user_id: &str, requirement_id: &uuid::Uuid) -> Result<()> {
        let path = self.root.join("registry/queues").join(format!("{}.yaml", user_id));
        if !path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&path)?;
        let mut entries: Vec<QueueEntry> = serde_yaml::from_str(&content).unwrap_or_default();
        entries.retain(|e| e.requirement_id != *requirement_id);
        let yaml = serde_yaml::to_string(&entries)?;
        std::fs::write(&path, yaml)?;
        self.auto_commit("update queue");
        Ok(())
    }

    fn queue_reorder(&self, user_id: &str, items: &[(uuid::Uuid, i64)]) -> Result<()> {
        let path = self.root.join("registry/queues").join(format!("{}.yaml", user_id));
        if !path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&path)?;
        let mut entries: Vec<QueueEntry> = serde_yaml::from_str(&content).unwrap_or_default();
        for (id, pos) in items {
            if let Some(entry) = entries.iter_mut().find(|e| e.requirement_id == *id) {
                entry.position = *pos;
            }
        }
        entries.sort_by_key(|e| e.position);
        let yaml = serde_yaml::to_string(&entries)?;
        std::fs::write(&path, yaml)?;
        self.auto_commit("reorder queue");
        Ok(())
    }

    fn queue_clear(&self, user_id: &str, _completed_only: bool) -> Result<()> {
        let path = self.root.join("registry/queues").join(format!("{}.yaml", user_id));
        if path.exists() {
            std::fs::remove_file(&path)?;
            self.auto_commit("clear queue");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_backend_create_and_load_empty() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");

        let backend = GitBackend::new(&root).unwrap();
        let store = backend.load().unwrap();
        assert_eq!(store.requirements.len(), 0);
        assert!(store.name.is_empty());
    }

    #[test]
    fn test_git_backend_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        // Create a store with some data
        let mut store = RequirementsStore::new();
        store.name = "Test Project".into();
        store.title = "My Test".into();

        let mut req1 = Requirement::new("First Req".into(), "Description 1".into());
        req1.spec_id = Some("FR-001".into());

        let mut req2 = Requirement::new("Second Req".into(), "Description 2".into());
        req2.spec_id = Some("BUG-001".into());

        store.requirements.push(req1);
        store.requirements.push(req2);

        backend.save(&store).unwrap();

        // Load and verify
        let loaded = backend.load().unwrap();
        assert_eq!(loaded.name, "Test Project");
        assert_eq!(loaded.requirements.len(), 2);

        // Verify files exist in sharded layout
        assert!(root.join("objects/FR/000/FR-001.yaml").exists());
        assert!(root.join("objects/BUG/000/BUG-001.yaml").exists());
        assert!(root.join("metadata.yaml").exists());
    }

    #[test]
    fn test_git_backend_crud() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        // Add
        let mut req = Requirement::new("CRUD Test".into(), "testing".into());
        req.spec_id = Some("FR-042".into());
        let added = backend.add_requirement(req.clone()).unwrap();
        assert_eq!(added.spec_id, Some("FR-042".into()));

        // Read by spec_id
        let found = backend.get_requirement_by_spec_id("FR-042").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().title, "CRUD Test");

        // Read by UUID
        let found = backend.get_requirement(&added.id).unwrap();
        assert!(found.is_some());

        // Update
        let mut updated = added.clone();
        updated.title = "Updated Title".into();
        backend.update_requirement(&updated).unwrap();

        let reloaded = backend.get_requirement_by_spec_id("FR-042").unwrap().unwrap();
        assert_eq!(reloaded.title, "Updated Title");

        // Delete
        backend.delete_requirement(&added.id).unwrap();
        let gone = backend.get_requirement_by_spec_id("FR-042").unwrap();
        assert!(gone.is_none());
    }

    #[test]
    fn test_git_backend_auto_assign_id() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        // Save initial metadata
        backend.save(&RequirementsStore::new()).unwrap();

        // Add requirement without spec_id — should auto-assign
        let req = Requirement::new("Auto ID".into(), "should get an ID".into());
        let added = backend.add_requirement(req).unwrap();
        assert!(added.spec_id.is_some());

        // Verify it's readable
        let spec_id = added.spec_id.as_ref().unwrap();
        let found = backend.get_requirement_by_spec_id(spec_id).unwrap();
        assert!(found.is_some());
    }

    #[test]
    fn test_git_backend_delete_removes_orphan_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        // Save with 2 requirements
        let mut store = RequirementsStore::new();
        let mut req1 = Requirement::new("Keep".into(), "kept".into());
        req1.spec_id = Some("FR-001".into());
        let mut req2 = Requirement::new("Remove".into(), "removed".into());
        req2.spec_id = Some("FR-002".into());
        store.requirements.push(req1);
        store.requirements.push(req2);
        backend.save(&store).unwrap();

        assert!(root.join("objects/FR/000/FR-001.yaml").exists());
        assert!(root.join("objects/FR/000/FR-002.yaml").exists());

        // Save again with only 1 requirement — FR-002 should be deleted
        store.requirements.retain(|r| r.spec_id.as_deref() == Some("FR-001"));
        backend.save(&store).unwrap();

        assert!(root.join("objects/FR/000/FR-001.yaml").exists());
        assert!(!root.join("objects/FR/000/FR-002.yaml").exists());
    }

    #[test]
    fn test_git_backend_with_dispenser() {
        use crate::dispenser::{IdMode, MemoryDispenser};
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");

        let dispenser = Arc::new(MemoryDispenser::new(IdMode::Distributed { node_id: 7 }));
        let handle = DispenserHandle(dispenser);
        let backend = GitBackend::new(&root).unwrap().with_dispenser(handle);

        // Save initial metadata
        backend.save(&RequirementsStore::new()).unwrap();

        // The dispenser should be injected into loaded stores
        let store = backend.load().unwrap();
        assert!(store.dispenser.is_some());
    }
}
