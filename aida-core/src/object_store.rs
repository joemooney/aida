// trace:ARCH-distributed-objectstore | ai:claude
//! Git-friendly object store for distributed AIDA.
//!
//! Stores requirements as individual YAML files in a sharded directory layout:
//! ```text
//! objects/
//!   FR/
//!     000/FR-001.yaml ... FR-1000.yaml
//!     001/FR-1001.yaml ... FR-2000.yaml
//!   BUG/
//!     000/BUG-001.yaml ...
//! ```
//!
//! Each file contains a single requirement serialized as YAML. The sharded
//! layout keeps directories under 1000 entries for filesystem performance
//! and git efficiency (58% faster incremental push vs flat layout — see
//! `docs/plans/2026-03-15-git-scaling-spike-results.md`).
//!
//! This module handles path computation and file I/O only. It does not
//! manage git operations (commit, push, pull) — those are the caller's
//! responsibility.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::models::Requirement;

/// Maximum files per shard directory.
const SHARD_SIZE: u32 = 1000;

/// Canonicalize a user-supplied spec_id to ASCII uppercase. Spec IDs are
/// always written uppercase (FR-1, BUG-1-042) but we accept any case from
/// the CLI, hooks, and trace comments. trace:STORY-X | ai:claude
pub fn canonical_spec_id(spec_id: &str) -> String {
    spec_id.to_ascii_uppercase()
}

/// Compute the shard directory number for a given sequence number.
/// Shard 000 holds sequences 1-1000, shard 001 holds 1001-2000, etc.
fn shard_number(seq: u32) -> u32 {
    if seq == 0 {
        0
    } else {
        (seq - 1) / SHARD_SIZE
    }
}

/// Compute the file path for a requirement in the sharded layout.
///
/// Given spec_id "FR-042" and objects_root "/repo/objects":
///   → /repo/objects/FR/000/FR-042.yaml
///
/// Given spec_id "FR-7-1500" (distributed mode) and objects_root "/repo/objects":
///   → /repo/objects/FR/001/FR-7-1500.yaml
///
/// The shard is computed from the sequence number (last numeric component).
/// Input is canonicalized to uppercase so callers may pass user input verbatim.
pub fn object_path(objects_root: &Path, spec_id: &str) -> Result<PathBuf> {
    let spec_id = canonical_spec_id(spec_id);
    let (type_prefix, seq) = parse_spec_id(&spec_id)?;
    let shard = format!("{:03}", shard_number(seq));
    let filename = format!("{}.yaml", spec_id);
    Ok(objects_root.join(&type_prefix).join(&shard).join(&filename))
}

/// The repo-relative path for a requirement's YAML (under `objects/`).
/// Matches `object_path()` but produces a relative `String` suitable for
/// passing to `git add`. trace:BUG-1-040 | ai:claude
pub fn relative_object_path(spec_id: &str) -> Result<String> {
    let spec_id = canonical_spec_id(spec_id);
    let (type_prefix, seq) = parse_spec_id(&spec_id)?;
    let shard = format!("{:03}", shard_number(seq));
    Ok(format!(
        "objects/{}/{}/{}.yaml",
        type_prefix, shard, spec_id
    ))
}

/// Parse a spec_id into (type_prefix, sequence_number).
///
/// Handles both centralized and distributed formats:
/// - "FR-042" → ("FR", 42)
/// - "FR-7-042" → ("FR", 42)
/// - "FEAT-3-1500" → ("FEAT", 1500)
fn parse_spec_id(spec_id: &str) -> Result<(String, u32)> {
    let parts: Vec<&str> = spec_id.split('-').collect();
    match parts.len() {
        // Centralized: TYPE-SEQ (e.g., "FR-042")
        2 => {
            let type_prefix = parts[0].to_uppercase();
            let seq: u32 = parts[1]
                .parse()
                .with_context(|| format!("Invalid sequence in spec_id: {}", spec_id))?;
            Ok((type_prefix, seq))
        }
        // Distributed: TYPE-NODEID-SEQ (e.g., "FR-7-042")
        3 => {
            let type_prefix = parts[0].to_uppercase();
            let seq: u32 = parts[2]
                .parse()
                .with_context(|| format!("Invalid sequence in spec_id: {}", spec_id))?;
            Ok((type_prefix, seq))
        }
        _ => anyhow::bail!(
            "Invalid spec_id format: {} (expected TYPE-SEQ or TYPE-NODE-SEQ)",
            spec_id
        ),
    }
}

/// Write a single requirement to the object store.
///
/// Creates parent directories as needed. Overwrites if the file already exists.
#[cfg(feature = "native")]
pub fn write_object(objects_root: &Path, req: &Requirement) -> Result<PathBuf> {
    let spec_id = req
        .spec_id
        .as_ref()
        .context("Requirement has no spec_id — cannot write to object store")?;

    let path = object_path(objects_root, spec_id)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    let yaml = serde_yaml::to_string(req)
        .with_context(|| format!("Failed to serialize requirement {}", spec_id))?;

    std::fs::write(&path, &yaml).with_context(|| format!("Failed to write {}", path.display()))?;

    Ok(path)
}

/// Write a requirement to the object store only if the serialized YAML
/// differs from what's already on disk. Returns `true` when a write
/// occurred (file was new or content changed) and `false` when the existing
/// file is byte-identical and was left untouched.
///
/// Combined with sorted-collection serializers, this lets bulk save paths
/// avoid producing spurious git diffs for unchanged requirements.
/// trace:BUG-1-040 | ai:claude
#[cfg(feature = "native")]
pub fn write_object_if_changed(objects_root: &Path, req: &Requirement) -> Result<bool> {
    let spec_id = req
        .spec_id
        .as_ref()
        .context("Requirement has no spec_id — cannot write to object store")?;

    let path = object_path(objects_root, spec_id)?;
    let yaml = serde_yaml::to_string(req)
        .with_context(|| format!("Failed to serialize requirement {}", spec_id))?;

    // Reader can race a concurrent git checkout of the orphan branch
    // (rename-based at the plumbing level); on Windows that surfaces as a
    // transient PermissionDenied/NotFound from `CreateFile`. Retry through
    // `read_atomic` so the "no-op when unchanged" optimization doesn't
    // spuriously rewrite an unchanged file. trace:TASK-346 | ai:claude
    if path.exists() {
        if let Ok(existing) = crate::read_atomic(&path) {
            if existing == yaml {
                return Ok(false);
            }
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }
    std::fs::write(&path, &yaml).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(true)
}

/// Read a single requirement from the object store by spec_id.
///
/// Routes through `read_atomic` so a concurrent git checkout of the orphan
/// branch (which is rename-based at the plumbing level) can't make this
/// fail with a transient Windows `ERROR_ACCESS_DENIED`/`ERROR_FILE_NOT_FOUND`
/// from `CreateFile`. trace:TASK-346 | ai:claude
#[cfg(feature = "native")]
pub fn read_object(objects_root: &Path, spec_id: &str) -> Result<Requirement> {
    let path = object_path(objects_root, spec_id)?;
    let yaml =
        crate::read_atomic(&path).with_context(|| format!("Failed to read {}", path.display()))?;
    let req: Requirement = serde_yaml::from_str(&yaml)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(req)
}

/// Read a single requirement from a specific file path.
///
/// Routes through `read_atomic` for the same reason as `read_object`:
/// concurrent rename-based writes from git plumbing on Windows.
/// trace:TASK-346 | ai:claude
#[cfg(feature = "native")]
pub fn read_object_from_path(path: &Path) -> Result<Requirement> {
    let yaml =
        crate::read_atomic(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let req: Requirement = serde_yaml::from_str(&yaml)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(req)
}

/// Delete a requirement file from the object store.
/// Returns true if the file existed and was removed.
#[cfg(feature = "native")]
pub fn delete_object(objects_root: &Path, spec_id: &str) -> Result<bool> {
    let path = object_path(objects_root, spec_id)?;
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("Failed to delete {}", path.display()))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Check if a requirement file exists in the object store.
#[cfg(feature = "native")]
pub fn object_exists(objects_root: &Path, spec_id: &str) -> Result<bool> {
    let path = object_path(objects_root, spec_id)?;
    Ok(path.exists())
}

/// List all requirement files in the object store.
/// Returns (spec_id, path) pairs.
#[cfg(feature = "native")]
pub fn list_objects(objects_root: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut results = Vec::new();

    if !objects_root.exists() {
        return Ok(results);
    }

    // Walk: objects_root / TYPE / SHARD / FILE.yaml
    for type_entry in std::fs::read_dir(objects_root)? {
        let type_entry = type_entry?;
        if !type_entry.file_type()?.is_dir() {
            continue;
        }
        for shard_entry in std::fs::read_dir(type_entry.path())? {
            let shard_entry = shard_entry?;
            if !shard_entry.file_type()?.is_dir() {
                continue;
            }
            for file_entry in std::fs::read_dir(shard_entry.path())? {
                let file_entry = file_entry?;
                let path = file_entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        results.push((stem.to_string(), path));
                    }
                }
            }
        }
    }

    Ok(results)
}

/// BUG-97 / TASK-223: shared recovery-hint suffix for any "failed to load /
/// failed to parse" surface. The first time a user sees a parse failure
/// they spend ~30 minutes diagnosing; the hint compresses that to seconds
/// by naming the most common cause (binary-version skew between the writer
/// and the reader) and the recovery steps.
///
/// Returns a multi-line string suitable for `Warning:` or `Error:` chains.
/// Callers typically combine it with the underlying parse error via
/// `anyhow::Context::with_context` or a manual `format!`.
///
/// The hint is intentionally generic about WHICH variant is unknown or
/// which field is missing — that information is already in the serde
/// error chain attached by `read_object`'s `with_context`. The hint
/// supplies the diagnostic + recovery narrative the serde error lacks.
pub fn parse_failure_hint(path: Option<&Path>) -> String {
    let mut hint = String::new();
    if let Some(p) = path {
        hint.push_str(&format!("  File: {}\n", p.display()));
    }
    hint.push_str(
        "  Likely cause: binary version mismatch — the file may have been written by a newer\n  \
                aida than the one reading it (a new enum variant, a renamed field, etc.).\n",
    );
    hint.push_str(
        "  Check:     `aida --version`  (and compare to the worktree that wrote the file)\n",
    );
    hint.push_str("  Recovery:  rebuild aida from a branch with the missing variant, then\n");
    hint.push_str("             `aida cache rebuild` to refresh the read projection.\n");
    hint.push_str("             `aida dev activate` (TASK-221) prefers a SHA-matching binary —\n");
    hint.push_str("             run it after rebuild to flip to the freshly-built binary.");
    hint
}

/// Load all requirements from the object store into a Vec.
#[cfg(feature = "native")]
pub fn load_all_objects(objects_root: &Path) -> Result<Vec<Requirement>> {
    let files = list_objects(objects_root)?;
    let mut requirements = Vec::with_capacity(files.len());

    for (spec_id, path) in &files {
        match read_object_from_path(path) {
            Ok(req) => requirements.push(req),
            Err(e) => {
                // BUG-97 / TASK-223: enrich the warning with the recovery
                // hint so users see actionable next steps the first time
                // a parse failure happens, not just "this thing broke."
                // trace:BUG-97 TASK-223 | ai:claude
                eprintln!("Warning: failed to load {} (parse error)", spec_id);
                eprintln!("  Detail: {}", e);
                eprintln!("{}", parse_failure_hint(Some(path)));
            }
        }
    }

    Ok(requirements)
}

/// Look up a requirement by UUID across all object files.
/// This is O(n) — for frequent lookups, use the SQLite read model instead.
#[cfg(feature = "native")]
pub fn find_by_uuid(objects_root: &Path, uuid: &Uuid) -> Result<Option<Requirement>> {
    let files = list_objects(objects_root)?;
    for (_spec_id, path) in &files {
        if let Ok(req) = read_object_from_path(path) {
            if req.id == *uuid {
                return Ok(Some(req));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shard_number() {
        assert_eq!(shard_number(1), 0);
        assert_eq!(shard_number(999), 0);
        assert_eq!(shard_number(1000), 0);
        assert_eq!(shard_number(1001), 1);
        assert_eq!(shard_number(2000), 1);
        assert_eq!(shard_number(2001), 2);
        assert_eq!(shard_number(100_000), 99);
    }

    #[test]
    fn test_parse_spec_id_centralized() {
        let (prefix, seq) = parse_spec_id("FR-042").unwrap();
        assert_eq!(prefix, "FR");
        assert_eq!(seq, 42);

        let (prefix, seq) = parse_spec_id("FEAT-1500").unwrap();
        assert_eq!(prefix, "FEAT");
        assert_eq!(seq, 1500);
    }

    #[test]
    fn test_parse_spec_id_distributed() {
        let (prefix, seq) = parse_spec_id("FR-7-042").unwrap();
        assert_eq!(prefix, "FR");
        assert_eq!(seq, 42);

        let (prefix, seq) = parse_spec_id("FEAT-3-1500").unwrap();
        assert_eq!(prefix, "FEAT");
        assert_eq!(seq, 1500);
    }

    #[test]
    fn test_parse_spec_id_invalid() {
        assert!(parse_spec_id("FR").is_err());
        assert!(parse_spec_id("FR-abc").is_err());
        assert!(parse_spec_id("FR-7-abc").is_err());
        assert!(parse_spec_id("A-B-C-D").is_err());
    }

    #[test]
    fn test_object_path_centralized() {
        let root = Path::new("/repo/objects");
        let path = object_path(root, "FR-042").unwrap();
        assert_eq!(path, PathBuf::from("/repo/objects/FR/000/FR-042.yaml"));

        let path = object_path(root, "FR-1500").unwrap();
        assert_eq!(path, PathBuf::from("/repo/objects/FR/001/FR-1500.yaml"));

        let path = object_path(root, "BUG-001").unwrap();
        assert_eq!(path, PathBuf::from("/repo/objects/BUG/000/BUG-001.yaml"));
    }

    #[test]
    fn test_object_path_distributed() {
        let root = Path::new("/repo/objects");
        let path = object_path(root, "FR-7-042").unwrap();
        assert_eq!(path, PathBuf::from("/repo/objects/FR/000/FR-7-042.yaml"));

        let path = object_path(root, "FEAT-3-1500").unwrap();
        assert_eq!(
            path,
            PathBuf::from("/repo/objects/FEAT/001/FEAT-3-1500.yaml")
        );
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_write_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let objects_root = dir.path().join("objects");

        let mut req = Requirement::new("Test Requirement".into(), "A test description".into());
        req.spec_id = Some("FR-042".into());
        req.owner = "joe".into();
        req.tags.insert("test".into());

        // Write
        let path = write_object(&objects_root, &req).unwrap();
        assert!(path.exists());
        assert_eq!(
            path,
            objects_root.join("FR").join("000").join("FR-042.yaml")
        );

        // Read back
        let loaded = read_object(&objects_root, "FR-042").unwrap();
        assert_eq!(loaded.id, req.id);
        assert_eq!(loaded.title, "Test Requirement");
        assert_eq!(loaded.spec_id, Some("FR-042".into()));
        assert_eq!(loaded.owner, "joe");
        assert!(loaded.tags.contains("test"));
    }

    /// SPIKE-46 conformance gate: AIDA's on-disk YAML format must stay stable,
    /// because a drift in our serializer breaks every external tool's
    /// byte-identical writes (and makes `write_object_if_changed` see spurious
    /// diffs). This guards the four write-conformance properties from
    /// docs/architecture/spike-46-store-interop/FINDINGS.md plus
    /// serialize→deserialize→serialize idempotence. If a struct/serde change
    /// alters the on-disk shape, this fails on purpose — update it deliberately
    /// and note the format change for downstream consumers. trace:SPIKE-46 | ai:claude
    #[test]
    fn requirement_yaml_holds_the_write_conformance_contract() {
        use crate::models::{Relationship, RelationshipType};

        let mut req = Requirement::new("Conformance fixture".into(), "body".into());
        req.spec_id = Some("TASK-999".into());
        // Insert tags + custom_fields OUT of order — the serializer must sort.
        req.tags.insert("tag-zebra".into());
        req.tags.insert("tag-alpha".into());
        req.tags.insert("tag-mango".into());
        req.custom_fields.insert("z_key".into(), "1".into());
        req.custom_fields.insert("a_key".into(), "2".into());
        // A custom relationship must serialize as a `!Custom` tag (the shape a
        // stock YAML loader trips on — hazard #1).
        req.relationships.push(Relationship {
            rel_type: RelationshipType::Custom("verifies-indirectly".into()),
            target_id: Uuid::now_v7(),
            created_at: Some(req.created_at),
            created_by: None,
        });
        // agreed_id stays None → must be omitted, not emitted as null.

        let yaml = serde_yaml::to_string(&req).expect("serialize");

        // 1. Sorted collections (yaml_helpers::serialize_sorted_*).
        let (a, m, z) = (
            yaml.find("tag-alpha").unwrap(),
            yaml.find("tag-mango").unwrap(),
            yaml.find("tag-zebra").unwrap(),
        );
        assert!(a < m && m < z, "tags must serialize sorted:\n{yaml}");
        assert!(
            yaml.find("a_key").unwrap() < yaml.find("z_key").unwrap(),
            "custom_fields keys must serialize sorted:\n{yaml}"
        );

        // 2. RelationshipType::Custom → `!Custom` tag.
        assert!(
            yaml.contains("!Custom"),
            "custom rel must tag as !Custom:\n{yaml}"
        );

        // 3. Timestamps are RFC3339 Zulu strings (nanosecond-Z), never `+00:00`.
        let created_line = yaml
            .lines()
            .find(|l| l.starts_with("created_at:"))
            .expect("created_at present");
        assert!(
            created_line.trim_end().ends_with('Z'),
            "timestamps must be RFC3339 Zulu: {created_line}"
        );
        assert!(
            !yaml.contains("+00:00"),
            "no offset-form timestamps:\n{yaml}"
        );

        // 4. Optional None fields omitted, not emitted as null.
        assert!(
            !yaml.contains("agreed_id"),
            "None optional fields must be omitted:\n{yaml}"
        );

        // 5. Round-trip idempotence: deserialize → reserialize is byte-stable,
        // so an unchanged spec re-written by AIDA produces no diff.
        let req2: Requirement = serde_yaml::from_str(&yaml).expect("deserialize");
        let yaml2 = serde_yaml::to_string(&req2).expect("reserialize");
        assert_eq!(
            yaml, yaml2,
            "serialize→deserialize→serialize must be byte-stable"
        );
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_write_read_distributed_id() {
        let dir = tempfile::tempdir().unwrap();
        let objects_root = dir.path().join("objects");

        let mut req = Requirement::new("Distributed Req".into(), "Node 7 created this".into());
        req.spec_id = Some("FR-7-048".into());

        write_object(&objects_root, &req).unwrap();
        let loaded = read_object(&objects_root, "FR-7-048").unwrap();
        assert_eq!(loaded.spec_id, Some("FR-7-048".into()));
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_list_objects() {
        let dir = tempfile::tempdir().unwrap();
        let objects_root = dir.path().join("objects");

        let specs = ["FR-001", "FR-002", "BUG-001", "FEAT-001"];
        for spec in &specs {
            let mut req = Requirement::new(format!("Req {}", spec), "desc".into());
            req.spec_id = Some(spec.to_string());
            write_object(&objects_root, &req).unwrap();
        }

        let listed = list_objects(&objects_root).unwrap();
        assert_eq!(listed.len(), 4);

        let spec_ids: Vec<&str> = listed.iter().map(|(s, _)| s.as_str()).collect();
        for spec in &specs {
            assert!(spec_ids.contains(spec), "Missing {}", spec);
        }
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_delete_object() {
        let dir = tempfile::tempdir().unwrap();
        let objects_root = dir.path().join("objects");

        let mut req = Requirement::new("To Delete".into(), "desc".into());
        req.spec_id = Some("FR-099".into());
        write_object(&objects_root, &req).unwrap();

        assert!(object_exists(&objects_root, "FR-099").unwrap());
        assert!(delete_object(&objects_root, "FR-099").unwrap());
        assert!(!object_exists(&objects_root, "FR-099").unwrap());
        assert!(!delete_object(&objects_root, "FR-099").unwrap()); // already gone
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_find_by_uuid() {
        let dir = tempfile::tempdir().unwrap();
        let objects_root = dir.path().join("objects");

        let mut req = Requirement::new("Findable".into(), "desc".into());
        req.spec_id = Some("FR-001".into());
        let target_uuid = req.id;
        write_object(&objects_root, &req).unwrap();

        let mut req2 = Requirement::new("Other".into(), "desc".into());
        req2.spec_id = Some("FR-002".into());
        write_object(&objects_root, &req2).unwrap();

        let found = find_by_uuid(&objects_root, &target_uuid).unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().title, "Findable");

        let not_found = find_by_uuid(&objects_root, &Uuid::now_v7()).unwrap();
        assert!(not_found.is_none());
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_shard_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let objects_root = dir.path().join("objects");

        // Seq 1000 → shard 000, seq 1001 → shard 001
        let sep = std::path::MAIN_SEPARATOR;
        let mut req_a = Requirement::new("Shard boundary A".into(), "desc".into());
        req_a.spec_id = Some("FR-1000".into());
        let path_a = write_object(&objects_root, &req_a).unwrap();
        assert!(path_a.to_str().unwrap().contains(&format!("{sep}000{sep}")));

        let mut req_b = Requirement::new("Shard boundary B".into(), "desc".into());
        req_b.spec_id = Some("FR-1001".into());
        let path_b = write_object(&objects_root, &req_b).unwrap();
        assert!(path_b.to_str().unwrap().contains(&format!("{sep}001{sep}")));
    }

    // BUG-97 / TASK-223: parse_failure_hint formatter tests.

    #[test]
    fn parse_hint_with_path_names_the_file() {
        let p = Path::new("/store/objects/STORY/000/STORY-86.yaml");
        let h = parse_failure_hint(Some(p));
        assert!(h.contains("File: /store/objects/STORY/000/STORY-86.yaml"));
    }

    #[test]
    fn parse_hint_mentions_binary_version_mismatch() {
        let h = parse_failure_hint(None);
        assert!(h.contains("binary version mismatch"));
        assert!(h.contains("newer aida") || h.contains("new enum variant"));
    }

    #[test]
    fn parse_hint_mentions_aida_version_check() {
        let h = parse_failure_hint(None);
        assert!(h.contains("aida --version"));
    }

    #[test]
    fn parse_hint_mentions_dev_activate_for_rebuild_flow() {
        let h = parse_failure_hint(None);
        assert!(h.contains("aida dev activate") || h.contains("TASK-221"));
    }

    #[test]
    fn parse_hint_mentions_cache_rebuild() {
        let h = parse_failure_hint(None);
        assert!(h.contains("aida cache rebuild"));
    }

    #[test]
    fn parse_hint_path_optional() {
        let h = parse_failure_hint(None);
        // No "File:" line when no path provided — still useful content.
        assert!(!h.contains("File:"));
        assert!(h.contains("Recovery:"));
    }
}
