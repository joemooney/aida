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

/// What a whole-store `save()` left alone.
// trace:BUG-1612 | ai:claude
#[derive(Debug, Default, Clone)]
pub(crate) struct SaveReport {
    /// Objects absent from the saved store that were kept because the store
    /// was not loaded with them (added concurrently, or no load snapshot).
    pub(crate) kept_unloaded: Vec<String>,
    /// Specs this store did not change whose object changed on disk after
    /// the load; they were not written, and the in-memory copies are stale.
    pub(crate) stale_untouched: Vec<String>,
}

/// A whole-store save refused because specs it would write (or delete), or
/// store-level `metadata.yaml` fields it changed, changed on disk after the
/// store was loaded. Nothing was written.
/// Downcast from the `anyhow::Error` to detect it.
// trace:BUG-1612 trace:BUG-1613 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreConflictError {
    /// The conflicting spec ids.
    pub specs: Vec<String>,
    /// The conflicting store-level fields of `metadata.yaml` (e.g. `features`).
    pub metadata_fields: Vec<String>,
}

impl std::fmt::Display for StoreConflictError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut what: Vec<String> = self.specs.clone();
        if !self.metadata_fields.is_empty() {
            what.push(format!(
                "the store's {} setting{}",
                self.metadata_fields.join(", "),
                if self.metadata_fields.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ));
        }
        let many = self.specs.len() + self.metadata_fields.len() > 1;
        write!(
            f,
            "refusing to save: {} changed on disk after this store was loaded, and this \
             save changes {} too (a concurrent edit). Nothing was written; reload and retry.",
            what.join(", "),
            if many { "them" } else { "it" }
        )
    }
}

impl std::error::Error for StoreConflictError {}

/// What `update_atomically_tracked` wrote.
// trace:BUG-1612 | ai:claude
#[derive(Debug, Default, Clone)]
pub(crate) struct AtomicWriteSummary {
    /// Requirements whose object was written (modified or created), as written.
    pub(crate) written: Vec<Requirement>,
    /// Spec ids of the requirements the transaction created.
    pub(crate) created: Vec<String>,
    /// UUIDs of the requirements the transaction removed.
    pub(crate) deleted: Vec<uuid::Uuid>,
    /// Whether `metadata.yaml` changed.
    pub(crate) metadata_changed: bool,
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

/// Store-level fields that are monotonic ID counters. They are merged as the
/// maximum of the disk and caller values (per key for the per-prefix maps), so
/// a save never lowers a counter and re-issues an ID.
// trace:BUG-1613 | ai:claude
const METADATA_COUNTER_FIELDS: [&str; 4] = [
    "next_feature_number",
    "next_spec_number",
    "prefix_counters",
    "meta_counters",
];

/// The outcome of merging a writer's metadata over the current disk copy.
// trace:BUG-1613 | ai:claude
struct MetadataMerge {
    /// What to write, or `None` when the disk copy already equals the merge
    /// (nothing store-level changed, so `metadata.yaml` is left alone).
    write: Option<StoreMetadata>,
}

/// Serialize metadata as a YAML value (map equality ignores key order, so the
/// `HashMap`-backed counters compare by content).
fn metadata_value(meta: &StoreMetadata) -> Result<serde_yaml::Value> {
    Ok(serde_yaml::to_value(meta)?)
}

/// Max-merge one counter field: an integer, or a map of prefix -> integer.
fn max_counter(
    disk: Option<&serde_yaml::Value>,
    mine: Option<&serde_yaml::Value>,
) -> serde_yaml::Value {
    use serde_yaml::Value;
    match (disk, mine) {
        (Some(Value::Mapping(d)), Some(Value::Mapping(m))) => {
            let mut out = d.clone();
            for (k, mv) in m {
                let keep = match (out.get(k).and_then(Value::as_u64), mv.as_u64()) {
                    (Some(dv), Some(mv)) => dv >= mv,
                    (Some(_), None) => true,
                    _ => false,
                };
                if !keep {
                    out.insert(k.clone(), mv.clone());
                }
            }
            Value::Mapping(out)
        }
        (Some(d), Some(m)) => match (d.as_u64(), m.as_u64()) {
            (Some(dv), Some(mv)) if mv > dv => m.clone(),
            (Some(_), _) => d.clone(),
            _ => m.clone(),
        },
        (Some(d), None) => d.clone(),
        (None, Some(m)) => m.clone(),
        (None, None) => Value::Null,
    }
}

/// Three-way merge of the store-level fields, the metadata analogue of the
/// per-spec compare-and-swap in [`GitBackend::save_reporting`] (BUG-1612),
/// at the granularity of one top-level `metadata.yaml` field:
/// - a field this writer did not change (equal to `base`) keeps the DISK
///   value, so a concurrent change to it survives;
/// - a field only this writer changed takes the writer's value;
/// - a field both changed, to different values, is a conflict (returned as the
///   field names; nothing may be written);
/// - ID counters are never a conflict: they take the max of disk and writer.
///
/// `base = None` (a store not loaded from this backend) means every field is
/// the writer's, as before, but counters are still never lowered. `disk =
/// None` (no `metadata.yaml` yet) writes the writer's copy.
// trace:BUG-1613 | ai:claude
fn merge_metadata(
    base: Option<&serde_yaml::Value>,
    mine: &StoreMetadata,
    disk: Option<&StoreMetadata>,
) -> Result<std::result::Result<MetadataMerge, Vec<String>>> {
    use serde_yaml::Value;
    let Some(disk) = disk else {
        return Ok(Ok(MetadataMerge {
            write: Some(mine.clone()),
        }));
    };
    let mine_v = metadata_value(mine)?;
    let disk_v = metadata_value(disk)?;
    let (Value::Mapping(mine_m), Value::Mapping(disk_m)) = (&mine_v, &disk_v) else {
        anyhow::bail!("store metadata did not serialize as a mapping");
    };
    let base_m = match base {
        Some(Value::Mapping(b)) => Some(b),
        _ => None,
    };
    let mut merged = disk_m.clone();
    let mut conflicts: Vec<String> = Vec::new();
    for (key, mine_field) in mine_m {
        let name = key.as_str().unwrap_or_default();
        let disk_field = disk_m.get(key);
        if METADATA_COUNTER_FIELDS.contains(&name) {
            merged.insert(key.clone(), max_counter(disk_field, Some(mine_field)));
            continue;
        }
        let Some(base_m) = base_m else {
            merged.insert(key.clone(), mine_field.clone());
            continue;
        };
        let base_field = base_m.get(key);
        if base_field == Some(mine_field) {
            continue; // untouched by this writer: the disk value stands
        }
        if disk_field == Some(mine_field) {
            continue; // same change on both sides
        }
        if disk_field == base_field {
            merged.insert(key.clone(), mine_field.clone());
        } else {
            conflicts.push(name.to_string());
        }
    }
    if !conflicts.is_empty() {
        conflicts.sort();
        return Ok(Err(conflicts));
    }
    let merged = Value::Mapping(merged);
    if merged == disk_v {
        return Ok(Ok(MetadataMerge { write: None }));
    }
    Ok(Ok(MetadataMerge {
        write: Some(serde_yaml::from_value(merged)?),
    }))
}

/// Resolve the queue-file user-id to use for `requested`, folding case at the
/// LOOKUP boundary only, rooted at `store_root` (the `.aida-store` directory).
/// The queue is stored as one YAML file per user-id
/// (`registry/queues/<user_id>.yaml`). Historically the lookup was
/// case-SENSITIVE, so a shell reporting `Joe` and another reporting `joe` split
/// one human across two queue files.
///
/// This scans the queues directory for an existing file whose stem matches
/// `requested` case-insensitively (composing the TASK-845 person-alias map on
/// top of the TASK-951 case-fold) and, if found, returns that EXISTING stem — so
/// `Joe` reads the queue already keyed under `joe`. When no existing file
/// matches, `requested` is returned unchanged, so a brand-new queue keeps the
/// shell's original casing: the stored key stays the raw shell `$USER`; we never
/// rewrite it, we only fold when comparing.
///
/// Read-only and side-effect-free (unlike [`GitBackend::new`], it never creates
/// directories), so the fast statusline `queue_depth` path can call it to fold
/// identity the SAME way `aida queue list` does.
// trace:BUG-675 trace:TASK-951 trace:TASK-845 | ai:claude
pub fn resolve_queue_user(store_root: &Path, requested: &str) -> String {
    let dir = store_root.join("registry/queues");
    let aliases = crate::alias::AliasRegistry::load(store_root);
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
            let logical_stem = decode_queue_user_filename(stem);
            // An exact (case-sensitive) hit always wins — return immediately so
            // the common path never rewrites the casing it was handed.
            if logical_stem == requested {
                return requested.to_string();
            }
            // Resolve each existing queue file to its canonical person too, so an
            // alias's stored queue file is matched even when the lookup uses a
            // different alias of the same person.
            if aliases.resolve(&logical_stem) == target {
                return logical_stem;
            }
        }
    }
    requested.to_string()
}

// Queue identities are logical user ids, not portable filenames. Role queues use
// `role:<name>`, which fails on Windows unless the filename layer escapes it.
// trace:BUG-1021 | ai:codex
fn encode_queue_user_filename(user_id: &str) -> String {
    let mut out = String::new();
    for b in user_id.bytes() {
        match b {
            b'%' => out.push_str("%25"),
            b':' => out.push_str("%3A"),
            b'/' => out.push_str("%2F"),
            b'\\' => out.push_str("%5C"),
            _ => out.push(b as char),
        }
    }
    out
}

fn decode_queue_user_filename(stem: &str) -> String {
    let bytes = stem.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &stem[i + 1..i + 3];
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn queue_relative_path(user_id: &str) -> String {
    format!(
        "registry/queues/{}.yaml",
        encode_queue_user_filename(user_id)
    )
}

fn queue_file_path(store_root: &Path, user_id: &str) -> PathBuf {
    let encoded = store_root.join(queue_relative_path(user_id));
    let legacy = store_root
        .join("registry/queues")
        .join(format!("{}.yaml", user_id));
    if !encoded.exists() && legacy.exists() {
        legacy
    } else {
        encoded
    }
}

fn queue_relative_path_for_file(store_root: &Path, path: &Path, user_id: &str) -> String {
    path.strip_prefix(store_root)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| queue_relative_path(user_id))
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
            // Route through the shared git helper so targeted store writes get
            // the same stale-index-lock recovery as bulk saves. trace:BUG-1164 | ai:codex
            let _ = crate::git_ops::add_all(&self.root, p);
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
        // Delegate to the free-function form so a caller OUTSIDE the backend
        // (the statusline `queue_depth` path) can fold identity IDENTICALLY
        // without constructing a `GitBackend` (whose `new` has directory-
        // creating side effects). trace:BUG-675 | ai:claude
        resolve_queue_user(&self.root, requested)
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

    /// The current `metadata.yaml`, or `None` when there is none yet.
    // trace:BUG-1613 | ai:claude
    fn read_metadata_if_present(&self) -> Result<Option<StoreMetadata>> {
        if !self.metadata_path.exists() {
            return Ok(None);
        }
        self.load_metadata().map(Some)
    }

    /// TASK-1065: load ONLY the store metadata into a `RequirementsStore` whose
    /// `requirements` vec is EMPTY. Reads a single `metadata.yaml` file — never
    /// scans the object YAMLs — so callers that need the store's name / features /
    /// id-config (e.g. the `aida status --full` Project + scaffolding sections)
    /// can get them without a full `load()`.
    // trace:TASK-1065 | ai:claude
    pub fn load_metadata_only(&self) -> Result<RequirementsStore> {
        let meta = self.load_metadata()?;
        Ok(self.assemble_store(meta, Vec::new()))
    }

    /// BUG-1629: [`Self::load_metadata_only`] for a store at `root`, strictly
    /// read-only. Unlike [`Self::new`] it never creates `objects/`, so a
    /// probe of a missing or partial store has no filesystem side effect.
    // trace:BUG-1629 | ai:claude
    pub fn read_metadata_only(root: &Path) -> Result<RequirementsStore> {
        let backend = Self {
            root: root.to_path_buf(),
            objects_root: root.join("objects"),
            metadata_path: root.join("metadata.yaml"),
            dispenser: None,
            auto_commit: false,
            oplog_enabled: false,
        };
        backend.load_metadata_only()
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
            loaded_objects: None,
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
        // trace:BUG-1613 | ai:claude — the baseline `finish` merges against.
        let base = metadata_value(&metadata)?;
        Ok(BulkWriter {
            backend: self,
            metadata,
            base,
            assigned: std::collections::HashSet::new(),
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

    /// Full-store saves are the compatibility boundary for older/projection
    /// callers. Preserve git-canonical fields that a caller may not have
    /// loaded at all, while leaving targeted `update_requirement` free to make
    /// intentional clears such as `aida undefer`.
    // trace:BUG-756 | ai:codex
    fn preserve_full_save_only_fields(
        mut incoming: Requirement,
        disk: &Requirement,
    ) -> Requirement {
        if !incoming.deferred && disk.deferred {
            incoming.deferred = true;
            incoming.deferred_at = disk.deferred_at;
            incoming.deferred_until = disk.deferred_until.clone();
        }
        if incoming.processing_record.is_empty() && !disk.processing_record.is_empty() {
            incoming.processing_record = disk.processing_record.clone();
        }
        if incoming.external_refs.is_empty() && !disk.external_refs.is_empty() {
            incoming.external_refs = disk.external_refs.clone();
        }
        if incoming.risk_notes.is_none() {
            incoming.risk_notes = disk.risk_notes.clone();
        }
        if incoming.test_coverage_notes.is_none() {
            incoming.test_coverage_notes = disk.test_coverage_notes.clone();
        }
        if incoming.implementation_summary.is_none() {
            incoming.implementation_summary = disk.implementation_summary.clone();
        }
        if incoming.decision_request.is_none() {
            incoming.decision_request = disk.decision_request.clone();
        }
        if incoming.failure_reason.is_none() {
            incoming.failure_reason = disk.failure_reason.clone();
        }

        for comment in &mut incoming.comments {
            if comment.session_id.is_some() {
                continue;
            }
            if let Some(old) = disk.comments.iter().find(|old| old.id == comment.id) {
                comment.session_id = old.session_id.clone();
            }
        }
        // CR-8: filing provenance is write-once — the on-disk stamp wins.
        // trace:CR-8 | ai:claude
        crate::provenance::preserve_from_disk(&mut incoming, disk);

        incoming
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

        // CR-8: filing provenance is write-once. If the incoming copy's stamp
        // differs from the on-disk one (dropped by a caller that built the
        // struct fresh, or rewritten), write the on-disk stamp back instead.
        // trace:CR-8 | ai:claude
        let existing = object_store::read_object(&self.objects_root, spec_id).ok();
        let provenance_fixed: Option<Requirement> = match &existing {
            Some(old) if old.filed_at != requirement.filed_at => {
                let mut fixed = requirement.clone();
                crate::provenance::preserve_from_disk(&mut fixed, old);
                Some(fixed)
            }
            _ => None,
        };

        // Record ops for changed fields (compare with existing if possible).
        if let Some(old) = existing {
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

        let to_write = provenance_fixed.as_ref().unwrap_or(requirement);
        let wrote = object_store::write_object_if_changed(&self.objects_root, to_write)?;
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
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        // trace:BUG-1612 | ai:claude — serialize with every other store writer.
        let _lock = self.lock_store()?;
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

    /// Acquire the store write lock (re-entrant per thread). Every write path
    /// holds it across its read-modify-write window.
    // trace:BUG-1612 | ai:claude
    pub(crate) fn lock_store(&self) -> Result<super::store_lock::StoreWriteGuard> {
        super::store_lock::acquire(&self.root)
    }

    /// Whole-store save (the `DatabaseBackend::save` body), under the store
    /// write lock.
    ///
    /// With a load snapshot (a store from `load()`), this is a per-spec
    /// compare-and-swap, planned in full before anything is written:
    /// - a spec whose file is unchanged since the snapshot is written;
    /// - a spec whose file changed on disk after the snapshot (a concurrent
    ///   edit), and that this store left untouched, is skipped: the disk copy
    ///   is newer and nothing of the caller's is lost;
    /// - a spec whose file changed on disk AND that this store also changed is
    ///   a conflict: the save writes NOTHING and returns a
    ///   [`StoreConflictError`], because writing would revert the concurrent
    ///   edit and skipping would drop the caller's;
    /// - an absent object is deleted only if the snapshot holds it unchanged.
    ///
    /// After writing, the snapshot is refreshed for every object written,
    /// created or deleted, so the same store can be saved again. A store with
    /// no snapshot deletes nothing, and a spec whose on-disk `modified_at` is
    /// newer than the incoming copy is a conflict (TASK-1161).
    // trace:BUG-1612 | ai:claude
    pub(crate) fn save_reporting(&self, store: &RequirementsStore) -> Result<SaveReport> {
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        let _lock = self.lock_store()?;

        let existing = object_store::list_objects(&self.objects_root)?;
        let existing_specs: std::collections::HashSet<String> =
            existing.iter().map(|(s, _)| s.clone()).collect();

        let snapshot = store.loaded_objects.as_ref();
        let mut current_specs = std::collections::HashSet::new();
        // (what to write, fingerprint of the caller's in-memory copy)
        let mut to_write: Vec<(Requirement, Option<u64>)> = Vec::new();
        let mut conflicts: Vec<String> = Vec::new();
        let mut stale_untouched: Vec<String> = Vec::new();
        // Specs whose in-memory copy already equals the (newer) disk copy.
        let mut already_current: Vec<(String, u64)> = Vec::new(); // (id, fingerprint)
                                                                  // CR-8: a spec with no on-disk object is being CREATED by this save
                                                                  // (findings / report / legacy full-store filing paths) — stamp its
                                                                  // filing provenance. Captured lazily, once per save.
                                                                  // trace:CR-8 | ai:claude
        let mut filing_provenance: Option<crate::models::FilingProvenance> = None;

        for req in &store.requirements {
            let Some(ref spec_id) = req.spec_id else {
                continue;
            };
            current_specs.insert(spec_id.clone());
            let disk_text = object_store::read_object_text(&self.objects_root, spec_id)?;
            if let Some(snapshot) = snapshot {
                let loaded_fp = snapshot.disk(spec_id);
                let disk_fp = disk_text.as_deref().map(object_store::content_fingerprint);
                let unchanged_since_load = match (loaded_fp, disk_fp) {
                    (Some(l), Some(d)) => l == d,
                    // New in this session and still absent: this save creates it.
                    (None, None) => true,
                    _ => false,
                };
                if !unchanged_since_load {
                    // "Touched" is judged against the caller baseline (the
                    // in-memory copy as of the last load/save), never the disk
                    // fingerprint: a save can write more than the in-memory
                    // copy (provenance stamp, preserved fields).
                    let mine_fp = object_store::content_fingerprint(&serde_yaml::to_string(req)?);
                    if disk_fp == Some(mine_fp) {
                        already_current.push((spec_id.clone(), mine_fp));
                    } else if snapshot.baseline(spec_id) == Some(mine_fp) {
                        // Untouched by this caller; the disk copy is newer.
                        stale_untouched.push(spec_id.clone());
                    } else {
                        conflicts.push(spec_id.clone());
                    }
                    continue;
                }
            }
            let disk = disk_text
                .as_deref()
                .map(serde_yaml::from_str::<Requirement>);
            let req_to_write = match disk {
                Some(Ok(disk)) => {
                    // Stale-write guard (no snapshot): a strictly NEWER
                    // on-disk `modified_at` means a concurrent targeted write
                    // landed after this copy was taken. trace:TASK-1161
                    if snapshot.is_none() && disk.modified_at > req.modified_at {
                        conflicts.push(spec_id.clone());
                        continue;
                    }
                    Self::preserve_full_save_only_fields(req.clone(), &disk)
                }
                // Absent (or unparseable, as before): this save creates it.
                _ => {
                    let mut created = req.clone();
                    if created.filed_at.is_none() {
                        let p = filing_provenance.get_or_insert_with(crate::provenance::capture);
                        crate::provenance::stamp_with(&mut created, p);
                    }
                    created
                }
            };
            let caller_fp = match snapshot {
                Some(_) => Some(object_store::content_fingerprint(&serde_yaml::to_string(
                    req,
                )?)),
                None => None,
            };
            to_write.push((req_to_write, caller_fp));
        }

        // Deletion plan. Safety: never delete a file we couldn't parse
        // (BUG-96: a newer binary's serde variant would be destroyed), and
        // never delete an object this store was not loaded with, or one that
        // changed on disk after the load (BUG-1612: another writer's work).
        // trace:BUG-96 trace:BUG-1612 | ai:claude
        let mut to_delete: Vec<String> = Vec::new();
        let mut preserved_unparseable: Vec<String> = Vec::new();
        let mut kept_unloaded: Vec<String> = Vec::new();
        for spec_id in &existing_specs {
            if current_specs.contains(spec_id) {
                continue;
            }
            let Some(loaded_fp) = snapshot.and_then(|s| s.disk(spec_id)) else {
                kept_unloaded.push(spec_id.clone());
                continue;
            };
            let Some(text) = object_store::read_object_text(&self.objects_root, spec_id)? else {
                continue;
            };
            if object_store::content_fingerprint(&text) != loaded_fp {
                conflicts.push(spec_id.clone());
                continue;
            }
            if serde_yaml::from_str::<Requirement>(&text).is_err() {
                preserved_unparseable.push(spec_id.clone());
                continue;
            }
            to_delete.push(spec_id.clone());
        }

        // BUG-1613: merge the store-level fields this caller changed over the
        // CURRENT metadata.yaml (re-read under the lock) instead of writing the
        // caller's whole in-memory copy, which reverted concurrent changes.
        // trace:BUG-1613 | ai:claude
        let mine_meta = Self::extract_metadata(store);
        let meta_base = snapshot.and_then(|s| s.metadata_baseline());
        let disk_meta = self.read_metadata_if_present()?;
        let meta_plan = merge_metadata(meta_base.as_ref(), &mine_meta, disk_meta.as_ref())?;

        if !conflicts.is_empty() || meta_plan.is_err() {
            conflicts.sort();
            return Err(StoreConflictError {
                specs: conflicts,
                metadata_fields: meta_plan.err().unwrap_or_default(),
            }
            .into());
        }
        let meta_plan = meta_plan.unwrap_or(MetadataMerge { write: None });

        // ---- write phase ----
        if let Some(meta) = &meta_plan.write {
            self.save_metadata(meta)?;
        }
        if let Some(snapshot) = snapshot {
            snapshot.record_metadata(metadata_value(&mine_meta)?);
        }
        let mut written_specs: Vec<String> = Vec::new();
        for (req, caller_fp) in &to_write {
            let spec_id = req.spec_id.as_deref().unwrap_or_default();
            if object_store::write_object_if_changed(&self.objects_root, req)? {
                written_specs.push(spec_id.to_string());
            }
            if let (Some(snapshot), Some(caller_fp)) = (snapshot, caller_fp) {
                if let Some(text) = object_store::read_object_text(&self.objects_root, spec_id)? {
                    snapshot.record(
                        spec_id,
                        object_store::content_fingerprint(&text),
                        *caller_fp,
                    );
                }
            }
        }
        if let Some(snapshot) = snapshot {
            for (spec_id, fp) in &already_current {
                snapshot.record(spec_id, *fp, *fp);
            }
        }
        let mut deleted_specs: Vec<String> = Vec::new();
        for spec_id in &to_delete {
            let _ = object_store::delete_object(&self.objects_root, spec_id);
            if let Some(snapshot) = snapshot {
                snapshot.remove(spec_id);
            }
            deleted_specs.push(spec_id.clone());
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
        kept_unloaded.sort();
        stale_untouched.sort();
        Ok(SaveReport {
            kept_unloaded,
            stale_untouched,
        })
    }

    /// Per-spec compare-and-swap update (the git-store `update_spec_atomically`).
    ///
    /// Under the store write lock: read ONLY `target`'s object file (located by
    /// its `spec_id`, identity checked against its `id`), apply `update_fn`,
    /// and write back only that object with a targeted commit. No other object
    /// is read, written, or deleted. Just before the write the file is re-read
    /// and compared with the bytes the update started from, so a writer that
    /// bypasses the lock (an older binary, a hand edit) is detected and the
    /// update is refused instead of silently clobbering it.
    // trace:BUG-1612 | ai:claude
    pub fn update_spec_atomically<F>(
        &self,
        target: &Requirement,
        update_fn: F,
    ) -> Result<Option<Requirement>>
    where
        F: FnOnce(&mut Requirement),
    {
        self.update_spec_atomically_with_subject(target, None, update_fn)
    }

    /// [`Self::update_spec_atomically`] with an explicit commit subject. With
    /// `Some(subject)` the store commit reads `<subject>: update 1
    /// requirement` (the shape `bulk_update` writes), so a caller that names
    /// why it wrote (e.g. `aida edit --tags --force` dropping structural tags)
    /// keeps that audit line while taking the per-spec compare-and-swap.
    // trace:TASK-1506 | ai:claude
    pub fn update_spec_atomically_with_subject<F>(
        &self,
        target: &Requirement,
        commit_subject: Option<&str>,
        update_fn: F,
    ) -> Result<Option<Requirement>>
    where
        F: FnOnce(&mut Requirement),
    {
        let spec_id = target.spec_id.as_deref().ok_or_else(|| {
            anyhow::anyhow!("Cannot update a requirement without a spec_id in the git store")
        })?;
        let spec_id = object_store::canonical_spec_id(spec_id);
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        let _lock = self.lock_store()?;

        let path = object_store::object_path(&self.objects_root, &spec_id)?;
        let before = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("Failed to read {}", path.display())),
        };
        let current: Requirement = serde_yaml::from_slice(&before)
            .with_context(|| format!("Failed to parse {}", path.display()))?;
        if current.id != target.id {
            anyhow::bail!(
                "{spec_id} now names a different requirement (uuid {} on disk, {} expected); \
                 nothing was written",
                current.id,
                target.id
            );
        }

        let mut next = current.clone();
        update_fn(&mut next);
        if next.id != current.id || next.spec_id != current.spec_id {
            anyhow::bail!(
                "update of {spec_id} tried to change its id or spec_id; nothing was written"
            );
        }
        if serde_yaml::to_string(&next)? == serde_yaml::to_string(&current)? {
            return Ok(Some(next));
        }

        self.ensure_object_unchanged(&spec_id, &path, &before)?;
        if let Some(written) = self.stage_requirement_update(&next)? {
            let rel = object_store::relative_object_path(written)?;
            let message = match commit_subject {
                Some(subject) => format!("{subject}: update 1 requirement"),
                None => format!("update {}", written),
            };
            self.auto_commit_paths(&message, &[&rel]);
        }
        Ok(Some(next))
    }

    /// Compare-and-swap check: the object at `path` must still hold `before`.
    // trace:BUG-1612 | ai:claude
    fn ensure_object_unchanged(&self, spec_id: &str, path: &Path, before: &[u8]) -> Result<()> {
        let now = std::fs::read(path).ok();
        if now.as_deref() != Some(before) {
            anyhow::bail!(
                "concurrent modification of {spec_id}: its object changed on disk while this \
                 update held the store write lock (a writer that bypasses the lock). Nothing \
                 was written; re-run the command to apply it to the current version."
            );
        }
        Ok(())
    }

    /// Whole-store transaction, written per spec (the git-store
    /// `update_atomically`).
    ///
    /// Under the store write lock: load the store, apply `update_fn`, then diff
    /// the result against the loaded snapshot and write ONLY what the closure
    /// changed: modified specs (targeted writes that record the same oplog ops
    /// as `update_requirement`), specs it added, specs it removed from the
    /// loaded store, and `metadata.yaml` when a store-level field changed. One
    /// commit stages exactly those paths. An object the closure never saw (one
    /// that failed to parse, or appeared after the load) is never touched, so
    /// nothing is ever deleted that was absent from the snapshot.
    ///
    /// Every modified or removed spec is compare-and-swapped against its loaded
    /// copy, and every added spec must not exist yet, all before anything is
    /// written; a mismatch (a writer that bypassed the lock) refuses the whole
    /// transaction.
    // trace:BUG-1612 | ai:claude
    pub(crate) fn update_atomically_tracked<F>(
        &self,
        update_fn: F,
    ) -> Result<(RequirementsStore, AtomicWriteSummary)>
    where
        F: FnOnce(&mut RequirementsStore),
    {
        use std::collections::HashMap;

        crate::git_ops::ensure_store_write_safe(&self.root)?;
        let _lock = self.lock_store()?;

        let mut store = self.load()?;
        let meta_before = serde_yaml::to_string(&Self::extract_metadata(&store))?;
        let mut before: HashMap<String, (uuid::Uuid, String)> = HashMap::new();
        for req in &store.requirements {
            if let Some(sid) = req.spec_id.as_deref() {
                before.insert(sid.to_string(), (req.id, serde_yaml::to_string(req)?));
            }
        }

        update_fn(&mut store);

        let meta_changed = serde_yaml::to_string(&Self::extract_metadata(&store))? != meta_before;
        let mut changed: Vec<&Requirement> = Vec::new();
        let mut created: Vec<&Requirement> = Vec::new();
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for req in &store.requirements {
            let Some(sid) = req.spec_id.as_deref() else {
                continue;
            };
            if !seen.insert(sid) {
                anyhow::bail!(
                    "update left two requirements with spec_id {sid}; nothing was written"
                );
            }
            match before.get(sid) {
                Some((_, yaml)) => {
                    if &serde_yaml::to_string(req)? != yaml {
                        changed.push(req);
                    }
                }
                None => created.push(req),
            }
        }
        let removed: Vec<(&String, uuid::Uuid)> = before
            .iter()
            .filter(|(sid, _)| !seen.contains(sid.as_str()))
            .map(|(sid, (id, _))| (sid, *id))
            .collect();

        // Verify every precondition before the first write.
        for sid in changed
            .iter()
            .filter_map(|r| r.spec_id.as_deref())
            .chain(removed.iter().map(|(sid, _)| sid.as_str()))
        {
            let on_disk = object_store::read_object(&self.objects_root, sid)
                .ok()
                .and_then(|r| serde_yaml::to_string(&r).ok());
            if on_disk.as_deref() != before.get(sid).map(|(_, y)| y.as_str()) {
                anyhow::bail!(
                    "concurrent modification of {sid}: its object changed on disk while this \
                     update held the store write lock (a writer that bypasses the lock). \
                     Nothing was written; re-run the command."
                );
            }
        }
        for req in &created {
            let sid = req.spec_id.as_deref().unwrap_or_default();
            if object_store::object_exists(&self.objects_root, sid)? {
                anyhow::bail!(
                    "{sid} already exists on disk but was not in the loaded store (a concurrent \
                     add); nothing was written"
                );
            }
        }

        let mut summary = AtomicWriteSummary {
            metadata_changed: meta_changed,
            ..Default::default()
        };
        let mut paths: Vec<String> = Vec::new();
        if meta_changed {
            self.save_metadata(&Self::extract_metadata(&store))?;
            paths.push("metadata.yaml".to_string());
        }
        for req in &changed {
            if let Some(sid) = self.stage_requirement_update(req)? {
                paths.push(object_store::relative_object_path(sid)?);
                summary.written.push((*req).clone());
            }
        }
        let mut filing_provenance: Option<crate::models::FilingProvenance> = None;
        for req in &created {
            let mut new_req = (*req).clone();
            if new_req.filed_at.is_none() {
                let p = filing_provenance.get_or_insert_with(crate::provenance::capture);
                crate::provenance::stamp_with(&mut new_req, p);
            }
            self.record_op(
                new_req.id,
                crate::oplog::OpKind::Create {
                    title: new_req.title.clone(),
                    description: new_req.description.clone(),
                    req_type: format!("{:?}", new_req.req_type),
                    status: new_req.effective_status(),
                    priority: new_req.effective_priority(),
                },
            );
            object_store::write_object(&self.objects_root, &new_req)?;
            let sid = new_req.spec_id.clone().unwrap_or_default();
            paths.push(object_store::relative_object_path(&sid)?);
            summary.created.push(sid);
            summary.written.push(new_req);
        }
        for (sid, id) in &removed {
            object_store::delete_object(&self.objects_root, sid)?;
            paths.push(object_store::relative_object_path(sid)?);
            summary.deleted.push(*id);
        }
        // The in-memory copies of created specs carry the provenance stamp
        // that was written to disk.
        for written in &summary.written {
            if let Some(slot) = store.requirements.iter_mut().find(|r| r.id == written.id) {
                slot.filed_at = written.filed_at.clone();
            }
        }

        if !paths.is_empty() {
            let n_written = summary.written.len();
            let n_deleted = summary.deleted.len();
            let message = match (n_written, n_deleted) {
                (0, 0) => "chore: update requirements store metadata".to_string(),
                (1, 0) if summary.created.len() == 1 => {
                    format!("add {} — {}", summary.created[0], summary.written[0].title)
                }
                (1, 0) => format!(
                    "update {}",
                    summary.written[0].spec_id.as_deref().unwrap_or("?")
                ),
                (0, 1) => format!("delete {}", removed[0].0),
                (w, 0) => format!("chore: update {} requirements", w),
                (0, d) => format!("chore: delete {} requirements", d),
                (w, d) => format!("chore: update {} requirements, delete {}", w, d),
            };
            let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
            self.auto_commit_paths(&message, &path_refs);
        }
        // Keep the returned store's load snapshot current, so a caller that
        // later saves it does not mistake these writes for concurrent edits.
        if let Some(snapshot) = store.loaded_objects.as_ref() {
            for written in &summary.written {
                let Some(sid) = written.spec_id.as_deref() else {
                    continue;
                };
                let Some(mine) = store.requirements.iter().find(|r| r.id == written.id) else {
                    continue;
                };
                let caller_fp = object_store::content_fingerprint(&serde_yaml::to_string(mine)?);
                if let Some(text) = object_store::read_object_text(&self.objects_root, sid)? {
                    snapshot.record(sid, object_store::content_fingerprint(&text), caller_fp);
                }
            }
            for (sid, _) in &removed {
                snapshot.remove(sid);
            }
            // trace:BUG-1613 | ai:claude
            snapshot.record_metadata(metadata_value(&Self::extract_metadata(&store))?);
        }
        Ok((store, summary))
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
        // BUG-1612: remember which objects were on disk at load time, so a
        // later whole-store save only deletes objects this caller loaded.
        // trace:BUG-1612 | ai:claude
        let (requirements, snapshot) =
            object_store::load_all_objects_with_fingerprints(&self.objects_root)?;
        let mut store = self.assemble_store(meta, requirements);
        let snapshot = crate::models::LoadSnapshot::new(snapshot);
        // trace:BUG-1613 | ai:claude — the metadata baseline a save merges against.
        snapshot.record_metadata(metadata_value(&Self::extract_metadata(&store))?);
        store.loaded_objects = Some(snapshot);
        Ok(store)
    }

    fn save(&self, store: &RequirementsStore) -> Result<()> {
        self.save_reporting(store).map(|_| ())
    }

    /// Per-spec compare-and-swap.
    // trace:BUG-1612 | ai:claude
    fn update_spec_atomically<F>(
        &self,
        target: &Requirement,
        update_fn: F,
    ) -> Result<Option<Requirement>>
    where
        F: FnOnce(&mut Requirement),
    {
        GitBackend::update_spec_atomically(self, target, update_fn)
    }

    /// Whole-store transaction written per spec.
    // trace:BUG-1612 | ai:claude
    fn update_atomically<F>(&self, update_fn: F) -> Result<RequirementsStore>
    where
        F: FnOnce(&mut RequirementsStore),
    {
        Ok(self.update_atomically_tracked(update_fn)?.0)
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
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        // trace:BUG-1612 | ai:claude — serialize with every other store writer.
        let _lock = self.lock_store()?;
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
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        // trace:BUG-1612 | ai:claude — serialize with every other store writer.
        let _lock = self.lock_store()?;
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
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        // trace:BUG-1612 | ai:claude — serialize with every other store writer.
        let _lock = self.lock_store()?;
        let mut req = requirement;
        // CR-8: stamp filing provenance at creation (write-once; a caller that
        // already stamped keeps its stamp). trace:CR-8 | ai:claude
        crate::provenance::stamp_if_absent(&mut req);

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
        let path = queue_file_path(&self.root, &user_id);
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
                    users.push(decode_queue_user_filename(stem));
                }
            }
        }
        users.sort();
        users.dedup();
        Ok(users)
    }

    fn queue_add(&self, entry: QueueEntry) -> Result<()> {
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        let dir = self.root.join("registry/queues");
        std::fs::create_dir_all(&dir)?;
        // trace:TASK-951 — resolve the FILENAME case-insensitively (so `Joe`
        // appends to the existing `joe.yaml`) without rewriting the stored
        // `entry.user_id` (BUG-89: the persisted key stays the raw shell `$USER`).
        let user_id = self.resolve_queue_user(&entry.user_id);
        let path = queue_file_path(&self.root, &user_id);
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
            &[&queue_relative_path_for_file(&self.root, &path, &user_id)],
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
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        // trace:TASK-951 — fold case at the lookup boundary.
        let user_id = self.resolve_queue_user(user_id);
        let path = queue_file_path(&self.root, &user_id);
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
            &[&queue_relative_path_for_file(&self.root, &path, &user_id)],
        );
        Ok(())
    }

    fn queue_reorder(&self, user_id: &str, items: &[(uuid::Uuid, i64)]) -> Result<()> {
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        // trace:TASK-951 — fold case at the lookup boundary.
        let user_id = self.resolve_queue_user(user_id);
        let path = queue_file_path(&self.root, &user_id);
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
            &[&queue_relative_path_for_file(&self.root, &path, &user_id)],
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
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        // trace:TASK-951 — fold case at the lookup boundary.
        let user_id = self.resolve_queue_user(user_id);
        let path = queue_file_path(&self.root, &user_id);
        if !path.exists() {
            return Ok(());
        }

        if !completed_only {
            // Original behavior: nuke the entire queue file.
            std::fs::remove_file(&path)?;
            self.auto_commit_paths(
                "clear queue",
                &[&queue_relative_path_for_file(&self.root, &path, &user_id)],
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
            &[&queue_relative_path_for_file(&self.root, &path, &user_id)],
        );
        Ok(())
    }

    // TASK-1052: bulk-remove the given requirement ids from the user's queue
    // file in a single write + commit. Returns the removed entries. Used by
    // queue-GC, whose caller has already decided which ids are dead (target
    // spec archived/Completed/Rejected) from the cache, so this stays a dumb
    // set-membership prune. trace:TASK-1052 | ai:claude
    fn queue_remove_many(&self, user_id: &str, ids: &[uuid::Uuid]) -> Result<Vec<QueueEntry>> {
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // trace:TASK-951 — fold case at the lookup boundary.
        let user_id = self.resolve_queue_user(user_id);
        let path = queue_file_path(&self.root, &user_id);
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
            &[&queue_relative_path_for_file(&self.root, &path, &user_id)],
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
    /// The metadata as loaded by `bulk_writer()`: what `finish` merges
    /// against, so only the counters this batch bumped are written.
    // trace:BUG-1613 | ai:claude
    base: serde_yaml::Value,
    /// Spec ids this writer assigned from the counters (not caller-supplied).
    assigned: std::collections::HashSet<String>,
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
            if let Some(sid) = &req.spec_id {
                self.assigned.insert(sid.clone());
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
        crate::git_ops::ensure_store_write_safe(&self.backend.root)?;
        // trace:BUG-1612 | ai:claude — serialize with every other store writer.
        let _lock = self.backend.lock_store()?;

        // BUG-1613: re-read metadata.yaml under the lock and merge this batch's
        // counter bumps over it (max, never lowered) instead of writing the
        // snapshot taken at `bulk_writer()`, which reverted concurrent changes.
        // An id this writer assigned that now exists on disk was issued
        // concurrently from the same counter: refuse rather than overwrite it.
        // Both checks run before anything is written.
        // trace:BUG-1613 | ai:claude
        let disk_meta = self.backend.read_metadata_if_present()?;
        let meta_plan = merge_metadata(Some(&self.base), &self.metadata, disk_meta.as_ref())?;
        let mut collided: Vec<String> = Vec::new();
        for sid in &self.assigned {
            if object_store::object_exists(&self.backend.objects_root, sid)? {
                collided.push(sid.clone());
            }
        }
        if !collided.is_empty() || meta_plan.is_err() {
            collided.sort();
            return Err(StoreConflictError {
                specs: collided,
                metadata_fields: meta_plan.err().unwrap_or_default(),
            }
            .into());
        }
        let meta_plan = meta_plan.unwrap_or(MetadataMerge { write: None });

        // Persist all object YAMLs first (skipping unchanged just in case the
        // caller is re-running an idempotent import).
        let mut written: Vec<String> = Vec::new();
        // CR-8: batch-created specs get filing provenance too (one capture
        // for the whole batch; write-once). trace:CR-8 | ai:claude
        let provenance = crate::provenance::capture();
        for staged in &self.staged {
            let mut stamped = staged.clone();
            let is_new = stamped.spec_id.as_deref().is_none_or(|sid| {
                object_store::read_object(&self.backend.objects_root, sid).is_err()
            });
            if is_new {
                crate::provenance::stamp_with(&mut stamped, &provenance);
            }
            let req = &stamped;
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

        // Persist the bumped counters, merged over the current disk copy.
        // trace:BUG-1613 | ai:claude
        let mut paths: Vec<String> = written
            .iter()
            .filter_map(|sid| object_store::relative_object_path(sid).ok())
            .collect();
        if let Some(meta) = &meta_plan.write {
            self.backend.save_metadata(meta)?;
            paths.push("metadata.yaml".to_string());
        }
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

    // Stale-write guard: a full-store save carrying a spec whose on-disk
    // copy is NEWER (a concurrent targeted edit landed after the store was
    // loaded) must NOT silently revert the edit's core fields — the stale
    // spec is skipped (with a warning) while non-stale specs still write.
    // Regression for the concurrent-edit-then-stale-save sequence.
    // trace:TASK-1161 | ai:claude
    #[test]
    fn stale_full_save_cannot_revert_newer_concurrent_edit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        let base = chrono::Utc::now();

        let mut store = RequirementsStore::new();
        let mut req = Requirement::new("Original title".into(), "desc".into());
        req.spec_id = Some("TASK-100".into());
        req.tags.insert("old-tag".into());
        req.modified_at = base;
        let mut sibling = Requirement::new("Sibling".into(), "desc".into());
        sibling.spec_id = Some("TASK-101".into());
        sibling.modified_at = base;
        store.requirements.push(req);
        store.requirements.push(sibling);
        backend.save(&store).unwrap();

        // Snapshot the store as a stale caller would have loaded it.
        let mut stale_store = backend.load().unwrap();

        // Concurrent targeted edit lands AFTER that load: newer
        // modified_at, changed core fields (status + tags + title).
        let mut fresh = backend
            .get_requirement_by_spec_id("TASK-100")
            .unwrap()
            .unwrap();
        fresh.set_status_from_str("In Progress");
        fresh.tags.insert("fresh-tag".into());
        fresh.title = "Edited title".into();
        fresh.modified_at = base + chrono::Duration::seconds(10);
        backend.update_requirement(&fresh).unwrap();

        // Mutate the stale copy of TASK-100 (what a revert would write) and
        // legitimately edit the sibling (newer than disk → must still write).
        for r in stale_store.requirements.iter_mut() {
            match r.spec_id.as_deref() {
                Some("TASK-100") => {
                    r.tags.insert("stale-tag".into());
                    r.title = "Stale overwrite".into();
                }
                Some("TASK-101") => {
                    r.title = "Sibling updated".into();
                    r.modified_at = base + chrono::Duration::seconds(20);
                }
                _ => {}
            }
        }
        // BUG-1612: the stale copy of TASK-100 was edited AND changed on disk
        // after the load, so the save is a conflict: it refuses and writes
        // nothing (not even the sibling), instead of dropping either edit.
        let err = backend.save(&stale_store).unwrap_err();
        let conflict = err
            .downcast_ref::<StoreConflictError>()
            .expect("a typed store conflict");
        assert_eq!(conflict.specs, vec!["TASK-100".to_string()]);

        // The concurrent edit survives — nothing from the stale copy landed.
        let after = backend
            .get_requirement_by_spec_id("TASK-100")
            .unwrap()
            .unwrap();
        assert!(
            matches!(after.status, RequirementStatus::InProgress),
            "stale save must not revert status, got {:?}",
            after.status
        );
        assert_eq!(after.title, "Edited title");
        assert!(after.tags.contains("fresh-tag"));
        assert!(
            !after.tags.contains("stale-tag"),
            "stale copy's tag edit must not land"
        );
        assert_eq!(
            after.modified_at,
            base + chrono::Duration::seconds(10),
            "on-disk modified_at must stay the newer edit's timestamp"
        );
        let sib = backend
            .get_requirement_by_spec_id("TASK-101")
            .unwrap()
            .unwrap();
        assert_eq!(sib.title, "Sibling", "a refused save writes nothing");

        // Reload and retry: the sibling edit lands.
        let mut fresh_store = backend.load().unwrap();
        for r in fresh_store.requirements.iter_mut() {
            if r.spec_id.as_deref() == Some("TASK-101") {
                r.title = "Sibling updated".into();
            }
        }
        backend.save(&fresh_store).unwrap();
        let sib = backend
            .get_requirement_by_spec_id("TASK-101")
            .unwrap()
            .unwrap();
        assert_eq!(sib.title, "Sibling updated");

        // The skipped spec must NOT be deleted by the deletion-tracking pass.
        assert!(root.join("objects/TASK/000/TASK-100.yaml").exists());
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
        // BUG-688: this negative-existence check assumes a CASE-SENSITIVE
        // filesystem. On macOS/Windows (case-insensitive by default) `Joe.yaml`
        // resolves to the already-present `joe.yaml`, so `.exists()` is true —
        // a filesystem artifact of the test env, not a product bug. The case-fold
        // LOOKUP assertions above and the "both land in one queue" check below
        // are filesystem-independent and still run on every platform.
        // trace:BUG-688 | ai:claude
        #[cfg(target_os = "linux")]
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

    // BUG-1021: role queue identities contain `:`, which is not a portable
    // filename character. The logical queue user remains `role:implementer`,
    // while the git-canonical filename is escaped.
    // trace:BUG-1021 | ai:codex
    #[test]
    fn test_role_queue_user_uses_portable_filename() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();

        let entry = sample_queue_entry("role:implementer", 1000);
        backend.queue_add(entry.clone()).unwrap();

        assert!(root
            .join("registry/queues/role%3Aimplementer.yaml")
            .exists());
        assert_eq!(backend.queue_users().unwrap(), vec!["role:implementer"]);
        let listed = backend.queue_list("role:implementer", false).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].requirement_id, entry.requirement_id);
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

    /// BUG-756: a full-store save must not let an older/projection caller erase
    /// git-canonical fields it never loaded. The live failure was an auto-bump
    /// `Storage::update_atomically` save that completed one spec and rewrote
    /// unrelated deferred specs without their `deferred*` fields.
    #[test]
    fn full_store_save_preserves_git_canonical_fields_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();
        backend.save(&RequirementsStore::new()).unwrap();

        let mut req = Requirement::new("Deferred work".into(), "wait for trigger".into());
        req.spec_id = Some("TASK-756".into());
        req.deferred = true;
        req.deferred_at = Some(chrono::Utc::now());
        req.deferred_until = Some("when the regression reproduces".into());
        req.processing_record
            .push(crate::models::ProcessingRecord::new(
                "aida".into(),
                "completed via merge".into(),
            ));
        req.comments.push(
            crate::models::Comment::new("joe".into(), "keep this session id".into())
                .with_session_id(Some("session-12345678".into())),
        );
        let added = backend.add_requirement(req).unwrap();

        let mut store = backend.load().unwrap();
        let degraded = store
            .requirements
            .iter_mut()
            .find(|r| r.id == added.id)
            .expect("seeded requirement should load");
        degraded.title = "Deferred work, touched by old writer".into();
        degraded.deferred = false;
        degraded.deferred_at = None;
        degraded.deferred_until = None;
        degraded.processing_record.clear();
        degraded.comments[0].session_id = None;

        backend.save(&store).unwrap();

        let reloaded = backend
            .get_requirement_by_spec_id("TASK-756")
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.title, "Deferred work, touched by old writer");
        assert!(
            reloaded.deferred,
            "full-store save must preserve defer flag"
        );
        assert!(
            reloaded.deferred_at.is_some(),
            "full-store save must preserve defer timestamp"
        );
        assert_eq!(
            reloaded.deferred_until.as_deref(),
            Some("when the regression reproduces")
        );
        assert_eq!(reloaded.processing_record.len(), 1);
        assert_eq!(
            reloaded.comments[0].session_id.as_deref(),
            Some("session-12345678")
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

        // BUG-1612: a store built in memory (no load snapshot) deletes
        // nothing, even when objects on disk are absent from it.
        store
            .requirements
            .retain(|r| r.spec_id.as_deref() == Some("FR-001"));
        backend.save(&store).unwrap();
        assert!(root.join("objects/FR/000/FR-002.yaml").exists());

        // A caller that LOADED the store and removed FR-002 deletes it.
        let mut store = backend.load().unwrap();
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

    // A writer must fail before touching disk when the store is on the
    // disposable HEAD used by an interrupted rebase. trace:BUG-1229 | ai:codex
    #[test]
    fn detached_store_refuses_add_without_file_or_commit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".aida-store");
        crate::git_ops::init(&root).unwrap();
        crate::git_ops::configure_user(&root, "Test", "test@example.com").unwrap();
        std::fs::write(root.join("metadata.yaml"), "name: test\n").unwrap();
        crate::git_ops::add(&root, &["metadata.yaml"]).unwrap();
        crate::git_ops::commit(&root, "seed").unwrap();
        let before = crate::git_ops::head_sha(&root).unwrap();
        let detached = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["checkout", "--detach"])
            .status()
            .unwrap();
        assert!(detached.success());

        let backend = GitBackend::new(&root).unwrap();
        let mut req = Requirement::new("Must not land".into(), "guard test".into());
        req.spec_id = Some("BUG-1229".into());
        let err = backend.add_requirement(req).unwrap_err().to_string();

        assert!(err.contains("store worktree detached"), "{err}");
        assert!(err.contains("rebase --abort"), "{err}");
        assert_eq!(crate::git_ops::head_sha(&root).unwrap(), before);
        assert!(!root.join("objects/BUG/001/BUG-1229.yaml").exists());
    }

    // trace:CR-8 | ai:claude — a new spec gets filing provenance, an edit
    // (targeted or full-store) never changes it, and a pre-CR-8 spec with no
    // stamp loads and edits fine without acquiring one.
    #[test]
    fn cr8_add_stamps_filing_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap().with_auto_commit(false);
        let added = backend
            .add_requirement(Requirement::new("new".into(), "d".into()))
            .unwrap();
        let stamped = added.filed_at.clone().expect("new spec must be stamped");
        assert!(stamped.aida_version.is_some());
        let on_disk =
            object_store::read_object(&backend.objects_root, added.spec_id.as_deref().unwrap())
                .unwrap();
        assert_eq!(on_disk.filed_at, Some(stamped));
    }

    #[test]
    fn cr8_edits_never_change_filing_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap().with_auto_commit(false);
        let mut req = Requirement::new("t".into(), "d".into());
        req.filed_at = Some(crate::models::FilingProvenance {
            code_sha: Some("aaa".into()),
            ..Default::default()
        });
        let added = backend.add_requirement(req).unwrap();
        let spec_id = added.spec_id.clone().unwrap();
        let original = added.filed_at.clone();

        // Targeted edit that tries to rewrite AND one that drops the stamp.
        let mut rewrite = added.clone();
        rewrite.title = "renamed".into();
        rewrite.filed_at = Some(crate::models::FilingProvenance {
            code_sha: Some("bbb".into()),
            ..Default::default()
        });
        backend.update_requirement(&rewrite).unwrap();
        let mut dropped = rewrite.clone();
        dropped.filed_at = None;
        backend.update_requirement(&dropped).unwrap();
        let disk = object_store::read_object(&backend.objects_root, &spec_id).unwrap();
        assert_eq!(disk.title, "renamed");
        assert_eq!(disk.filed_at, original);

        // Full-store save carrying a rewritten stamp.
        let mut store = backend.load().unwrap();
        for r in &mut store.requirements {
            r.filed_at = None;
            r.description = "changed".into();
        }
        backend.save(&store).unwrap();
        let disk = object_store::read_object(&backend.objects_root, &spec_id).unwrap();
        assert_eq!(disk.description, "changed");
        assert_eq!(disk.filed_at, original);
    }

    #[test]
    fn cr8_legacy_spec_without_provenance_loads_and_stays_unstamped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap().with_auto_commit(false);
        let mut legacy = Requirement::new("legacy".into(), "d".into());
        legacy.spec_id = Some("TASK-9".into());
        object_store::write_object_if_changed(&backend.objects_root, &legacy).unwrap();
        let path = object_store::object_path(&backend.objects_root, "TASK-9").unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("filed_at"));

        let loaded = backend.load().unwrap();
        let mut req = loaded.requirements[0].clone();
        assert_eq!(req.filed_at, None);
        req.title = "edited".into();
        backend.update_requirement(&req).unwrap();
        let disk = object_store::read_object(&backend.objects_root, "TASK-9").unwrap();
        assert_eq!(disk.title, "edited");
        assert_eq!(
            disk.filed_at, None,
            "an edit must not stamp a pre-CR-8 spec"
        );
    }

    #[test]
    fn cr8_full_store_save_stamps_only_new_specs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap().with_auto_commit(false);
        let mut legacy = Requirement::new("legacy".into(), "d".into());
        legacy.spec_id = Some("TASK-1".into());
        object_store::write_object_if_changed(&backend.objects_root, &legacy).unwrap();

        let mut store = backend.load().unwrap();
        let mut fresh = Requirement::new("fresh".into(), "d".into());
        fresh.spec_id = Some("TASK-2".into());
        store.requirements.push(fresh);
        backend.save(&store).unwrap();

        let old = object_store::read_object(&backend.objects_root, "TASK-1").unwrap();
        let new = object_store::read_object(&backend.objects_root, "TASK-2").unwrap();
        assert_eq!(old.filed_at, None);
        assert!(
            new.filed_at.is_some(),
            "a spec created by a full-store save is stamped"
        );
    }

    // ---- BUG-1612: per-spec compare-and-swap, no whole-store rewrite -------

    /// A git-initialized store with `n` seeded specs (TASK-1..TASK-n).
    // trace:BUG-1612 | ai:claude
    fn bug1612_git_store(n: usize) -> (tempfile::TempDir, PathBuf, GitBackend, Vec<Requirement>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = GitBackend::new(&root).unwrap();
        backend.save(&RequirementsStore::new()).unwrap();
        for args in [
            &["init", "-q"][..],
            &["config", "user.email", "test@example.com"][..],
            &["config", "user.name", "Test"][..],
        ] {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
        }
        let mut seeded = Vec::new();
        for i in 1..=n {
            let mut r = Requirement::new(format!("Spec {i}"), format!("desc {i}"));
            r.spec_id = Some(format!("TASK-{i}"));
            seeded.push(backend.add_requirement(r).unwrap());
        }
        (dir, root, backend, seeded)
    }

    fn bug1612_head_files(root: &Path) -> Vec<String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "show",
                "--no-renames",
                "--name-only",
                "--pretty=format:",
                "HEAD",
            ])
            .output()
            .unwrap();
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect()
    }

    fn bug1612_external(title: &str, spec_id: &str) -> Requirement {
        let mut r = Requirement::new(title.into(), "written by another writer".into());
        r.spec_id = Some(spec_id.into());
        r
    }

    /// Two writers: a spec added by another writer between the atomic
    /// update's load and its write is NOT deleted, and a stale whole-store
    /// save of a snapshot that predates the add does not delete it either.
    // trace:BUG-1612 | ai:claude
    #[test]
    fn bug1612_concurrent_add_between_load_and_atomic_update_is_not_deleted() {
        let (_dir, root, backend, _) = bug1612_git_store(1);
        let stale_snapshot = backend.load().unwrap();

        let objects = root.join("objects");
        backend
            .update_atomically(|s| {
                let r = s
                    .requirements
                    .iter_mut()
                    .find(|r| r.spec_id.as_deref() == Some("TASK-1"))
                    .unwrap();
                r.title = "edited".into();
                // Writer B lands while writer A is between load and write.
                object_store::write_object(&objects, &bug1612_external("added by B", "TASK-2"))
                    .unwrap();
            })
            .unwrap();

        assert!(object_store::object_exists(&objects, "TASK-2").unwrap());
        assert_eq!(
            object_store::read_object(&objects, "TASK-1").unwrap().title,
            "edited"
        );
        // Targeted commit: only TASK-1's object (plus the oplog), never B's.
        let files = bug1612_head_files(&root);
        assert!(files.contains(&"objects/TASK/000/TASK-1.yaml".to_string()));
        assert!(
            !files.iter().any(|f| f.contains("TASK-2")),
            "the atomic update must not stage another writer's object: {files:?}"
        );

        // A whole-store save of a snapshot loaded before B's add keeps it.
        backend.save(&stale_snapshot).unwrap();
        assert!(
            object_store::object_exists(&objects, "TASK-2").unwrap(),
            "a stale whole-store save must not delete a spec it never loaded"
        );
    }

    /// Two real writer threads: A's atomic update holds the store lock while
    /// B adds a spec. B blocks until A finishes; both writes survive.
    // trace:BUG-1612 | ai:claude
    #[test]
    fn bug1612_two_writer_threads_add_and_atomic_update_both_survive() {
        let (_dir, root, backend, _) = bug1612_git_store(1);
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let root_b = root.clone();
        let writer_b = std::thread::spawn(move || {
            rx.recv().unwrap();
            let b = GitBackend::new(&root_b).unwrap();
            let mut r = Requirement::new("added by B".into(), "d".into());
            r.spec_id = Some("TASK-2".into());
            b.add_requirement(r).unwrap();
        });
        backend
            .update_atomically(|s| {
                tx.send(()).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(300));
                s.requirements[0].title = "edited by A".into();
            })
            .unwrap();
        writer_b.join().unwrap();

        let objects = root.join("objects");
        assert!(object_store::object_exists(&objects, "TASK-2").unwrap());
        assert_eq!(
            object_store::read_object(&objects, "TASK-1").unwrap().title,
            "edited by A"
        );
    }

    /// A concurrent modification of the SAME spec by a writer that bypasses
    /// the lock is detected and refused, never silently clobbered — on both
    /// the per-spec path and the whole-store transaction.
    // trace:BUG-1612 | ai:claude
    #[test]
    fn bug1612_concurrent_same_spec_modification_is_refused_not_clobbered() {
        let (_dir, root, backend, seeded) = bug1612_git_store(1);
        let objects = root.join("objects");
        let target = seeded[0].clone();

        let err = backend
            .update_spec_atomically(&target, |r| {
                let mut ext = r.clone();
                ext.title = "external edit".into();
                object_store::write_object(&objects, &ext).unwrap();
                r.title = "my edit".into();
            })
            .unwrap_err();
        assert!(err.to_string().contains("concurrent modification"), "{err}");
        assert_eq!(
            object_store::read_object(&objects, "TASK-1").unwrap().title,
            "external edit"
        );

        let err = backend
            .update_atomically(|s| {
                let mut ext = s.requirements[0].clone();
                ext.title = "second external edit".into();
                object_store::write_object(&objects, &ext).unwrap();
                s.requirements[0].title = "my second edit".into();
            })
            .unwrap_err();
        assert!(err.to_string().contains("concurrent modification"), "{err}");
        assert_eq!(
            object_store::read_object(&objects, "TASK-1").unwrap().title,
            "second external edit"
        );
    }

    /// Lock-respecting concurrent writers of the same spec serialize: no
    /// update is lost.
    // trace:BUG-1612 | ai:claude
    #[test]
    fn bug1612_concurrent_same_spec_updates_serialize_without_lost_writes() {
        let (_dir, root, _backend, seeded) = bug1612_git_store(1);
        let target = seeded[0].clone();
        let per_thread = 8;
        let handles: Vec<_> = (0..2)
            .map(|t| {
                let root = root.clone();
                let target = target.clone();
                std::thread::spawn(move || {
                    let b = GitBackend::new(&root).unwrap().with_auto_commit(false);
                    for i in 0..per_thread {
                        b.update_spec_atomically(&target, |r| {
                            r.tags.insert(format!("t{t}-{i}"));
                        })
                        .unwrap()
                        .unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let after = object_store::read_object(&root.join("objects"), "TASK-1").unwrap();
        assert_eq!(after.tags.len(), 2 * per_thread, "{:?}", after.tags);
    }

    /// The per-spec path reads and writes one object: it never lists (let
    /// alone parses) the store, and its commit stages only that object.
    // trace:BUG-1612 | ai:claude
    #[test]
    fn bug1612_per_spec_update_does_not_scan_the_store() {
        use crate::object_store::{FULL_SCAN_COUNT, OBJECT_LIST_COUNT};
        let (_dir, root, backend, seeded) = bug1612_git_store(5);
        let target = seeded[2].clone();

        OBJECT_LIST_COUNT.with(|c| c.set(0));
        FULL_SCAN_COUNT.with(|c| c.set(0));
        let updated = backend
            .update_spec_atomically(&target, |r| r.title = "targeted".into())
            .unwrap()
            .unwrap();
        assert_eq!(
            OBJECT_LIST_COUNT.with(|c| c.get()),
            0,
            "per-spec path listed the store"
        );
        assert_eq!(
            FULL_SCAN_COUNT.with(|c| c.get()),
            0,
            "per-spec path scanned by uuid"
        );
        assert_eq!(updated.title, "targeted");

        let files = bug1612_head_files(&root);
        let objects: Vec<&String> = files.iter().filter(|f| f.starts_with("objects/")).collect();
        assert_eq!(objects, vec!["objects/TASK/000/TASK-3.yaml"], "{files:?}");

        // A missing spec is Ok(None) and the closure does not run.
        let mut ghost = target.clone();
        ghost.spec_id = Some("TASK-99".into());
        let mut ran = false;
        assert!(backend
            .update_spec_atomically(&ghost, |_| ran = true)
            .unwrap()
            .is_none());
        assert!(!ran);

        // A spec_id that now names a different uuid is refused.
        let mut imposter = target.clone();
        imposter.id = uuid::Uuid::now_v7();
        assert!(backend.update_spec_atomically(&imposter, |_| {}).is_err());

        // The closure may not rename the spec.
        assert!(backend
            .update_spec_atomically(&target, |r| r.spec_id = Some("TASK-50".into()))
            .is_err());
    }

    /// The whole-store transaction still supports multi-spec edits, adds
    /// and explicit removals — writing only those objects.
    // trace:BUG-1612 | ai:claude
    #[test]
    fn bug1612_atomic_transaction_writes_only_touched_objects() {
        let (_dir, root, backend, _) = bug1612_git_store(4);
        let objects = root.join("objects");
        let store = backend
            .update_atomically(|s| {
                for r in s.requirements.iter_mut() {
                    if r.spec_id.as_deref() == Some("TASK-1") {
                        r.title = "one".into();
                    }
                }
                s.requirements
                    .retain(|r| r.spec_id.as_deref() != Some("TASK-2"));
                let prefix = s.get_type_prefix(&crate::models::RequirementType::Task);
                s.add_requirement_with_id(
                    Requirement::new("fresh".into(), "d".into()),
                    None,
                    prefix.as_deref(),
                );
            })
            .unwrap();
        let fresh = store.requirements.last().unwrap().clone();
        let fresh_sid = fresh.spec_id.clone().unwrap();

        assert!(!object_store::object_exists(&objects, "TASK-2").unwrap());
        assert!(object_store::object_exists(&objects, &fresh_sid).unwrap());
        assert!(fresh.filed_at.is_some(), "a created spec is stamped");
        let mut files: Vec<String> = bug1612_head_files(&root)
            .into_iter()
            .filter(|f| f.starts_with("objects/"))
            .collect();
        files.sort();
        let mut expected = vec![
            "objects/TASK/000/TASK-1.yaml".to_string(),
            "objects/TASK/000/TASK-2.yaml".to_string(),
            object_store::relative_object_path(&fresh_sid).unwrap(),
        ];
        expected.sort();
        assert_eq!(files, expected);

        // A no-op transaction writes and commits nothing.
        let head = |root: &Path| {
            String::from_utf8(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(root)
                    .args(["rev-parse", "HEAD"])
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
        };
        let before = head(&root);
        backend.update_atomically(|_| {}).unwrap();
        assert_eq!(head(&root), before);
    }

    /// The `Storage` façade on a directory store goes through the same
    /// per-spec paths.
    // trace:BUG-1612 | ai:claude
    #[test]
    fn bug1612_storage_facade_uses_per_spec_paths() {
        let (_dir, root, _backend, seeded) = bug1612_git_store(2);
        let storage = crate::storage::Storage::new(&root);
        let objects = root.join("objects");
        storage
            .update_atomically(|s| {
                s.requirements[0].description = "facade edit".into();
                object_store::write_object(&objects, &bug1612_external("late", "TASK-3")).unwrap();
            })
            .unwrap();
        assert!(object_store::object_exists(&objects, "TASK-3").unwrap());

        let updated = storage
            .update_spec_atomically(&seeded[1], |r| r.title = "facade per-spec".into())
            .unwrap()
            .unwrap();
        assert_eq!(updated.title, "facade per-spec");
        assert_eq!(
            object_store::read_object(&objects, "TASK-2").unwrap().title,
            "facade per-spec"
        );
    }

    /// A whole-store save of a stale snapshot never reverts a concurrent edit
    /// (even one that kept `modified_at`) to a spec it did not touch, and
    /// still writes its own changes.
    // trace:BUG-1612 | ai:claude
    #[test]
    fn bug1612_stale_save_skips_untouched_concurrent_edits() {
        let (_dir, root, backend, seeded) = bug1612_git_store(3);
        let objects = root.join("objects");
        let mut stale = backend.load().unwrap();
        backend
            .update_spec_atomically(&seeded[0], |r| r.title = "concurrent".into())
            .unwrap();
        for r in stale.requirements.iter_mut() {
            if r.spec_id.as_deref() == Some("TASK-3") {
                r.title = "mine".into();
            }
        }
        let report = backend.save_reporting(&stale).unwrap();
        assert_eq!(
            object_store::read_object(&objects, "TASK-1").unwrap().title,
            "concurrent"
        );
        assert_eq!(
            object_store::read_object(&objects, "TASK-3").unwrap().title,
            "mine"
        );
        assert_eq!(report.stale_untouched, vec!["TASK-1"]);
    }

    /// Editing, or removing, a spec that changed on disk after the load is a
    /// conflict: the save returns a typed error and writes nothing.
    // trace:BUG-1612 | ai:claude
    #[test]
    fn bug1612_stale_save_conflict_is_an_error_and_writes_nothing() {
        let (_dir, root, backend, seeded) = bug1612_git_store(3);
        let objects = root.join("objects");
        let mut stale = backend.load().unwrap();
        backend
            .update_spec_atomically(&seeded[0], |r| r.title = "concurrent 1".into())
            .unwrap();
        backend
            .update_spec_atomically(&seeded[1], |r| r.title = "concurrent 2".into())
            .unwrap();
        for r in stale.requirements.iter_mut() {
            match r.spec_id.as_deref() {
                Some("TASK-1") => r.title = "clobber".into(),
                Some("TASK-3") => r.title = "mine".into(),
                _ => {}
            }
        }
        stale
            .requirements
            .retain(|r| r.spec_id.as_deref() != Some("TASK-2"));
        let err = backend.save(&stale).unwrap_err();
        let conflict = err.downcast_ref::<StoreConflictError>().unwrap();
        assert_eq!(conflict.specs, vec!["TASK-1", "TASK-2"]);
        assert_eq!(
            object_store::read_object(&objects, "TASK-1").unwrap().title,
            "concurrent 1"
        );
        assert_eq!(
            object_store::read_object(&objects, "TASK-2").unwrap().title,
            "concurrent 2"
        );
        assert_eq!(
            object_store::read_object(&objects, "TASK-3").unwrap().title,
            "Spec 3",
            "a refused save writes nothing"
        );
    }

    /// One loaded store saved repeatedly (the aida-server pattern): every
    /// save lands, including repeated edits of one spec, a spec created in
    /// the session and then edited, and one created and then deleted.
    // trace:BUG-1612 | ai:claude
    #[test]
    fn bug1612_repeated_saves_of_one_loaded_store_all_land() {
        let (_dir, root, backend, _) = bug1612_git_store(2);
        let objects = root.join("objects");
        let title = |sid: &str| object_store::read_object(&objects, sid).unwrap().title;
        let mut store = backend.load().unwrap();
        let edit = |store: &mut RequirementsStore, sid: &str, t: &str| {
            let r = store
                .requirements
                .iter_mut()
                .find(|r| r.spec_id.as_deref() == Some(sid))
                .unwrap();
            r.title = t.into();
        };

        // Second and third edits to the same spec.
        edit(&mut store, "TASK-1", "first");
        backend.save(&store).unwrap();
        edit(&mut store, "TASK-1", "second");
        backend.save(&store).unwrap();
        edit(&mut store, "TASK-1", "third");
        backend.save(&store).unwrap();
        assert_eq!(title("TASK-1"), "third");

        // Create in session, then edit.
        let mut fresh = Requirement::new("created".into(), "d".into());
        fresh.spec_id = Some("TASK-10".into());
        store.requirements.push(fresh);
        backend.save(&store).unwrap();
        edit(&mut store, "TASK-10", "created then edited");
        backend.save(&store).unwrap();
        assert_eq!(title("TASK-10"), "created then edited");

        // Create in session, then delete.
        let mut doomed = Requirement::new("doomed".into(), "d".into());
        doomed.spec_id = Some("TASK-11".into());
        store.requirements.push(doomed);
        backend.save(&store).unwrap();
        assert!(object_store::object_exists(&objects, "TASK-11").unwrap());
        store
            .requirements
            .retain(|r| r.spec_id.as_deref() != Some("TASK-11"));
        backend.save(&store).unwrap();
        assert!(!object_store::object_exists(&objects, "TASK-11").unwrap());

        // A clone has its own snapshot: saving a clone does not make the
        // original's stale copy look current.
        let mut original = backend.load().unwrap();
        let mut clone = original.clone();
        edit(&mut clone, "TASK-2", "from clone");
        backend.save(&clone).unwrap();
        edit(&mut original, "TASK-2", "from original");
        assert!(backend
            .save(&original)
            .unwrap_err()
            .downcast_ref::<StoreConflictError>()
            .is_some());
        assert_eq!(title("TASK-2"), "from clone");
    }

    /// After `update_atomically` returns, saving the returned store is not
    /// mistaken for a concurrent edit.
    // trace:BUG-1612 | ai:claude
    #[test]
    fn bug1612_store_returned_by_atomic_update_can_be_saved() {
        let (_dir, root, backend, _) = bug1612_git_store(1);
        let mut store = backend
            .update_atomically(|s| s.requirements[0].title = "atomic".into())
            .unwrap();
        store.requirements[0].title = "then saved".into();
        backend.save(&store).unwrap();
        assert_eq!(
            object_store::read_object(&root.join("objects"), "TASK-1")
                .unwrap()
                .title,
            "then saved"
        );
    }

    /// The store write lock file is empty and never staged, even in a store
    /// with no `.gitignore` and a whole-tree `git add -A .` (db sync /
    /// auto-push). It is excluded from git the first time it is taken.
    // trace:BUG-1612 | ai:claude
    #[test]
    fn bug1612_lock_file_is_never_staged() {
        let (_dir, root, backend, _) = bug1612_git_store(1);
        assert!(!root.join(".gitignore").exists());
        let mut r = Requirement::new("more".into(), "d".into());
        r.spec_id = Some("TASK-2".into());
        backend.add_requirement(r).unwrap();
        let lock = root.join(".aida").join("store-write.lock");
        assert!(lock.exists());
        assert_eq!(
            std::fs::metadata(&lock).unwrap().len(),
            0,
            "lock file stays empty"
        );

        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap()
        };
        git(&["add", "-A", "."]);
        git(&["commit", "-q", "-m", "sync"]);
        let tracked = git(&["ls-files"]);
        assert!(
            !tracked.contains("store-write.lock"),
            "the lock file must never be committed: {tracked}"
        );
        assert!(git(&["status", "--porcelain"]).trim().is_empty());
    }

    /// Review repro (a): a spec created through this store is stamped with
    /// filing provenance on disk only. A later concurrent edit of it must not
    /// make a save that does not touch it fail.
    // trace:BUG-1612 | ai:claude
    #[test]
    fn bug1612_created_spec_then_concurrent_edit_is_not_a_false_conflict() {
        let (_dir, root, backend, _) = bug1612_git_store(1);
        let objects = root.join("objects");
        let mut store = backend.load().unwrap();
        let mut fresh = Requirement::new("nine".into(), "d".into());
        fresh.spec_id = Some("TASK-9".into());
        store.requirements.push(fresh.clone());
        backend.save(&store).unwrap();
        assert!(object_store::read_object(&objects, "TASK-9")
            .unwrap()
            .filed_at
            .is_some());

        // Another writer edits TASK-9.
        backend
            .update_spec_atomically(&fresh, |r| r.title = "edited elsewhere".into())
            .unwrap();

        // This store edits only TASK-1 and saves: no conflict.
        store.requirements[0].title = "mine".into();
        let report = backend.save_reporting(&store).unwrap();
        assert_eq!(report.stale_untouched, vec!["TASK-9"]);
        assert_eq!(
            object_store::read_object(&objects, "TASK-1").unwrap().title,
            "mine"
        );
        assert_eq!(
            object_store::read_object(&objects, "TASK-9").unwrap().title,
            "edited elsewhere"
        );
    }

    /// Review repro (b): a field the save preserves from disk (here
    /// `risk_notes`, cleared in memory) must not make the spec look touched
    /// afterwards.
    // trace:BUG-1612 | ai:claude
    #[test]
    fn bug1612_preserved_field_then_concurrent_edit_is_not_a_false_conflict() {
        let (_dir, root, backend, seeded) = bug1612_git_store(1);
        let objects = root.join("objects");
        backend
            .update_spec_atomically(&seeded[0], |r| r.risk_notes = Some("risky".into()))
            .unwrap();
        let mut store = backend.load().unwrap();
        store.requirements[0].risk_notes = None;
        backend.save(&store).unwrap();
        // The whole-store save preserves the on-disk risk_notes (BUG-756).
        assert_eq!(
            object_store::read_object(&objects, "TASK-1")
                .unwrap()
                .risk_notes
                .as_deref(),
            Some("risky")
        );

        backend
            .update_spec_atomically(&seeded[0], |r| r.title = "edited elsewhere".into())
            .unwrap();

        let mut five = Requirement::new("five".into(), "d".into());
        five.spec_id = Some("TASK-5".into());
        store.requirements.push(five);
        backend.save(&store).unwrap();
        assert!(object_store::object_exists(&objects, "TASK-5").unwrap());
        assert_eq!(
            object_store::read_object(&objects, "TASK-1").unwrap().title,
            "edited elsewhere"
        );
    }

    // ---- BUG-1613: metadata.yaml merges or conflicts, never reverts ----

    /// A second writer on the same store (its own backend instance).
    // trace:BUG-1613 | ai:claude
    fn bug1613_other(root: &Path) -> GitBackend {
        GitBackend::new(root).unwrap()
    }

    /// A seeded store with per-prefix numbering, so each type has a counter.
    fn bug1613_per_prefix_store() -> (tempfile::TempDir, PathBuf, GitBackend) {
        let (dir, root, backend, _) = bug1612_git_store(1);
        backend
            .update_atomically(|s| {
                s.id_config.numbering = crate::models::NumberingStrategy::PerPrefix;
            })
            .unwrap();
        (dir, root, backend)
    }

    fn bug1613_disk_meta(root: &Path) -> StoreMetadata {
        bug1613_other(root).load_metadata().unwrap()
    }

    fn bug1613_bug(title: &str) -> Requirement {
        let mut r = Requirement::new(title.into(), "d".into());
        r.req_type = crate::models::RequirementType::Bug;
        r
    }

    /// A save that changed only a spec leaves metadata.yaml alone, so a
    /// feature added concurrently between its load and save survives.
    // trace:BUG-1613 | ai:claude
    #[test]
    fn bug1613_save_keeps_concurrent_feature_add() {
        let (_dir, root, backend, _) = bug1612_git_store(2);
        let mut stale = backend.load().unwrap();
        bug1613_other(&root)
            .update_atomically(|s| {
                s.add_feature("Concurrent", "CONC").unwrap();
            })
            .unwrap();
        for r in stale.requirements.iter_mut() {
            if r.spec_id.as_deref() == Some("TASK-1") {
                r.title = "mine".into();
            }
        }
        backend.save(&stale).unwrap();
        let meta = bug1613_disk_meta(&root);
        assert!(
            meta.features.iter().any(|f| f.name == "Concurrent"),
            "the concurrent feature add was reverted"
        );
        assert_eq!(
            object_store::read_object(&root.join("objects"), "TASK-1")
                .unwrap()
                .title,
            "mine"
        );
    }

    /// Two writers change DIFFERENT store-level fields: both changes survive.
    // trace:BUG-1613 | ai:claude
    #[test]
    fn bug1613_save_merges_different_metadata_fields() {
        let (_dir, root, backend, _) = bug1612_git_store(1);
        let mut stale = backend.load().unwrap();
        bug1613_other(&root)
            .update_atomically(|s| {
                s.add_feature("Concurrent", "CONC").unwrap();
            })
            .unwrap();
        stale.title = "My title".into();
        backend.save(&stale).unwrap();
        let meta = bug1613_disk_meta(&root);
        assert_eq!(meta.title, "My title");
        assert!(meta.features.iter().any(|f| f.name == "Concurrent"));
        // The concurrent add bumped the feature counter; it is not lowered.
        assert!(meta.next_feature_number >= 2);

        // The refreshed baseline lets the same store be saved again without
        // reverting the concurrent feature (its copy is still stale).
        stale.description = "again".into();
        backend.save(&stale).unwrap();
        let meta = bug1613_disk_meta(&root);
        assert_eq!(meta.description, "again");
        assert!(meta.features.iter().any(|f| f.name == "Concurrent"));
    }

    /// Two writers change the SAME store-level field to different values: the
    /// loser gets a typed conflict and nothing is written.
    // trace:BUG-1613 | ai:claude
    #[test]
    fn bug1613_save_same_metadata_field_is_a_conflict() {
        let (_dir, root, backend, _) = bug1612_git_store(1);
        let mut stale = backend.load().unwrap();
        bug1613_other(&root)
            .update_atomically(|s| s.title = "theirs".into())
            .unwrap();
        stale.title = "mine".into();
        for r in stale.requirements.iter_mut() {
            r.title = "spec edit".into();
        }
        let err = backend.save(&stale).unwrap_err();
        let conflict = err
            .downcast_ref::<StoreConflictError>()
            .expect("a typed store conflict");
        assert!(conflict.specs.is_empty());
        assert_eq!(conflict.metadata_fields, vec!["title"]);
        assert!(err.to_string().contains("title"));
        assert_eq!(bug1613_disk_meta(&root).title, "theirs");
        assert_eq!(
            object_store::read_object(&root.join("objects"), "TASK-1")
                .unwrap()
                .title,
            "Spec 1",
            "a refused save writes nothing"
        );
    }

    /// A concurrent counter bump between load and save survives: counters are
    /// merged as the max of disk and caller, never lowered, while the caller's
    /// own counter bump (another prefix) lands too.
    // trace:BUG-1613 | ai:claude
    #[test]
    fn bug1613_save_keeps_concurrent_counter_bump() {
        let (_dir, root, backend) = bug1613_per_prefix_store();
        let mut stale = backend.load().unwrap();
        let theirs = bug1613_other(&root)
            .add_requirement(Requirement::new("theirs".into(), "d".into()))
            .unwrap();
        let their_sid = theirs.spec_id.clone().unwrap();
        let their_prefix = their_sid.rsplit_once('-').unwrap().0.to_string();
        let disk_before = bug1613_disk_meta(&root);
        let their_counter = disk_before.prefix_counters[&their_prefix];

        // The caller files a BUG (a different prefix) and adds a feature.
        let prefix = stale.get_type_prefix(&crate::models::RequirementType::Bug);
        stale.add_requirement_with_id(bug1613_bug("mine"), None, prefix.as_deref());
        stale.add_feature("Mine", "MINE").unwrap();
        assert_ne!(prefix.as_deref(), Some(their_prefix.as_str()));
        backend.save(&stale).unwrap();

        let meta = bug1613_disk_meta(&root);
        assert_eq!(
            meta.prefix_counters[&their_prefix], their_counter,
            "the concurrent counter bump was reverted"
        );
        assert!(meta.prefix_counters[prefix.as_deref().unwrap()] >= 2);
        assert!(meta.features.iter().any(|f| f.name == "Mine"));
        assert!(object_store::object_exists(&root.join("objects"), &their_sid).unwrap());
    }

    /// A caller whose in-memory counter would re-issue an id a concurrent
    /// writer already used gets a conflict on that spec; the other object and
    /// the higher counter are untouched.
    // trace:BUG-1613 | ai:claude
    #[test]
    fn bug1613_save_reissued_id_is_a_conflict() {
        let (_dir, root, backend) = bug1613_per_prefix_store();
        let mut stale = backend.load().unwrap();
        let theirs = bug1613_other(&root)
            .add_requirement(bug1613_bug("theirs"))
            .unwrap();
        let sid = theirs.spec_id.clone().unwrap();
        let counter = bug1613_disk_meta(&root).prefix_counters["BUG"];
        let prefix = stale.get_type_prefix(&crate::models::RequirementType::Bug);
        stale.add_requirement_with_id(bug1613_bug("mine"), None, prefix.as_deref());
        assert_eq!(
            stale.requirements.last().unwrap().spec_id.as_deref(),
            Some(sid.as_str())
        );
        let err = backend.save(&stale).unwrap_err();
        let conflict = err.downcast_ref::<StoreConflictError>().unwrap();
        assert_eq!(conflict.specs, vec![sid.clone()]);
        assert_eq!(
            object_store::read_object(&root.join("objects"), &sid)
                .unwrap()
                .title,
            "theirs"
        );
        assert_eq!(bug1613_disk_meta(&root).prefix_counters["BUG"], counter);
    }

    /// BulkWriter::finish merges its counter bumps over the CURRENT
    /// metadata.yaml: a feature added and a counter bumped (another prefix)
    /// between `bulk_writer()` and `finish()` both survive.
    // trace:BUG-1613 | ai:claude
    #[test]
    fn bug1613_bulk_writer_keeps_concurrent_metadata_changes() {
        let (_dir, root, backend) = bug1613_per_prefix_store();
        let mut writer = backend.bulk_writer().unwrap();
        let other = bug1613_other(&root);
        other
            .update_atomically(|s| {
                s.add_feature("Concurrent", "CONC").unwrap();
            })
            .unwrap();
        let theirs = other
            .add_requirement(Requirement::new("theirs".into(), "d".into()))
            .unwrap();
        let their_prefix = theirs
            .spec_id
            .as_deref()
            .unwrap()
            .rsplit_once('-')
            .unwrap()
            .0
            .to_string();
        let their_counter = bug1613_disk_meta(&root).prefix_counters[&their_prefix];

        writer.add(bug1613_bug("bulk 1")).unwrap();
        writer.add(bug1613_bug("bulk 2")).unwrap();
        assert_eq!(writer.finish("test(bulk)").unwrap(), 2);

        let meta = bug1613_disk_meta(&root);
        assert!(
            meta.features.iter().any(|f| f.name == "Concurrent"),
            "the concurrent feature add was reverted"
        );
        assert_eq!(meta.prefix_counters[&their_prefix], their_counter);
        assert!(meta.prefix_counters["BUG"] >= 3, "the batch's bumps landed");
        let loaded = backend.load().unwrap();
        assert_eq!(
            loaded
                .requirements
                .iter()
                .filter(|r| r.title.starts_with("bulk "))
                .count(),
            2
        );
    }

    /// A BulkWriter id that a concurrent writer issued from the same counter
    /// meanwhile is a conflict: finish writes nothing and the other object and
    /// counter survive.
    // trace:BUG-1613 | ai:claude
    #[test]
    fn bug1613_bulk_writer_reissued_id_is_a_conflict() {
        let (_dir, root, backend) = bug1613_per_prefix_store();
        let mut writer = backend.bulk_writer().unwrap();
        let theirs = bug1613_other(&root)
            .add_requirement(bug1613_bug("theirs"))
            .unwrap();
        let sid = theirs.spec_id.clone().unwrap();
        let counter = bug1613_disk_meta(&root).prefix_counters["BUG"];
        let mine = writer.add(bug1613_bug("bulk")).unwrap().clone();
        assert_eq!(mine.spec_id.as_deref(), Some(sid.as_str()));
        let err = writer.finish("test(bulk)").unwrap_err();
        let conflict = err.downcast_ref::<StoreConflictError>().unwrap();
        assert_eq!(conflict.specs, vec![sid.clone()]);
        assert_eq!(
            object_store::read_object(&root.join("objects"), &sid)
                .unwrap()
                .title,
            "theirs"
        );
        assert_eq!(bug1613_disk_meta(&root).prefix_counters["BUG"], counter);
    }

    /// The merge is pure field logic: untouched fields keep disk, counters
    /// take the max, and a store with no baseline still never lowers one.
    // trace:BUG-1613 | ai:claude
    #[test]
    fn bug1613_merge_metadata_rules() {
        let mut base = StoreMetadata::default();
        base.prefix_counters.insert("BUG".into(), 5);
        let base_v = metadata_value(&base).unwrap();
        let mut disk = base.clone();
        disk.prefix_counters.insert("BUG".into(), 9);
        disk.description = "disk".into();
        disk.next_spec_number = 4;
        let mut mine = base.clone();
        mine.next_spec_number = 12;
        mine.prefix_counters.insert("BUG".into(), 7);
        mine.prefix_counters.insert("TASK".into(), 2);
        mine.name = "mine".into();

        let merged = merge_metadata(Some(&base_v), &mine, Some(&disk))
            .unwrap()
            .unwrap()
            .write
            .unwrap();
        assert_eq!(merged.prefix_counters["BUG"], 9);
        assert_eq!(merged.prefix_counters["TASK"], 2);
        assert_eq!(merged.next_spec_number, 12);
        assert_eq!(merged.name, "mine");
        assert_eq!(merged.description, "disk");

        // Nothing changed by the writer: nothing to write.
        let noop = merge_metadata(Some(&base_v), &base, Some(&disk))
            .unwrap()
            .unwrap();
        assert!(noop.write.is_none());

        // No baseline (a store not loaded from the backend): the writer's
        // fields win, as before, but the counter is not lowered.
        let legacy = merge_metadata(None, &mine, Some(&disk))
            .unwrap()
            .unwrap()
            .write
            .unwrap();
        assert_eq!(legacy.description, "");
        assert_eq!(legacy.prefix_counters["BUG"], 9);
    }
}
