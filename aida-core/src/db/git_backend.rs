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

#[cfg(all(unix, feature = "native"))]
#[path = "work_intake.rs"]
mod work_intake;

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
    /// Whether the staged write under the current store lock includes an
    /// advisor-gated status advance (`TASK-1737`).
    // trace:TASK-1737 | ai:antigravity
    pending_authority_advance: std::sync::atomic::AtomicBool,
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

/// What a [`GitBackend::bulk_update_atomically`] batch wrote and what it left
/// alone. Every count is a candidate the caller offered that the in-lock
/// re-read rejected, so a sweep can report why it wrote fewer specs than it
/// selected.
// trace:BUG-1671 | ai:claude
#[derive(Debug, Default, Clone)]
pub struct BulkAtomicReport {
    /// The specs the batch kept, as written (the fresh object with the
    /// caller's mutation applied). A caller with a write-through cache upserts
    /// exactly these.
    pub written: Vec<Requirement>,
    /// Candidates whose stored object no longer qualified: the caller's
    /// predicate said no, or the spec_id now names a different uuid.
    pub skipped_changed: usize,
    /// Candidates whose stored object is gone (deleted since selection).
    pub skipped_missing: usize,
    /// Candidates whose stored object could not be read or parsed. Counted,
    /// never fatal: one corrupt object must not abort a whole sweep.
    pub skipped_unreadable: usize,
}

impl BulkAtomicReport {
    /// How many candidates the batch declined to write, for any reason.
    // trace:BUG-1671 | ai:claude
    pub fn skipped(&self) -> usize {
        self.skipped_changed + self.skipped_missing + self.skipped_unreadable
    }
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

/// The user-facing name of a `metadata.yaml` field, for conflict messages.
/// `metadata_fields` keeps the raw field names for callers that match on them.
// trace:BUG-1641 | ai:claude
fn metadata_field_label(field: &str) -> &str {
    match field {
        "name" => "project name",
        "title" => "project title",
        "description" => "project description",
        "users" => "user list",
        "teams" => "team list",
        "id_config" => "ID format settings",
        "features" => "feature list",
        "next_feature_number" => "feature numbering",
        "next_spec_number" => "ID numbering",
        "prefix_counters" => "per-prefix ID numbering",
        "meta_counters" => "internal ID numbering",
        "relationship_definitions" => "relationship types",
        "reaction_definitions" => "reaction types",
        "type_definitions" => "requirement types",
        "allowed_prefixes" => "allowed ID prefixes",
        "restrict_prefixes" => "ID prefix restriction",
        "ai_prompts" => "AI prompt settings",
        "baselines" => "baselines",
        other => other,
    }
}

impl std::fmt::Display for StoreConflictError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut what: Vec<String> = self.specs.clone();
        if !self.metadata_fields.is_empty() {
            let labels: Vec<&str> = self
                .metadata_fields
                .iter()
                .map(|field| metadata_field_label(field))
                .collect();
            what.push(format!("the project's {}", labels.join(", ")));
        }
        write!(
            f,
            "refusing to save: this save and a concurrent edit both changed {} after this \
             store was loaded. Nothing was written; reload and retry.",
            what.join(", ")
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
    // Absent built-in keys mean AIDA's defaults, matching `RequirementsStore`;
    // WORK metadata writes omit unchanged built-ins (see work_intake).
    // trace:STORY-1742 | ai:claude
    #[serde(default = "crate::models::default_reaction_definitions")]
    reaction_definitions: Vec<crate::models::ReactionDefinition>,
    #[serde(default)]
    meta_counters: std::collections::HashMap<String, u32>,
    #[serde(default = "crate::models::default_type_definitions")]
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
/// a save never lowers a counter and re-issues an ID. That also means a caller
/// cannot lower a counter, or remove a per-prefix key, through an ordinary
/// save: the higher disk value is kept. The one supported way to do that is
/// [`RequirementsStore::reset_id_counters`], which marks the store so the
/// merge treats the counters like any other field (see [`merge_metadata`]).
// trace:BUG-1613 trace:BUG-1641 | ai:claude
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

/// Raise every ID counter in `target` to at least its value in `other` (per
/// key for the per-prefix maps). Never lowers one.
// trace:BUG-1641 | ai:claude
fn raise_counters_to(target: &mut StoreMetadata, other: &StoreMetadata) {
    target.next_feature_number = target.next_feature_number.max(other.next_feature_number);
    target.next_spec_number = target.next_spec_number.max(other.next_spec_number);
    for (map, theirs) in [
        (&mut target.prefix_counters, &other.prefix_counters),
        (&mut target.meta_counters, &other.meta_counters),
    ] {
        for (key, value) in theirs {
            let slot = map.entry(key.clone()).or_insert(*value);
            *slot = (*slot).max(*value);
        }
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
/// - ID counters are never a conflict: they take the max of disk and writer,
///   unless `reset_counters` is set (the caller reset them on purpose through
///   [`RequirementsStore::reset_id_counters`]). Then the counters follow the
///   three rules above like any other field, so a reset lands (lowered values
///   and removed per-prefix keys included) when only this writer changed them,
///   and a concurrent counter change is a conflict instead of being lowered.
///
/// `base = None` (a store not loaded from this backend) means every field is
/// the writer's, as before, but counters are still never lowered unless
/// `reset_counters` is set. `disk = None` (no `metadata.yaml` yet) writes the
/// writer's copy.
// trace:BUG-1613 trace:BUG-1641 | ai:claude
fn merge_metadata(
    base: Option<&serde_yaml::Value>,
    mine: &StoreMetadata,
    disk: Option<&StoreMetadata>,
    reset_counters: bool,
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
        if !reset_counters && METADATA_COUNTER_FIELDS.contains(&name) {
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
    /// [`DatabaseBackend::get_requirement_by_spec_id`] for the id exactly
    /// as given (no node-alias expansion).
    fn get_requirement_by_exact_spec_id(&self, spec_id: &str) -> Result<Option<Requirement>> {
        if let Some(req) = self.get_requirement_by_object_file(spec_id)? {
            return Ok(Some(req));
        }
        // File doesn't exist by spec_id — try agreed_id scan. A malformed id
        // was already answered not-found above (BUG-599).
        let spec_id = object_store::canonical_spec_id(spec_id);
        if !object_store::valid_spec_id_format(&spec_id) {
            return Ok(None);
        }
        #[cfg(test)]
        super::cache_refresh::test_count("agreed_id_store_scan");
        let files = object_store::list_objects(&self.objects_root)?;
        for (_name, path) in &files {
            if let Ok(req) = object_store::read_object_from_path(path) {
                // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
                if req
                    .agreed_id
                    .as_deref()
                    .is_some_and(|a| crate::id_identity::same_id(a, &spec_id))
                {
                    return Ok(Some(req));
                }
            }
        }
        Ok(None)
    }

    /// The object file `spec_id` names, read directly: no agreed-id scan of
    /// the store and no node-alias expansion. A caller whose id index has
    /// already ruled out every agreed-id match uses this instead of
    /// [`Self::get_requirement_by_exact_spec_id`], whose scan reads every
    /// object in the store.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-018 | ai:claude
    pub(crate) fn get_requirement_by_object_file(
        &self,
        spec_id: &str,
    ) -> Result<Option<Requirement>> {
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
        // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude — leading
        // zeros are insignificant: `TASK-0042` reads `TASK-42.yaml`. Only
        // the one shard directory is listed. With several twins (a
        // pre-migration collision) the first in sorted order answers; write
        // paths refuse the ambiguity before reaching here.
        if let Some(stem) = object_store::numeric_twin_stems(&self.objects_root, &spec_id)?
            .into_iter()
            .next()
        {
            return Ok(Some(object_store::read_object(&self.objects_root, &stem)?));
        }
        Ok(None)
    }

    /// The UUID of the requirement already holding `id` (as its object file
    /// under any padding, or as an agreed id under any padding), for
    /// mint-time admission. The agreed-id half scans the store's text only
    /// when no object file matches.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
    pub fn id_holder(&self, id: &str) -> Result<Option<uuid::Uuid>> {
        for stem in object_store::numeric_twin_stems(&self.objects_root, id)? {
            if let Ok(req) = object_store::read_object(&self.objects_root, &stem) {
                return Ok(Some(req.id));
            }
        }
        self.agreed_id_holder(id, None)
    }

    /// The UUID of a requirement whose `agreed_id` names `id`'s number under
    /// any padding. `skip_stem` excludes the object filed under exactly that
    /// literal id (the file a same-literal re-import replaces).
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
    fn agreed_id_holder(&self, id: &str, skip_stem: Option<&str>) -> Result<Option<uuid::Uuid>> {
        if crate::id_identity::IdKey::parse(id).is_none() {
            return Ok(None);
        }
        Ok(self.agreed_id_index()?.holder(id, skip_stem))
    }

    /// Every object's `agreed_id`, keyed by numeric identity, from ONE text
    /// pass over the object files. Built once per add or batch, so admitting
    /// N rows costs one scan instead of N (most objects carry an agreed id,
    /// so the owning UUID is read from the file's top-level `id:` line, and
    /// the YAML is parsed only when that line is missing).
    // trace:TASK-B6B1278890774EF993D2EBC327862470-149 | ai:claude
    pub(crate) fn agreed_id_index(&self) -> Result<AgreedIdIndex> {
        let mut by_key: std::collections::HashMap<String, Vec<(String, uuid::Uuid)>> =
            std::collections::HashMap::new();
        for (name, path) in object_store::list_objects(&self.objects_root)? {
            let Ok(text) = crate::read_atomic(&path) else {
                continue;
            };
            let field = |key: &str| {
                text.lines().find_map(|line| {
                    line.strip_prefix(key)
                        .map(|v| v.trim().trim_matches('"').trim_matches('\'').to_string())
                })
            };
            let Some(agreed) =
                field("agreed_id:").filter(|v| !v.is_empty() && v != "~" && v != "null")
            else {
                continue;
            };
            let Some(key) = crate::id_identity::IdKey::parse(&agreed) else {
                continue;
            };
            let owner = match field("id:").and_then(|v| uuid::Uuid::parse_str(&v).ok()) {
                Some(u) => u,
                None => match object_store::read_object_from_path(&path) {
                    Ok(req) => req.id,
                    Err(_) => continue,
                },
            };
            by_key
                .entry(key.to_string())
                .or_default()
                .push((name, owner));
        }
        Ok(AgreedIdIndex { by_key })
    }

    /// Create a new git backend rooted at the given directory.
    /// Creates the directory structure if it doesn't exist.
    pub fn new(root: &Path) -> Result<Self> {
        let objects_root = root.join("objects");
        let metadata_path = root.join("metadata.yaml");

        #[cfg(all(unix, feature = "native"))]
        if !objects_root.is_dir() || !metadata_path.is_file() {
            crate::transport_policy::refuse_unsupported(root)?;
        }
        std::fs::create_dir_all(&objects_root)
            .with_context(|| format!("Failed to create objects dir: {}", objects_root.display()))?;

        Ok(Self {
            root: root.to_path_buf(),
            objects_root,
            metadata_path,
            dispenser: None,
            auto_commit: true,
            oplog_enabled: true,
            pending_authority_advance: std::sync::atomic::AtomicBool::new(false),
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

    // trace:TASK-1737 | ai:antigravity
    fn with_authority_trailers(&self, message: &str, record_authority: bool) -> String {
        if !record_authority {
            return message.to_string();
        }
        match crate::lifecycle::resolve_commit_authority(&self.root) {
            Some(auth) => format!("{message}{}", auth.format_trailers()),
            None => message.to_string(),
        }
    }

    /// Stage every YAML the backend is responsible for and commit. Used by
    /// the bulk `save()` path that legitimately touches the entire object
    /// tree. For per-operation paths, prefer `auto_commit_paths` so we don't
    /// pull in unrelated drift from sibling files.
    /// trace:BUG-1-040 | ai:claude
    fn path_present_or_tracked(&self, rel: &str) -> bool {
        self.root.join(rel).exists()
            || crate::git_ops::host_git_command(&self.root)
                .args(["ls-files", "--error-unmatch", "--", rel])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
    }

    fn auto_commit(&self, message: &str) {
        let record_authority = self
            .pending_authority_advance
            .swap(false, std::sync::atomic::Ordering::SeqCst);
        if !self.auto_commit || !crate::git_ops::is_git_repo(&self.root) {
            return;
        }
        // A contained session leaves the commit to the host, and says so.
        // trace:BUG-1929 trace:BUG-2140 | ai:antigravity
        if crate::git_ops::store_commit_deferred_to_host(&self.root) {
            crate::git_ops::notice_store_commit_deferred(&self.root);
            return;
        }
        if let Err(e) = crate::git_ops::refuse_embedded_repos(&self.root) {
            eprintln!("warning: store change written but not committed: {e:#}");
            return;
        }
        if self.path_present_or_tracked("objects") {
            if let Err(e) = crate::git_ops::add_all(&self.root, "objects") {
                eprintln!("warning: store change written but not committed: {e:#}");
                return;
            }
        }
        if self.metadata_path.exists() {
            if let Err(e) = crate::git_ops::add(&self.root, &["metadata.yaml"]) {
                eprintln!("warning: store change written but not committed: {e:#}");
                return;
            }
        }
        if self.root.join("oplog.yaml").exists() {
            if let Err(e) = crate::git_ops::add(&self.root, &["oplog.yaml"]) {
                eprintln!("warning: store change written but not committed: {e:#}");
                return;
            }
        }
        let commit_message = self.with_authority_trailers(message, record_authority);
        if let Err(e) = crate::git_ops::commit(&self.root, &commit_message) {
            // Never succeed silently: the write is on disk but uncommitted.
            // trace:BUG-1929 | ai:claude
            eprintln!("warning: store change written but not committed: {e:#}");
        }
    }

    /// Stage only the listed paths (relative to repo root) and commit. Skips
    /// the commit when nothing is staged so no-op operations don't churn
    /// commit history. Use `git add -A <path>` semantics so deletions are
    /// captured along with adds/modifications.
    // trace:BUG-1-040 trace:BUG-2140 | ai:antigravity
    fn auto_commit_paths(&self, message: &str, paths: &[&str]) {
        let record_authority = self
            .pending_authority_advance
            .swap(false, std::sync::atomic::Ordering::SeqCst);
        if !self.auto_commit || !crate::git_ops::is_git_repo(&self.root) {
            return;
        }
        // trace:BUG-1929 trace:BUG-2140 | ai:antigravity
        if crate::git_ops::store_commit_deferred_to_host(&self.root) {
            crate::git_ops::notice_store_commit_deferred(&self.root);
            return;
        }
        if let Err(e) = crate::git_ops::refuse_embedded_repos(&self.root) {
            eprintln!("warning: store change written but not committed: {e:#}");
            return;
        }
        for p in paths {
            if !self.path_present_or_tracked(p) {
                continue;
            }
            // `git add -A <path>` so a removed file is staged as a deletion
            // (plain `git add` only handles add/modify).
            // Route through the shared git helper so targeted store writes get
            // the same stale-index-lock recovery as bulk saves. trace:BUG-1164 | ai:codex
            if let Err(e) = crate::git_ops::add_all(&self.root, p) {
                eprintln!("warning: store change written but not committed: {e:#}");
                return;
            }
        }
        // Always include oplog.yaml when present — every targeted op records
        // an op and we want it captured in the same commit.
        // BUG-2140: when staging oplog.yaml, also stage any object/metadata/block
        // files from earlier deferred oplog operations (whose target_id is in
        // uncommitted `oplog.yaml` entries not already covered by `paths`),
        // without staging unrelated concurrent object writes (BUG-1612).
        if self.root.join("oplog.yaml").exists() {
            if let Err(e) = self.stage_oplog_and_deferred_targets(paths) {
                eprintln!("warning: store change written but not committed: {e:#}");
                return;
            }
        }
        // Don't make an empty commit if nothing was actually staged.
        // Store git goes through the hardened host builder (TASK-1775).
        // trace:TASK-1775 | ai:claude
        let staged = crate::git_ops::host_git_command(&self.root)
            .args(["diff", "--cached", "--quiet"])
            .status()
            .map(|s| !s.success()) // non-zero exit means there ARE staged changes
            .unwrap_or(false);
        if staged {
            let commit_message = self.with_authority_trailers(message, record_authority);
            if let Err(e) = crate::git_ops::commit(&self.root, &commit_message) {
                // trace:BUG-1929 | ai:claude
                eprintln!("warning: store change written but not committed: {e:#}");
            }
        }
    }

    /// Read and parse a requirement YAML from `HEAD:<rel_path>` when that path
    /// exists in `HEAD`. Returns `Ok(None)` when `HEAD` or `HEAD:<rel_path>`
    /// does not exist; propagates git or YAML parse failures as `Err`.
    // trace:BUG-2140 | ai:antigravity
    fn read_head_object(&self, rel_path: &str) -> Result<Option<Requirement>> {
        let spec = format!("HEAD:{rel_path}");
        let exists = crate::git_ops::host_git_command(&self.root)
            .args(["cat-file", "-e", &spec])
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !exists {
            return Ok(None);
        }
        let out = crate::git_ops::host_git_command(&self.root)
            .args(["show", &spec])
            .output()
            .with_context(|| format!("failed to run git show {spec}"))?;
        if !out.status.success() {
            anyhow::bail!(
                "git show {spec} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let req = serde_yaml::from_slice::<Requirement>(&out.stdout)
            .with_context(|| format!("failed to parse {spec}"))?;
        Ok(Some(req))
    }

    /// Stage `oplog.yaml` along with any object/metadata/block files belonging
    /// to earlier deferred oplog operations whose `target_id` is not already
    /// covered by `paths` (BUG-2140), while preserving BUG-1612's targeted
    /// commit invariant so unrelated unlogged object writes are never staged.
    ///
    /// Propagates any `oplog.yaml`, `HEAD:oplog.yaml`, or dirty object parse
    /// failure as `Err` so `auto_commit_paths` never commits `oplog.yaml`
    /// without its companion objects, and skips the `HEAD` probe when
    /// `oplog.yaml` is clean relative to `HEAD`.
    // trace:BUG-2140 trace:BUG-1612 | ai:antigravity
    fn stage_oplog_and_deferred_targets(&self, paths: &[&str]) -> Result<()> {
        let head_has_oplog = crate::git_ops::host_git_command(&self.root)
            .args(["cat-file", "-e", "HEAD:oplog.yaml"])
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if head_has_oplog {
            let oplog_clean = crate::git_ops::host_git_command(&self.root)
                .args(["diff", "--quiet", "HEAD", "--", "oplog.yaml"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if oplog_clean {
                return Ok(());
            }
        }

        let oplog_path = self.root.join("oplog.yaml");
        let log = crate::oplog::OpLog::load(&oplog_path).context("failed to load oplog.yaml")?;
        if !log.operations.is_empty() {
            let committed_ids: std::collections::HashSet<uuid::Uuid> = if head_has_oplog {
                let out = crate::git_ops::host_git_command(&self.root)
                    .args(["show", "HEAD:oplog.yaml"])
                    .output()
                    .context("failed to run git show HEAD:oplog.yaml")?;
                if !out.status.success() {
                    anyhow::bail!(
                        "git show HEAD:oplog.yaml failed: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    );
                }
                let head_log: crate::oplog::OpLog = serde_yaml::from_slice(&out.stdout)
                    .context("failed to parse HEAD:oplog.yaml")?;
                head_log.operations.into_iter().map(|op| op.id).collect()
            } else {
                std::collections::HashSet::new()
            };

            let mut uncommitted_targets: std::collections::HashSet<uuid::Uuid> = log
                .operations
                .iter()
                .filter(|op| !committed_ids.contains(&op.id))
                .map(|op| op.target_id)
                .collect();

            for p in paths {
                if p.starts_with("objects/") {
                    let disk_path = self.root.join(p);
                    if disk_path.exists() {
                        let req = object_store::read_object_from_path(&disk_path)?;
                        uncommitted_targets.remove(&req.id);
                    } else if let Some(req) = self.read_head_object(p)? {
                        uncommitted_targets.remove(&req.id);
                    }
                }
            }

            if !uncommitted_targets.is_empty() {
                if self.objects_root.exists() {
                    let status_out = crate::git_ops::host_git_command(&self.root)
                        .args([
                            "status",
                            "--porcelain",
                            "-z",
                            "--untracked-files=all",
                            "--",
                            "objects",
                        ])
                        .output()
                        .context("failed to inspect dirty store object paths")?;
                    if !status_out.status.success() {
                        anyhow::bail!(
                            "git status -- objects failed: {}",
                            String::from_utf8_lossy(&status_out.stderr).trim()
                        );
                    }
                    let mut entries = status_out
                        .stdout
                        .split(|&b| b == 0)
                        .filter(|s| !s.is_empty());
                    while let Some(entry) = entries.next() {
                        if entry.len() < 4 {
                            continue;
                        }
                        let status_x = entry[0];
                        let rel = std::str::from_utf8(&entry[3..])
                            .context("non-UTF-8 path in store git status")?;
                        if matches!(status_x, b'R' | b'C') {
                            let _ = entries.next();
                        }
                        if !rel.ends_with(".yaml") || paths.contains(&rel) {
                            continue;
                        }
                        let disk_path = self.root.join(rel);
                        let req_opt = if disk_path.exists() {
                            Some(object_store::read_object_from_path(&disk_path)?)
                        } else {
                            self.read_head_object(rel)?
                        };
                        if let Some(req) = req_opt {
                            if uncommitted_targets.contains(&req.id) {
                                crate::git_ops::add_all(&self.root, rel)?;
                            }
                        }
                    }
                }
                if self.metadata_path.exists() {
                    crate::git_ops::add(&self.root, &["metadata.yaml"])?;
                }
                if self.root.join("registry/blocks.yaml").exists() {
                    crate::git_ops::add(&self.root, &["registry/blocks.yaml"])?;
                }
            }
        }
        crate::git_ops::add(&self.root, &["oplog.yaml"])
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
        // No `exists()` pre-check: a lock-free reader (`queue_list`) can race a
        // writer's temp+rename of this file, and on Windows the open itself
        // can transiently fail (`NotFound` / `PermissionDenied`) while the
        // rename is in flight. `read_atomic` retries those there (and returns
        // immediately on Unix, where they are never transient); a `NotFound`
        // that survives the retry is a genuinely absent file, i.e. the empty
        // queue. trace:BUG-1677 | ai:claude
        let content = match crate::fs_atomic::read_atomic(path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Failed to read queue file {}", path.display()))
            }
        };
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
            pending_authority_advance: std::sync::atomic::AtomicBool::new(false),
        };
        backend.load_metadata_only()
    }

    /// Save metadata to the metadata.yaml file.
    fn save_metadata(&self, meta: &StoreMetadata) -> Result<()> {
        self.admit_metadata(meta)?;
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
            // trace:BUG-2017 | ai:codex
            dispenser: crate::allocation_authority::dispenser(&self.root, self.dispenser.clone()),
            loaded_objects: None,
            id_counters_reset: crate::models::CounterResetFlag::default(),
        }
    }

    /// Begin a bulk-add session. Buffers new requirements in memory while
    /// assigning IDs via the live metadata counters, then flushes all YAMLs
    /// + a counter-bumped metadata.yaml in a single commit when `finish` is
    ///   called. Avoids the load-iterate-write pattern of `update_atomically`
    /// + `save`, which is overkill when the operation is purely additive.
    ///   trace:FR-1-002 | ai:claude
    pub fn bulk_writer(&self) -> Result<BulkWriter<'_>> {
        // trace:STORY-1742 | ai:claude — WORK defers identities to `finish`.
        #[cfg(all(unix, feature = "native"))]
        let work = self.work_admission()?.is_some();
        #[cfg(not(all(unix, feature = "native")))]
        let work = false;
        let metadata = self.load_metadata()?;
        // trace:BUG-1613 | ai:claude — the baseline `finish` merges against.
        let base = metadata_value(&metadata)?;
        Ok(BulkWriter {
            backend: self,
            work,
            metadata,
            base,
            assigned: std::collections::HashSet::new(),
            staged: Vec::new(),
            agreed: None,
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
        // Preserve data omitted by full-store projection callers. Targeted
        // edits can still explicitly clear a verify block.
        // trace:STORY-1673 | ai:codex
        if incoming.verify.is_none() {
            incoming.verify = disk.verify.clone();
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

        // Context preservation/stamping is shared with targeted and transaction writes.
        // trace:STORY-1674 | ai:codex
        crate::provenance::preserve_from_disk(&mut incoming, disk);
        if disk.hypothesis_lock.is_some() {
            incoming.hypothesis_lock = disk.hypothesis_lock.clone();
        }

        incoming
    }

    /// Preflight hypothesis locks against the canonical graph under StoreLock.
    /// Append lock-only writes for rows affected indirectly by a result change.
    /// Call once for the ENTIRE transaction, before metadata, oplog or YAML writes.
    // trace:STORY-1675 | ai:codex
    fn prepare_hypothesis_updates(
        &self,
        updates: &mut Vec<Requirement>,
        deleted: &[String],
    ) -> Result<()> {
        use std::collections::{HashMap, HashSet};
        self.admit_allocations(updates)?;
        if updates.is_empty() && deleted.is_empty() {
            return Ok(());
        }
        let mut ids = HashSet::new();
        let mut specs = HashSet::new();
        for row in updates.iter() {
            if row.spec_id.is_none() {
                anyhow::bail!("cannot update requirement without spec_id; nothing was written");
            }
            if !ids.insert(row.id)
                || row
                    .spec_id
                    .as_deref()
                    .is_some_and(|sid| !specs.insert(object_store::canonical_spec_id(sid)))
            {
                anyhow::bail!("duplicate requirement in mutation batch; nothing was written");
            }
        }
        // Refuse to overwrite an existing object whose lock evidence cannot
        // be read. Unrelated unreadable objects keep the loader's usual policy.
        let mut existing = Vec::with_capacity(updates.len());
        for next in updates.iter() {
            let old = match next.spec_id.as_deref() {
                Some(sid) if object_store::object_exists(&self.objects_root, sid)? => Some(
                    object_store::read_object(&self.objects_root, sid).with_context(|| {
                        format!(
                            "cannot validate hypothesis evidence for {sid}; nothing was written"
                        )
                    })?,
                ),
                _ => None,
            };
            existing.push(old);
        }
        // Ordinary single-row refinements cannot create or remove a result
        // boundary. Keep the BUG-1612 no-store-scan contract for those edits.
        // An edge change, a status change into or out of Completed, or any
        // hypothesis tag/stamp requires the graph. trace:STORY-1675 | ai:claude
        let completed = |r: &Requirement| r.effective_status().eq_ignore_ascii_case("completed");
        if deleted.is_empty()
            && updates.iter().zip(&existing).all(|(next, old)| {
                old.as_ref().is_some_and(|old| {
                    old.id == next.id
                        && old.hypothesis_lock.is_none()
                        && next.hypothesis_lock.is_none()
                        && !crate::hypothesis::has_tag(old)
                        && !crate::hypothesis::has_tag(next)
                        && !completed(old)
                        && !completed(next)
                        && old.relationships == next.relationships
                })
            })
        {
            return Ok(());
        }
        let before = object_store::load_all_objects(&self.objects_root)?;
        let mut after = before.clone();
        after.retain(|r| !r.spec_id.as_ref().is_some_and(|s| deleted.contains(s)));
        let by_spec: HashMap<_, _> = before
            .iter()
            .filter_map(|r| {
                r.spec_id
                    .as_deref()
                    .map(|s| (object_store::canonical_spec_id(s), r))
            })
            .collect();
        // An update carrying an existing spec_id under a NEW uuid replaces that
        // ordinary row (re-run imports, identity repair): the old row leaves
        // the planned graph. Hypothesis identities can never be replaced.
        // trace:STORY-1675 | ai:claude
        let mut replaced = HashSet::new();
        for update in updates.iter() {
            let Some(old) = update
                .spec_id
                .as_deref()
                .and_then(|s| by_spec.get(&object_store::canonical_spec_id(s)))
            else {
                continue;
            };
            if old.id == update.id {
                continue;
            }
            if old.hypothesis_lock.is_some() || crate::hypothesis::has_tag(old) {
                anyhow::bail!(
                    "{} cannot replace a hypothesis identity; nothing was written",
                    old.display_id()
                );
            }
            replaced.insert(old.id);
        }
        after.retain(|r| !replaced.contains(&r.id));
        let mut indices: HashMap<_, _> = after.iter().enumerate().map(|(i, r)| (r.id, i)).collect();
        for update in updates.iter() {
            if let Some(&i) = indices.get(&update.id) {
                after[i] = update.clone();
            } else {
                indices.insert(update.id, after.len());
                after.push(update.clone());
            }
        }
        crate::hypothesis::enforce(&before, &mut after)?;
        let update_indices: HashMap<_, _> =
            updates.iter().enumerate().map(|(i, r)| (r.id, i)).collect();
        let old_locks: HashMap<_, _> = before.iter().map(|r| (r.id, &r.hypothesis_lock)).collect();
        for row in after {
            if let Some(&i) = update_indices.get(&row.id) {
                updates[i] = row;
            } else if old_locks
                .get(&row.id)
                .is_some_and(|old| **old != row.hypothesis_lock)
            {
                updates.push(row);
            }
        }
        Ok(())
    }

    // trace:BUG-2017 | ai:codex — repair/import cannot advance internal global counters.
    fn admit_metadata(&self, incoming: &StoreMetadata) -> Result<()> {
        let mode = crate::allocation_authority::resolve(&self.root)?;
        let incoming_ids: Vec<_> = incoming
            .users
            .iter()
            .map(|u| (u.id, u.spec_id.clone()))
            .chain(incoming.teams.iter().map(|t| (t.id, t.spec_id.clone())))
            .collect();
        if mode.authoritative && !incoming_ids.iter().any(|(_, id)| id.as_deref() == Some("")) {
            return Ok(());
        }
        let prior = self.load_metadata()?;
        let prior_ids: Vec<_> = prior
            .users
            .iter()
            .map(|u| (u.id, u.spec_id.clone()))
            .chain(prior.teams.iter().map(|t| (t.id, t.spec_id.clone())))
            .collect();
        // Failed infallible metadata allocation must not publish an empty ID,
        // including on HOME. Preserve an unchanged pre-existing legacy value.
        // trace:BUG-2017 | ai:codex
        if incoming_ids.iter().any(|(uuid, id)| {
            id.as_deref() == Some("")
                && !prior_ids
                    .iter()
                    .any(|(old_uuid, old_id)| old_uuid == uuid && old_id == id)
        }) {
            // trace:BUG-b6b1278890774ef993d2ebc327862470-001 | ai:claude
            anyhow::bail!(
                "{}",
                crate::allocation_authority::failure_message(
                    "metadata ID allocation failed",
                    "nothing was written"
                )
            )
        }
        if mode.authoritative {
            return Ok(());
        }
        crate::allocation_authority::admit_meta_ids(&self.root, &incoming_ids, &prior_ids)?;
        if incoming.next_spec_number != prior.next_spec_number
            || incoming.next_feature_number != prior.next_feature_number
            || incoming.prefix_counters != prior.prefix_counters
            || incoming.meta_counters != prior.meta_counters
        {
            anyhow::bail!(
                "global counter changes refuse on a non-authoritative clone; nothing was written"
            )
        }
        Ok(())
    }

    // trace:BUG-2017 | ai:codex
    fn admit_allocations(&self, incoming: &[Requirement]) -> Result<()> {
        // The legacy infallible model API signals allocation failure with an
        // empty ID. Refuse before metadata/oplog even on explicit HOME.
        // trace:BUG-2017 | ai:codex
        if incoming.iter().any(|r| r.spec_id.as_deref() == Some("")) {
            // trace:BUG-b6b1278890774ef993d2ebc327862470-001 | ai:claude
            anyhow::bail!(
                "{}",
                crate::allocation_authority::failure_message(
                    "requirement ID allocation failed",
                    "no metadata or object was written"
                )
            )
        }
        if crate::allocation_authority::resolve(&self.root)?.authoritative {
            return Ok(());
        }
        let existing = object_store::list_objects(&self.objects_root)?
            .into_iter()
            .map(|(sid, _)| object_store::read_object(&self.objects_root, &sid))
            .collect::<Result<Vec<_>>>()?;
        crate::allocation_authority::admit(&self.root, incoming, &existing)
    }

    /// Mint a fresh spec ID for `template` exactly as `add_requirement` would
    /// (same prefix rules, same dispenser, same [`crate::id_identity::allocate_id`]
    /// skip of numbers used under any padding) and persist the advanced
    /// counters. Used by `aida doctor --fix id-collisions`.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
    pub fn mint_spec_id(&self, template: &Requirement) -> Result<String> {
        let _lock = self.lock_store()?;
        let meta = self.load_metadata()?;
        let mut temp_store = self.assemble_store(meta, Vec::new());
        let type_prefix = temp_store.get_type_prefix(&template.req_type);
        let mut probe = template.clone();
        probe.spec_id = None;
        probe.agreed_id = None;
        let minted = crate::id_identity::allocate_id(
            || {
                temp_store.add_requirement_with_id(probe.clone(), None, type_prefix.as_deref());
                Ok(temp_store
                    .requirements
                    .last()
                    .and_then(|r| r.spec_id.clone())
                    .unwrap_or_default())
            },
            &|id| self.id_holder(id),
            template.id,
        )?;
        if minted.is_empty() {
            anyhow::bail!(
                "{}",
                crate::allocation_authority::failure_message(
                    "requirement ID allocation failed",
                    "nothing was renumbered"
                )
            );
        }
        self.save_metadata(&Self::extract_metadata(&temp_store))?;
        Ok(minted)
    }

    /// Predict, WITHOUT advancing any counter or writing anything, a free ID
    /// [`Self::mint_spec_id`] could hand `template`: the next number (for a
    /// node-local clone, past this node's highest filed number) plus
    /// `headroom` (so IDs minted meanwhile do not land on it),
    /// skipping numbers already used under any padding and any key in
    /// `reserved` (earlier predictions in the same plan). Used by the
    /// `aida doctor --fix id-collisions --dry-run` mapping.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
    pub fn peek_spec_id(
        &self,
        template: &Requirement,
        headroom: u32,
        reserved: &std::collections::HashSet<String>,
    ) -> Result<String> {
        let meta = self.load_metadata()?;
        let mut temp_store = self.assemble_store(meta, Vec::new());
        let prefix = template
            .prefix_override
            .as_deref()
            .map(str::to_uppercase)
            .or_else(|| temp_store.get_type_prefix(&template.req_type))
            .unwrap_or_else(|| "REQ".to_string());
        let free = |id: &str| -> Result<bool> {
            Ok(!id.is_empty()
                && !reserved.contains(&crate::id_identity::id_key(id))
                && self.id_holder(id)?.is_none())
        };
        let budget = headroom as usize + crate::id_identity::MAX_ALLOCATION_SKIPS * 4;
        if temp_store
            .dispenser
            .as_ref()
            .is_some_and(|d| d.node_local_only())
        {
            // A node-local clone dispenser takes its counter's write lock even
            // to peek, so predict from the store instead: past the highest
            // number this node already filed under the prefix.
            let node = crate::allocation_authority::node(&self.root)?;
            let local = crate::id_identity::IdKey::parse(&format!("{prefix}-{node}-1"))
                .map(|k| k.prefix)
                .unwrap_or_default();
            let high = object_store::list_objects(&self.objects_root)?
                .iter()
                .filter_map(|(stem, _)| crate::id_identity::IdKey::parse(stem))
                .filter(|k| k.prefix == local)
                .filter_map(|k| k.seq.parse::<u32>().ok())
                .max()
                .unwrap_or(0);
            let width = crate::id_identity::installed_widths().node_mint_width();
            let start = high.saturating_add(1).saturating_add(headroom);
            for seq in start..start.saturating_add(budget as u32) {
                let id = format!("{prefix}-{node}-{seq:0width$}");
                if free(&id)? {
                    return Ok(id);
                }
            }
        } else if let Some(dispenser) = temp_store.dispenser.clone() {
            let key = if dispenser.node_local_only()
                || temp_store.id_config.numbering != crate::models::NumberingStrategy::Global
            {
                prefix.clone()
            } else {
                "GLOBAL".to_string()
            };
            let start = dispenser.peek(&key)?.saturating_add(headroom);
            for seq in start..start.saturating_add(budget as u32) {
                let id = dispenser.format_id(&prefix, seq)?;
                if free(&id)? {
                    return Ok(id);
                }
            }
        } else {
            // No dispenser: the counters live only in this in-memory copy,
            // which is never saved, so generating ahead advances nothing.
            let mut probe = template.clone();
            probe.spec_id = None;
            probe.agreed_id = None;
            let type_prefix = temp_store.get_type_prefix(&template.req_type);
            for drawn in 0..budget {
                temp_store.add_requirement_with_id(probe.clone(), None, type_prefix.as_deref());
                let id = temp_store
                    .requirements
                    .last()
                    .and_then(|r| r.spec_id.clone())
                    .unwrap_or_default();
                if drawn >= headroom as usize && free(&id)? {
                    return Ok(id);
                }
            }
        }
        anyhow::bail!("no free spec ID found for prefix {prefix} within {budget} numbers")
    }

    /// Renumber requirements in ONE store commit: each `(old, new)` pair
    /// moves the object filed as the literal `old` to `new.spec_id` (same
    /// UUID), and every `referrers` row (other objects whose text was
    /// rewritten) is staged alongside. Refuses before writing anything when
    /// an old object is missing, belongs to another UUID, or the new number
    /// is already used under any padding.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
    pub fn renumber_requirements(
        &self,
        renames: &[(String, Requirement)],
        referrers: &[Requirement],
        subject: &str,
    ) -> Result<usize> {
        #[cfg(all(unix, feature = "native"))]
        if self.work_admission()?.is_some() {
            anyhow::bail!("renumbering is not supported on a WORK-transport clone");
        }
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        let _lock = self.lock_store()?;
        for (old, new) in renames {
            let on_disk = object_store::read_object(&self.objects_root, old)
                .with_context(|| format!("renumber: cannot read {old}"))?;
            anyhow::ensure!(
                on_disk.id == new.id,
                "renumber: {old} belongs to {}, not {}; nothing was written",
                on_disk.id,
                new.id
            );
            let new_sid = new
                .spec_id
                .as_deref()
                .context("renumber: target has no spec_id")?;
            anyhow::ensure!(
                object_store::numeric_twin_stems(&self.objects_root, new_sid)?.is_empty(),
                "renumber: {new_sid} is already used; nothing was written"
            );
        }
        let mut paths: Vec<String> = Vec::new();
        if self.metadata_path.exists() {
            paths.push("metadata.yaml".to_string());
        }
        for (old, new) in renames {
            let old_path = object_store::object_path(&self.objects_root, old)?;
            object_store::write_object(&self.objects_root, new)?;
            std::fs::remove_file(&old_path)
                .with_context(|| format!("renumber: remove {}", old_path.display()))?;
            paths.push(object_store::relative_object_path(old)?);
            paths.push(object_store::relative_object_path(
                new.spec_id.as_deref().unwrap_or_default(),
            )?);
        }
        for row in referrers {
            if let Some(sid) = self.stage_requirement_update(row)? {
                paths.push(object_store::relative_object_path(sid)?);
            }
        }
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        self.auto_commit_paths(
            &format!(
                "{subject}: renumber {} requirement(s), rewrite {} referrer(s)",
                renames.len(),
                referrers.len()
            ),
            &refs,
        );
        Ok(renames.len())
    }

    /// Mint-time numeric admission for a NEW row carrying caller-supplied
    /// ids: refuse when another requirement already holds the number under
    /// any padding. Runs on every clone, authoritative included — the
    /// authoritative early return in [`Self::admit_allocations`] is how padded
    /// twins were once written.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
    ///
    /// Only a DIFFERENTLY padded twin refuses: a row written under the exact
    /// same literal id replaces that one file (re-run imports rely on it),
    /// and same-literal duplicates are the existing collision guards' job.
    ///
    /// `agreed` is the add's or batch's one [`Self::agreed_id_index`];
    /// `staged` holds the rows a batch has accepted but not yet written, so a
    /// later row cannot take an id an earlier row of the same batch holds.
    // trace:TASK-B6B1278890774EF993D2EBC327862470-149 | ai:claude
    fn admit_numeric_identity(
        &self,
        req: &Requirement,
        agreed: &AgreedIdIndex,
        staged: &[Requirement],
    ) -> Result<()> {
        let Some(id) = req.spec_id.as_deref().filter(|s| !s.trim().is_empty()) else {
            return Ok(());
        };
        let holder = |i: &str| -> Result<Option<uuid::Uuid>> {
            let names = |v: Option<&str>| v.is_some_and(|v| crate::id_identity::same_id(v, i));
            if let Some(row) = staged.iter().find(|r| {
                r.id != req.id && (names(r.spec_id.as_deref()) || names(r.agreed_id.as_deref()))
            }) {
                return Ok(Some(row.id));
            }
            for stem in object_store::numeric_twin_stems(&self.objects_root, i)? {
                if stem.eq_ignore_ascii_case(i.trim()) {
                    continue;
                }
                if let Ok(other) = object_store::read_object(&self.objects_root, &stem) {
                    return Ok(Some(other.id));
                }
            }
            // The agreed-id half (AC2: spec_id OR agreed_id holds a number):
            // an explicit id on the authoritative clone is refused while
            // another spec carries it as its agreed id.
            Ok(agreed.holder(i, Some(i.trim())))
        };
        crate::id_identity::admit_new_id(id, &holder, req.id)
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
        let existing = if object_store::object_exists(&self.objects_root, spec_id)? {
            Some(object_store::read_object(&self.objects_root, spec_id)?)
        } else {
            None
        };
        let (stamped, ops, authority_advance) = Self::prepare_requirement_update(
            requirement,
            existing.as_ref(),
            &crate::provenance::WriteContext::capture(),
        )?;
        for kind in ops {
            self.record_op(requirement.id, kind);
        }

        let wrote = object_store::write_object_if_changed(&self.objects_root, &stamped)?;
        if wrote && authority_advance {
            self.pending_authority_advance
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(if wrote { Some(spec_id) } else { None })
    }

    // Pure row/op preparation shared by ordinary staging and WORK plans.
    // trace:STORY-1742 | ai:codex
    fn prepare_requirement_update(
        requirement: &Requirement,
        existing: Option<&Requirement>,
        context: &crate::provenance::WriteContext,
    ) -> Result<(Requirement, Vec<crate::oplog::OpKind>, bool)> {
        let mut ops = Vec::new();
        let mut stamped = requirement.clone();
        crate::provenance::stamp_write(&mut stamped, existing, context)?;

        // Record ops for changed fields (compare with existing if possible).
        let authority_advance = existing.is_some_and(|old| {
            crate::lifecycle::status_advance_records_authority_trailers(
                &old.status,
                &requirement.status,
            )
        });
        if let Some(old) = existing {
            if old.title != requirement.title {
                ops.push(crate::oplog::OpKind::SetTitle {
                    title: requirement.title.clone(),
                });
            }
            if old.effective_status() != requirement.effective_status() {
                ops.push(crate::oplog::OpKind::SetStatus {
                    status: requirement.effective_status(),
                });
            }
            if old.effective_priority() != requirement.effective_priority() {
                ops.push(crate::oplog::OpKind::SetPriority {
                    priority: requirement.effective_priority(),
                });
            }
            if old.owner != requirement.owner {
                ops.push(crate::oplog::OpKind::SetOwner {
                    owner: requirement.owner.clone(),
                });
            }
            if old.description != requirement.description {
                ops.push(crate::oplog::OpKind::SetDescription {
                    description: requirement.description.clone(),
                });
            }
            // BUG-587: tag edits must reach the oplog too. Previously `aida edit
            // --tags` wrote ONLY the YAML object and recorded no operation, so
            // tag changes bypassed the CRDT substrate entirely — the oplog was an
            // incomplete record of the spec's history. Emit one AddTag per added
            // tag and one RemoveTag per removed tag, matching the documented
            // "every status flip, priority change, tag edit lands as a structured
            // row" invariant. trace:BUG-587 | ai:claude
            for added in requirement.tags.difference(&old.tags) {
                ops.push(crate::oplog::OpKind::AddTag { tag: added.clone() });
            }
            for removed in old.tags.difference(&requirement.tags) {
                ops.push(crate::oplog::OpKind::RemoveTag {
                    tag: removed.clone(),
                });
            }
        }

        Ok((stamped, ops, authority_advance))
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
        // trace:STORY-1742 | ai:claude
        #[cfg(all(unix, feature = "native"))]
        if let Some(admission) = self.work_admission()? {
            return self.work_bulk_update(&admission, requirements, commit_subject);
        }
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        // trace:BUG-1612 | ai:claude — serialize with every other store writer.
        let _lock = self.lock_store()?;
        let mut planned = requirements.to_vec();
        self.prepare_hypothesis_updates(&mut planned, &[])?;
        let mut changed: Vec<String> = Vec::new();
        for requirement in &planned {
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

    /// [`Self::bulk_update`]'s compare-and-swap sibling: apply a decision to
    /// many specs in ONE commit, with every eligibility decision taken on the
    /// object read INSIDE the store write lock.
    ///
    /// A sweep picks its candidates from a cache projection and (at best)
    /// re-reads each object to confirm them — but that read happens before the
    /// write path takes the lock, so a concurrent writer can land between the
    /// re-check and `bulk_update`'s write, and the whole-object write reverts
    /// it. Here the lock is taken first and each `target`'s object is re-read
    /// under it; `keep` sees that fresh copy and returns whether it still
    /// qualifies. Only what `keep` accepted is mutated and written, so a spec
    /// changed after candidate selection is skipped instead of overwritten.
    ///
    /// Per target: the stored object is read (absent -> `skipped_missing`,
    /// unreadable/unparsable -> `skipped_unreadable`), its uuid must still
    /// match `target.id` (else `skipped_changed`), then `keep(&mut fresh)`
    /// decides. `false` -> `skipped_changed`. Nothing aborts the batch: a
    /// sweep is best-effort over many specs, so one unreadable object is
    /// counted, not fatal.
    ///
    /// This is `update_spec_atomically` N times under one lock and one commit,
    /// which is what keeps a large sweep a single store commit (BUG-425).
    // trace:BUG-1671 | ai:claude
    pub fn bulk_update_atomically<F>(
        &self,
        targets: &[Requirement],
        commit_subject: &str,
        mut keep: F,
    ) -> Result<BulkAtomicReport>
    where
        F: FnMut(&mut Requirement) -> bool,
    {
        // trace:STORY-1742 | ai:claude
        #[cfg(all(unix, feature = "native"))]
        if let Some(admission) = self.work_admission()? {
            return self.work_bulk_update_atomically(&admission, targets, commit_subject, keep);
        }
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        // trace:BUG-1671 | ai:claude — one lock spans every re-check and every
        // write, so no lock-respecting writer can slip between them.
        let _lock = self.lock_store()?;
        let mut report = BulkAtomicReport::default();
        let mut planned = Vec::new();
        let mut baselines = Vec::new();
        let mut changed: Vec<String> = Vec::new();
        for target in targets {
            let Some(spec_id) = target.spec_id.as_deref() else {
                // Nothing to read the stored object by.
                report.skipped_unreadable += 1;
                continue;
            };
            let spec_id = object_store::canonical_spec_id(spec_id);
            let path = match object_store::object_path(&self.objects_root, &spec_id) {
                Ok(p) => p,
                Err(_) => {
                    report.skipped_unreadable += 1;
                    continue;
                }
            };
            // One read serves both the re-check and the compare-and-swap
            // baseline, so the lock is held for N reads, not 2N.
            let before = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    report.skipped_missing += 1;
                    continue;
                }
                Err(_) => {
                    report.skipped_unreadable += 1;
                    continue;
                }
            };
            let fresh: Requirement = match serde_yaml::from_slice(&before) {
                Ok(req) => req,
                Err(_) => {
                    report.skipped_unreadable += 1;
                    continue;
                }
            };
            if fresh.id != target.id {
                // The spec_id now names a different requirement.
                report.skipped_changed += 1;
                continue;
            }
            let mut next = fresh.clone();
            if !keep(&mut next) {
                report.skipped_changed += 1;
                continue;
            }
            if next.id != fresh.id || next.spec_id != fresh.spec_id {
                anyhow::bail!(
                    "update of {spec_id} tried to change its id or spec_id; nothing was written"
                );
            }
            baselines.push((spec_id, path, before));
            planned.push(next);
        }
        let accepted: std::collections::HashSet<_> = planned.iter().map(|r| r.id).collect();
        self.prepare_hypothesis_updates(&mut planned, &[])?;
        for (sid, path, bytes) in baselines {
            self.ensure_object_unchanged(&sid, &path, &bytes)?;
        }
        for next in planned {
            if let Some(written) = self.stage_requirement_update(&next)? {
                changed.push(written.to_string());
            }
            // Report the stamped object as persisted, not the caller's copy.
            // trace:STORY-1674 | ai:claude
            if accepted.contains(&next.id) {
                let sid = next
                    .spec_id
                    .as_deref()
                    .expect("planned updates are keyed by spec_id");
                report
                    .written
                    .push(object_store::read_object(&self.objects_root, sid)?);
            }
        }
        if !changed.is_empty() {
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
        }
        Ok(report)
    }

    /// Acquire the store write lock (re-entrant per thread). Every write path
    /// holds it across its read-modify-write window.
    // trace:BUG-1612 | ai:claude
    pub(crate) fn lock_store(&self) -> Result<super::store_lock::StoreWriteGuard> {
        let guard = super::store_lock::acquire(&self.root)?;
        // trace:TASK-1717 | ai:claude — callers check the store state before
        // waiting; a sync holding the lock can leave a failed rebase behind
        // in between, so the outermost holder checks again once it owns it.
        if guard.is_outermost() {
            self.pending_authority_advance
                .store(false, std::sync::atomic::Ordering::SeqCst);
            crate::git_ops::ensure_store_write_safe(&self.root)?;
        }
        Ok(guard)
    }

    // Serialize a queue-file read-modify-write (registry/queues/<user>.yaml)
    // with every other store writer. Every queue-file writer (`queue_add`,
    // `queue_remove_for_role`, `queue_reorder`, `queue_clear`,
    // `queue_remove_many`) holds the returned guard from before the queue file
    // is read until after its auto-commit, so no lock-respecting writer
    // (another queue command, a spec save, a bulk import) can land between the
    // read and the write-back and have its entries dropped. The write-back
    // itself is temp+rename (`fs_atomic::write_atomic`), so a lock-free reader
    // (`queue_list`) sees either the old file or the new one, never a torn one.
    // On Linux/macOS the rename is atomic for the open too; on Windows the
    // reader's open can transiently fail while the rename is in flight, which
    // `read_queue_file` absorbs by reading through `fs_atomic::read_atomic`.
    //
    // Lock order: the store write lock is the ONLY lock a queue writer takes.
    // Nothing else is acquired inside it: `read_queue_file`, `get_requirement`
    // (used by `queue_clear --completed`) and `auto_commit_paths` are lock-free,
    // and the queue writers never touch the SQLite cache or its lock. The lock
    // is re-entrant per thread, so a caller that already holds it (for example
    // `CachedGitBackend` write paths, which take it before delegating to the
    // inner backend) can call a queue writer without deadlocking; a different
    // thread or process blocks until the holder releases. As of this writing
    // no `lock_store()` holder calls a queue writer, so there is no nested
    // order to maintain; if one appears, it must take the store lock FIRST
    // and the cache lock (if any) second, matching `CachedGitBackend`.
    // trace:BUG-1677 | ai:claude
    fn lock_queue_write(&self) -> Result<super::store_lock::StoreWriteGuard> {
        // Queue registry writes have no prepared WORK plan yet; refuse them
        // before any effect. trace:STORY-1742 | ai:claude
        #[cfg(all(unix, feature = "native"))]
        crate::transport_policy::refuse_unsupported(&self.root)?;
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        self.lock_store()
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
        self.save_reporting_with_origin(store, false)
    }

    fn save_reporting_with_origin(
        &self,
        store: &RequirementsStore,
        imported: bool,
    ) -> Result<SaveReport> {
        // Imports carry foreign provenance and have no prepared WORK plan.
        // trace:STORY-1742 | ai:claude
        #[cfg(all(unix, feature = "native"))]
        if imported {
            crate::transport_policy::refuse_unsupported(&self.root)?;
        } else if let Some(admission) = self.work_admission()? {
            return self.work_full_save(&admission, store);
        }
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        let _lock = self.lock_store()?;

        self.admit_allocations(&store.requirements)?;
        self.admit_metadata(&Self::extract_metadata(store))?;
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

        // A new path may be an existing UUID renamed by ID migration. Resolve
        // that identity once per full save, so old unknown events remain old.
        // trace:STORY-1674 | ai:codex
        // Only an on-disk spec missing from the incoming store can be a
        // rename's old path, so the map reads just those (none: no reads).
        // trace:STORY-1674 | ai:claude
        // trace:BUG-2017 | ai:codex — compare the on-disk canonical namespace.
        let incoming_specs: std::collections::HashSet<String> = store
            .requirements
            .iter()
            .filter_map(|r| r.spec_id.as_deref())
            .map(object_store::canonical_spec_id)
            .collect();
        let mut previous_by_uuid: Option<std::collections::HashMap<uuid::Uuid, Requirement>> = None;

        for req in &store.requirements {
            let Some(ref spec_id) = req.spec_id else {
                continue;
            };
            current_specs.insert(object_store::canonical_spec_id(spec_id));
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
                    let mut next = Self::preserve_full_save_only_fields(req.clone(), &disk);
                    if imported {
                        crate::provenance::stamp_import_write(&mut next, Some(&disk))?;
                    } else {
                        crate::provenance::stamp_write(
                            &mut next,
                            Some(&disk),
                            &crate::provenance::WriteContext::capture(),
                        )?;
                    }
                    next
                }
                // Absent (or unparseable, as before): this save creates it.
                _ => {
                    if previous_by_uuid.is_none() {
                        let mut by_uuid = std::collections::HashMap::new();
                        for (old_id, _) in existing
                            .iter()
                            .filter(|(id, _)| !incoming_specs.contains(id.as_str()))
                        {
                            if let Ok(old) = object_store::read_object(&self.objects_root, old_id) {
                                if by_uuid.insert(old.id, old).is_some() {
                                    anyhow::bail!("cannot resolve renamed requirement: duplicate UUID in store");
                                }
                            }
                        }
                        previous_by_uuid = Some(by_uuid);
                    }
                    if let Some(old) = previous_by_uuid.as_ref().and_then(|m| m.get(&req.id)) {
                        let mut renamed = Self::preserve_full_save_only_fields(req.clone(), old);
                        if imported {
                            crate::provenance::stamp_import_write(&mut renamed, Some(old))?;
                        } else {
                            crate::provenance::stamp_write(
                                &mut renamed,
                                Some(old),
                                &crate::provenance::WriteContext::capture(),
                            )?;
                        }
                        renamed
                    } else {
                        let mut created = req.clone();
                        if !imported && created.filed_at.is_none() {
                            let p =
                                filing_provenance.get_or_insert_with(crate::provenance::capture);
                            crate::provenance::stamp_with(&mut created, p);
                        }
                        if imported {
                            crate::provenance::stamp_import_write(&mut created, None)?;
                        } else {
                            crate::provenance::stamp_write(
                                &mut created,
                                None,
                                &crate::provenance::WriteContext::capture(),
                            )?;
                        }
                        created
                    }
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
        let meta_plan = merge_metadata(
            meta_base.as_ref(),
            &mine_meta,
            disk_meta.as_ref(),
            store.id_counters_reset.is_set(),
        )?;

        if !conflicts.is_empty() || meta_plan.is_err() {
            conflicts.sort();
            return Err(StoreConflictError {
                specs: conflicts,
                metadata_fields: meta_plan.err().unwrap_or_default(),
            }
            .into());
        }
        let meta_plan = meta_plan.unwrap_or(MetadataMerge { write: None });

        // Lock policy is part of the all-or-nothing preflight, including
        // full-store MCP saves and lock-only companion rows. trace:STORY-1675
        let mut planned: Vec<_> = to_write.iter().map(|(r, _)| r.clone()).collect();
        self.prepare_hypothesis_updates(&mut planned, &to_delete)?;
        let caller_fps: std::collections::HashMap<_, _> =
            to_write.iter().map(|(r, fp)| (r.id, *fp)).collect();
        let callers: std::collections::HashMap<_, _> =
            store.requirements.iter().map(|r| (r.id, r)).collect();
        to_write = planned
            .into_iter()
            .map(|r| {
                let fp = match caller_fps.get(&r.id) {
                    Some(fp) => *fp,
                    None if snapshot.is_some() => callers
                        .get(&r.id)
                        .map(|mine| {
                            serde_yaml::to_string(mine)
                                .map(|s| object_store::content_fingerprint(&s))
                        })
                        .transpose()?,
                    None => None,
                };
                Ok((r, fp))
            })
            .collect::<Result<Vec<_>>>()?;
        let written_ids: std::collections::HashSet<_> = to_write
            .iter()
            .filter_map(|(r, _)| r.spec_id.as_ref())
            .collect();
        already_current.retain(|(sid, _)| !written_ids.contains(sid));

        // ---- write phase ----
        if let Some(meta) = &meta_plan.write {
            self.save_metadata(meta)?;
        }
        if let Some(snapshot) = snapshot {
            snapshot.record_metadata(metadata_value(&mine_meta)?);
        }
        // A reset is one-shot: once merged, the store max-merges again.
        // trace:BUG-1641 | ai:claude
        store.id_counters_reset.clear();
        let mut written_specs: Vec<String> = Vec::new();
        for (req, caller_fp) in &to_write {
            let spec_id = req.spec_id.as_deref().unwrap_or_default();
            let authority_advance = !imported
                && object_store::read_object(&self.objects_root, spec_id)
                    .ok()
                    .is_some_and(|old| {
                        crate::lifecycle::status_advance_records_authority_trailers(
                            &old.status,
                            &req.status,
                        )
                    });
            if object_store::write_object_if_changed(&self.objects_root, req)? {
                written_specs.push(spec_id.to_string());
                if authority_advance {
                    self.pending_authority_advance
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
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
    /// and write back that object with a targeted commit. Ordinary untagged
    /// refinements read only that object. Graph-affecting edits also preflight
    /// hypothesis locks and may write lock-only companion rows (STORY-1675).
    /// Just before the write the file is re-read
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
        // trace:STORY-1742 | ai:claude
        #[cfg(all(unix, feature = "native"))]
        if let Some(admission) = self.work_admission()? {
            return self.work_update_spec(&admission, target, commit_subject, update_fn);
        }
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
        let mut planned = vec![next];
        self.prepare_hypothesis_updates(&mut planned, &[])?;
        self.ensure_object_unchanged(&spec_id, &path, &before)?;
        let next = planned.iter().find(|r| r.id == current.id).unwrap().clone();
        let mut paths = Vec::new();
        for row in &planned {
            if let Some(written) = self.stage_requirement_update(row)? {
                paths.push(object_store::relative_object_path(written)?);
            }
        }
        if !paths.is_empty() {
            let message = match commit_subject {
                Some(subject) => format!(
                    "{subject}: update {} requirement{}",
                    paths.len(),
                    if paths.len() == 1 { "" } else { "s" }
                ),
                None => format!("update {}", spec_id),
            };
            self.auto_commit_paths(
                &message,
                &paths.iter().map(String::as_str).collect::<Vec<_>>(),
            );
        }
        Ok(Some(object_store::read_object(
            &self.objects_root,
            &spec_id,
        )?))
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

        #[cfg(all(unix, feature = "native"))]
        {
            let admission = crate::transport_policy::Admission::enter(&self.root)?;
            if admission.enabled() {
                return self.work_update(&admission, None, |store| {
                    update_fn(store);
                    Ok(())
                });
            }
        }
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        let _lock = self.lock_store()?;

        let mut store = self.load()?;
        let meta_before = metadata_value(&Self::extract_metadata(&store))?;
        let mut before: HashMap<String, (uuid::Uuid, String)> = HashMap::new();
        for req in &store.requirements {
            if let Some(sid) = req.spec_id.as_deref() {
                before.insert(sid.to_string(), (req.id, serde_yaml::to_string(req)?));
            }
        }

        let hypotheses_before = store.requirements.clone();
        update_fn(&mut store);
        self.admit_allocations(&store.requirements)?;
        self.admit_metadata(&Self::extract_metadata(&store))?;
        crate::hypothesis::enforce(&hypotheses_before, &mut store.requirements)?;

        let meta_after = Self::extract_metadata(&store);
        let meta_changed = metadata_value(&meta_after)? != meta_before;
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
        // Store-level fields go through the same merge as a whole-store save,
        // so the counter rules are identical on both paths: a counter is never
        // lowered unless the update reset it through `reset_id_counters`, and a
        // field changed on disk by a writer that bypassed the lock is a conflict.
        // trace:BUG-1641 | ai:claude
        let meta_write = if meta_changed {
            let disk_meta = self.read_metadata_if_present()?;
            match merge_metadata(
                Some(&meta_before),
                &meta_after,
                disk_meta.as_ref(),
                store.id_counters_reset.is_set(),
            )? {
                Ok(plan) => plan.write,
                Err(fields) => {
                    return Err(StoreConflictError {
                        specs: Vec::new(),
                        metadata_fields: fields,
                    }
                    .into())
                }
            }
        } else {
            None
        };

        let mut summary = AtomicWriteSummary {
            metadata_changed: meta_write.is_some(),
            ..Default::default()
        };
        let mut paths: Vec<String> = Vec::new();
        if let Some(meta) = &meta_write {
            self.save_metadata(meta)?;
            paths.push("metadata.yaml".to_string());
        }
        for req in &changed {
            if let Some(sid) = self.stage_requirement_update(req)? {
                paths.push(object_store::relative_object_path(sid)?);
                summary
                    .written
                    .push(object_store::read_object(&self.objects_root, sid)?);
            }
        }
        let mut filing_provenance: Option<crate::models::FilingProvenance> = None;
        // A spec_id change is a delete + create of the same UUID. Stamp it
        // against its old copy, as the full-save path does, so legacy events
        // keep their unknown context instead of acquiring this session's.
        // trace:STORY-1674 | ai:claude
        let mut renamed_from: HashMap<uuid::Uuid, Requirement> = HashMap::new();
        for (sid, id) in &removed {
            if let Some((_, yaml)) = before.get(sid.as_str()) {
                renamed_from.insert(*id, serde_yaml::from_str(yaml)?);
            }
        }
        for req in &created {
            let mut new_req = (*req).clone();
            let previous = renamed_from.get(&new_req.id);
            if previous.is_none() && new_req.filed_at.is_none() {
                let p = filing_provenance.get_or_insert_with(crate::provenance::capture);
                crate::provenance::stamp_with(&mut new_req, p);
            }
            crate::provenance::stamp_write(
                &mut new_req,
                previous,
                &crate::provenance::WriteContext::capture(),
            )?;
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
                *slot = written.clone();
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
            snapshot.record_metadata(metadata_value(&meta_after)?);
        }
        // trace:BUG-1641 | ai:claude — a reset is one-shot.
        store.id_counters_reset.clear();
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

    // trace:STORY-1674 | ai:codex
    fn save_imported(&self, store: &RequirementsStore) -> Result<()> {
        // Keep the destination-loaded import's optimistic/deletion baseline.
        // Foreign exporters clear their source baseline at the transfer call site.
        self.save_reporting_with_origin(store, true).map(|_| ())
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
        // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
        crate::node_alias::resolve_literal_first(spec_id, |id| {
            self.get_requirement_by_exact_spec_id(id)
        })
    }

    fn get_requirement(&self, id: &uuid::Uuid) -> Result<Option<Requirement>> {
        object_store::find_by_uuid(&self.objects_root, id)
    }

    fn update_requirement(&self, requirement: &Requirement) -> Result<()> {
        #[cfg(all(unix, feature = "native"))]
        {
            let admission = crate::transport_policy::Admission::enter(&self.root)?;
            if admission.enabled() {
                self.work_update(&admission, None, |store| {
                    let row = store
                        .requirements
                        .iter_mut()
                        .find(|row| row.id == requirement.id)
                        .context("WORK update target was not found")?;
                    *row = requirement.clone();
                    Ok(())
                })?;
                return Ok(());
            }
        }
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        let _lock = self.lock_store()?;
        // trace:STORY-1675 | ai:codex
        let mut planned = vec![requirement.clone()];
        self.prepare_hypothesis_updates(&mut planned, &[])?;
        let mut paths = Vec::new();
        for row in &planned {
            if let Some(sid) = self.stage_requirement_update(row)? {
                paths.push(object_store::relative_object_path(sid)?);
            }
        }
        if !paths.is_empty() {
            self.auto_commit_paths(
                &format!("update {}", requirement.display_id()),
                &paths.iter().map(String::as_str).collect::<Vec<_>>(),
            );
        }
        Ok(())
    }

    fn delete_requirement(&self, id: &uuid::Uuid) -> Result<()> {
        #[cfg(all(unix, feature = "native"))]
        {
            let admission = crate::transport_policy::Admission::enter(&self.root)?;
            if admission.enabled() {
                self.work_update(&admission, None, |store| {
                    anyhow::ensure!(
                        store.requirements.iter().any(|row| row.id == *id),
                        "WORK deletion target was not found"
                    );
                    store.requirements.retain(|row| row.id != *id);
                    Ok(())
                })?;
                return Ok(());
            }
        }
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        // trace:BUG-1612 | ai:claude — serialize with every other store writer.
        let _lock = self.lock_store()?;
        if let Some(req) = object_store::find_by_uuid(&self.objects_root, id)? {
            if let Some(ref spec_id) = req.spec_id {
                // Capture pre-existing completed-result evidence before removal.
                // trace:STORY-1675 | ai:codex
                let mut companions = Vec::new();
                self.prepare_hypothesis_updates(&mut companions, &[spec_id.clone()])?;
                let mut paths = Vec::new();
                for row in &companions {
                    if let Some(sid) = self.stage_requirement_update(row)? {
                        paths.push(object_store::relative_object_path(sid)?);
                    }
                }
                self.record_op(*id, crate::oplog::OpKind::Archive);
                object_store::delete_object(&self.objects_root, spec_id)?;
                let rel = object_store::relative_object_path(spec_id)?;
                // trace:BUG-1-040 | ai:claude
                paths.push(rel);
                self.auto_commit_paths(
                    &format!("delete {}", spec_id),
                    &paths.iter().map(String::as_str).collect::<Vec<_>>(),
                );
                return Ok(());
            }
        }
        anyhow::bail!("Requirement not found: {}", id)
    }

    fn add_requirement(&self, requirement: Requirement) -> Result<Requirement> {
        #[cfg(all(unix, feature = "native"))]
        {
            let admission = crate::transport_policy::Admission::enter(&self.root)?;
            if admission.enabled() {
                let id = requirement.id;
                let (store, _) = self.work_update(&admission, None, |store| {
                    Self::work_add_to_store(store, requirement)
                })?;
                return store
                    .requirements
                    .into_iter()
                    .find(|row| row.id == id)
                    .context("Admitted WORK requirement was not found after commit");
            }
        }
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        // trace:BUG-1612 | ai:claude — serialize with every other store writer.
        let _lock = self.lock_store()?;
        let mut req = requirement;
        if req.spec_id.is_some() || req.agreed_id.is_some() {
            self.admit_allocations(std::slice::from_ref(&req))?;
            if req.spec_id.is_some() {
                // trace:TASK-B6B1278890774EF993D2EBC327862470-149 | ai:claude
                self.admit_numeric_identity(&req, &self.agreed_id_index()?, &[])?;
            }
        }
        // CR-8: stamp filing provenance at creation (write-once; a caller that
        // already stamped keeps its stamp). trace:CR-8 | ai:claude
        crate::provenance::stamp_if_absent(&mut req);
        crate::provenance::stamp_write(
            &mut req,
            None,
            &crate::provenance::WriteContext::capture(),
        )?;

        let mut new_metadata = None;
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
            // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude — mint
            // through the shared seam: a number already used under any
            // padding is skipped, never handed out as a padded twin.
            let minted = crate::id_identity::allocate_id(
                || {
                    temp_store.add_requirement_with_id(req.clone(), None, type_prefix.as_deref());
                    Ok(temp_store
                        .requirements
                        .last()
                        .and_then(|r| r.spec_id.clone())
                        .unwrap_or_default())
                },
                &|id| self.id_holder(id),
                req.id,
            )?;
            req.spec_id = Some(minted);
            // Persist the updated counters back to metadata
            let updated_meta = Self::extract_metadata(&temp_store);
            new_metadata = Some(updated_meta);
        }

        // trace:STORY-1675 | ai:codex
        let mut planned = vec![req.clone()];
        self.prepare_hypothesis_updates(&mut planned, &[])?;
        req = planned.iter().find(|r| r.id == req.id).unwrap().clone();
        let mut paths = Vec::new();
        if let Some(meta) = new_metadata {
            self.save_metadata(&meta)?;
            paths.push("metadata.yaml".to_string());
        } else if self.metadata_path.exists() {
            paths.push("metadata.yaml".to_string());
        }
        for row in planned.iter().filter(|r| r.id != req.id) {
            if let Some(sid) = self.stage_requirement_update(row)? {
                paths.push(object_store::relative_object_path(sid)?);
            }
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
            self.auto_commit_paths(&format!("add {} — {}", spec_id, req.title), &{
                paths.push(rel);
                paths.iter().map(String::as_str).collect::<Vec<_>>()
            });
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
        // Held across the read-modify-write AND the commit below.
        // trace:BUG-1677 | ai:claude
        let _lock = self.lock_queue_write()?;
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
        // Temp+rename so a lock-free reader never sees a torn file.
        // trace:BUG-1677 | ai:claude
        crate::fs_atomic::write_atomic(&path, yaml)?;
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
        // Held across the read-modify-write AND the commit below.
        // trace:BUG-1677 | ai:claude
        let _lock = self.lock_queue_write()?;
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
        // Temp+rename so a lock-free reader never sees a torn file.
        // trace:BUG-1677 | ai:claude
        crate::fs_atomic::write_atomic(&path, yaml)?;
        self.auto_commit_paths(
            "update queue",
            &[&queue_relative_path_for_file(&self.root, &path, &user_id)],
        );
        Ok(())
    }

    fn queue_reorder(&self, user_id: &str, items: &[(uuid::Uuid, i64)]) -> Result<()> {
        // Held across the read-modify-write AND the commit below.
        // trace:BUG-1677 | ai:claude
        let _lock = self.lock_queue_write()?;
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
        // Temp+rename so a lock-free reader never sees a torn file.
        // trace:BUG-1677 | ai:claude
        crate::fs_atomic::write_atomic(&path, yaml)?;
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
        // Held across the read-modify-write AND the commit below.
        // trace:BUG-1677 | ai:claude
        let _lock = self.lock_queue_write()?;
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
            // Temp+rename so a lock-free reader never sees a torn file.
            // trace:BUG-1677 | ai:claude
            crate::fs_atomic::write_atomic(&path, yaml)?;
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
        // Held across the read-modify-write AND the commit below.
        // trace:BUG-1677 | ai:claude
        let _lock = self.lock_queue_write()?;
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
            // Temp+rename so a lock-free reader never sees a torn file.
            // trace:BUG-1677 | ai:claude
            crate::fs_atomic::write_atomic(&path, yaml)?;
        }
        self.auto_commit_paths(
            "gc dead queue entries",
            &[&queue_relative_path_for_file(&self.root, &path, &user_id)],
        );
        Ok(removed)
    }

    /// The store lock spans `still_dead` and the queue write, so a spec
    /// reopened after the sweep selected its entry cannot be missed: the
    /// reopen's own write needs this lock. See the trait doc.
    // trace:BUG-1671 | ai:claude
    fn queue_remove_many_if(
        &self,
        user_id: &str,
        ids: &[uuid::Uuid],
        still_dead: &dyn Fn(&uuid::Uuid) -> bool,
    ) -> Result<Vec<QueueEntry>> {
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // trace:BUG-1671 | ai:claude — re-check and remove under one hold.
        let _lock = self.lock_store()?;
        let live: Vec<uuid::Uuid> = ids.iter().copied().filter(|id| still_dead(id)).collect();
        self.queue_remove_many(user_id, &live)
    }

    /// The role-scoped counterpart of [`Self::queue_remove_many_if`]: the store
    /// lock spans `still_dead` and the queue write.
    // trace:BUG-1671 | ai:claude
    fn queue_remove_for_role_if(
        &self,
        user_id: &str,
        requirement_id: &uuid::Uuid,
        role: Option<&str>,
        still_dead: &dyn Fn(&uuid::Uuid) -> bool,
    ) -> Result<bool> {
        crate::git_ops::ensure_store_write_safe(&self.root)?;
        // trace:BUG-1671 | ai:claude — re-check and remove under one hold.
        let _lock = self.lock_store()?;
        if !still_dead(requirement_id) {
            return Ok(false);
        }
        self.queue_remove_for_role(user_id, requirement_id, role)?;
        Ok(true)
    }
}

/// Agreed-id holders by numeric identity (see [`GitBackend::agreed_id_index`]).
// trace:TASK-B6B1278890774EF993D2EBC327862470-149 | ai:claude
pub(crate) struct AgreedIdIndex {
    /// Id key -> `(object stem, owning uuid)`, in object-listing order.
    by_key: std::collections::HashMap<String, Vec<(String, uuid::Uuid)>>,
}

impl AgreedIdIndex {
    /// The first requirement whose agreed id names `id`'s number, skipping
    /// the object filed under exactly `skip_stem`.
    fn holder(&self, id: &str, skip_stem: Option<&str>) -> Option<uuid::Uuid> {
        self.by_key
            .get(&crate::id_identity::id_key(id))?
            .iter()
            .find(|(stem, _)| !skip_stem.is_some_and(|s| stem.eq_ignore_ascii_case(s)))
            .map(|(_, owner)| *owner)
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
    /// Enabled WORK: `add` buffers only; `finish` prepares the whole batch.
    work: bool,
    metadata: StoreMetadata,
    /// The metadata as loaded by `bulk_writer()`: what `finish` merges
    /// against, so only the counters this batch bumped are written.
    // trace:BUG-1613 | ai:claude
    base: serde_yaml::Value,
    /// Spec ids this writer assigned from the counters (not caller-supplied).
    assigned: std::collections::HashSet<String>,
    staged: Vec<Requirement>,
    /// The agreed-id index for numeric admission, built on the first
    /// caller-supplied id and reused for the rest of the batch.
    // trace:TASK-B6B1278890774EF993D2EBC327862470-149 | ai:claude
    agreed: Option<AgreedIdIndex>,
}

impl<'a> BulkWriter<'a> {
    /// Add a requirement to the batch, assigning a spec_id from the live
    /// metadata counters when missing. Objects and oplog entries are buffered
    /// until `finish()`. An attached persistent dispenser may reserve a local
    /// sequence during `add()`; candidate construction is not a pure preview.
    // trace:BUG-2017 | ai:codex
    pub fn add(&mut self, mut req: Requirement) -> Result<&Requirement> {
        if self.work {
            // trace:STORY-1742 | ai:claude
            self.staged.push(req);
            return Ok(self.staged.last().unwrap());
        }
        // trace:BUG-2017 | ai:codex
        if req.spec_id.is_some() || req.agreed_id.is_some() {
            self.backend.admit_allocations(std::slice::from_ref(&req))?;
            if req.spec_id.is_some() {
                // trace:TASK-B6B1278890774EF993D2EBC327862470-149 | ai:claude —
                // one agreed-id index for the whole batch.
                if self.agreed.is_none() {
                    self.agreed = Some(self.backend.agreed_id_index()?);
                }
                let agreed = self.agreed.as_ref().expect("index built above");
                self.backend
                    .admit_numeric_identity(&req, agreed, &self.staged)?;
            }
        }
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
        self.backend.admit_allocations(std::slice::from_ref(&req))?;
        self.staged.push(req);
        Ok(self.staged.last().unwrap())
    }

    /// Re-assign every id this writer assigned that now exists on disk (a
    /// concurrent writer issued it from the same counter). New ids come from
    /// this batch's counters raised to at least the current disk counters, and
    /// skip any id already on disk or already in the batch. Runs under the
    /// store lock, before anything is written.
    // trace:BUG-1641 | ai:claude
    fn reassign_collided_ids(&mut self, disk_meta: Option<&StoreMetadata>) -> Result<()> {
        let objects_root = &self.backend.objects_root;
        let mut collided: Vec<usize> = Vec::new();
        for (i, req) in self.staged.iter().enumerate() {
            let Some(sid) = req.spec_id.as_deref() else {
                continue;
            };
            // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude — any padding counts.
            if self.assigned.contains(sid)
                && !object_store::numeric_twin_stems(objects_root, sid)?.is_empty()
            {
                collided.push(i);
            }
        }
        if collided.is_empty() {
            return Ok(());
        }
        let mut counters = self.metadata.clone();
        if let Some(disk) = disk_meta {
            raise_counters_to(&mut counters, disk);
        }
        let mut tmp = self.backend.assemble_store(counters, Vec::new());
        let mut taken: std::collections::HashSet<String> = self
            .staged
            .iter()
            .filter_map(|r| r.spec_id.as_deref().map(crate::id_identity::id_key))
            .collect();
        for i in collided {
            let old = self.staged[i].spec_id.take().unwrap_or_default();
            self.assigned.remove(&old);
            let type_prefix = tmp.get_type_prefix(&self.staged[i].req_type);
            // Each attempt advances the counter, so this ends; the bound only
            // guards a dispenser that keeps handing out the same number.
            let mut new_sid = None;
            for _ in 0..10_000 {
                let mut probe = self.staged[i].clone();
                probe.spec_id = None;
                tmp.add_requirement_with_id(probe, None, type_prefix.as_deref());
                let sid = tmp
                    .requirements
                    .pop()
                    .and_then(|r| r.spec_id)
                    .unwrap_or_default();
                if !sid.is_empty()
                    && !taken.contains(&crate::id_identity::id_key(&sid))
                    && object_store::numeric_twin_stems(objects_root, &sid)?.is_empty()
                {
                    new_sid = Some(sid);
                    break;
                }
            }
            let Some(sid) = new_sid else {
                anyhow::bail!(
                    "could not assign a free id to replace {old}, which another writer used \
                     meanwhile; nothing was written"
                );
            };
            taken.insert(crate::id_identity::id_key(&sid));
            self.assigned.insert(sid.clone());
            self.staged[i].spec_id = Some(sid);
        }
        self.metadata.next_feature_number = tmp.next_feature_number;
        self.metadata.next_spec_number = tmp.next_spec_number;
        self.metadata.prefix_counters = tmp.prefix_counters;
        self.metadata.meta_counters = tmp.meta_counters;
        Ok(())
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
    ///
    /// An id this writer assigned that a concurrent writer used meanwhile (the
    /// same counter, read before the other writer bumped it) is re-assigned
    /// under the store lock from the current counters, so the batch lands in
    /// full without overwriting the other object. Caller-supplied ids are
    /// written as given.
    pub fn finish(mut self, commit_subject: &str) -> Result<usize> {
        // trace:STORY-1742 | ai:claude
        #[cfg(all(unix, feature = "native"))]
        match (self.backend.work_admission()?, self.work) {
            (Some(admission), true) => {
                return self
                    .backend
                    .work_bulk_finish(&admission, self.staged, commit_subject)
            }
            (None, false) => {}
            _ => {
                anyhow::bail!("Transport enrollment changed during this batch; nothing was written")
            }
        }
        crate::git_ops::ensure_store_write_safe(&self.backend.root)?;
        // trace:BUG-1612 | ai:claude — serialize with every other store writer.
        let _lock = self.backend.lock_store()?;

        // BUG-1613: re-read metadata.yaml under the lock and merge this batch's
        // counter bumps over it (max, never lowered) instead of writing the
        // snapshot taken at `bulk_writer()`, which reverted concurrent changes.
        // An id this writer assigned that now exists on disk was issued
        // concurrently from the same counter: it is re-assigned (BUG-1641)
        // rather than overwriting the other object or refusing the batch.
        // Everything here runs before anything is written.
        // trace:BUG-1613 trace:BUG-1641 | ai:claude
        let disk_meta = self.backend.read_metadata_if_present()?;
        self.reassign_collided_ids(disk_meta.as_ref())?;
        let meta_plan =
            merge_metadata(Some(&self.base), &self.metadata, disk_meta.as_ref(), false)?;
        let meta_plan = match meta_plan {
            Ok(plan) => plan,
            Err(fields) => {
                return Err(StoreConflictError {
                    specs: Vec::new(),
                    metadata_fields: fields,
                }
                .into())
            }
        };

        // trace:STORY-1675 | ai:codex — imports share the same preflight.
        let count = self.staged.len();
        let imported: std::collections::HashSet<_> = self.staged.iter().map(|r| r.id).collect();
        self.backend
            .prepare_hypothesis_updates(&mut self.staged, &[])?;
        self.backend.admit_metadata(&self.metadata)?;

        // Persist all object YAMLs first (skipping unchanged just in case the
        // caller is re-running an idempotent import).
        let mut written: Vec<String> = Vec::new();
        // CR-8: batch-created specs get filing provenance too (one capture
        // for the whole batch; write-once). trace:CR-8 | ai:claude
        let provenance = crate::provenance::capture();
        for staged in &self.staged {
            if !imported.contains(&staged.id) {
                if let Some(sid) = self.backend.stage_requirement_update(staged)? {
                    written.push(sid.to_string());
                }
                continue;
            }
            let mut stamped = staged.clone();
            let previous = match stamped.spec_id.as_deref() {
                Some(sid) if object_store::object_exists(&self.backend.objects_root, sid)? => {
                    Some(object_store::read_object(&self.backend.objects_root, sid)?)
                }
                _ => None,
            };
            if previous.is_none() {
                crate::provenance::stamp_with(&mut stamped, &provenance);
            }
            crate::provenance::stamp_write(
                &mut stamped,
                previous.as_ref(),
                &crate::provenance::WriteContext::capture(),
            )?;
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
    // trace:BUG-2017 | ai:codex — legacy CRUD fixtures explicitly model HOME.
    // WORK/default/refusal coverage lives in allocation_authority::tests.
    fn home_fixture_backend(root: &std::path::Path) -> anyhow::Result<super::GitBackend> {
        crate::allocation_authority::home_fixture_backend(root)
    }
    use super::*;

    #[test]
    fn test_git_backend_create_and_load_empty() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");

        let backend = home_fixture_backend(&root).unwrap();
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
        let backend = home_fixture_backend(&root).unwrap();

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
        let backend = home_fixture_backend(&root).unwrap();

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
        let backend = home_fixture_backend(&root).unwrap();

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
        let backend = home_fixture_backend(&root).unwrap();

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
        let backend = home_fixture_backend(&root).unwrap();

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
        let backend = home_fixture_backend(&root).unwrap();

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
        let backend = home_fixture_backend(&root).unwrap();

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
        let backend = home_fixture_backend(&root).unwrap();

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
        let backend = home_fixture_backend(&root).unwrap();

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
        let backend = home_fixture_backend(&root).unwrap();

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
        let backend = home_fixture_backend(&root).unwrap();

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
        let backend = home_fixture_backend(&root).unwrap();
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
        let backend = home_fixture_backend(&root).unwrap();

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
        let backend = home_fixture_backend(&root).unwrap();

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

    // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
    fn bug_046_plant(backend: &GitBackend, spec_id: &str, title: &str) -> Requirement {
        let mut r = Requirement::new(title.into(), String::new());
        r.spec_id = Some(spec_id.into());
        object_store::write_object(&backend.objects_root, &r).unwrap();
        r
    }

    /// AC1: leading zeros are insignificant for lookup, both directions.
    #[test]
    fn bug_046_padded_and_unpadded_ids_resolve_to_one_object() {
        let dir = tempfile::tempdir().unwrap();
        let backend = home_fixture_backend(&dir.path().join("aida-store")).unwrap();
        backend.save(&RequirementsStore::new()).unwrap();
        let a = bug_046_plant(&backend, "TASK-0418", "padded");
        let b = bug_046_plant(&backend, "TASK-7", "plain");
        for id in ["TASK-418", "task-00418", "TASK-0418"] {
            let got = backend.get_requirement_by_spec_id(id).unwrap().unwrap();
            assert_eq!(got.id, a.id, "{id}");
        }
        assert_eq!(
            backend
                .get_requirement_by_spec_id("TASK-007")
                .unwrap()
                .unwrap()
                .id,
            b.id
        );
        assert!(backend
            .get_requirement_by_spec_id("TASK-4180")
            .unwrap()
            .is_none());
        assert_eq!(backend.id_holder("TASK-418").unwrap(), Some(a.id));
    }

    /// AC2 regression: a number already used under another padding is
    /// refused when supplied explicitly and skipped when dispensed.
    #[test]
    fn bug_046_mint_never_collides_with_a_padded_twin() {
        let dir = tempfile::tempdir().unwrap();
        let backend = home_fixture_backend(&dir.path().join("aida-store")).unwrap();
        backend.save(&RequirementsStore::new()).unwrap();
        let first = backend
            .add_requirement(Requirement::new("first".into(), String::new()))
            .unwrap();
        let key = crate::id_identity::IdKey::parse(first.spec_id.as_deref().unwrap()).unwrap();
        let next: u64 = key.seq.parse::<u64>().unwrap() + 1;
        // Plant the NEXT number under a padding the dispenser never writes.
        let twin = format!("{}-0000{next}", key.prefix);
        let planted = bug_046_plant(&backend, &twin, "planted twin");
        let second = backend
            .add_requirement(Requirement::new("second".into(), String::new()))
            .unwrap();
        let minted = second.spec_id.as_deref().unwrap();
        assert!(
            !crate::id_identity::same_id(minted, &twin),
            "minted {minted} collides with planted {twin}"
        );
        let mut explicit = Requirement::new("explicit".into(), String::new());
        explicit.spec_id = Some(format!("{}-{next}", key.prefix));
        let err = backend.add_requirement(explicit).unwrap_err();
        assert!(
            err.downcast_ref::<crate::id_identity::IdTakenError>()
                .is_some(),
            "{err:#}"
        );
        assert_eq!(
            backend
                .id_holder(&format!("{}-{next}", key.prefix))
                .unwrap(),
            Some(planted.id)
        );
    }

    /// AC3 plumbing: a renumber moves the object (same UUID) and refuses a
    /// target number already in use.
    #[test]
    fn bug_046_renumber_moves_the_object_and_refuses_a_taken_target() {
        let dir = tempfile::tempdir().unwrap();
        let backend = home_fixture_backend(&dir.path().join("aida-store")).unwrap();
        backend.save(&RequirementsStore::new()).unwrap();
        let a = bug_046_plant(&backend, "TASK-0418", "padded");
        let _b = bug_046_plant(&backend, "TASK-418", "plain");
        let _c = bug_046_plant(&backend, "TASK-9", "other");
        let mut taken = a.clone();
        taken.spec_id = Some("TASK-009".into());
        assert!(backend
            .renumber_requirements(&[("TASK-0418".into(), taken)], &[], "test")
            .is_err());
        let mut moved = a.clone();
        moved.spec_id = Some("TASK-77".into());
        backend
            .renumber_requirements(&[("TASK-0418".into(), moved)], &[], "test")
            .unwrap();
        assert!(!object_store::object_exists(&backend.objects_root, "TASK-0418").unwrap());
        assert_eq!(
            backend
                .get_requirement_by_spec_id("TASK-77")
                .unwrap()
                .unwrap()
                .id,
            a.id
        );
        assert_eq!(
            object_store::numeric_twin_stems(&backend.objects_root, "TASK-418").unwrap(),
            vec!["TASK-418".to_string()]
        );
    }

    /// AC2: a number another spec holds as its AGREED id (under any padding)
    /// is taken, so a dispensed local ID never lands on it.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046.ac5bd8aa | ai:claude
    #[test]
    fn bug_046_id_holder_considers_agreed_ids() {
        let dir = tempfile::tempdir().unwrap();
        let backend = home_fixture_backend(&dir.path().join("aida-store")).unwrap();
        backend.save(&RequirementsStore::new()).unwrap();
        let mut holder = Requirement::new("agreed holder".into(), String::new());
        holder.spec_id = Some("TASK-7-001".into());
        holder.agreed_id = Some("TASK-0055".into());
        object_store::write_object(&backend.objects_root, &holder).unwrap();
        for id in ["TASK-55", "TASK-055", "task-0055"] {
            assert_eq!(backend.id_holder(id).unwrap(), Some(holder.id), "{id}");
        }
        assert_eq!(backend.id_holder("TASK-550").unwrap(), None);
        assert_eq!(backend.id_holder("BUG-55").unwrap(), None);
        let mut explicit = Requirement::new("explicit".into(), String::new());
        explicit.spec_id = Some("TASK-55".into());
        let err =
            crate::id_identity::admit_new_id("TASK-55", &|i| backend.id_holder(i), explicit.id)
                .unwrap_err();
        assert!(err
            .downcast_ref::<crate::id_identity::IdTakenError>()
            .is_some());
    }

    /// AC2 through the real add path on the AUTHORITATIVE clone: an explicit
    /// caller-supplied `TASK-0035` is refused while another spec holds agreed
    /// id `TASK-35`; nothing is written. A same-literal re-add of an object
    /// whose agreed id is its own spec id still replaces that one file.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046.ac5bd8aa | ai:claude
    #[test]
    fn bug_046_add_refuses_an_explicit_id_held_as_another_specs_agreed_id() {
        let dir = tempfile::tempdir().unwrap();
        let backend = home_fixture_backend(&dir.path().join("aida-store")).unwrap();
        backend.save(&RequirementsStore::new()).unwrap();
        assert!(
            crate::allocation_authority::resolve(&backend.root)
                .unwrap()
                .authoritative,
            "fixture must be the authoritative clone"
        );
        let mut holder = Requirement::new("agreed holder".into(), String::new());
        holder.spec_id = Some("TASK-7-001".into());
        holder.agreed_id = Some("TASK-35".into());
        object_store::write_object(&backend.objects_root, &holder).unwrap();

        let mut explicit = Requirement::new("explicit".into(), String::new());
        explicit.spec_id = Some("TASK-0035".into());
        let err = backend.add_requirement(explicit).unwrap_err();
        assert!(
            err.downcast_ref::<crate::id_identity::IdTakenError>()
                .is_some(),
            "{err:#}"
        );
        assert!(
            object_store::numeric_twin_stems(&backend.objects_root, "TASK-35")
                .unwrap()
                .is_empty()
        );

        let mut own = Requirement::new("own agreed".into(), String::new());
        own.spec_id = Some("TASK-36".into());
        own.agreed_id = Some("TASK-36".into());
        object_store::write_object(&backend.objects_root, &own).unwrap();
        let mut reimport = Requirement::new("re-import".into(), String::new());
        reimport.spec_id = Some("TASK-36".into());
        reimport.agreed_id = Some("TASK-36".into());
        backend.add_requirement(reimport).unwrap();
    }

    /// Numeric admission inside ONE batch: the second row of a 2-row batch
    /// whose id is the first row's number under another padding (or the
    /// first row's agreed id) is refused, although neither is on disk yet.
    /// The agreed-id index is built once and reused for the batch.
    // trace:TASK-B6B1278890774EF993D2EBC327862470-149 | ai:claude
    #[test]
    fn bulk_add_refuses_a_row_colliding_with_an_earlier_row_of_the_batch() {
        let dir = tempfile::tempdir().unwrap();
        let backend = home_fixture_backend(&dir.path().join("aida-store")).unwrap();
        backend.save(&RequirementsStore::new()).unwrap();
        let mut holder = Requirement::new("agreed holder".into(), String::new());
        holder.spec_id = Some("TASK-7-001".into());
        holder.agreed_id = Some("TASK-0090".into());
        object_store::write_object(&backend.objects_root, &holder).unwrap();

        let mut writer = backend.bulk_writer().unwrap();
        let mut first = Requirement::new("first".into(), String::new());
        first.spec_id = Some("TASK-0077".into());
        first.agreed_id = Some("TASK-0078".into());
        writer.add(first.clone()).unwrap();
        assert!(
            writer.agreed.is_some(),
            "the index is built on the first explicit id"
        );

        for (id, owner) in [
            ("TASK-77", first.id),
            ("TASK-078", first.id),
            ("TASK-90", holder.id),
        ] {
            let mut second = Requirement::new("second".into(), String::new());
            second.spec_id = Some(id.into());
            let err = writer.add(second).unwrap_err();
            let taken = err
                .downcast_ref::<crate::id_identity::IdTakenError>()
                .unwrap_or_else(|| panic!("{id}: {err:#}"));
            assert_eq!(taken.owner, owner, "{id}");
        }
        let mut fine = Requirement::new("fine".into(), String::new());
        fine.spec_id = Some("TASK-79".into());
        writer.add(fine).unwrap();
        assert_eq!(writer.finish("test batch").unwrap(), 2);
    }

    /// BUG-425: bulk_update commits all changed YAMLs in exactly ONE commit
    /// (the whole point — vs update_requirement's one-commit-per-spec that
    /// turned a 679-spec archive sweep into 679 commits).
    #[test]
    fn bulk_update_writes_all_changed_in_a_single_commit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = home_fixture_backend(&root).unwrap();
        backend.save(&RequirementsStore::new()).unwrap();

        // GitBackend::new doesn't git-init, and auto_commit_paths no-ops
        // outside a git repo — so make the store a real repo, else there are
        // no commits to count. (This is the property under test.)
        let git = |args: &[&str]| {
            crate::test_git::git_command()
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
            let out = crate::test_git::git_command()
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

    /// BUG-1671: the batched compare-and-swap decides on the object read under
    /// the store lock — a spec changed after the caller picked it is skipped,
    /// not overwritten — and still lands in ONE commit (BUG-425).
    // trace:BUG-1671 | ai:claude
    #[test]
    fn bulk_update_atomically_decides_on_the_stored_object_in_one_commit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = home_fixture_backend(&root).unwrap();
        backend.save(&RequirementsStore::new()).unwrap();
        let git = |args: &[&str]| {
            crate::test_git::git_command()
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);

        let mut reqs = Vec::new();
        for i in 1..=3 {
            let r = Requirement::new(format!("Atomic bulk {i}"), format!("desc {i}"));
            reqs.push(backend.add_requirement(r).unwrap());
        }
        // The caller's copy of #2 is stale: on disk it has already been
        // reopened. Its spec_id is also reused by a different uuid for #3.
        let mut reopened = reqs[1].clone();
        reopened.status = crate::models::RequirementStatus::InProgress;
        backend.update_requirement(&reopened).unwrap();
        let mut impostor = reqs[2].clone();
        impostor.id = uuid::Uuid::new_v4();

        let targets = vec![reqs[0].clone(), reqs[1].clone(), impostor];
        let count_commits = || -> usize {
            let out = crate::test_git::git_command()
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

        let report = backend
            .bulk_update_atomically(&targets, "chore(archive)", |req| {
                // Only a spec that is still Draft on disk qualifies.
                if req.status != crate::models::RequirementStatus::Draft {
                    return false;
                }
                req.archived = true;
                true
            })
            .unwrap();

        assert_eq!(report.written.len(), 1, "only the untouched spec qualifies");
        assert_eq!(
            report.written[0].id, reqs[0].id,
            "the written spec is the one still eligible on disk"
        );
        assert_eq!(
            report.skipped_changed, 2,
            "the reopened spec and the uuid mismatch are both skipped: {report:?}"
        );
        assert_eq!(report.skipped_missing, 0);
        assert_eq!(report.skipped_unreadable, 0);
        assert_eq!(report.skipped(), 2);
        assert_eq!(
            count_commits(),
            before + 1,
            "one commit for the whole batch, as bulk_update gives"
        );

        let loaded = backend.load().unwrap();
        let archived: Vec<&str> = loaded
            .requirements
            .iter()
            .filter(|r| r.archived)
            .filter_map(|r| r.spec_id.as_deref())
            .collect();
        assert_eq!(archived, vec![reqs[0].spec_id.as_deref().unwrap()]);
        let on_disk = backend
            .get_requirement_by_spec_id(reqs[1].spec_id.as_deref().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            on_disk.status,
            crate::models::RequirementStatus::InProgress,
            "the reopen survived the batch"
        );
    }

    /// BUG-1671: a candidate whose object is gone or unparsable is counted and
    /// skipped — one damaged spec must not abort a sweep over hundreds.
    // trace:BUG-1671 | ai:claude
    #[test]
    fn bulk_update_atomically_counts_missing_and_unreadable_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = home_fixture_backend(&root).unwrap();
        backend.save(&RequirementsStore::new()).unwrap();

        let mut reqs = Vec::new();
        for i in 1..=3 {
            let r = Requirement::new(format!("Damaged bulk {i}"), format!("desc {i}"));
            reqs.push(backend.add_requirement(r).unwrap());
        }
        // #2's object is unparsable, #3's is deleted.
        let objects = root.join("objects");
        let corrupt =
            object_store::object_path(&objects, reqs[1].spec_id.as_deref().unwrap()).unwrap();
        std::fs::write(&corrupt, ": not yaml at all\n\t- [").unwrap();
        let gone =
            object_store::object_path(&objects, reqs[2].spec_id.as_deref().unwrap()).unwrap();
        std::fs::remove_file(&gone).unwrap();

        let report = backend
            .bulk_update_atomically(&reqs, "chore(archive)", |req| {
                req.archived = true;
                true
            })
            .expect("one damaged object must not fail the batch");

        assert_eq!(report.written.len(), 1);
        assert_eq!(report.skipped_unreadable, 1, "{report:?}");
        assert_eq!(report.skipped_missing, 1, "{report:?}");
        assert_eq!(report.skipped_changed, 0, "{report:?}");
        assert_eq!(
            std::fs::read_to_string(&corrupt).unwrap(),
            ": not yaml at all\n\t- [",
            "an unreadable object is left exactly as it was, not rewritten"
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
        let backend = home_fixture_backend(&root).unwrap();
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
        let backend = home_fixture_backend(&root).unwrap();

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
        let backend = home_fixture_backend(&root).unwrap();

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
        let backend = home_fixture_backend(&root).unwrap();

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
        let backend = home_fixture_backend(&root).unwrap();

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
        let backend = home_fixture_backend(&root).unwrap();

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
        let backend = home_fixture_backend(&root).unwrap();

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
        let backend = home_fixture_backend(&root).unwrap().with_dispenser(handle);

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
        let backend = home_fixture_backend(&root).unwrap().with_auto_commit(false);

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
        let backend = home_fixture_backend(&root).unwrap().with_auto_commit(false);

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
        let detached = crate::test_git::git_command()
            .arg("-C")
            .arg(&root)
            .args(["checkout", "--detach"])
            .status()
            .unwrap();
        assert!(detached.success());

        let backend = home_fixture_backend(&root).unwrap();
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
        let backend = home_fixture_backend(&root).unwrap().with_auto_commit(false);
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
        let backend = home_fixture_backend(&root).unwrap().with_auto_commit(false);
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
        let backend = home_fixture_backend(&root).unwrap().with_auto_commit(false);
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
        let backend = home_fixture_backend(&root).unwrap().with_auto_commit(false);
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

    // trace:STORY-1674 | ai:codex
    #[test]
    fn story1674_canonical_write_routes_persist_context_and_return_stamped_rows() {
        use crate::models::{Comment, HistoryEntry};
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("AIDA_SESSION_ID", "session1674"),
            ("AIDA_HARNESS_VERSION", "harness1674"),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("store");
        let backend = home_fixture_backend(&root).unwrap().with_auto_commit(false);
        let mut req = Requirement::new("new".into(), "body".into());
        req.comments
            .push(Comment::new("writer".into(), "new comment".into()));
        let added = backend.add_requirement(req).unwrap();
        assert_eq!(
            added.filed_at.as_ref().unwrap().harness_version.as_deref(),
            Some("harness1674")
        );
        assert_eq!(added.comments[0].session_id.as_deref(), Some("session1674"));
        let sid = added.spec_id.as_deref().unwrap();
        let read = || object_store::read_object(&backend.objects_root, sid).unwrap();
        let check = |req: &Requirement| {
            let event = req.history.last().expect("write must have an event");
            assert_eq!(event.session_id.as_deref(), Some("session1674"));
            assert_eq!(event.harness_version.as_deref(), Some("harness1674"));
        };
        let mut edit = added.clone();
        edit.title = "direct edit".into();
        edit.history
            .push(HistoryEntry::new("writer".into(), Vec::new()));
        backend.update_requirement(&edit).unwrap();
        check(&read());
        // A retry with a pre-stamp copy must produce identical persisted bytes.
        let prior = serde_yaml::to_string(&read()).unwrap();
        backend.update_requirement(&edit).unwrap();
        assert_eq!(serde_yaml::to_string(&read()).unwrap(), prior);
        // Full save, per-spec atomic, bulk CAS, whole-store transaction.
        let mut store = backend.load().unwrap();
        store.requirements[0].description = "full save".into();
        backend.save(&store).unwrap();
        check(&read());
        let current = read();
        let next = backend
            .update_spec_atomically(&current, |r| r.title = "per-spec".into())
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_yaml::to_string(&next).unwrap(),
            serde_yaml::to_string(&read()).unwrap()
        );
        check(&next);
        let report = backend
            .bulk_update_atomically(&[next], "test", |r| {
                r.title = "bulk CAS".into();
                true
            })
            .unwrap();
        assert_eq!(
            serde_yaml::to_string(&report.written[0]).unwrap(),
            serde_yaml::to_string(&read()).unwrap()
        );
        let (store, summary) = backend
            .update_atomically_tracked(|s| s.requirements[0].title = "transaction".into())
            .unwrap();
        check(&store.requirements[0]);
        assert_eq!(
            serde_yaml::to_string(&store.requirements[0]).unwrap(),
            serde_yaml::to_string(&summary.written[0]).unwrap()
        );
        assert_eq!(
            serde_yaml::to_string(&summary.written[0]).unwrap(),
            serde_yaml::to_string(&read()).unwrap()
        );
        let mut writer = backend.bulk_writer().unwrap();
        let mut new = Requirement::new("bulk created".into(), "body".into());
        let mut c = Comment::new("writer".into(), "parent".into());
        c.replies
            .push(Comment::new_reply("writer".into(), "reply".into(), c.id));
        new.comments.push(c);
        let created_id = writer.add(new).unwrap().spec_id.clone().unwrap();
        writer.finish("test provenance").unwrap();
        let new = object_store::read_object(&backend.objects_root, &created_id).unwrap();
        assert_eq!(
            new.comments[0].replies[0].harness_version.as_deref(),
            Some("harness1674")
        );
        // The shared Storage facade used by MCP must persist the same metadata.
        let storage = crate::storage::Storage::new(root);
        let store = storage
            .update_atomically(|s| {
                s.requirements
                    .iter_mut()
                    .find(|r| r.id == added.id)
                    .unwrap()
                    .title = "facade edit".into()
            })
            .unwrap();
        check(
            store
                .requirements
                .iter()
                .find(|r| r.id == added.id)
                .unwrap(),
        );
    }

    // trace:STORY-1674 | ai:codex
    #[test]
    fn story1674_full_save_id_rename_preserves_legacy_event_context() {
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("AIDA_SESSION_ID", "renamer"),
            ("AIDA_HARNESS_VERSION", "rename-harness"),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("store");
        let backend = home_fixture_backend(&root).unwrap().with_auto_commit(false);
        let mut legacy = Requirement::new("legacy".into(), "body".into());
        legacy.spec_id = Some("TASK-1".into());
        legacy
            .history
            .push(crate::models::HistoryEntry::new("old".into(), Vec::new()));
        legacy.comments.push(crate::models::Comment::new(
            "old".into(),
            "old comment".into(),
        ));
        object_store::write_object(&backend.objects_root, &legacy).unwrap();
        let mut store = backend.load().unwrap();
        store.requirements[0].spec_id = Some("TASK-2".into());
        backend.save(&store).unwrap();
        assert!(!object_store::object_exists(&backend.objects_root, "TASK-1").unwrap());
        let renamed = object_store::read_object(&backend.objects_root, "TASK-2").unwrap();
        assert!(renamed.filed_at.is_none());
        assert!(renamed.history[0].session_id.is_none());
        assert!(renamed.comments[0].session_id.is_none());
        assert_eq!(
            renamed.history.last().unwrap().session_id.as_deref(),
            Some("renamer")
        );
        assert_eq!(
            renamed.history.last().unwrap().harness_version.as_deref(),
            Some("rename-harness")
        );
    }

    // A spec_id change inside a whole-store transaction is delete + create of
    // the same UUID: legacy events keep unknown context, filing provenance is
    // not invented, and only the new write event carries this session.
    // trace:STORY-1674 | ai:claude
    #[test]
    fn story1674_transaction_id_rename_preserves_legacy_event_context() {
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("AIDA_SESSION_ID", "renamer"),
            ("AIDA_HARNESS_VERSION", "rename-harness"),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("store");
        let backend = home_fixture_backend(&root).unwrap().with_auto_commit(false);
        let mut legacy = Requirement::new("legacy".into(), "body".into());
        legacy.spec_id = Some("TASK-1".into());
        legacy.history.push(crate::models::HistoryEntry::new(
            "old".into(),
            vec![crate::models::FieldChange {
                field_name: "title".into(),
                old_value: "a".into(),
                new_value: "legacy".into(),
            }],
        ));
        legacy.comments.push(crate::models::Comment::new(
            "old".into(),
            "old comment".into(),
        ));
        object_store::write_object(&backend.objects_root, &legacy).unwrap();
        let (_, summary) = backend
            .update_atomically_tracked(|s| s.requirements[0].spec_id = Some("TASK-2".into()))
            .unwrap();
        assert_eq!(summary.created, vec!["TASK-2".to_string()]);
        assert!(!object_store::object_exists(&backend.objects_root, "TASK-1").unwrap());
        let renamed = object_store::read_object(&backend.objects_root, "TASK-2").unwrap();
        assert!(renamed.filed_at.is_none(), "no invented filing provenance");
        assert!(renamed.history[0].session_id.is_none());
        assert!(renamed.history[0].harness_version.is_none());
        assert!(renamed.comments[0].session_id.is_none());
        assert!(renamed.comments[0].harness_version.is_none());
        let event = renamed.history.last().unwrap();
        assert!(event.is_write_event());
        assert_eq!(event.session_id.as_deref(), Some("renamer"));
    }

    // ---- BUG-1612: per-spec compare-and-swap, no whole-store rewrite -------

    /// A git-initialized store with `n` seeded specs (TASK-1..TASK-n).
    // trace:BUG-1612 | ai:claude
    fn bug1612_git_store(n: usize) -> (tempfile::TempDir, PathBuf, GitBackend, Vec<Requirement>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = home_fixture_backend(&root).unwrap();
        backend.save(&RequirementsStore::new()).unwrap();
        for args in [
            &["init", "-q"][..],
            &["config", "user.email", "test@example.com"][..],
            &["config", "user.name", "Test"][..],
        ] {
            crate::test_git::git_command()
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
        let out = crate::test_git::git_command()
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
            let b = home_fixture_backend(&root_b).unwrap();
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
                    let b = home_fixture_backend(&root).unwrap().with_auto_commit(false);
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
                crate::test_git::git_command()
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
            let out = crate::test_git::git_command()
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
        home_fixture_backend(root).unwrap()
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
    /// meanwhile is re-assigned under the lock: the whole batch lands, the
    /// other object is untouched, and the counter moves past both.
    // trace:BUG-1613 trace:BUG-1641 | ai:claude
    #[test]
    fn bug1641_bulk_writer_reassigns_a_reissued_id() {
        let (_dir, root, backend) = bug1613_per_prefix_store();
        let mut writer = backend.bulk_writer().unwrap();
        let theirs = bug1613_other(&root)
            .add_requirement(bug1613_bug("theirs"))
            .unwrap();
        let sid = theirs.spec_id.clone().unwrap();
        let counter = bug1613_disk_meta(&root).prefix_counters["BUG"];
        let mine = writer.add(bug1613_bug("bulk 1")).unwrap().clone();
        assert_eq!(mine.spec_id.as_deref(), Some(sid.as_str()));
        let second = writer.add(bug1613_bug("bulk 2")).unwrap().clone();
        assert_eq!(writer.finish("test(bulk)").unwrap(), 2);

        let objects = root.join("objects");
        assert_eq!(
            object_store::read_object(&objects, &sid).unwrap().title,
            "theirs"
        );
        // The non-colliding id keeps what add() returned.
        let second_sid = second.spec_id.clone().unwrap();
        assert_eq!(
            object_store::read_object(&objects, &second_sid)
                .unwrap()
                .title,
            "bulk 2"
        );
        let loaded = backend.load().unwrap();
        let bulk1: Vec<_> = loaded
            .requirements
            .iter()
            .filter(|r| r.title == "bulk 1")
            .collect();
        assert_eq!(bulk1.len(), 1);
        let new_sid = bulk1[0].spec_id.clone().unwrap();
        assert_ne!(new_sid, sid);
        assert_ne!(new_sid, second_sid);
        assert!(new_sid.starts_with("BUG-"));
        let meta = bug1613_disk_meta(&root);
        assert!(
            meta.prefix_counters["BUG"] > counter,
            "the counter moves past the re-assigned id"
        );
        // The next id handed out does not collide with anything written.
        let next = bug1613_other(&root)
            .add_requirement(bug1613_bug("after"))
            .unwrap();
        let next_sid = next.spec_id.unwrap();
        assert!(![sid.as_str(), new_sid.as_str(), second_sid.as_str()].contains(&next_sid.as_str()));
    }

    /// Re-assignment starts from the DISK counters when the disk moved
    /// further than the batch: another writer issued BUG-001..003, the batch
    /// collided on BUG-001, so its spec becomes BUG-004 (not BUG-002 or
    /// BUG-003, which would overwrite theirs). A caller-supplied id in the
    /// same batch is written as given.
    // trace:BUG-1641 | ai:claude
    #[test]
    fn bug1641_bulk_writer_reassigns_past_a_disk_that_moved_further() {
        let (_dir, root, backend) = bug1613_per_prefix_store();
        let mut writer = backend.bulk_writer().unwrap();
        let other = bug1613_other(&root);
        let theirs: Vec<String> = (1..=3)
            .map(|i| {
                other
                    .add_requirement(bug1613_bug(&format!("theirs {i}")))
                    .unwrap()
                    .spec_id
                    .unwrap()
            })
            .collect();
        assert_eq!(theirs, vec!["BUG-001", "BUG-002", "BUG-003"]);

        let mine = writer.add(bug1613_bug("mine")).unwrap().clone();
        assert_eq!(mine.spec_id.as_deref(), Some("BUG-001"));
        let mut supplied = bug1613_bug("supplied");
        supplied.spec_id = Some("BUG-50".into());
        writer.add(supplied).unwrap();
        assert_eq!(writer.finish("test(bulk)").unwrap(), 2);

        let objects = root.join("objects");
        for (i, sid) in theirs.iter().enumerate() {
            assert_eq!(
                object_store::read_object(&objects, sid).unwrap().title,
                format!("theirs {}", i + 1),
                "the other writer's {sid} was overwritten"
            );
        }
        assert_eq!(
            object_store::read_object(&objects, "BUG-004")
                .unwrap()
                .title,
            "mine"
        );
        assert_eq!(
            object_store::read_object(&objects, "BUG-50").unwrap().title,
            "supplied"
        );
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

        let merged = merge_metadata(Some(&base_v), &mine, Some(&disk), false)
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
        let noop = merge_metadata(Some(&base_v), &base, Some(&disk), false)
            .unwrap()
            .unwrap();
        assert!(noop.write.is_none());

        // No baseline (a store not loaded from the backend): the writer's
        // fields win, as before, but the counter is not lowered.
        let legacy = merge_metadata(None, &mine, Some(&disk), false)
            .unwrap()
            .unwrap()
            .write
            .unwrap();
        assert_eq!(legacy.description, "");
        assert_eq!(legacy.prefix_counters["BUG"], 9);
    }

    // ---- BUG-1641: explicit counter reset, wording, spec-only save ----

    /// An ID-format migration resets the counters and the save writes them,
    /// lowered values and removed per-prefix keys included, instead of keeping
    /// the higher counters already on disk.
    // trace:BUG-1641 | ai:claude
    #[test]
    fn bug1641_migrate_counter_reset_lands() {
        let (_dir, root, backend) = bug1613_per_prefix_store();
        backend
            .update_atomically(|s| {
                s.prefix_counters.insert("BUG".into(), 90);
                s.prefix_counters.insert("GONE".into(), 40);
            })
            .unwrap();
        let mut store = backend.load().unwrap();
        store.migrate_to_new_id_format();
        assert!(store.id_counters_reset.is_set());
        let expected = store.prefix_counters.clone();
        backend.save(&store).unwrap();
        assert!(
            !store.id_counters_reset.is_set(),
            "a saved reset is one-shot"
        );
        let meta = bug1613_disk_meta(&root);
        assert_eq!(meta.prefix_counters, expected);
        assert!(!meta.prefix_counters.contains_key("GONE"));
        assert!(!meta.prefix_counters.contains_key("BUG"));
        assert_eq!(meta.next_spec_number, 1);

        // Saving the same store again does not re-assert the reset over a
        // counter another writer bumped since.
        bug1613_other(&root)
            .add_requirement(bug1613_bug("after reset"))
            .unwrap();
        let bumped = bug1613_disk_meta(&root).prefix_counters["BUG"];
        store.title = "again".into();
        backend.save(&store).unwrap();
        let meta = bug1613_disk_meta(&root);
        assert_eq!(meta.prefix_counters["BUG"], bumped);
        assert_eq!(meta.title, "again");
    }

    /// Without the explicit reset, an ordinary save cannot lower a counter or
    /// drop a per-prefix key: the higher disk value is kept.
    // trace:BUG-1641 | ai:claude
    #[test]
    fn bug1641_plain_save_never_lowers_a_counter() {
        let (_dir, root, backend) = bug1613_per_prefix_store();
        backend
            .update_atomically(|s| {
                s.prefix_counters.insert("BUG".into(), 90);
            })
            .unwrap();
        let mut store = backend.load().unwrap();
        store.prefix_counters.clear();
        store.next_spec_number = 1;
        backend.save(&store).unwrap();
        assert_eq!(bug1613_disk_meta(&root).prefix_counters["BUG"], 90);
    }

    /// A reset raced by a concurrent counter bump is a conflict, not a silent
    /// lowering of the other writer's counter; nothing is written.
    // trace:BUG-1641 | ai:claude
    #[test]
    fn bug1641_reset_over_concurrent_bump_is_a_conflict() {
        let (_dir, root, backend) = bug1613_per_prefix_store();
        backend
            .update_atomically(|s| {
                s.prefix_counters.insert("BUG".into(), 90);
            })
            .unwrap();
        let mut store = backend.load().unwrap();
        bug1613_other(&root)
            .add_requirement(bug1613_bug("theirs"))
            .unwrap();
        let before = bug1613_disk_meta(&root).prefix_counters.clone();
        store.reset_id_counters();
        let err = backend.save(&store).unwrap_err();
        let conflict = err.downcast_ref::<StoreConflictError>().unwrap();
        assert_eq!(conflict.metadata_fields, vec!["prefix_counters"]);
        assert_eq!(bug1613_disk_meta(&root).prefix_counters, before);
    }

    /// update_atomically follows the same counter rules as save(): a plain
    /// lowering is ignored, an explicit reset lands.
    // trace:BUG-1641 | ai:claude
    #[test]
    fn bug1641_update_atomically_counter_rules_match_save() {
        let (_dir, root, backend) = bug1613_per_prefix_store();
        backend
            .update_atomically(|s| {
                s.prefix_counters.insert("BUG".into(), 90);
            })
            .unwrap();
        backend
            .update_atomically(|s| {
                s.prefix_counters.insert("BUG".into(), 3);
            })
            .unwrap();
        assert_eq!(bug1613_disk_meta(&root).prefix_counters["BUG"], 90);
        let after_reset = backend
            .update_atomically(|s| s.reset_id_counters())
            .unwrap();
        assert!(
            !after_reset.id_counters_reset.is_set(),
            "a saved reset is one-shot"
        );
        let meta = bug1613_disk_meta(&root);
        assert!(!meta.prefix_counters.contains_key("BUG"));
        assert_eq!(meta.next_spec_number, 1);
    }

    /// The conflict message names store-level fields in user-facing words,
    /// never the internal field names.
    // trace:BUG-1641 | ai:claude
    #[test]
    fn bug1641_conflict_message_uses_friendly_names() {
        let err = StoreConflictError {
            specs: Vec::new(),
            metadata_fields: vec!["type_definitions".into(), "features".into()],
        };
        let text = err.to_string();
        assert!(!text.contains("type_definitions"), "{text}");
        assert!(!text.contains("setting"), "{text}");
        assert!(text.contains("requirement types"), "{text}");
        assert!(text.contains("feature list"), "{text}");
        assert!(!text.contains(" it "), "{text}");
        assert!(!text.contains(" them "), "{text}");
    }

    /// A save that changed only a spec neither rewrites nor commits
    /// metadata.yaml.
    // trace:BUG-1641 | ai:claude
    #[test]
    fn bug1641_spec_only_save_leaves_metadata_alone() {
        let (_dir, root, backend, _) = bug1612_git_store(2);
        let git = |args: &[&str]| {
            crate::test_git::git_command()
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap()
        };
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "baseline"]);
        let meta_path = root.join("metadata.yaml");
        let bytes_before = std::fs::read(&meta_path).unwrap();
        let mtime_before = std::fs::metadata(&meta_path).unwrap().modified().unwrap();
        let head_before = git(&["rev-parse", "HEAD"]).stdout;

        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut store = backend.load().unwrap();
        for r in store.requirements.iter_mut() {
            if r.spec_id.as_deref() == Some("TASK-1") {
                r.title = "spec only".into();
            }
        }
        backend.save(&store).unwrap();

        assert_ne!(
            git(&["rev-parse", "HEAD"]).stdout,
            head_before,
            "the spec change was committed"
        );
        let files = bug1612_head_files(&root);
        assert!(files.iter().any(|f| f.contains("TASK-1")), "{files:?}");
        assert!(!files.iter().any(|f| f == "metadata.yaml"), "{files:?}");
        assert_eq!(std::fs::read(&meta_path).unwrap(), bytes_before);
        assert_eq!(
            std::fs::metadata(&meta_path).unwrap().modified().unwrap(),
            mtime_before,
            "metadata.yaml was rewritten"
        );
        assert!(git(&["status", "--porcelain", "metadata.yaml"])
            .stdout
            .is_empty());
    }

    // trace:BUG-1677 | ai:claude — every queue writer is a read-modify-write
    // on one file; without the store lock two of them interleave (both read
    // the same entries, the later write drops the earlier writer's change),
    // and without an atomic write-back a lock-free reader sees a torn file.
    // The tests below use the git backend on a plain directory, where each
    // writer opens its own lock descriptor, so threads contend exactly as
    // processes do under flock(2).

    fn bug1677_backend(root: &Path) -> GitBackend {
        home_fixture_backend(root).unwrap()
    }

    fn bug1677_seed(backend: &GitBackend, user: &str, n: usize, base: i64) -> Vec<uuid::Uuid> {
        (0..n)
            .map(|i| {
                let e = sample_queue_entry(user, base + (i as i64 + 1) * 1000);
                let id = e.requirement_id;
                backend.queue_add(e).unwrap();
                id
            })
            .collect()
    }

    // queue_clear is covered for lock blocking and re-entrancy above, but is
    // deliberately excluded from this lost-update mix: racing a clear against
    // adds/removes has no single expected final queue, so the assertion below
    // could not distinguish a correct serialized outcome from a lost update.
    // Writer mix: queue_add (new entries), queue_remove_for_role and
    // queue_remove_many (each draining its own seeded set), queue_reorder
    // (each thread repositions its own stable subset), plus lock-free readers
    // that must always parse the file and always see every stable entry.
    #[test]
    fn bug_1677_concurrent_queue_writers_lose_nothing_and_readers_see_no_torn_file() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Barrier};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = bug1677_backend(&root);
        let user = "carol";

        const REMOVERS: usize = 3;
        const REMOVES_EACH: usize = 10;
        const MANY_REMOVERS: usize = 2;
        const MANY_CALLS_EACH: usize = 4;
        const MANY_IDS_PER_CALL: usize = 3;
        const REORDERERS: usize = 3;
        const REORDER_IDS_EACH: usize = 5;
        const REORDER_ROUNDS: usize = 8;
        const ADDERS: usize = 3;
        const ADDS_EACH: usize = 25;
        const READERS: usize = 2;

        let to_remove = bug1677_seed(&backend, user, REMOVERS * REMOVES_EACH, 0);
        let to_remove_many = bug1677_seed(
            &backend,
            user,
            MANY_REMOVERS * MANY_CALLS_EACH * MANY_IDS_PER_CALL,
            100_000,
        );
        let stable = bug1677_seed(&backend, user, REORDERERS * REORDER_IDS_EACH, 200_000);
        let seeded_total = to_remove.len() + to_remove_many.len() + stable.len();
        assert_eq!(backend.queue_list(user, false).unwrap().len(), seeded_total);

        let writers = ADDERS + REMOVERS + MANY_REMOVERS + REORDERERS;
        let barrier = Arc::new(Barrier::new(writers + READERS));
        let stop = Arc::new(AtomicBool::new(false));
        let mut handles: Vec<std::thread::JoinHandle<Result<()>>> = Vec::new();
        let mut expected_added: Vec<uuid::Uuid> = Vec::new();
        for _ in 0..ADDERS {
            let entries: Vec<QueueEntry> = (0..ADDS_EACH)
                .map(|_| sample_queue_entry(user, i64::MAX))
                .collect();
            expected_added.extend(entries.iter().map(|e| e.requirement_id));
            let (root, barrier) = (root.clone(), Arc::clone(&barrier));
            handles.push(std::thread::spawn(move || {
                let backend = bug1677_backend(&root);
                barrier.wait();
                for e in entries {
                    backend.queue_add(e)?;
                }
                Ok(())
            }));
        }
        for chunk in to_remove.chunks(REMOVES_EACH) {
            let ids: Vec<uuid::Uuid> = chunk.to_vec();
            let (root, barrier) = (root.clone(), Arc::clone(&barrier));
            handles.push(std::thread::spawn(move || {
                let backend = bug1677_backend(&root);
                barrier.wait();
                for id in &ids {
                    backend.queue_remove_for_role("carol", id, None)?;
                }
                Ok(())
            }));
        }
        for chunk in to_remove_many.chunks(MANY_CALLS_EACH * MANY_IDS_PER_CALL) {
            let ids: Vec<uuid::Uuid> = chunk.to_vec();
            let (root, barrier) = (root.clone(), Arc::clone(&barrier));
            handles.push(std::thread::spawn(move || {
                let backend = bug1677_backend(&root);
                barrier.wait();
                for call in ids.chunks(MANY_IDS_PER_CALL) {
                    let removed = backend.queue_remove_many("carol", call)?;
                    anyhow::ensure!(
                        removed.len() == call.len(),
                        "queue_remove_many removed {} of {}",
                        removed.len(),
                        call.len()
                    );
                }
                Ok(())
            }));
        }
        // Each reorderer owns a distinct subset of the stable ids and moves it
        // REORDER_ROUNDS times; the LAST round's positions must be on disk.
        let mut expected_positions: Vec<(uuid::Uuid, i64)> = Vec::new();
        for (t, chunk) in stable.chunks(REORDER_IDS_EACH).enumerate() {
            let ids: Vec<uuid::Uuid> = chunk.to_vec();
            let final_round = (REORDER_ROUNDS - 1) as i64;
            for (i, id) in ids.iter().enumerate() {
                expected_positions.push((
                    *id,
                    1_000_000 + (t as i64) * 100_000 + final_round * 1_000 + i as i64,
                ));
            }
            let (root, barrier) = (root.clone(), Arc::clone(&barrier));
            handles.push(std::thread::spawn(move || {
                let backend = bug1677_backend(&root);
                barrier.wait();
                for round in 0..REORDER_ROUNDS as i64 {
                    let items: Vec<(uuid::Uuid, i64)> = ids
                        .iter()
                        .enumerate()
                        .map(|(i, id)| {
                            (
                                *id,
                                1_000_000 + (t as i64) * 100_000 + round * 1_000 + i as i64,
                            )
                        })
                        .collect();
                    backend.queue_reorder("carol", &items)?;
                }
                Ok(())
            }));
        }
        let mut readers = Vec::new();
        for _ in 0..READERS {
            let (root, barrier, stop) = (root.clone(), Arc::clone(&barrier), Arc::clone(&stop));
            let stable = stable.clone();
            readers.push(std::thread::spawn(move || -> Result<usize> {
                let backend = bug1677_backend(&root);
                barrier.wait();
                let mut reads = 0usize;
                while !stop.load(Ordering::Relaxed) {
                    let seen = backend
                        .queue_list("carol", false)
                        .map_err(|e| anyhow::anyhow!("reader hit a torn queue file: {e}"))?;
                    let missing = stable
                        .iter()
                        .filter(|id| !seen.iter().any(|e| e.requirement_id == **id))
                        .count();
                    anyhow::ensure!(
                        missing == 0,
                        "reader saw a torn queue file: {missing} stable entries missing \
                         (read {} entries)",
                        seen.len()
                    );
                    reads += 1;
                }
                Ok(reads)
            }));
        }

        let mut errors = Vec::new();
        for h in handles {
            if let Err(e) = h.join().expect("queue writer thread panicked") {
                errors.push(e.to_string());
            }
        }
        stop.store(true, Ordering::Relaxed);
        let mut total_reads = 0;
        for r in readers {
            match r.join().expect("reader thread panicked") {
                Ok(n) => total_reads += n,
                Err(e) => errors.push(e.to_string()),
            }
        }
        assert!(
            errors.is_empty(),
            "queue writers/readers failed: {errors:?}"
        );
        assert!(total_reads > 0, "readers never ran");

        let final_entries = backend.queue_list(user, false).unwrap();
        let mut remaining: Vec<uuid::Uuid> =
            final_entries.iter().map(|e| e.requirement_id).collect();
        remaining.sort();
        let mut expected: Vec<uuid::Uuid> = expected_added
            .iter()
            .chain(stable.iter())
            .copied()
            .collect();
        expected.sort();
        let lost_added = expected_added
            .iter()
            .filter(|id| !remaining.contains(id))
            .count();
        let lost_stable = stable.iter().filter(|id| !remaining.contains(id)).count();
        let unremoved = to_remove
            .iter()
            .chain(to_remove_many.iter())
            .filter(|id| remaining.contains(id))
            .count();
        assert_eq!(
            (lost_added, lost_stable, unremoved),
            (0, 0, 0),
            "lost {lost_added} added and {lost_stable} stable entries, {unremoved} removals \
             undone; {} entries remain",
            remaining.len()
        );
        assert_eq!(remaining, expected);
        let stale_reorders: Vec<_> = expected_positions
            .iter()
            .filter(|(id, pos)| {
                final_entries
                    .iter()
                    .find(|e| e.requirement_id == *id)
                    .is_none_or(|e| e.position != *pos)
            })
            .collect();
        assert!(
            stale_reorders.is_empty(),
            "{} reorders were overwritten by a concurrent writer: {stale_reorders:?}",
            stale_reorders.len()
        );
        assert!(
            final_entries.iter().all(|e| e.position != i64::MAX),
            "the append sentinel must be resolved under the lock"
        );
        assert!(
            std::fs::read_dir(root.join("registry/queues"))
                .unwrap()
                .flatten()
                .all(|d| d.path().extension().and_then(|x| x.to_str()) == Some("yaml")),
            "atomic-write staging files must not be left behind"
        );
    }

    // An absent queue file is the empty queue (the writer may not have
    // created it yet), and a missing parent directory is too: the read no
    // longer depends on an `exists()` pre-check that a concurrent rename can
    // invalidate.
    #[test]
    fn bug_1677_absent_queue_file_reads_as_empty_queue() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = bug1677_backend(&root);
        assert!(backend.queue_list("nobody", false).unwrap().is_empty());
        let missing = root.join("registry/queues/nobody.yaml");
        assert!(GitBackend::read_queue_file(&missing).unwrap().is_empty());
        std::fs::create_dir_all(root.join("registry/queues")).unwrap();
        assert!(GitBackend::read_queue_file(&missing).unwrap().is_empty());
        let unreadable = root.join("registry/queues");
        assert!(
            GitBackend::read_queue_file(&unreadable).is_err(),
            "a real read error (a directory) must still surface"
        );
    }

    // Each of the five writers blocks while ANOTHER thread holds the store
    // lock and proceeds once it is released: the lock is really taken.
    #[test]
    fn bug_1677_queue_writers_block_while_another_thread_holds_the_store_lock() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let backend = bug1677_backend(&root);
        let seeded = bug1677_seed(&backend, "dan", 3, 0);
        let (id_a, id_b, id_c) = (seeded[0], seeded[1], seeded[2]);

        type Writer = Box<dyn FnOnce(&GitBackend) -> Result<()> + Send>;
        let writers: Vec<(&str, Writer)> = vec![
            (
                "queue_add",
                Box::new(|b: &GitBackend| b.queue_add(sample_queue_entry("dan", i64::MAX))),
            ),
            (
                "queue_reorder",
                Box::new(move |b: &GitBackend| b.queue_reorder("dan", &[(id_c, 5)])),
            ),
            (
                "queue_remove_for_role",
                Box::new(move |b: &GitBackend| b.queue_remove_for_role("dan", &id_a, None)),
            ),
            (
                "queue_remove_many",
                Box::new(move |b: &GitBackend| b.queue_remove_many("dan", &[id_b]).map(|_| ())),
            ),
            (
                "queue_clear",
                Box::new(|b: &GitBackend| b.queue_clear("dan", false)),
            ),
        ];
        for (name, writer) in writers {
            let guard = backend.lock_store().unwrap();
            let (tx, rx) = mpsc::channel();
            let r2 = root.clone();
            let handle = std::thread::spawn(move || {
                let b = bug1677_backend(&r2);
                let result = writer(&b);
                tx.send((Instant::now(), result)).unwrap();
            });
            std::thread::sleep(Duration::from_millis(200));
            assert!(
                rx.try_recv().is_err(),
                "{name} must block while another thread holds the store lock"
            );
            let released = Instant::now();
            drop(guard);
            let (finished, result) = rx
                .recv_timeout(Duration::from_secs(30))
                .unwrap_or_else(|_| panic!("{name} never completed after the lock was released"));
            handle.join().unwrap();
            result.unwrap_or_else(|e| panic!("{name} failed: {e}"));
            assert!(finished >= released, "{name} finished before the release");
        }
    }

    // A caller that already holds the store lock on ITS OWN thread (as the
    // cached backend's write paths do) can call every queue writer without
    // deadlocking: the lock is re-entrant per thread. The channel timeout is
    // the guard; the default lock wait is 120s, so a regression shows up here
    // as a 30s failure rather than a hung test.
    #[test]
    fn bug_1677_queue_writers_reenter_a_store_lock_held_by_the_caller() {
        use std::sync::mpsc;
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let b = bug1677_backend(&root);
            let run = || -> Result<usize> {
                let _outer = b.lock_store()?;
                let ids = bug1677_seed(&b, "erin", 3, 0);
                b.queue_add(sample_queue_entry("erin", i64::MAX))?;
                b.queue_reorder("erin", &[(ids[2], 5)])?;
                b.queue_remove_for_role("erin", &ids[0], None)?;
                let removed = b.queue_remove_many("erin", &[ids[1]])?;
                anyhow::ensure!(removed.len() == 1);
                let after_remove = b.queue_list("erin", false)?.len();
                b.queue_clear("erin", false)?;
                assert!(b.queue_list("erin", false)?.is_empty());
                assert!(
                    super::super::store_lock::held_by_this_thread(&root),
                    "the caller's outer hold must survive the inner releases"
                );
                Ok(after_remove)
            };
            tx.send(run()).unwrap();
        });
        let result = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("queue writers deadlocked against the caller's own store lock");
        handle.join().unwrap();
        assert_eq!(result.unwrap(), 2);
    }

    // trace:TASK-1717 | ai:claude — a writer checks the store state before it
    // waits for the lock; the holder can leave a rebase behind in between (a
    // sync whose own clean-up failed). The writer checks again once it owns
    // the lock and refuses, instead of writing onto the rebase HEAD.
    #[test]
    fn task_1717_writer_rechecks_store_state_after_acquiring_the_lock() {
        use std::process::Command;
        use std::sync::mpsc;
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        std::fs::create_dir_all(&root).unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.email", "t@example.com"],
            &["config", "user.name", "t"],
            &["commit", "-q", "--allow-empty", "-m", "seed"],
        ] {
            assert!(crate::test_git::git_command()
                .current_dir(&root)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        let backend = bug1677_backend(&root);
        let guard = backend.lock_store().unwrap();
        let (tx, rx) = mpsc::channel();
        let r2 = root.clone();
        let handle = std::thread::spawn(move || {
            let b = bug1677_backend(&r2);
            tx.send(b.queue_add(sample_queue_entry("fay", 1))).unwrap();
        });
        std::thread::sleep(Duration::from_millis(200));
        assert!(rx.try_recv().is_err(), "the writer waits for the lock");
        std::fs::create_dir_all(root.join(".git").join("rebase-merge")).unwrap();
        drop(guard);
        let result = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("writer never finished");
        handle.join().unwrap();
        let err = result.expect_err("a rebase left behind must be refused");
        assert!(
            err.to_string().contains("refusing AIDA store write"),
            "{err:#}"
        );
        std::fs::remove_dir_all(root.join(".git").join("rebase-merge")).unwrap();
        assert!(backend.queue_list("fay", false).unwrap().is_empty());
    }

    // trace:BUG-2140 | ai:antigravity
    #[cfg(unix)]
    #[test]
    fn bug_2140_stage_oplog_and_deferred_targets_stages_deferred_objects_and_refuses_corrupt_state()
    {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aida-store");
        std::fs::create_dir_all(&root).unwrap();
        for args in [
            &["init", "-q", "-b", "aida-store"][..],
            &["config", "user.email", "t@example.com"],
            &["config", "user.name", "t"],
        ] {
            assert!(crate::test_git::git_command()
                .current_dir(&root)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        let backend = home_fixture_backend(&root)
            .unwrap()
            .with_auto_commit(true)
            .with_oplog(true);
        let r1 = backend
            .add_requirement(Requirement::new("Seed".into(), "seed".into()))
            .unwrap();
        assert_eq!(r1.spec_id.as_deref(), Some("FR-001"));

        // Defer a second requirement write by locking `.git/objects` read-only.
        let git_objects = root.join(".git/objects");
        std::fs::set_permissions(&git_objects, std::fs::Permissions::from_mode(0o555)).unwrap();
        let r2 = backend
            .add_requirement(Requirement::new("Deferred".into(), "deferred".into()))
            .unwrap();
        std::fs::set_permissions(&git_objects, std::fs::Permissions::from_mode(0o755)).unwrap();

        let r2_rel = object_store::relative_object_path(r2.spec_id.as_deref().unwrap()).unwrap();
        let r2_disk = root.join(&r2_rel);
        let r2_valid = std::fs::read_to_string(&r2_disk).unwrap();

        // Corrupt the deferred object on disk: `stage_oplog_and_deferred_targets`
        // must return `Err` instead of silently committing `oplog.yaml` without
        // its companion object (F4).
        std::fs::write(&r2_disk, "id: [corrupt yaml").unwrap();
        assert!(
            backend.stage_oplog_and_deferred_targets(&[]).is_err(),
            "corrupt deferred object must fail stage_oplog_and_deferred_targets"
        );
        std::fs::write(&r2_disk, &r2_valid).unwrap();

        // Corrupt `oplog.yaml` on disk: `stage_oplog_and_deferred_targets` must
        // return `Err` as well.
        let oplog_path = root.join("oplog.yaml");
        let oplog_valid = std::fs::read_to_string(&oplog_path).unwrap();
        std::fs::write(&oplog_path, "operations: [corrupt yaml").unwrap();
        assert!(
            backend.stage_oplog_and_deferred_targets(&[]).is_err(),
            "corrupt oplog.yaml must fail stage_oplog_and_deferred_targets"
        );
        std::fs::write(&oplog_path, &oplog_valid).unwrap();

        // Once valid, a targeted host update on `r1` commits `r1`, `oplog.yaml`,
        // AND the earlier deferred `r2` object in the same commit.
        let mut r1_edit = r1.clone();
        r1_edit.title = "Seed updated".into();
        backend.update_requirement(&r1_edit).unwrap();

        let status = crate::test_git::git_command()
            .current_dir(&root)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&status.stdout).trim().is_empty(),
            "deferred object + oplog must be committed cleanly alongside targeted update"
        );
    }
}
