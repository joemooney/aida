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

use super::traits::{BackendType, DatabaseBackend};
use crate::models::{
    DispenserHandle, QueueEntry, Requirement, RequirementStatus, RequirementsStore,
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
        let node_id: String = self
            .dispenser
            .as_ref()
            .and_then(|d| d.state().ok())
            .map(|s| match s.mode {
                crate::dispenser::IdMode::Distributed { node_id } => node_id,
                _ => "0".to_string(),
            })
            .unwrap_or_else(|| "0".to_string());

        if let Ok(mut log) = crate::oplog::OpLog::load(&oplog_path) {
            if log.node_id == "0" && node_id != "0" {
                log.node_id = node_id;
            }
            log.append(target_id, "aida".into(), kind);
            let _ = log.save(&oplog_path);
        }
    }

    /// Stage every YAML the backend is responsible for and commit. Used by
    /// the bulk `save()` path that legitimately touches the entire object
    /// tree. For per-operation paths, prefer `auto_commit_paths` so we don't
    /// pull in unrelated drift from sibling files.
    /// trace:BUG-1-040 | ai:claude
    fn auto_commit(&self, message: &str) {
        if !self.auto_commit || !crate::git_ops::is_git_repo(&self.root) {
            return;
        }
        let _ = crate::git_ops::add_all(&self.root, "objects");
        if self.metadata_path.exists() {
            let _ = crate::git_ops::add(&self.root, &["metadata.yaml"]);
        }
        if self.root.join("oplog.yaml").exists() {
            let _ = crate::git_ops::add(&self.root, &["oplog.yaml"]);
        }
        let _ = crate::git_ops::commit(&self.root, message);
    }

    /// Stage only the listed paths (relative to repo root) and commit. Skips
    /// the commit when nothing is staged so no-op operations don't churn
    /// commit history. Use `git add -A <path>` semantics so deletions are
    /// captured along with adds/modifications.
    /// trace:BUG-1-040 | ai:claude
    fn auto_commit_paths(&self, message: &str, paths: &[&str]) {
        if !self.auto_commit || !crate::git_ops::is_git_repo(&self.root) {
            return;
        }
        for p in paths {
            // `git add -A <path>` so a removed file is staged as a deletion
            // (plain `git add` only handles add/modify).
            let _ = std::process::Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(["add", "-A", p])
                .output();
        }
        // Always include oplog.yaml when present — every targeted op records
        // an op and we want it captured in the same commit.
        if self.root.join("oplog.yaml").exists() {
            let _ = crate::git_ops::add(&self.root, &["oplog.yaml"]);
        }
        // Don't make an empty commit if nothing was actually staged.
        let staged = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(["diff", "--cached", "--quiet"])
            .status()
            .map(|s| !s.success()) // non-zero exit means there ARE staged changes
            .unwrap_or(false);
        if staged {
            let _ = crate::git_ops::commit(&self.root, message);
        }
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

    /// Begin a bulk-add session. Buffers new requirements in memory while
    /// assigning IDs via the live metadata counters, then flushes all YAMLs
    /// + a counter-bumped metadata.yaml in a single commit when `finish` is
    ///   called. Avoids the load-iterate-write pattern of `update_atomically`
    /// + `save`, which is overkill when the operation is purely additive.
    ///   trace:FR-1-002 | ai:claude
    pub fn bulk_writer(&self) -> Result<BulkWriter<'_>> {
        let metadata = self.load_metadata()?;
        Ok(BulkWriter {
            backend: self,
            metadata,
            staged: Vec::new(),
        })
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

    /// Record the granular field ops for one requirement update and write its
    /// YAML — WITHOUT committing. Returns `Some(spec_id)` when the on-disk YAML
    /// actually changed (so the caller can stage + commit it), `None` when it
    /// was already up to date. Single source of truth shared by
    /// `update_requirement` (commits the one path) and `bulk_update` (batches
    /// many writes into one commit). trace:BUG-425 | ai:claude
    fn stage_requirement_update<'a>(
        &self,
        requirement: &'a Requirement,
    ) -> Result<Option<&'a str>> {
        let spec_id = requirement.spec_id.as_deref().ok_or_else(|| {
            anyhow::anyhow!("Cannot update requirement without spec_id in git backend")
        })?;

        // Record ops for changed fields (compare with existing if possible).
        if let Ok(old) = object_store::read_object(&self.objects_root, spec_id) {
            if old.title != requirement.title {
                self.record_op(
                    requirement.id,
                    crate::oplog::OpKind::SetTitle {
                        title: requirement.title.clone(),
                    },
                );
            }
            if old.effective_status() != requirement.effective_status() {
                self.record_op(
                    requirement.id,
                    crate::oplog::OpKind::SetStatus {
                        status: requirement.effective_status(),
                    },
                );
            }
            if old.effective_priority() != requirement.effective_priority() {
                self.record_op(
                    requirement.id,
                    crate::oplog::OpKind::SetPriority {
                        priority: requirement.effective_priority(),
                    },
                );
            }
            if old.owner != requirement.owner {
                self.record_op(
                    requirement.id,
                    crate::oplog::OpKind::SetOwner {
                        owner: requirement.owner.clone(),
                    },
                );
            }
            if old.description != requirement.description {
                self.record_op(
                    requirement.id,
                    crate::oplog::OpKind::SetDescription {
                        description: requirement.description.clone(),
                    },
                );
            }
        }

        let wrote = object_store::write_object_if_changed(&self.objects_root, requirement)?;
        Ok(if wrote { Some(spec_id) } else { None })
    }

    /// Apply field updates to many existing requirements in a SINGLE git commit
    /// — vs `update_requirement`'s one-commit-per-spec, which turns a bulk
    /// operation (e.g. `aida archive --older-than`) into hundreds of commits
    /// (BUG-425: a 679-spec sweep made 679 commits). Records exactly the same
    /// granular field ops `update_requirement` does (via the shared
    /// `stage_requirement_update`), so the oplog and per-spec YAML history stay
    /// faithful; only the commit is batched. Returns the count whose on-disk
    /// YAML actually changed (unchanged reqs are skipped, so re-running is a
    /// no-op). trace:BUG-425 | ai:claude
    pub fn bulk_update(&self, requirements: &[Requirement], commit_subject: &str) -> Result<usize> {
        let mut changed: Vec<String> = Vec::new();
        for requirement in requirements {
            if let Some(spec_id) = self.stage_requirement_update(requirement)? {
                changed.push(spec_id.to_string());
            }
        }
        if changed.is_empty() {
            return Ok(0);
        }
        let paths: Vec<String> = changed
            .iter()
            .filter_map(|sid| object_store::relative_object_path(sid).ok())
            .collect();
        let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
        let n = changed.len();
        let message = format!(
            "{}: update {} requirement{}",
            commit_subject,
            n,
            if n == 1 { "" } else { "s" }
        );
        self.auto_commit_paths(&message, &path_refs);
        Ok(n)
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

        // Track which specs are in the current store, plus the subset we
        // actually had to write. With deterministic serde the on-disk YAML
        // for an unchanged requirement matches what we'd serialize, so we
        // compare-then-skip to avoid spurious writes (and the noisy commits
        // they produce). trace:BUG-1-040 | ai:claude
        let mut current_specs = std::collections::HashSet::new();
        let mut written_specs: Vec<String> = Vec::new();
        for req in &store.requirements {
            if let Some(ref spec_id) = req.spec_id {
                current_specs.insert(spec_id.clone());
                if object_store::write_object_if_changed(&self.objects_root, req)? {
                    written_specs.push(spec_id.clone());
                }
            }
        }

        // Delete object files that are no longer in the store.
        //
        // Safety: never delete a file we couldn't parse. `current_specs` is
        // built from the in-memory store, which `load()` populates by
        // skipping-and-warning on parse failures (see
        // `object_store::load_all_objects`). An unparseable file is therefore
        // absent from `current_specs` *not because the user deleted it* but
        // because this binary couldn't read it — typically because a newer
        // binary wrote a serde variant this one doesn't recognize. Deleting
        // here silently destroys the other binary's work and is the
        // exact failure mode BUG-96 documents (incident 2026-05-13: six
        // STORY/VIS/CON/ADR/PRIN/TERM files removed by a single
        // `aida add`). Skip-and-warn mirrors the load-side policy so the
        // file survives until a binary that *can* parse it runs.
        // trace:BUG-96 | ai:claude
        let mut deleted_specs: Vec<String> = Vec::new();
        let mut preserved_unparseable: Vec<String> = Vec::new();
        for spec_id in &existing_specs {
            if !current_specs.contains(spec_id) {
                if object_store::read_object(&self.objects_root, spec_id).is_err() {
                    preserved_unparseable.push(spec_id.clone());
                    continue;
                }
                let _ = object_store::delete_object(&self.objects_root, spec_id);
                deleted_specs.push(spec_id.clone());
            }
        }
        if !preserved_unparseable.is_empty() {
            eprintln!(
                "Warning: preserved {} unparseable object file(s) during save: {} \
                 (a binary that can parse them will pick them up)",
                preserved_unparseable.len(),
                preserved_unparseable.join(", ")
            );
        }

        // Pick a commit message that reflects what actually changed instead
        // of the legacy generic "chore: update requirements store" used even
        // when one file moved. trace:BUG-1-040 | ai:claude
        let message = match (written_specs.len(), deleted_specs.len()) {
            (0, 0) => "chore: refresh requirements store metadata".to_string(),
            (1, 0) => format!("update {}", written_specs[0]),
            (0, 1) => format!("delete {}", deleted_specs[0]),
            (w, 0) => format!("chore: update {} requirements", w),
            (0, d) => format!("chore: delete {} requirements", d),
            (w, d) => format!("chore: update {} requirements, delete {}", w, d),
        };
        self.auto_commit(&message);
        Ok(())
    }

    // Override individual CRUD for efficiency — don't reload everything each time

    fn get_requirement_by_spec_id(&self, spec_id: &str) -> Result<Option<Requirement>> {
        let spec_id = object_store::canonical_spec_id(spec_id);
        // BUG-97: distinguish "file doesn't exist" (legitimate not-found,
        // fall through to agreed_id scan and ultimately Ok(None)) from
        // "file exists but failed to parse" (propagate the parse error so
        // the caller surfaces the real cause instead of a misleading
        // "Requirement not found"). Without this split, both cases ended
        // up at the agreed_id scan, then Ok(None), then the caller's
        // "Requirement not found" hint — sending the user down a
        // wrong-spec-id investigation when the YAML was actually fine,
        // just incompatible with the binary's enums.
        // trace:BUG-97 | ai:claude
        if object_store::object_exists(&self.objects_root, &spec_id)? {
            // File exists; any error from read_object now is a parse
            // failure worth propagating (with the file path + serde
            // detail already attached by read_object's `with_context`).
            let req = object_store::read_object(&self.objects_root, &spec_id)?;
            return Ok(Some(req));
        }
        // File doesn't exist by spec_id — try agreed_id scan.
        let files = object_store::list_objects(&self.objects_root)?;
        for (_name, path) in &files {
            if let Ok(req) = object_store::read_object_from_path(path) {
                if req.agreed_id.as_deref() == Some(spec_id.as_str()) {
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
        // Record granular field ops + write the YAML via the shared helper,
        // then targeted-commit only the one YAML this op touched (when it
        // actually changed). The op-recording logic lives in
        // `stage_requirement_update` so `bulk_update` stays faithful to it.
        // trace:BUG-1-040 trace:BUG-425 | ai:claude
        if let Some(spec_id) = self.stage_requirement_update(requirement)? {
            let rel = object_store::relative_object_path(spec_id)?;
            self.auto_commit_paths(&format!("update {}", spec_id), &[&rel]);
        }
        Ok(())
    }

    fn delete_requirement(&self, id: &uuid::Uuid) -> Result<()> {
        if let Some(req) = object_store::find_by_uuid(&self.objects_root, id)? {
            if let Some(ref spec_id) = req.spec_id {
                self.record_op(*id, crate::oplog::OpKind::Archive);
                object_store::delete_object(&self.objects_root, spec_id)?;
                let rel = object_store::relative_object_path(spec_id)?;
                // trace:BUG-1-040 | ai:claude
                self.auto_commit_paths(&format!("delete {}", spec_id), &[&rel]);
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
        self.record_op(
            req.id,
            crate::oplog::OpKind::Create {
                title: req.title.clone(),
                description: req.description.clone(),
                req_type: format!("{:?}", req.req_type),
                status: req.effective_status(),
                priority: req.effective_priority(),
            },
        );

        object_store::write_object(&self.objects_root, &req)?;
        let spec_id = req.spec_id.as_deref().unwrap_or("unknown");
        // Targeted stage: only the new YAML + metadata.yaml (counters bumped).
        // trace:BUG-1-040 | ai:claude
        if let Ok(rel) = object_store::relative_object_path(spec_id) {
            self.auto_commit_paths(
                &format!("add {} — {}", spec_id, req.title),
                &[&rel, "metadata.yaml"],
            );
        }
        Ok(req)
    }

    fn exists(&self) -> bool {
        self.root.exists()
    }

    // Queue operations — stored as registry/queues/{user_id}.yaml

    fn queue_list(&self, user_id: &str, _include_completed: bool) -> Result<Vec<QueueEntry>> {
        let path = self
            .root
            .join("registry/queues")
            .join(format!("{}.yaml", user_id));
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
        let user_id = entry.user_id.clone();
        let path = dir.join(format!("{}.yaml", user_id));
        let mut entries = if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            serde_yaml::from_str::<Vec<QueueEntry>>(&content).unwrap_or_default()
        } else {
            Vec::new()
        };
        // Upsert: replace if same requirement_id exists.
        entries.retain(|e| e.requirement_id != entry.requirement_id);

        // STORY-72: callers pass `i64::MAX` as a "append to bottom"
        // sentinel (queue add path in aida-cli) and expect the backend to
        // resolve it to `existing_max + 1000`. Earlier this path stored
        // the sentinel as-is, so every item ended up with `position:
        // i64::MAX` and any subsequent reorder math (`--before`, the new
        // `--after`) either silently no-op'd or overflowed. Resolve here
        // so the on-disk YAML always carries real positions.
        // trace:STORY-72 | ai:claude
        let mut entry = entry;
        if entry.position == i64::MAX {
            let max_existing = entries
                .iter()
                .filter(|e| e.position != i64::MAX)
                .map(|e| e.position)
                .max()
                .unwrap_or(0);
            entry.position = max_existing.saturating_add(1000);
        }
        entries.push(entry);
        entries.sort_by_key(|e| e.position);
        let yaml = serde_yaml::to_string(&entries)?;
        std::fs::write(&path, yaml)?;
        self.auto_commit_paths(
            "update queue",
            &[&format!("registry/queues/{}.yaml", user_id)],
        );
        Ok(())
    }

    fn queue_remove(&self, user_id: &str, requirement_id: &uuid::Uuid) -> Result<()> {
        let path = self
            .root
            .join("registry/queues")
            .join(format!("{}.yaml", user_id));
        if !path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(&path)?;
        let mut entries: Vec<QueueEntry> = serde_yaml::from_str(&content).unwrap_or_default();
        entries.retain(|e| e.requirement_id != *requirement_id);
        let yaml = serde_yaml::to_string(&entries)?;
        std::fs::write(&path, yaml)?;
        self.auto_commit_paths(
            "update queue",
            &[&format!("registry/queues/{}.yaml", user_id)],
        );
        Ok(())
    }

    fn queue_reorder(&self, user_id: &str, items: &[(uuid::Uuid, i64)]) -> Result<()> {
        let path = self
            .root
            .join("registry/queues")
            .join(format!("{}.yaml", user_id));
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
        self.auto_commit_paths(
            "reorder queue",
            &[&format!("registry/queues/{}.yaml", user_id)],
        );
        Ok(())
    }

    // TASK-1-109: --completed used to be a no-op on the git backend
    // (parameter was named _completed_only, indicating intentional
    // ignore). Implementing per the sqlite_backend semantics — when
    // completed_only is true, keep entries whose backing spec is NOT
    // Completed; remove only the ones whose spec is. Orphan entries
    // (backing spec deleted) are left in place; use
    // `aida queue prune --orphaned` (TASK-537) for those. Failure to
    // look up a requirement (transient I/O error) errs on the safe
    // side: keep the entry. trace:TASK-1-109 | ai:claude
    fn queue_clear(&self, user_id: &str, completed_only: bool) -> Result<()> {
        let path = self
            .root
            .join("registry/queues")
            .join(format!("{}.yaml", user_id));
        if !path.exists() {
            return Ok(());
        }

        if !completed_only {
            // Original behavior: nuke the entire queue file.
            std::fs::remove_file(&path)?;
            self.auto_commit_paths(
                "clear queue",
                &[&format!("registry/queues/{}.yaml", user_id)],
            );
            return Ok(());
        }

        // --completed: filter entries by backing spec status.
        let content = std::fs::read_to_string(&path)?;
        let entries: Vec<QueueEntry> = serde_yaml::from_str(&content).unwrap_or_default();
        let original_len = entries.len();
        let kept: Vec<QueueEntry> = entries
            .into_iter()
            .filter(|entry| {
                // Keep unless the spec exists AND is Completed.
                // Orphan or read error → keep (let prune handle those).
                match self.get_requirement(&entry.requirement_id) {
                    Ok(Some(req)) => !matches!(req.status, RequirementStatus::Completed),
                    _ => true,
                }
            })
            .collect();

        if kept.len() == original_len {
            // No completed entries to remove — nothing to do.
            return Ok(());
        }

        if kept.is_empty() {
            // Every queued entry's backing spec is Completed — delete
            // the file (matches the full-clear behavior).
            std::fs::remove_file(&path)?;
        } else {
            let yaml = serde_yaml::to_string(&kept)?;
            std::fs::write(&path, yaml)?;
        }

        self.auto_commit_paths(
            "clear completed queue entries",
            &[&format!("registry/queues/{}.yaml", user_id)],
        );
        Ok(())
    }
}

/// Buffers a batch of new requirements for write-behind commit. Created via
/// `GitBackend::bulk_writer()`. Use `add()` per requirement (IDs are assigned
/// from the live metadata counters), then `finish()` to write all YAMLs +
/// metadata.yaml and produce a single commit.
///
/// Use this for purely-additive bulk paths (Jira/GitHub/JSON imports) where
/// `update_atomically(|store| { for ... { store.add_requirement_with_id(...) } })`
/// pulls every existing requirement into memory just to ignore it.
///
/// trace:FR-1-002 | ai:claude
pub struct BulkWriter<'a> {
    backend: &'a GitBackend,
    metadata: StoreMetadata,
    staged: Vec<Requirement>,
}

impl<'a> BulkWriter<'a> {
    /// Add a requirement to the batch, assigning a spec_id from the live
    /// metadata counters when missing. The requirement is buffered in memory;
    /// nothing is written to disk until `finish()` is called.
    pub fn add(&mut self, mut req: Requirement) -> Result<&Requirement> {
        if req.spec_id.is_none() {
            // Reuse RequirementsStore::add_requirement_with_id by building a
            // throwaway store carrying the live metadata; the `staged` Vec
            // contains in-flight specs so the counter advances correctly.
            let mut tmp = self
                .backend
                .assemble_store(self.metadata.clone(), Vec::new());
            tmp.requirements = self.staged.clone();
            let type_prefix = tmp.get_type_prefix(&req.req_type);
            let req_clone = req.clone();
            tmp.add_requirement_with_id(req_clone, None, type_prefix.as_deref());
            if let Some(last) = tmp.requirements.last() {
                req.spec_id = last.spec_id.clone();
            }
            // Pull the bumped counters back into our metadata snapshot so the
            // next add() sees them. Cheaper than re-extracting the whole.
            self.metadata.next_feature_number = tmp.next_feature_number;
            self.metadata.next_spec_number = tmp.next_spec_number;
            self.metadata.prefix_counters = tmp.prefix_counters.clone();
            self.metadata.meta_counters = tmp.meta_counters.clone();
        }
        self.staged.push(req);
        Ok(self.staged.last().unwrap())
    }

    /// Number of requirements buffered in this batch.
    pub fn len(&self) -> usize {
        self.staged.len()
    }

    /// True when no requirements have been buffered yet.
    pub fn is_empty(&self) -> bool {
        self.staged.is_empty()
    }

    /// Flush all buffered requirements to disk and produce a single commit.
    /// Returns the number of requirements written. The commit message is
    /// "{prefix}: import N requirements" — pass a context-specific prefix
    /// like "chore" or "feat(jira)".
    pub fn finish(self, commit_subject: &str) -> Result<usize> {
        // Persist all object YAMLs first (skipping unchanged just in case the
        // caller is re-running an idempotent import).
        let mut written: Vec<String> = Vec::new();
        for req in &self.staged {
            if object_store::write_object_if_changed(&self.backend.objects_root, req)? {
                if let Some(spec) = &req.spec_id {
                    written.push(spec.clone());
                }
            }
            // Record the create op for each new requirement (matches
            // GitBackend::add_requirement's bookkeeping)
            self.backend.record_op(
                req.id,
                crate::oplog::OpKind::Create {
                    title: req.title.clone(),
                    description: req.description.clone(),
                    req_type: format!("{:?}", req.req_type),
                    status: req.effective_status(),
                    priority: req.effective_priority(),
                },
            );
        }

        // Persist the bumped metadata
        self.backend.save_metadata(&self.metadata)?;

        // Stage only the written YAMLs + metadata.yaml + oplog.yaml in one shot
        let mut paths: Vec<String> = written
            .iter()
            .filter_map(|sid| object_store::relative_object_path(sid).ok())
            .collect();
        paths.push("metadata.yaml".to_string());
        let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();

        let count = self.staged.len();
        let message = format!(
            "{}: import {} requirement{}",
            commit_subject,
            count,
            if count == 1 { "" } else { "s" }
        );
        self.backend.auto_commit_paths(&message, &path_refs);

        Ok(count)
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
    fn test_bulk_writer_imports_in_one_batch() {
        // trace:FR-1-002 | ai:claude
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        // Seed the metadata so add_requirement_with_id has a starting counter.
        let mut seed_store = RequirementsStore::new();
        seed_store.name = "BulkWriterTest".into();
        backend.save(&seed_store).unwrap();

        // Drain three new reqs through BulkWriter in one go.
        let mut writer = backend.bulk_writer().unwrap();
        for i in 1..=3 {
            let req = Requirement::new(format!("Bulk req {}", i), format!("Description {}", i));
            writer.add(req).unwrap();
        }
        assert_eq!(writer.len(), 3);
        let n = writer.finish("test(bulk)").unwrap();
        assert_eq!(n, 3);

        // All three YAMLs landed.
        let loaded = backend.load().unwrap();
        let bulk_titles: Vec<&str> = loaded
            .requirements
            .iter()
            .filter(|r| r.title.starts_with("Bulk req"))
            .map(|r| r.title.as_str())
            .collect();
        assert_eq!(bulk_titles.len(), 3);

        // Each got a unique spec_id (counter advanced inside the batch).
        let spec_ids: std::collections::HashSet<&str> = loaded
            .requirements
            .iter()
            .filter(|r| r.title.starts_with("Bulk req"))
            .filter_map(|r| r.spec_id.as_deref())
            .collect();
        assert_eq!(
            spec_ids.len(),
            3,
            "each bulk req must get a distinct spec_id"
        );
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

        let reloaded = backend
            .get_requirement_by_spec_id("FR-042")
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.title, "Updated Title");

        // Delete
        backend.delete_requirement(&added.id).unwrap();
        let gone = backend.get_requirement_by_spec_id("FR-042").unwrap();
        assert!(gone.is_none());
    }

    /// BUG-425: bulk_update commits all changed YAMLs in exactly ONE commit
    /// (the whole point — vs update_requirement's one-commit-per-spec that
    /// turned a 679-spec archive sweep into 679 commits).
    #[test]
    fn bulk_update_writes_all_changed_in_a_single_commit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();
        backend.save(&RequirementsStore::new()).unwrap();

        // GitBackend::new doesn't git-init, and auto_commit_paths no-ops
        // outside a git repo — so make the store a real repo, else there are
        // no commits to count. (This is the property under test.)
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);

        // Seed three requirements (each auto-commits once now that it's a repo).
        let mut reqs = Vec::new();
        for i in 1..=3 {
            let r = Requirement::new(format!("Bulk update {i}"), format!("desc {i}"));
            reqs.push(backend.add_requirement(r).unwrap());
        }

        let count_commits = || -> usize {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["rev-list", "--count", "HEAD"])
                .output()
                .unwrap();
            String::from_utf8(out.stdout)
                .unwrap()
                .trim()
                .parse()
                .unwrap()
        };
        let before = count_commits();

        // Archive all three and commit them as one batch.
        for r in &mut reqs {
            r.archived = true;
        }
        let n = backend.bulk_update(&reqs, "chore(archive)").unwrap();
        assert_eq!(n, 3, "all three YAMLs changed");

        assert_eq!(
            count_commits(),
            before + 1,
            "bulk_update must produce exactly ONE commit, not one per spec"
        );

        // Changes persisted to disk.
        let loaded = backend.load().unwrap();
        assert_eq!(loaded.requirements.iter().filter(|r| r.archived).count(), 3);

        // Re-running with the same (now-unchanged) reqs is a no-op: nothing
        // changed on disk → no new commit.
        let n2 = backend.bulk_update(&reqs, "chore(archive)").unwrap();
        assert_eq!(n2, 0, "unchanged reqs write nothing");
        assert_eq!(
            count_commits(),
            before + 1,
            "no-op bulk_update must not add an empty commit"
        );
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
        store
            .requirements
            .retain(|r| r.spec_id.as_deref() == Some("FR-001"));
        backend.save(&store).unwrap();

        assert!(root.join("objects/FR/000/FR-001.yaml").exists());
        assert!(!root.join("objects/FR/000/FR-002.yaml").exists());
    }

    /// BUG-96: a bulk `save()` must NEVER delete a YAML file just because
    /// `load_all_objects` couldn't parse it. The failure mode this prevents
    /// is a newer binary writing a serde variant the current binary lacks;
    /// without this guard, the next save sweeps the unrecognized file out
    /// of the store and the work is lost. trace:BUG-96 | ai:claude
    #[test]
    fn test_save_preserves_unparseable_object_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        // Seed: one parseable spec written via the normal path.
        let mut keeper = Requirement::new("Keeper".into(), "kept".into());
        keeper.spec_id = Some("FR-001".into());
        let mut store = RequirementsStore::new();
        store.requirements.push(keeper.clone());
        backend.save(&store).unwrap();

        // Drop in an unparseable file as if a newer binary wrote it. The
        // path matches the sharded layout so `list_objects` will pick it up
        // but `read_object` will fail. The bug we're guarding against is
        // `save()` deleting this file because it isn't in `current_specs`.
        let unparseable_path = root.join("objects/STORY/000/STORY-86.yaml");
        std::fs::create_dir_all(unparseable_path.parent().unwrap()).unwrap();
        std::fs::write(
            &unparseable_path,
            "id: 019e21ea-e521-76e2-9b34-52626ea25a4b\n\
             status: !FromTheFutureVariant\n\
             title: a story from a newer binary\n",
        )
        .unwrap();
        assert!(unparseable_path.exists());

        // Trigger the bulk save path. The current store still contains
        // only FR-001; STORY-86 is "missing" from the store from save()'s
        // perspective — exactly the condition that used to delete the file.
        backend.save(&store).unwrap();

        // The keeper must still be on disk.
        assert!(root.join("objects/FR/000/FR-001.yaml").exists());

        // The unparseable file must survive — this is the BUG-96 guarantee.
        assert!(
            unparseable_path.exists(),
            "save() deleted an unparseable file (BUG-96 regression)"
        );
    }

    /// TASK-1-109: queue_clear's completed_only flag was a no-op on this
    /// backend; now it filters queue entries by their backing requirement's
    /// status, removing only the entries whose spec is Completed.
    /// trace:TASK-1-109 | ai:claude
    #[test]
    fn queue_clear_completed_only_filters_by_backing_spec_status() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        // Set up two requirements: one Completed, one Approved.
        let mut store = RequirementsStore::new();
        let mut done_req = Requirement::new("done work".into(), "shipped".into());
        done_req.spec_id = Some("TASK-1".into());
        done_req.status = RequirementStatus::Completed;
        let mut todo_req = Requirement::new("pending work".into(), "still queued".into());
        todo_req.spec_id = Some("TASK-2".into());
        todo_req.status = RequirementStatus::Approved;
        let done_id = done_req.id;
        let todo_id = todo_req.id;
        store.requirements.push(done_req);
        store.requirements.push(todo_req);
        backend.save(&store).unwrap();

        // Queue both requirements for the same user.
        let user = "alice";
        backend
            .queue_add(QueueEntry {
                user_id: user.into(),
                requirement_id: done_id,
                position: 0,
                added_by: user.into(),
                note: None,
                added_at: chrono::Utc::now(),
                for_role: None,
                for_scope: None,
                for_session: None,
                added_by_machine: None,
            })
            .unwrap();
        backend
            .queue_add(QueueEntry {
                user_id: user.into(),
                requirement_id: todo_id,
                position: 1,
                added_by: user.into(),
                note: None,
                added_at: chrono::Utc::now(),
                for_role: None,
                for_scope: None,
                for_session: None,
                added_by_machine: None,
            })
            .unwrap();
        let before = backend.queue_list(user, true).unwrap();
        assert_eq!(before.len(), 2, "both entries should be queued");

        // Run --completed clear.
        backend
            .queue_clear(user, /* completed_only */ true)
            .unwrap();

        // The Completed-backed entry should be gone; the Approved-backed
        // entry should still be queued.
        let after = backend.queue_list(user, true).unwrap();
        assert_eq!(
            after.len(),
            1,
            "only the Completed-backed entry should be removed"
        );
        assert_eq!(
            after[0].requirement_id, todo_id,
            "the surviving entry should reference the still-Approved requirement"
        );
    }

    /// trace:TASK-1-109 | ai:claude
    #[test]
    fn queue_clear_without_completed_flag_wipes_everything() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        let mut store = RequirementsStore::new();
        let mut r = Requirement::new("any work".into(), "queued".into());
        r.spec_id = Some("TASK-1".into());
        r.status = RequirementStatus::Approved;
        let req_id = r.id;
        store.requirements.push(r);
        backend.save(&store).unwrap();

        let user = "bob";
        backend
            .queue_add(QueueEntry {
                user_id: user.into(),
                requirement_id: req_id,
                position: 0,
                added_by: user.into(),
                note: None,
                added_at: chrono::Utc::now(),
                for_role: None,
                for_scope: None,
                for_session: None,
                added_by_machine: None,
            })
            .unwrap();

        backend
            .queue_clear(user, /* completed_only */ false)
            .unwrap();

        let after = backend.queue_list(user, true).unwrap();
        assert!(
            after.is_empty(),
            "bare queue_clear should remove every entry regardless of status"
        );
    }

    /// BUG-96: even when `save()` is also writing a brand-new add, an
    /// unparseable neighbour must survive. The 2026-05-13 incident was
    /// exactly this shape: `aida add` of TASK-0396 deleted six other specs
    /// in the same commit. trace:BUG-96 | ai:claude
    #[test]
    fn test_save_preserves_unparseable_alongside_new_add() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        // Initial state: empty store, persist metadata.
        backend.save(&RequirementsStore::new()).unwrap();

        // Stage an unparseable file the current binary can't decode.
        let unparseable_path = root.join("objects/STORY/000/STORY-86.yaml");
        std::fs::create_dir_all(unparseable_path.parent().unwrap()).unwrap();
        std::fs::write(
            &unparseable_path,
            "id: 019e21ea-e521-76e2-9b34-52626ea25a4b\n\
             status: !FromTheFutureVariant\n\
             title: a story from a newer binary\n",
        )
        .unwrap();

        // Now do an "aida add" equivalent: load, push, save.
        let mut store = backend.load().unwrap();
        let mut newcomer = Requirement::new("New work".into(), "freshly added".into());
        newcomer.spec_id = Some("TASK-1".into());
        store.requirements.push(newcomer);
        backend.save(&store).unwrap();

        // The new file is added.
        assert!(root.join("objects/TASK/000/TASK-1.yaml").exists());

        // The unparseable file is preserved — the headline BUG-96 promise.
        assert!(
            unparseable_path.exists(),
            "save() with a concurrent add deleted an unparseable neighbour (BUG-96 regression)"
        );
    }

    #[test]
    fn test_git_backend_with_dispenser() {
        use crate::dispenser::{IdMode, MemoryDispenser};
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");

        let dispenser = Arc::new(MemoryDispenser::new(IdMode::Distributed {
            node_id: "7".to_string(),
        }));
        let handle = DispenserHandle(dispenser);
        let backend = GitBackend::new(&root).unwrap().with_dispenser(handle);

        // Save initial metadata
        backend.save(&RequirementsStore::new()).unwrap();

        // The dispenser should be injected into loaded stores
        let store = backend.load().unwrap();
        assert!(store.dispenser.is_some());
    }
}
