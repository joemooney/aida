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

    /// Count requirement objects in the store WITHOUT parsing their YAML.
    /// O(files) directory reads — backs `aida cache status`'s store count
    /// without a full `load()` (BUG-664).
    // trace:BUG-664
    pub fn object_count(&self) -> Result<usize> {
        object_store::count_objects(&self.objects_root)
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

    /// Read a user's queue file, PROPAGATING any parse error instead of
    /// swallowing it into an empty `Vec`.
    ///
    /// This is the data-loss guard (TASK-712): the queue read-modify-write paths
    /// (`queue_add`, `queue_remove`, `queue_reorder`, `queue_clear`) read the
    /// current entries, mutate, then write the whole file back. The previous
    /// `serde_yaml::from_str(...).unwrap_or_default()` turned a momentarily
    /// unparseable file (a partial write, or a forward-version file written by a
    /// newer AIDA) into an empty Vec — and the subsequent write-back then
    /// SILENTLY TRUNCATED every prior queue entry. Mirroring BUG-96's
    /// skip-and-warn for object YAML, we instead surface the error so the caller
    /// aborts WITHOUT overwriting the file; the corrupt/forward-version file is
    /// left intact for inspection. A genuinely absent file is still the empty
    /// queue (returns `Ok(vec![])`). trace:TASK-712
    fn read_queue_file(path: &Path) -> Result<Vec<QueueEntry>> {
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read queue file {}", path.display()))?;
        if content.trim().is_empty() {
            return Ok(Vec::new());
        }
        serde_yaml::from_str::<Vec<QueueEntry>>(&content).with_context(|| {
            format!(
                "Failed to parse queue file {} — refusing to overwrite it \
                 (a partial write or a file from a newer AIDA version?). \
                 Inspect/repair it by hand rather than risk truncating queued work.",
                path.display()
            )
        })
    }

    /// Resolve the queue-file user-id to use for `requested`, folding case at the
    /// LOOKUP boundary only. The queue is stored as one YAML file per user-id
    /// (`registry/queues/<user_id>.yaml`). Historically the lookup was
    /// case-SENSITIVE, so a shell reporting `Joe` and another reporting `joe`
    /// split one human across two queue files.
    ///
    /// This scans the queues directory for an existing file whose stem matches
    /// `requested` case-insensitively and, if found, returns that EXISTING stem —
    /// so `Joe` now reads/writes the queue already keyed under `joe`. When no
    /// existing file matches, `requested` is returned unchanged, so a brand-new
    /// queue keeps the shell's original casing: the stored key stays the raw
    /// shell `$USER`; we never rewrite it, we only fold when comparing.
    //
    // BUG-89 (queue keyed off raw shell user; storage unchanged).
    // trace:TASK-951 | ai:claude
    fn resolve_queue_user(&self, requested: &str) -> String {
        let dir = self.root.join("registry/queues");
        // trace:TASK-845 — compose TASK-951's case-fold (first) with the
        // operator-curated person-alias map (second): `requested` resolves to
        // its CANONICAL person, so a queue keyed under any of a person's aliases
        // (`joe.mooney@gmail.com`) is found when looked up under another (`joe`).
        // The map is empty by default, in which case `resolve` is exactly the
        // TASK-951 case-fold and the behaviour is unchanged.
        let aliases = crate::alias::AliasRegistry::load(&self.root);
        let target = aliases.resolve(requested);
        let read = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            // No queues dir yet → nothing to match against; keep original casing.
            Err(_) => return requested.to_string(),
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                // An exact (case-sensitive) hit always wins — return immediately
                // so the common path never rewrites the casing it was handed.
                if stem == requested {
                    return requested.to_string();
                }
                // Resolve each existing queue file to its canonical person too,
                // so an alias's stored queue file is matched even when the
                // lookup uses a different alias of the same person.
                if aliases.resolve(stem) == target {
                    return stem.to_string();
                }
            }
        }
        requested.to_string()
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
            // BUG-587: tag edits must reach the oplog too. Previously `aida edit
            // --tags` wrote ONLY the YAML object and recorded no operation, so
            // tag changes bypassed the CRDT substrate entirely — the oplog was an
            // incomplete record of the spec's history. Emit one AddTag per added
            // tag and one RemoveTag per removed tag, matching the documented
            // "every status flip, priority change, tag edit lands as a structured
            // row" invariant. trace:BUG-587 | ai:claude
            for added in requirement.tags.difference(&old.tags) {
                self.record_op(
                    requirement.id,
                    crate::oplog::OpKind::AddTag { tag: added.clone() },
                );
            }
            for removed in old.tags.difference(&requirement.tags) {
                self.record_op(
                    requirement.id,
                    crate::oplog::OpKind::RemoveTag {
                        tag: removed.clone(),
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
        //
        // BUG-599: a malformed id (e.g. `not-a-real-id`, a UUID-shaped string)
        // makes `object_path`/`object_exists` return Err("Invalid spec_id
        // format ..."), which a caller would then dress up with the
        // version-mismatch `parse_failure_hint` — alarming, wrong guidance for
        // a simple typo. A malformed id can never name a stored object, so
        // treat it as a plain not-found (Ok(None)); the CLI surfaces a friendly
        // format hint before this point. trace:BUG-599 | ai:claude
        if !object_store::valid_spec_id_format(&spec_id) {
            return Ok(None);
        }
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
            // TASK-856: derive the type prefix from the store's id_config the
            // same way the full-store add path does (the CLI add handler used
            // to pass `store.get_type_prefix(req_type)` into update_atomically).
            // Without it a single-row add fell back to the generic "REQ"/"GEN"
            // prefix instead of the type-correct one (TASK-N, BUG-N, …). The
            // requirement's own `prefix_override`, when set, still wins inside
            // add_requirement_with_id. trace:TASK-856 | ai:claude
            let type_prefix = temp_store.get_type_prefix(&req.req_type);
            // Generate ID using the store's configured strategy
            let req_clone = req.clone();
            temp_store.add_requirement_with_id(req_clone, None, type_prefix.as_deref());
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
        // trace:TASK-951 — fold case at the lookup boundary so `Joe` finds the
        // queue keyed under `joe`. Storage casing is left as-is.
        let user_id = self.resolve_queue_user(user_id);
        let path = self
            .root
            .join("registry/queues")
            .join(format!("{}.yaml", user_id));
        // trace:TASK-712 — propagate parse errors instead of unwrap_or_default.
        Self::read_queue_file(&path)
    }

    // Enumerate every user id with a persisted queue file
    // (registry/queues/<user_id>.yaml). Read-only; powers the fleet-wide
    // `aida queue list --all-users` view. trace:STORY-672
    fn queue_users(&self) -> Result<Vec<String>> {
        let dir = self.root.join("registry/queues");
        let mut users: Vec<String> = Vec::new();
        let read = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            // No queues directory yet → no users.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(users),
            Err(e) => return Err(e.into()),
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    users.push(stem.to_string());
                }
            }
        }
        users.sort();
        users.dedup();
        Ok(users)
    }

    fn queue_add(&self, entry: QueueEntry) -> Result<()> {
        let dir = self.root.join("registry/queues");
        std::fs::create_dir_all(&dir)?;
        // trace:TASK-951 — resolve the FILENAME case-insensitively (so `Joe`
        // appends to the existing `joe.yaml`) without rewriting the stored
        // `entry.user_id` (BUG-89: the persisted key stays the raw shell `$USER`).
        let user_id = self.resolve_queue_user(&entry.user_id);
        let path = dir.join(format!("{}.yaml", user_id));
        // trace:TASK-712 — a parse error here aborts BEFORE the write-back below,
        // so a momentarily-unparseable queue file is never silently truncated.
        let mut entries = Self::read_queue_file(&path)?;
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
        self.queue_remove_for_role(user_id, requirement_id, None)
    }

    // BUG-529: honor an optional routing-role filter so `aida queue remove
    // <id> --for <role>` drops only the entry queued for that role, leaving a
    // sibling entry (same spec queued for a different role) intact. With
    // `role == None` this is the historical role-blind remove.
    // trace:BUG-529 | ai:claude
    fn queue_remove_for_role(
        &self,
        user_id: &str,
        requirement_id: &uuid::Uuid,
        role: Option<&str>,
    ) -> Result<()> {
        // trace:TASK-951 — fold case at the lookup boundary.
        let user_id = self.resolve_queue_user(user_id);
        let path = self
            .root
            .join("registry/queues")
            .join(format!("{}.yaml", user_id));
        if !path.exists() {
            return Ok(());
        }
        // trace:TASK-712 — abort on parse error rather than overwrite with [].
        let mut entries = Self::read_queue_file(&path)?;
        entries.retain(|e| {
            // Keep entries that don't match the requirement at all.
            if e.requirement_id != *requirement_id {
                return true;
            }
            // Requirement matches: drop it only if the role filter also
            // matches (or there's no role filter). trace:BUG-529 | ai:claude
            match role {
                None => false,
                Some(r) => !e
                    .for_role
                    .as_deref()
                    .is_some_and(|er| er.eq_ignore_ascii_case(r)),
            }
        });
        let yaml = serde_yaml::to_string(&entries)?;
        std::fs::write(&path, yaml)?;
        self.auto_commit_paths(
            "update queue",
            &[&format!("registry/queues/{}.yaml", user_id)],
        );
        Ok(())
    }

    fn queue_reorder(&self, user_id: &str, items: &[(uuid::Uuid, i64)]) -> Result<()> {
        // trace:TASK-951 — fold case at the lookup boundary.
        let user_id = self.resolve_queue_user(user_id);
        let path = self
            .root
            .join("registry/queues")
            .join(format!("{}.yaml", user_id));
        if !path.exists() {
            return Ok(());
        }
        // trace:TASK-712 — abort on parse error rather than overwrite with [].
        let mut entries = Self::read_queue_file(&path)?;
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
        // trace:TASK-951 — fold case at the lookup boundary.
        let user_id = self.resolve_queue_user(user_id);
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
        // trace:TASK-712 — abort on parse error rather than overwrite with [].
        let entries = Self::read_queue_file(&path)?;
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

    // TASK-1052: bulk-remove the given requirement ids from the user's queue
    // file in a single write + commit. Returns the removed entries. Used by
    // queue-GC, whose caller has already decided which ids are dead (target
    // spec archived/Completed/Rejected) from the cache, so this stays a dumb
    // set-membership prune. trace:TASK-1052 | ai:claude
    fn queue_remove_many(&self, user_id: &str, ids: &[uuid::Uuid]) -> Result<Vec<QueueEntry>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // trace:TASK-951 — fold case at the lookup boundary.
        let user_id = self.resolve_queue_user(user_id);
        let path = self
            .root
            .join("registry/queues")
            .join(format!("{}.yaml", user_id));
        if !path.exists() {
            return Ok(Vec::new());
        }
        let dead: std::collections::HashSet<uuid::Uuid> = ids.iter().copied().collect();
        // trace:TASK-712 — abort on parse error rather than overwrite with [].
        let entries = Self::read_queue_file(&path)?;
        let mut removed: Vec<QueueEntry> = Vec::new();
        let mut kept: Vec<QueueEntry> = Vec::new();
        for entry in entries.into_iter() {
            if dead.contains(&entry.requirement_id) {
                removed.push(entry);
            } else {
                kept.push(entry);
            }
        }
        if removed.is_empty() {
            return Ok(Vec::new());
        }
        if kept.is_empty() {
            std::fs::remove_file(&path)?;
        } else {
            let yaml = serde_yaml::to_string(&kept)?;
            std::fs::write(&path, yaml)?;
        }
        self.auto_commit_paths(
            "gc dead queue entries",
            &[&format!("registry/queues/{}.yaml", user_id)],
        );
        Ok(removed)
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

    // BUG-599: a malformed id (not TYPE-SEQ / TYPE-NODE-SEQ) must resolve to a
    // plain Ok(None) not-found, NOT an Err that the CLI would dress up with the
    // version-mismatch/rebuild `parse_failure_hint`.
    #[test]
    fn malformed_spec_id_is_not_found_not_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        for bad in [
            "not-a-real-id",
            "BADID",
            "019ee0ed-2e4d-7652-a71e-d521f071af27",
        ] {
            let got = backend.get_requirement_by_spec_id(bad);
            assert!(
                matches!(got, Ok(None)),
                "malformed id {bad:?} should be Ok(None), got {got:?}"
            );
        }
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

    fn sample_queue_entry(user_id: &str, position: i64) -> QueueEntry {
        QueueEntry {
            user_id: user_id.to_string(),
            requirement_id: uuid::Uuid::new_v4(),
            position,
            added_by: user_id.to_string(),
            note: None,
            added_at: chrono::Utc::now(),
            for_role: None,
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        }
    }

    // trace:TASK-712 — a momentarily-unparseable queue file must NOT be silently
    // truncated by the next read-modify-write. Before the fix, queue_add read the
    // corrupt file as an empty Vec, then wrote back only the new entry, destroying
    // every prior queued item. Now the parse error propagates and the file is left
    // untouched.
    #[test]
    fn test_queue_add_does_not_truncate_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        let queues_dir = root.join("registry/queues");
        std::fs::create_dir_all(&queues_dir).unwrap();
        let path = queues_dir.join("alice.yaml");

        // Simulate a partial write / forward-version file that serde can't parse.
        let corrupt = "this: [is not, a: valid] sequence of QueueEntry\n\t- broken";
        std::fs::write(&path, corrupt).unwrap();

        // queue_add must REFUSE rather than overwrite.
        let err = backend
            .queue_add(sample_queue_entry("alice", i64::MAX))
            .unwrap_err();
        assert!(
            err.to_string().contains("Failed to parse queue file")
                || err.to_string().contains("refusing to overwrite"),
            "unexpected error: {err}"
        );

        // The corrupt file is left byte-for-byte intact — no data loss.
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, corrupt, "corrupt queue file must not be overwritten");
    }

    // trace:TASK-712 — the normal (parseable) read-modify-write path still works:
    // adding to a file with existing entries preserves the existing ones.
    #[test]
    fn test_queue_add_preserves_existing_entries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        let e1 = sample_queue_entry("bob", 1000);
        let e2 = sample_queue_entry("bob", 2000);
        backend.queue_add(e1.clone()).unwrap();
        backend.queue_add(e2.clone()).unwrap();

        let entries = backend.queue_list("bob", false).unwrap();
        assert_eq!(entries.len(), 2, "both entries should survive");
        let ids: Vec<_> = entries.iter().map(|e| e.requirement_id).collect();
        assert!(ids.contains(&e1.requirement_id));
        assert!(ids.contains(&e2.requirement_id));
    }

    // TASK-1052: queue-GC's bulk-remove drops exactly the named ids in a single
    // write and returns the removed entries; entries NOT named survive. (The
    // dead/alive determination lives in the cache-fast CLI predicate; this is
    // the dumb set-membership prune underneath it.) trace:TASK-1052 | ai:claude
    #[test]
    fn test_queue_remove_many_drops_named_keeps_rest() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        let keep1 = sample_queue_entry("carol", 1000);
        let drop1 = sample_queue_entry("carol", 2000);
        let keep2 = sample_queue_entry("carol", 3000);
        let drop2 = sample_queue_entry("carol", 4000);
        for e in [&keep1, &drop1, &keep2, &drop2] {
            backend.queue_add(e.clone()).unwrap();
        }

        let removed = backend
            .queue_remove_many("carol", &[drop1.requirement_id, drop2.requirement_id])
            .unwrap();
        assert_eq!(removed.len(), 2, "both named entries removed");
        let removed_ids: std::collections::HashSet<_> =
            removed.iter().map(|e| e.requirement_id).collect();
        assert!(removed_ids.contains(&drop1.requirement_id));
        assert!(removed_ids.contains(&drop2.requirement_id));

        let survivors = backend.queue_list("carol", false).unwrap();
        let survivor_ids: std::collections::HashSet<_> =
            survivors.iter().map(|e| e.requirement_id).collect();
        assert_eq!(survivors.len(), 2, "unnamed entries survive");
        assert!(survivor_ids.contains(&keep1.requirement_id));
        assert!(survivor_ids.contains(&keep2.requirement_id));

        // An empty id list is a no-op (no spurious write / removal).
        let none = backend.queue_remove_many("carol", &[]).unwrap();
        assert!(none.is_empty());
        assert_eq!(backend.queue_list("carol", false).unwrap().len(), 2);
    }

    // TASK-951: the queue is keyed off the shell user, stored as one
    // `registry/queues/<user_id>.yaml` file. The lookup folds case so a queue
    // filed under `joe` is found when the shell later reports `Joe` — one human
    // is no longer split across machines whose shells differ only in casing. The
    // STORED filename keeps its original casing (BUG-89 safety).
    // trace:TASK-951 | ai:claude
    #[test]
    fn test_queue_matches_across_user_case() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        // Filed under lowercase `joe`.
        let entry = sample_queue_entry("joe", 1000);
        backend.queue_add(entry.clone()).unwrap();

        // The stored file keeps its original lowercase casing — nothing rewrote it.
        let joe_path = root.join("registry/queues/joe.yaml");
        assert!(joe_path.exists(), "stored key keeps original casing");

        // A shell reporting `Joe` (and `JOE`) finds the same queue.
        let as_joe = backend.queue_list("Joe", false).unwrap();
        assert_eq!(as_joe.len(), 1, "`Joe` matches the queue filed under `joe`");
        assert_eq!(as_joe[0].requirement_id, entry.requirement_id);
        assert_eq!(backend.queue_list("JOE", false).unwrap().len(), 1);

        // Adding as `Joe` appends to the SAME existing file — no second
        // `Joe.yaml` is created (BUG-89: we fold the lookup, not the storage).
        let entry2 = sample_queue_entry("Joe", 2000);
        backend.queue_add(entry2.clone()).unwrap();
        assert!(
            !root.join("registry/queues/Joe.yaml").exists(),
            "no duplicate case-variant queue file"
        );
        let both = backend.queue_list("joe", false).unwrap();
        assert_eq!(both.len(), 2, "both entries land in the one queue");

        // Removing as `JOE` clears from the same file.
        backend.queue_remove("JOE", &entry.requirement_id).unwrap();
        assert_eq!(backend.queue_list("joe", false).unwrap().len(), 1);
    }

    // TASK-845: the queue resolution composes the case-fold with the person-alias
    // map on the store. A queue filed under `joe` is found when the same human's
    // OTHER machine looks it up under `joe.mooney@gmail.com`, once the two are
    // linked. The stored filename is never rewritten (BUG-89 safety preserved).
    // trace:TASK-845 | ai:claude
    #[test]
    fn test_queue_matches_across_person_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        // Filed under `joe` on one host.
        let entry = sample_queue_entry("joe", 1000);
        backend.queue_add(entry.clone()).unwrap();
        assert!(root.join("registry/queues/joe.yaml").exists());

        // Without a link, a genuinely-different string does NOT match (the
        // case-fold alone can't bridge `joe` ↔ `joe.mooney@gmail.com`).
        assert!(
            backend
                .queue_list("joe.mooney@gmail.com", false)
                .unwrap()
                .is_empty(),
            "unlinked alias must not match"
        );

        // Operator links the two identities on the shared store.
        let mut aliases = crate::alias::AliasRegistry::default();
        aliases.link("joe", "joe.mooney@gmail.com");
        aliases.save(&root).unwrap();

        // Now the alias finds the SAME queue.
        let as_alias = backend.queue_list("joe.mooney@gmail.com", false).unwrap();
        assert_eq!(as_alias.len(), 1, "linked alias finds joe's queue");
        assert_eq!(as_alias[0].requirement_id, entry.requirement_id);

        // Adding under the alias appends to the SAME `joe.yaml` — no second file,
        // stored value untouched.
        let entry2 = sample_queue_entry("joe.mooney@gmail.com", 2000);
        backend.queue_add(entry2).unwrap();
        assert!(
            !root
                .join("registry/queues/joe.mooney@gmail.com.yaml")
                .exists(),
            "no duplicate alias queue file — resolution folds the lookup, not storage"
        );
        assert_eq!(
            backend.queue_list("joe", false).unwrap().len(),
            2,
            "both entries land in joe's one queue"
        );
    }

    // BUG-529: `queue_remove_for_role` with a role filter drops ONLY the entry
    // queued for that role; a sibling entry for the same spec queued for a
    // different role survives. The role-blind path (role == None) still wipes
    // every entry for the spec. trace:BUG-529 | ai:claude
    #[test]
    fn test_queue_remove_for_role_filters_by_role() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        // Same spec queued twice for the same user, once per role. (queue_add
        // upserts by requirement_id, so write the two-entry state directly —
        // it can arise via cross-machine merge or manual queue edits.)
        let spec_id = uuid::Uuid::new_v4();
        let mut impl_entry = sample_queue_entry("dave", 1000);
        impl_entry.requirement_id = spec_id;
        impl_entry.for_role = Some("implementer".to_string());
        let mut adv_entry = sample_queue_entry("dave", 2000);
        adv_entry.requirement_id = spec_id;
        adv_entry.for_role = Some("advisor".to_string());
        let queues_dir = root.join("registry/queues");
        std::fs::create_dir_all(&queues_dir).unwrap();
        std::fs::write(
            queues_dir.join("dave.yaml"),
            serde_yaml::to_string(&vec![impl_entry, adv_entry]).unwrap(),
        )
        .unwrap();
        assert_eq!(backend.queue_list("dave", false).unwrap().len(), 2);

        // Remove ONLY the advisor entry — implementer entry must remain.
        backend
            .queue_remove_for_role("dave", &spec_id, Some("advisor"))
            .unwrap();
        let after = backend.queue_list("dave", false).unwrap();
        assert_eq!(after.len(), 1, "implementer entry must survive");
        assert_eq!(after[0].for_role.as_deref(), Some("implementer"));

        // Case-insensitive: a differently-cased role still matches the
        // canonical stored value.
        backend
            .queue_remove_for_role("dave", &spec_id, Some("IMPLEMENTER"))
            .unwrap();
        assert!(backend.queue_list("dave", false).unwrap().is_empty());
    }

    // BUG-529: role == None preserves the historical role-blind remove —
    // every entry for the spec is dropped. trace:BUG-529 | ai:claude
    #[test]
    fn test_queue_remove_for_role_none_is_role_blind() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        let spec_id = uuid::Uuid::new_v4();
        let mut a = sample_queue_entry("erin", 1000);
        a.requirement_id = spec_id;
        a.for_role = Some("implementer".to_string());
        let mut b = sample_queue_entry("erin", 2000);
        b.requirement_id = spec_id;
        b.for_role = Some("advisor".to_string());
        let queues_dir = root.join("registry/queues");
        std::fs::create_dir_all(&queues_dir).unwrap();
        std::fs::write(
            queues_dir.join("erin.yaml"),
            serde_yaml::to_string(&vec![a, b]).unwrap(),
        )
        .unwrap();

        backend
            .queue_remove_for_role("erin", &spec_id, None)
            .unwrap();
        assert!(
            backend.queue_list("erin", false).unwrap().is_empty(),
            "role-blind remove drops every entry for the spec"
        );
    }

    // trace:TASK-712 — an empty / whitespace-only queue file is the empty queue,
    // not a parse error (regression guard for read_queue_file's empty handling).
    #[test]
    fn test_queue_read_empty_file_is_empty_queue() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();
        let queues_dir = root.join("registry/queues");
        std::fs::create_dir_all(&queues_dir).unwrap();
        std::fs::write(queues_dir.join("carol.yaml"), "   \n").unwrap();

        let entries = backend.queue_list("carol", false).unwrap();
        assert!(entries.is_empty());
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

    // trace:BUG-587 — `aida edit --tags` must emit AddTag/RemoveTag oplog ops.
    // Before the fix `stage_requirement_update` had no tags branch, so tag edits
    // wrote ONLY the YAML object and recorded zero operations — bypassing the
    // CRDT substrate. This counts ops on the edit path and asserts tag deltas
    // now land in the oplog like every other field edit.
    #[test]
    fn test_tag_edit_emits_oplog_ops() {
        use crate::oplog::{OpKind, OpLog};

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        // Disable auto_commit so the test doesn't need a git repo; oplog is
        // independent of the commit step.
        let backend = GitBackend::new(&root).unwrap().with_auto_commit(false);

        // Seed an on-disk object (the diff branch only fires when the old
        // object exists). Use update_requirement so the same edit path runs.
        let mut req = Requirement::new("Tagged".into(), "desc".into());
        req.spec_id = Some("TASK-1".into());
        req.tags.insert("keep".to_string());
        req.tags.insert("drop".to_string());
        object_store::write_object_if_changed(&backend.objects_root, &req).unwrap();

        let oplog_path = root.join("oplog.yaml");

        // Edit: add "added", remove "drop", keep "keep".
        let mut edited = req.clone();
        edited.tags.remove("drop");
        edited.tags.insert("added".to_string());
        backend.update_requirement(&edited).unwrap();

        let log = OpLog::load(&oplog_path).unwrap();
        let mut added = Vec::new();
        let mut removed = Vec::new();
        for op in &log.operations {
            match &op.kind {
                OpKind::AddTag { tag } => added.push(tag.clone()),
                OpKind::RemoveTag { tag } => removed.push(tag.clone()),
                _ => {}
            }
        }
        assert!(
            added.contains(&"added".to_string()),
            "AddTag op for the newly-added tag must be recorded (got added={added:?})"
        );
        assert!(
            removed.contains(&"drop".to_string()),
            "RemoveTag op for the removed tag must be recorded (got removed={removed:?})"
        );
        // The untouched tag must NOT churn an op.
        assert!(!added.contains(&"keep".to_string()));
        assert!(!removed.contains(&"keep".to_string()));
    }

    // trace:BUG-587 — a tag-only edit alongside a scalar edit records BOTH an
    // AddTag op and the scalar op (substrate stays a complete record).
    #[test]
    fn test_tag_and_scalar_edit_both_recorded() {
        use crate::oplog::{OpKind, OpLog};

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap().with_auto_commit(false);

        let mut req = Requirement::new("Title".into(), "desc".into());
        req.spec_id = Some("TASK-2".into());
        object_store::write_object_if_changed(&backend.objects_root, &req).unwrap();

        let mut edited = req.clone();
        edited.tags.insert("newtag".to_string());
        edited.title = "New Title".to_string();
        backend.update_requirement(&edited).unwrap();

        let log = OpLog::load(&root.join("oplog.yaml")).unwrap();
        let has_addtag = log
            .operations
            .iter()
            .any(|op| matches!(&op.kind, OpKind::AddTag { tag } if tag == "newtag"));
        let has_settitle = log
            .operations
            .iter()
            .any(|op| matches!(&op.kind, OpKind::SetTitle { title } if title == "New Title"));
        assert!(has_addtag, "tag edit must emit AddTag");
        assert!(has_settitle, "scalar edit must still emit SetTitle");
    }
}
