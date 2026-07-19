//! Session manifest: planned-cluster bookkeeping written by /aida-pickup.
//!
//! When /aida-pickup confirms a multi-item cluster, it writes the planned
//! list to `.aida/sessions/<session-id>.manifest.toml`. Other sessions can
//! see "this spec is planned by session X" so two concurrent runs don't
//! grab the same item. `aida session show --plan` reads it back to render
//! a Done / In progress / Pending status table (glyphs via the registry).
//!
//! Why a separate file from the lease: lease state is "what is this
//! session" (scope, branch, worktree, role); manifest state is "what does
//! this session intend to do." Splitting them keeps the lease load path
//! ignorant of plan history and lets us update plan rows without
//! re-serializing the lease.
//!
//! trace:STORY-98 | ai:claude
//!
//! ## File shape
//!
//! ```toml
//! session_id = "019e15743920"
//! planned_at = "2026-05-11T05:14:00Z"
//! plan_source = "user prompt"   # or "auto" / "queue work"
//!
//! [plan]                        # TASK-95: optional, from a docs/plans/ file
//! plan_file = "docs/plans/2026-05-11-bug-73.md"
//! critical_files = ["aida-core/src/db/cache.rs"]
//! followups = ["revert-handling cleanup"]
//! verification = "cargo test -p aida-core"
//!
//! [[items]]
//! spec_id = "BUG-73"
//! position = 1
//! status_at_plan = "Approved"
//! started_at = "2026-05-11T05:15:00Z"  # written when item flips InProgress
//! completed_at = "2026-05-11T05:30:00Z" # written on Completed/Rejected
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestItem {
    pub spec_id: String,
    pub position: u32,
    pub status_at_plan: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Free-form note, e.g. "skipped — already covered by SPEC-X". Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// TASK-95: plan content extracted from a matching `docs/plans/` file when
/// `aida queue work` set this session up. Lets /aida-pickup hand the
/// implementer their brief (blast radius, definition of done, deferred
/// work) without grepping for the plan. trace:TASK-95 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlanContext {
    /// Repo-relative path to the `docs/plans/` file this brief came from.
    pub plan_file: String,
    /// `## Critical Files` enumeration — the must-touch blast radius.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub critical_files: Vec<String>,
    /// `## Followups` bullets — out-of-scope items the `aida queue done`
    /// handler later offers to file as TASKs (TASK-96).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub followups: Vec<String>,
    /// `## Verification` fenced script — the executable definition of done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionManifest {
    pub session_id: String,
    pub planned_at: chrono::DateTime<chrono::Utc>,
    /// "user prompt" when /aida-pickup confirmed a user-specified cluster;
    /// "auto" when the skill defaulted to the head item. Free-form.
    pub plan_source: String,
    /// TASK-112: the `claude` conversation this session launched with.
    /// Recorded by `aida queue work` — either the UUID it minted for a
    /// fresh launch (`claude --session-id <uuid>`) or the id it resumed
    /// (`--resume`). Lets a later `aida queue work --resume <scope>` find
    /// the conversation to continue. `None` for sessions launched before
    /// TASK-112 or by paths that don't record it.
    /// Declared as a scalar before `plan`/`items` so TOML serializes it
    /// ahead of the `[plan]` table and `[[items]]` array-of-tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_session_id: Option<String>,
    /// TASK-272: the batch this session is draining — the bare `NAME` from
    /// `aida queue work --batch NAME` (no `batch:` prefix). Recorded when the
    /// session was set up via `--batch`; lets `/aida-pickup` detect batch
    /// context and offer cluster-mode continuation as the primary next-step,
    /// and `/aida-pr` frame the PR as the batch. `None` for non-batch
    /// sessions. Declared as a scalar before `plan`/`items` so TOML
    /// serializes it ahead of the `[plan]` table and `[[items]]` array.
    /// trace:TASK-272 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_name: Option<String>,
    /// TASK-95: plan brief pre-populated from a matching `docs/plans/` file
    /// at `aida queue work` time. `None` when no plan file was found.
    /// Declared before `items` so TOML serialization emits the `[plan]`
    /// table before the `[[items]]` array-of-tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanContext>,
    pub items: Vec<ManifestItem>,
}

impl SessionManifest {
    /// Lookup an item by SPEC-ID (case-insensitive).
    pub fn item(&self, spec_id: &str) -> Option<&ManifestItem> {
        self.items
            .iter()
            .find(|it| it.spec_id.eq_ignore_ascii_case(spec_id))
    }

    pub fn item_mut(&mut self, spec_id: &str) -> Option<&mut ManifestItem> {
        self.items
            .iter_mut()
            .find(|it| it.spec_id.eq_ignore_ascii_case(spec_id))
    }
}

pub fn manifest_path(project_root: &Path, session_id: &str) -> PathBuf {
    project_root
        .join(".aida")
        .join("sessions")
        .join(format!("{}.manifest.toml", session_id))
}

pub fn load(path: &Path) -> Result<SessionManifest> {
    // Reader can race a concurrent `save` mid-`write_atomic`; on Windows
    // that surfaces as a transient PermissionDenied/NotFound from
    // `CreateFile`. Retry through `read_atomic` so a sibling session
    // checking `planned_by_other` never fails on a transient open.
    // trace:TASK-346 | ai:claude
    let content = aida_core::read_atomic(path)
        .with_context(|| format!("read manifest {}", path.display()))?;
    let manifest: SessionManifest =
        toml::from_str(&content).with_context(|| format!("parse manifest {}", path.display()))?;
    Ok(manifest)
}

pub fn save(path: &Path, manifest: &SessionManifest) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let serialized = toml::to_string_pretty(manifest).context("serialize manifest")?;
    // Atomic write: multiple concurrent `aida queue work` sessions can race
    // the manifest file — a torn write makes the plan unreadable for every
    // session reading it back. trace:TASK-331 | ai:claude
    aida_core::write_atomic(path, serialized)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Walk `.aida/sessions/` and return every manifest currently on disk.
/// Errors on individual files are swallowed (best-effort scan) so a single
/// malformed manifest can't block the chip rendering for the rest.
pub fn list_all(project_root: &Path) -> Vec<SessionManifest> {
    list_all_with_paths(project_root)
        .into_iter()
        .map(|(_, m)| m)
        .collect()
}

/// Like `list_all` but also returns the manifest file's path — used by
/// `session prune` to remove orphan manifests whose owning lease has been
/// deleted. trace:BUG-80 | ai:claude
pub fn list_all_with_paths(project_root: &Path) -> Vec<(PathBuf, SessionManifest)> {
    let dir = project_root.join(".aida").join("sessions");
    if !dir.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".manifest.toml") {
            continue;
        }
        if let Ok(m) = load(&p) {
            out.push((p, m));
        }
    }
    out
}

/// For a given spec_id, return the session_id of any *other* session
/// whose manifest plans it. `viewer_session` is the current session's id
/// (skipped from the search) — pass empty string when there's no active
/// session. Case-insensitive on spec_id. Returns the first match (multiple
/// sessions claiming the same spec is a separate problem; we surface the
/// first signal and let the user reconcile).
pub fn planned_by_other(
    manifests: &[SessionManifest],
    spec_id: &str,
    viewer_session: &str,
) -> Option<String> {
    for m in manifests {
        if m.session_id == viewer_session {
            continue;
        }
        if m.item(spec_id).is_some() {
            return Some(m.session_id.clone());
        }
    }
    None
}

/// Mark `spec_id` as "started" (records started_at = now) in the manifest
/// at `path`, if such an item exists. No-op when the manifest doesn't
/// exist or doesn't list the spec. Best-effort: serialization errors are
/// returned so callers can warn, but missing manifest is *not* an error
/// (most edits happen outside a planned cluster).
pub fn mark_started(path: &Path, spec_id: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let mut manifest = load(path)?;
    let Some(item) = manifest.item_mut(spec_id) else {
        return Ok(false);
    };
    if item.started_at.is_none() {
        item.started_at = Some(chrono::Utc::now());
    }
    save(path, &manifest)?;
    Ok(true)
}

/// Mark `spec_id` as "completed" (records completed_at = now). Same
/// semantics as `mark_started` for missing-file / missing-item.
pub fn mark_completed(path: &Path, spec_id: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let mut manifest = load(path)?;
    let Some(item) = manifest.item_mut(spec_id) else {
        return Ok(false);
    };
    if item.completed_at.is_none() {
        item.completed_at = Some(chrono::Utc::now());
    }
    save(path, &manifest)?;
    Ok(true)
}

/// Status of an item in the manifest, derived from its timestamps and the
/// requirement's current store status. We don't store "in progress" or
/// "done" directly in the manifest — it'd drift from the store. Instead,
/// we read the requirement's current status as the source of truth and
/// use manifest timestamps for the "when did this leg start / end" axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStatus {
    Pending,
    InProgress,
    Done,
}

impl ItemStatus {
    /// The status glyph. InProgress / Done route through the glyph registry so
    /// an `[ui] glyphs = "ascii"` / `AIDA_GLYPHS=ascii` profile re-renders them
    /// (default Unicode reproduces the historical literals byte-for-byte).
    /// Pending keeps its small white-circle literal — it is not a registry glyph.
    // trace:TASK-840 | ai:claude
    pub fn glyph(self) -> &'static str {
        let root = crate::find_project_root().ok();
        match self {
            ItemStatus::Pending => "○",
            ItemStatus::InProgress => {
                crate::glyphs::get(crate::glyphs::Glyph::InFlight, root.as_deref())
            }
            ItemStatus::Done => crate::glyphs::get(crate::glyphs::Glyph::Check, root.as_deref()),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ItemStatus::Pending => "Pending",
            ItemStatus::InProgress => "In progress",
            ItemStatus::Done => "Done",
        }
    }
}

/// Classify a manifest item using the requirement's current status string
/// (matches the form printed by `format!("{}", req.status)` — "Approved",
/// "In Progress", "Completed", "Rejected", etc.).
pub fn classify_item(item: &ManifestItem, current_status: Option<&str>) -> ItemStatus {
    if let Some(s) = current_status {
        let lower = s.to_ascii_lowercase();
        if lower == "completed" || lower == "rejected" {
            return ItemStatus::Done;
        }
        if lower == "in progress" || lower == "in-progress" {
            return ItemStatus::InProgress;
        }
    }
    // No store info — fall back to manifest timestamps.
    if item.completed_at.is_some() {
        ItemStatus::Done
    } else if item.started_at.is_some() {
        ItemStatus::InProgress
    } else {
        ItemStatus::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(spec: &str, pos: u32) -> ManifestItem {
        ManifestItem {
            spec_id: spec.to_string(),
            position: pos,
            status_at_plan: "Approved".to_string(),
            started_at: None,
            completed_at: None,
            note: None,
        }
    }

    fn manifest(id: &str, specs: &[(&str, u32)]) -> SessionManifest {
        SessionManifest {
            session_id: id.to_string(),
            planned_at: chrono::Utc::now(),
            plan_source: "test".to_string(),
            claude_session_id: None,
            batch_name: None,
            plan: None,
            items: specs.iter().map(|(s, p)| item(s, *p)).collect(),
        }
    }

    #[test]
    fn item_lookup_case_insensitive() {
        let m = manifest("S1", &[("STORY-98", 1)]);
        assert!(m.item("story-98").is_some());
        assert!(m.item("STORY-98").is_some());
        assert!(m.item("STORY-99").is_none());
    }

    #[test]
    fn planned_by_other_skips_viewer() {
        let manifests = vec![
            manifest("S1", &[("BUG-73", 1)]),
            manifest("S2", &[("BUG-73", 1)]),
        ];
        // Viewer is S1 → returns S2.
        assert_eq!(
            planned_by_other(&manifests, "BUG-73", "S1"),
            Some("S2".to_string())
        );
        // Viewer is S2 → returns S1.
        assert_eq!(
            planned_by_other(&manifests, "BUG-73", "S2"),
            Some("S1".to_string())
        );
        // Viewer is S3 (not in list) → returns the first match (S1).
        assert_eq!(
            planned_by_other(&manifests, "BUG-73", "S3"),
            Some("S1".to_string())
        );
        // Spec not planned → None.
        assert_eq!(planned_by_other(&manifests, "STORY-99", "S1"), None);
    }

    #[test]
    fn classify_uses_store_status_over_timestamps() {
        let mut it = item("X", 1);
        it.started_at = Some(chrono::Utc::now());
        // Store says completed → Done (overrides started_at).
        assert_eq!(classify_item(&it, Some("Completed")), ItemStatus::Done);
        // Store says approved (the plan-time status) → InProgress because
        // we have a started_at. We trust the store for terminal/InProgress,
        // but the store can lag on the "started" boundary (manifest is
        // written before `aida edit --status in-progress`).
        assert_eq!(classify_item(&it, Some("Approved")), ItemStatus::InProgress);
    }

    #[test]
    fn classify_falls_back_to_timestamps_when_no_status() {
        let mut it = item("X", 1);
        assert_eq!(classify_item(&it, None), ItemStatus::Pending);
        it.started_at = Some(chrono::Utc::now());
        assert_eq!(classify_item(&it, None), ItemStatus::InProgress);
        it.completed_at = Some(chrono::Utc::now());
        assert_eq!(classify_item(&it, None), ItemStatus::Done);
    }

    #[test]
    fn list_all_with_paths_returns_pairs() {
        // trace:BUG-80 | ai:claude
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join(".aida").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();

        let p1 = sessions.join("s1.manifest.toml");
        save(&p1, &manifest("s1", &[("X", 1)])).unwrap();
        let p2 = sessions.join("s2.manifest.toml");
        save(&p2, &manifest("s2", &[("Y", 1)])).unwrap();

        let pairs = list_all_with_paths(dir.path());
        assert_eq!(pairs.len(), 2);
        let ids: Vec<_> = pairs.iter().map(|(_, m)| m.session_id.as_str()).collect();
        assert!(ids.contains(&"s1"));
        assert!(ids.contains(&"s2"));
        // Each pair's path is the file we wrote.
        for (p, m) in &pairs {
            assert!(p.exists());
            assert!(p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap()
                .starts_with(&m.session_id));
        }
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.manifest.toml");
        let m = SessionManifest {
            session_id: "abc12345".to_string(),
            planned_at: chrono::Utc::now(),
            plan_source: "user prompt".to_string(),
            claude_session_id: None,
            batch_name: None,
            plan: None,
            items: vec![item("STORY-1", 1), item("BUG-2", 2)],
        };
        save(&path, &m).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.session_id, "abc12345");
        assert_eq!(loaded.items.len(), 2);
        assert_eq!(loaded.items[0].spec_id, "STORY-1");
    }

    #[test]
    fn batch_name_survives_roundtrip() {
        // TASK-272: `aida queue work --batch NAME` records the batch on the
        // manifest so /aida-pickup can detect batch context. Verify the
        // field round-trips and that a missing key deserializes to None.
        let dir = tempfile::tempdir().unwrap();

        // With a batch name set.
        let path = dir.path().join("batched.manifest.toml");
        let mut m = manifest("s-batch", &[("TASK-260", 1)]);
        m.batch_name = Some("workflow-hint-polish".to_string());
        save(&path, &m).unwrap();
        let serialized = std::fs::read_to_string(&path).unwrap();
        assert!(
            serialized.contains("batch_name = \"workflow-hint-polish\""),
            "serialized manifest should carry batch_name:\n{serialized}"
        );
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.batch_name.as_deref(), Some("workflow-hint-polish"));

        // Without one — the skip_serializing_if Option means the key is
        // absent on disk and deserializes back to None.
        let plain = dir.path().join("plain.manifest.toml");
        save(&plain, &manifest("s-plain", &[("TASK-1", 1)])).unwrap();
        let plain_text = std::fs::read_to_string(&plain).unwrap();
        assert!(
            !plain_text.contains("batch_name"),
            "non-batch manifest must not emit a batch_name key:\n{plain_text}"
        );
        assert_eq!(load(&plain).unwrap().batch_name, None);

        // batch_name is a scalar; it MUST serialize before the `[plan]`
        // table — a scalar emitted after a table is invalid TOML and would
        // fail to round-trip. Exercise the field-order constraint with a
        // PlanContext present alongside the batch name. trace:TASK-272
        let with_plan = dir.path().join("with-plan.manifest.toml");
        let mut mp = manifest("s-plan", &[("TASK-260", 1)]);
        mp.batch_name = Some("display-polish".to_string());
        mp.plan = Some(PlanContext {
            plan_file: "docs/plans/x.md".to_string(),
            critical_files: vec!["aida-cli/src/main.rs".to_string()],
            followups: vec![],
            verification: None,
        });
        save(&with_plan, &mp).unwrap();
        let reloaded = load(&with_plan).unwrap();
        assert_eq!(reloaded.batch_name.as_deref(), Some("display-polish"));
        assert_eq!(
            reloaded.plan.as_ref().map(|p| p.plan_file.as_str()),
            Some("docs/plans/x.md")
        );
    }

    #[test]
    fn mark_started_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.manifest.toml");
        let m = SessionManifest {
            session_id: "s1".to_string(),
            planned_at: chrono::Utc::now(),
            plan_source: "test".to_string(),
            claude_session_id: None,
            batch_name: None,
            plan: None,
            items: vec![item("X", 1)],
        };
        save(&path, &m).unwrap();
        assert!(mark_started(&path, "X").unwrap());
        let first = load(&path).unwrap().items[0].started_at.unwrap();
        // Sleep would be flaky; just verify a second call doesn't clobber.
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(mark_started(&path, "X").unwrap());
        let second = load(&path).unwrap().items[0].started_at.unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn mark_on_missing_file_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.manifest.toml");
        assert!(!mark_started(&path, "X").unwrap());
        assert!(!mark_completed(&path, "X").unwrap());
    }

    #[test]
    fn mark_on_missing_item_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.manifest.toml");
        let m = SessionManifest {
            session_id: "s1".to_string(),
            planned_at: chrono::Utc::now(),
            plan_source: "test".to_string(),
            claude_session_id: None,
            batch_name: None,
            plan: None,
            items: vec![item("X", 1)],
        };
        save(&path, &m).unwrap();
        assert!(!mark_started(&path, "MISSING").unwrap());
    }

    // AC6 (TASK-331): concurrent-writer stress test on the manifest path.
    // N threads repeatedly save distinct manifests to the same file while a
    // reader loads it in a tight loop. Every load must parse cleanly — a
    // torn write makes `toml::from_str` fail. A bare `std::fs::write` (which
    // this path used before TASK-331) loses this race.
    #[test]
    fn concurrent_manifest_saves_never_tear() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        const WRITERS: usize = 6;
        const ROUNDS: usize = 40;

        let dir = tempfile::tempdir().unwrap();
        let path = Arc::new(dir.path().join("S.manifest.toml"));
        save(&path, &manifest("seed", &[("STORY-1", 1)])).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let reader = {
            let path = Arc::clone(&path);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    load(&path).expect("manifest load observed a torn write");
                }
            })
        };

        let writers: Vec<_> = (0..WRITERS)
            .map(|w| {
                let path = Arc::clone(&path);
                std::thread::spawn(move || {
                    for r in 0..ROUNDS {
                        // Vary item count so each save is a different length —
                        // a torn write is then unambiguous, not masked by an
                        // equal-size overwrite.
                        let specs: Vec<(String, u32)> = (0..=(w + r % 5))
                            .map(|i| (format!("SPEC-{w}-{i}"), i as u32 + 1))
                            .collect();
                        let refs: Vec<(&str, u32)> =
                            specs.iter().map(|(s, p)| (s.as_str(), *p)).collect();
                        save(&path, &manifest(&format!("S{w}"), &refs)).unwrap();
                    }
                })
            })
            .collect();

        for h in writers {
            h.join().unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        reader.join().unwrap();
        load(&path).expect("manifest unreadable after the storm");
    }

    // AC7 (TASK-331): grep guard — the manifest write path is known-concurrent
    // and must stay atomic. Flags any bare `fs::write` reintroduced into the
    // production code (everything before the test module), nudging toward
    // aida_core::write_atomic.
    #[test]
    fn manifest_write_path_stays_atomic() {
        let src = include_str!("session_manifest.rs");
        let production = src.split("#[cfg(test)]").next().unwrap_or(src);
        for (n, line) in production.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            assert!(
                !line.contains("fs::write("),
                "session_manifest.rs:{} uses a bare fs::write on a \
                 known-concurrent path — use aida_core::write_atomic instead \
                 (torn-write race, TASK-331)",
                n + 1
            );
        }
    }

    // TASK-346: grep guard for the read side. The manifest's `load` is
    // routinely contended with a sibling session's `save` (both run during
    // an autonomous drain), and on Windows the open can transiently fail
    // mid-rename. Flags any bare `fs::read_to_string` reintroduced into the
    // production code, nudging toward aida_core::read_atomic.
    #[test]
    fn manifest_read_path_stays_atomic() {
        let src = include_str!("session_manifest.rs");
        let production = src.split("#[cfg(test)]").next().unwrap_or(src);
        for (n, line) in production.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            assert!(
                !line.contains("fs::read_to_string("),
                "session_manifest.rs:{} uses a bare fs::read_to_string on a \
                 known-concurrent path — use aida_core::read_atomic instead \
                 (Windows transient-open race, TASK-346)",
                n + 1
            );
        }
    }
}
