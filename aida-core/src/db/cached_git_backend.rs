//! Git-canonical storage with a SQLite cache view.
//!
//! Per EPIC-1-001 / docs/plans/2026-05-02-git-canonical-storage.md:
//! the inner GitBackend is the writer-of-record; the Cache is a
//! rebuildable read projection. Writes go to git first, then update the
//! cache (write-through). Reads delegate to git for now — Phase 2 will
//! switch list/search reads to the cache.
//!
//! trace:EPIC-1-001 | ai:claude

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use super::cache::{ArchiveFilter, Cache, DeferFilter, ListFilter, RequirementSummary};
use super::git_backend::GitBackend;
use super::traits::{BackendType, DatabaseBackend, UpdateResult};
use crate::models::{QueueEntry, Requirement, RequirementsStore, User};

pub struct CachedGitBackend {
    inner: GitBackend,
    cache: Cache,
}

impl CachedGitBackend {
    /// Open an existing git store at `git_root` with a SQLite cache at
    /// `cache_path`. If the cache is missing or stale (HEAD-SHA mismatch),
    /// it is rebuilt before this constructor returns.
    pub fn open(git_root: &Path, cache_path: &Path) -> Result<Self> {
        let inner = GitBackend::new(git_root)?;
        Self::with_inner(inner, cache_path)
    }

    /// Wrap an already-configured GitBackend (e.g., one that was built with
    /// `.with_dispenser(...)`). The cache is opened or created at
    /// `cache_path` and rebuilt if stale before this returns.
    pub fn with_inner(inner: GitBackend, cache_path: &Path) -> Result<Self> {
        let cache = Cache::open(cache_path)?;
        let backend = CachedGitBackend { inner, cache };
        backend.ensure_cache_fresh()?;
        Ok(backend)
    }

    /// Default cache location for a project's git store at `git_root`:
    /// `<project_root>/.aida/cache.db`. We never put the cache *inside* the
    /// store directory — that would pollute the orphan branch's worktree —
    /// so the probe starts at `git_root.parent()` and walks up.
    pub fn default_cache_path(git_root: &Path) -> PathBuf {
        let mut probe = match git_root.parent() {
            Some(p) => p.to_path_buf(),
            None => return git_root.with_extension("cache.db"),
        };
        for _ in 0..6 {
            if probe.join(".aida").is_dir() {
                return probe.join(".aida").join("cache.db");
            }
            match probe.parent() {
                Some(p) => probe = p.to_path_buf(),
                None => break,
            }
        }
        // Fall back to a sibling file next to the store root so we still
        // never write inside it.
        git_root.with_extension("cache.db")
    }

    pub fn cache(&self) -> &Cache {
        &self.cache
    }

    pub fn inner(&self) -> &GitBackend {
        &self.inner
    }

    /// Read the current git HEAD on the store branch. Empty string if not in a
    /// git repo (e.g., test fixture); stale check then collapses to "always
    /// fresh" which is fine for non-git scenarios.
    fn current_head_sha(&self) -> String {
        crate::git_ops::head_sha(self.inner.path()).unwrap_or_default()
    }

    /// If the cache is stale (or missing source SHA), rebuild it from the
    /// inner GitBackend. Cheap when fresh — just a meta lookup + string
    /// compare.
    fn ensure_cache_fresh(&self) -> Result<()> {
        let head = self.current_head_sha();
        if !self.cache.is_stale(&head)? {
            return Ok(());
        }
        let store = self
            .inner
            .load()
            .context("Failed to load git store for cache rebuild")?;
        self.cache.rebuild_from_store(&store, &head)?;
        Ok(())
    }

    /// Cache-backed list query with filter pushdown. Returns lightweight
    /// summaries — full Requirement records require a follow-up
    /// `get_requirement` call. Triggers a stale-check first so callers
    /// always see fresh data.
    pub fn list_summaries(&self, filter: &ListFilter) -> Result<Vec<RequirementSummary>> {
        self.ensure_cache_fresh()?;
        self.cache.list_summaries(filter)
    }

    /// STORY-632: deterministic local graph-centrality (in/out degree + heft)
    /// for a single spec, read from the cache. Triggers a stale-check first so
    /// the inbound axis reflects the latest committed relationship graph (a
    /// HEAD change since the last rebuild forces a fresh full recompute).
    /// trace:STORY-632 | ai:claude
    pub fn degrees(&self, id: &Uuid) -> Result<crate::db::Degrees> {
        self.ensure_cache_fresh()?;
        self.cache.degrees_for_id(id)
    }

    /// Resolve a requirement UUID to its `Requirement` by reading ONLY
    /// the one target YAML — the targeted-read substitute for `get_requirement`
    /// (which delegates to `object_store::find_by_uuid`, a scan of EVERY object
    /// file). The UUID → `spec_id` mapping comes from the cache (an indexed
    /// single-row lookup); the canonical record is then read from git by
    /// `spec_id` so the returned `Requirement` is always sourced from the
    /// git-canonical YAML, never the cache projection.
    ///
    /// Does NOT trigger a stale-rebuild: callers use this on the single-spec
    /// write path right after the backend was opened (which already ran
    /// `ensure_cache_fresh`), and a miss here degrades gracefully to `Ok(None)`
    /// rather than forcing a full scan — the opposite of the bug being fixed.
    /// Returns `Ok(None)` when the UUID isn't in the cache (not-yet-rebuilt row)
    /// or its YAML is gone.
    // trace:BUG-634 | ai:claude
    pub fn get_requirement_by_uuid_targeted(&self, id: &Uuid) -> Result<Option<Requirement>> {
        let Some(spec_id) = self.cache.spec_id_for_uuid(id)? else {
            return Ok(None);
        };
        // Read the canonical record from git by spec_id (one YAML), then verify
        // the UUID matches — a cache row could be stale if a spec_id was reused,
        // so the UUID check keeps the resolution exact.
        match self.inner.get_requirement_by_spec_id(&spec_id)? {
            Some(req) if req.id == *id => Ok(Some(req)),
            _ => Ok(None),
        }
    }

    /// Cache-backed FTS5 search across spec_id, agreed_id, title, description.
    /// `archive` controls the archive axis (STORY-441); `defer` the defer axis
    /// (STORY-584).
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        archive: ArchiveFilter,
        defer: DeferFilter,
    ) -> Result<Vec<RequirementSummary>> {
        self.ensure_cache_fresh()?;
        self.cache.search(query, limit, archive, defer)
    }

    /// Force a full cache rebuild, regardless of staleness. Used by the
    /// `aida cache rebuild` CLI command.
    pub fn rebuild_cache(&self) -> Result<usize> {
        let head = self.current_head_sha();
        let store = self.inner.load()?;
        let n = self.cache.rebuild_from_store(&store, &head)?;
        Ok(n)
    }

    /// Re-stamp the cache HEAD-SHA after a write so the next stale-check
    /// passes. Called from write paths after `upsert_requirement` /
    /// `delete_requirement` succeed. Best-effort: if git HEAD can't be read
    /// (auto_commit disabled, not in a git repo) we leave the SHA alone and
    /// the cache will be considered fresh until a real commit happens.
    ///
    /// HAZARD this guards (TASK-712): a long-lived `CachedGitBackend` whose
    /// store branch was advanced by an EXTERNAL `git pull` (without reopening
    /// the backend) holds a cache that is stale — it is missing the pulled rows.
    /// A single-row write then upserts just the one written row and, if we
    /// blindly stamped the post-write HEAD, would mark the cache *fresh* while
    /// the pulled rows are still absent — they'd stay invisible until the next
    /// HEAD move. The single-row write paths do NOT call `ensure_cache_fresh`
    /// first (they are write-through, not read-then-write), so we cannot assume
    /// freshness here.
    ///
    /// Guard: only advance the recorded SHA when the cache was already current
    /// with the PRE-write HEAD (`pre_write_head`). If it was stale (recorded SHA
    /// differs, e.g. an external pull moved HEAD underneath us), we instead
    /// CLEAR the recorded SHA — exactly the "mark stale" signal used on a cache
    /// upsert failure — so the next read does a full rebuild from the store and
    /// picks up the pulled rows. This trades one extra rebuild for correctness.
    /// trace:TASK-712
    fn restamp_head(&self, pre_write_head: &str) {
        let head = self.current_head_sha();
        if head.is_empty() {
            return;
        }
        let recorded = self.cache.source_head_sha().ok().flatten();
        // Fresh iff the cache was recorded at the pre-write HEAD (the write's
        // own commit then advanced HEAD from pre_write_head to `head`). A
        // recorded SHA that is neither the pre-write HEAD nor absent means the
        // cache never ingested some externally-committed state → don't claim
        // freshness; force a rebuild.
        let was_current = match recorded.as_deref() {
            None => true,                   // first stamp / freshly rebuilt
            Some(s) => s == pre_write_head, // unchanged since we last freshened
        };
        if was_current {
            let _ = self.cache.set_source_head_sha(&head);
        } else {
            let _ = self.cache.set_source_head_sha("");
        }
    }

    /// An EPIC's status is a read-only rollup of its children, so a child's
    /// status flip (or a newly-added/removed child) must refresh the PARENT
    /// epic's cached status. The single-row cache upsert can't do this on its
    /// own — it has no view of the epic's other children. Here we have the inner
    /// git store, so for each epic the written `req` is a child of (its
    /// reciprocal `Parent`/`Child` edges name the parent), we re-read the epic +
    /// its children from git, roll their statuses up, and re-stamp the epic's
    /// cache row. This keeps `aida list --status in-progress` truthful for epics
    /// without waiting for a full rebuild. Best-effort: a failed lookup leaves
    /// the epic to be corrected on the next full rebuild (the
    /// rebuildable-projection contract).
    ///
    /// Scope note: this rolls up the epic's DIRECT children only (cheap, no
    /// whole-store load on every write), whereas the full rebuild
    /// (`compute_epic_statuses` → `derive_epic_status` → transitive
    /// `child_status_rollup`) is the AUTHORITATIVE transitive value. The two
    /// agree for the overwhelmingly common case; an epic whose only finished
    /// work lives in a grandchild is reconciled on the next rebuild — same
    /// approximation contract as the cache's `in_degree` / `blocked` axes.
    // trace:BUG-626 | ai:claude
    fn refresh_parent_epic_status(&self, req: &Requirement) {
        use crate::graph_walk::status_rollup;
        use crate::models::{RelationshipType, RequirementType};

        // A child stores the reciprocal edge to its parent (`Child --> parent`),
        // and some stores also carry `Parent --> parent`; union both so the
        // parent resolves whichever orientation was written. The epic itself, if
        // it is the thing written, is freshened by the cache upsert directly.
        let mut parent_ids: Vec<Uuid> = req
            .relationships
            .iter()
            .filter(|rel| {
                matches!(
                    rel.rel_type,
                    RelationshipType::Parent | RelationshipType::Child
                )
            })
            .map(|rel| rel.target_id)
            .collect();
        parent_ids.sort();
        parent_ids.dedup();

        for parent_id in parent_ids {
            // Load the candidate parent; skip anything that isn't an epic.
            let Ok(Some(epic)) = self.inner.get_requirement(&parent_id) else {
                continue;
            };
            if epic.req_type != RequirementType::Epic {
                continue;
            }
            // Resolve the epic's children from its OWN outbound Parent/Child
            // edges, load each, and roll the statuses up over a tiny store
            // (the epic + its children) — the same `status_rollup` the cache
            // rebuild path uses, so the two agree.
            let mut subset: Vec<Requirement> = vec![epic.clone()];
            let mut child_ids: Vec<Uuid> = Vec::new();
            for rel in &epic.relationships {
                if matches!(
                    rel.rel_type,
                    RelationshipType::Parent | RelationshipType::Child
                ) {
                    child_ids.push(rel.target_id);
                }
            }
            child_ids.sort();
            child_ids.dedup();
            for cid in &child_ids {
                if let Ok(Some(child)) = self.inner.get_requirement(cid) {
                    subset.push(child);
                }
            }
            let store = RequirementsStore {
                requirements: subset,
                ..Default::default()
            };
            let rollup = status_rollup(&store, &child_ids);
            let _ = self.cache.recompute_epic_status(&parent_id, &rollup);
        }
    }

    /// Write-through batched update (BUG-425): apply many requirement updates
    /// in a SINGLE store commit (via `GitBackend::bulk_update`), then upsert
    /// each into the cache and re-stamp the HEAD-SHA once. Mirrors
    /// `update_requirement`'s write-through cache handling, batched. If any
    /// cache upsert fails the cache is marked stale so the next read rebuilds
    /// from the store. Returns the count whose YAML actually changed.
    /// trace:BUG-425 | ai:claude
    pub fn bulk_update(&self, requirements: &[Requirement], commit_subject: &str) -> Result<usize> {
        // Capture HEAD BEFORE the write so restamp_head can tell our own commit
        // apart from an external pull that moved HEAD underneath us. trace:TASK-712
        let pre_write_head = self.current_head_sha();
        let n = self.inner.bulk_update(requirements, commit_subject)?;
        let mut cache_ok = true;
        for req in requirements {
            if let Err(e) = self.cache.upsert_requirement(req) {
                eprintln!(
                    "warning: cache upsert failed during bulk_update, cache marked stale: {}",
                    e
                );
                cache_ok = false;
                break;
            }
        }
        if cache_ok {
            // BUG-626: refresh each touched child's parent epic rollup status.
            for req in requirements {
                self.refresh_parent_epic_status(req);
            }
            self.restamp_head(&pre_write_head);
        } else {
            let _ = self.cache.set_source_head_sha("");
        }
        Ok(n)
    }
}

impl DatabaseBackend for CachedGitBackend {
    fn backend_type(&self) -> BackendType {
        // Surface as Git so callers that branch on backend type still work;
        // the cache is an implementation detail.
        BackendType::Git
    }

    fn path(&self) -> &Path {
        self.inner.path()
    }

    fn load(&self) -> Result<RequirementsStore> {
        // Phase 1: reads delegate to git. Phase 2 will switch list/search
        // to the cache.
        self.ensure_cache_fresh()?;
        self.inner.load()
    }

    fn save(&self, store: &RequirementsStore) -> Result<()> {
        // Bulk save: write through git, then full-rebuild the cache to
        // guarantee invariants (additions, modifications, deletions all
        // captured). Cheap enough for current scale.
        self.inner.save(store)?;
        let head = self.current_head_sha();
        self.cache.rebuild_from_store(store, &head)?;
        Ok(())
    }

    // ---- single-row CRUD: write-through with cache upsert/delete ----------

    fn get_requirement(&self, id: &Uuid) -> Result<Option<Requirement>> {
        self.inner.get_requirement(id)
    }

    fn get_requirement_by_spec_id(&self, spec_id: &str) -> Result<Option<Requirement>> {
        self.inner.get_requirement_by_spec_id(spec_id)
    }

    fn list_requirements(&self, include_archived: bool) -> Result<Vec<Requirement>> {
        self.ensure_cache_fresh()?;
        self.inner.list_requirements(include_archived)
    }

    fn add_requirement(&self, requirement: Requirement) -> Result<Requirement> {
        // trace:TASK-712 — capture HEAD before the write (see restamp_head).
        let pre_write_head = self.current_head_sha();
        let added = self.inner.add_requirement(requirement)?;
        if let Err(e) = self.cache.upsert_requirement(&added) {
            // Cache write failure is non-fatal — mark stale by clearing the
            // recorded SHA so the next read triggers a rebuild.
            let _ = self.cache.set_source_head_sha("");
            eprintln!("warning: cache upsert failed, cache marked stale: {}", e);
        } else {
            // BUG-626: a new child shifts its parent epic's rollup status.
            self.refresh_parent_epic_status(&added);
            self.restamp_head(&pre_write_head);
        }
        Ok(added)
    }

    fn update_requirement(&self, requirement: &Requirement) -> Result<()> {
        // trace:TASK-712 — capture HEAD before the write (see restamp_head).
        let pre_write_head = self.current_head_sha();
        self.inner.update_requirement(requirement)?;
        if let Err(e) = self.cache.upsert_requirement(requirement) {
            let _ = self.cache.set_source_head_sha("");
            eprintln!("warning: cache upsert failed, cache marked stale: {}", e);
        } else {
            // BUG-626: a child's status (or hierarchy edge) change shifts its
            // parent epic's rollup status — refresh the parent epic's row.
            self.refresh_parent_epic_status(requirement);
            self.restamp_head(&pre_write_head);
        }
        Ok(())
    }

    fn update_requirement_versioned(&self, requirement: &Requirement) -> Result<UpdateResult> {
        // trace:TASK-712 — capture HEAD before the write (see restamp_head).
        let pre_write_head = self.current_head_sha();
        let result = self.inner.update_requirement_versioned(requirement)?;
        if matches!(result, UpdateResult::Success) {
            if let Err(e) = self.cache.upsert_requirement(requirement) {
                let _ = self.cache.set_source_head_sha("");
                eprintln!("warning: cache upsert failed, cache marked stale: {}", e);
            } else {
                // BUG-626: refresh the parent epic's rollup status. trace:BUG-626
                self.refresh_parent_epic_status(requirement);
                self.restamp_head(&pre_write_head);
            }
        }
        Ok(result)
    }

    fn delete_requirement(&self, id: &Uuid) -> Result<()> {
        // trace:TASK-712 — capture HEAD before the write (see restamp_head).
        let pre_write_head = self.current_head_sha();
        self.inner.delete_requirement(id)?;
        if let Err(e) = self.cache.delete_requirement(id) {
            let _ = self.cache.set_source_head_sha("");
            eprintln!("warning: cache delete failed, cache marked stale: {}", e);
        } else {
            self.restamp_head(&pre_write_head);
        }
        Ok(())
    }

    // ---- delegated trait methods that don't touch requirements directly ---

    fn get_user(&self, id: &Uuid) -> Result<Option<User>> {
        self.inner.get_user(id)
    }

    fn get_user_by_handle(&self, handle: &str) -> Result<Option<User>> {
        self.inner.get_user_by_handle(handle)
    }

    fn list_users(&self, include_archived: bool) -> Result<Vec<User>> {
        self.inner.list_users(include_archived)
    }

    fn add_user(&self, user: User) -> Result<User> {
        self.inner.add_user(user)
    }

    fn update_user(&self, user: &User) -> Result<()> {
        self.inner.update_user(user)
    }

    fn delete_user(&self, id: &Uuid) -> Result<()> {
        self.inner.delete_user(id)
    }

    fn queue_list(&self, user_id: &str, include_completed: bool) -> Result<Vec<QueueEntry>> {
        self.inner.queue_list(user_id, include_completed)
    }

    // trace:STORY-672
    fn queue_users(&self) -> Result<Vec<String>> {
        self.inner.queue_users()
    }

    fn queue_add(&self, entry: QueueEntry) -> Result<()> {
        self.inner.queue_add(entry)
    }

    fn queue_remove(&self, user_id: &str, requirement_id: &Uuid) -> Result<()> {
        self.inner.queue_remove(user_id, requirement_id)
    }

    // trace:BUG-529 | ai:claude
    fn queue_remove_for_role(
        &self,
        user_id: &str,
        requirement_id: &Uuid,
        role: Option<&str>,
    ) -> Result<()> {
        self.inner
            .queue_remove_for_role(user_id, requirement_id, role)
    }

    fn queue_reorder(&self, user_id: &str, items: &[(Uuid, i64)]) -> Result<()> {
        self.inner.queue_reorder(user_id, items)
    }

    fn queue_clear(&self, user_id: &str, completed_only: bool) -> Result<()> {
        self.inner.queue_clear(user_id, completed_only)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_req(spec_id: &str, title: &str) -> Requirement {
        let mut r = Requirement::new(title.into(), "desc".into());
        r.spec_id = Some(spec_id.into());
        r
    }

    #[test]
    fn write_through_roundtrip() {
        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_path = dir.path().join(".aida").join("cache.db");

        std::fs::create_dir_all(&store_root).unwrap();
        let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();

        // Cache starts empty.
        assert_eq!(backend.cache().requirement_count().unwrap(), 0);

        // Add → cache picks it up.
        let req = sample_req("FR-1-001", "first");
        let req_id = req.id;
        backend.add_requirement(req).unwrap();
        assert_eq!(backend.cache().requirement_count().unwrap(), 1);

        // Update → cache replaces row.
        let mut updated = backend.get_requirement(&req_id).unwrap().unwrap();
        updated.title = "first updated".into();
        backend.update_requirement(&updated).unwrap();
        assert_eq!(backend.cache().requirement_count().unwrap(), 1);

        // Delete → cache row gone.
        backend.delete_requirement(&req_id).unwrap();
        assert_eq!(backend.cache().requirement_count().unwrap(), 0);
    }

    // `get_requirement_by_uuid_targeted` resolves a UUID to its canonical record
    // by reading ONLY the one target YAML (cache lookup for UUID -> spec_id, then
    // a single-spec git read) — never a full scan. It must return the same record
    // `get_requirement` (the full-scan path) returns for a present UUID, `None`
    // for an unknown UUID, and must source the record from git (so a stale cache
    // row never masquerades as the canonical value). trace:BUG-634 | ai:claude
    #[test]
    fn get_requirement_by_uuid_targeted_resolves_only_the_target() {
        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_path = dir.path().join(".aida").join("cache.db");
        std::fs::create_dir_all(&store_root).unwrap();
        let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();

        let a = backend
            .add_requirement(sample_req("FR-1-001", "alpha"))
            .unwrap();
        let b = backend
            .add_requirement(sample_req("FR-1-002", "bravo"))
            .unwrap();

        // Present UUID → same record the full-scan resolver returns.
        let targeted = backend.get_requirement_by_uuid_targeted(&a.id).unwrap();
        let scanned = backend.get_requirement(&a.id).unwrap();
        assert_eq!(targeted.as_ref().map(|r| r.id), Some(a.id));
        assert_eq!(
            targeted.as_ref().map(|r| r.spec_id.clone()),
            scanned.as_ref().map(|r| r.spec_id.clone()),
            "targeted resolve must match the full-scan resolve"
        );
        assert_eq!(targeted.unwrap().title, "alpha");

        // The other spec resolves independently.
        assert_eq!(
            backend
                .get_requirement_by_uuid_targeted(&b.id)
                .unwrap()
                .map(|r| r.title),
            Some("bravo".to_string())
        );

        // Unknown UUID → None (not an error, not a stray hit).
        assert!(backend
            .get_requirement_by_uuid_targeted(&uuid::Uuid::now_v7())
            .unwrap()
            .is_none());

        // The cache's UUID → spec_id mapping underpins the resolver.
        assert_eq!(
            backend.cache().spec_id_for_uuid(&a.id).unwrap().as_deref(),
            Some("FR-1-001")
        );
        assert!(backend
            .cache()
            .spec_id_for_uuid(&uuid::Uuid::now_v7())
            .unwrap()
            .is_none());
    }

    /// BUG-425: CachedGitBackend::bulk_update must write through to the cache
    /// (the archived rows must move out of the non-archived view), same
    /// guarantee as update_requirement but batched into one store commit.
    #[test]
    fn bulk_update_writes_through_to_cache() {
        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_path = dir.path().join(".aida").join("cache.db");
        std::fs::create_dir_all(&store_root).unwrap();
        let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();

        let mut reqs = Vec::new();
        for i in 1..=3 {
            reqs.push(
                backend
                    .add_requirement(sample_req(&format!("FR-1-00{i}"), &format!("req {i}")))
                    .unwrap(),
            );
        }
        let non_archived = |b: &CachedGitBackend| {
            b.list_summaries(&ListFilter {
                archive: ArchiveFilter::NonArchivedOnly,
                ..Default::default()
            })
            .unwrap()
            .len()
        };
        assert_eq!(non_archived(&backend), 3);

        // Bulk-archive all three.
        for r in &mut reqs {
            r.archived = true;
        }
        assert_eq!(backend.bulk_update(&reqs, "chore(archive)").unwrap(), 3);

        // Cache reflects the archive: gone from the non-archived view, present
        // in the archived view.
        assert_eq!(
            non_archived(&backend),
            0,
            "all archived → none non-archived"
        );
        let archived = backend
            .list_summaries(&ListFilter {
                archive: ArchiveFilter::ArchivedOnly,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(archived.len(), 3, "cache reflects the bulk archive");
    }

    // trace:TASK-712 — a long-lived backend whose store HEAD was advanced by an
    // external writer (simulating a `git pull`) must NOT lose the externally
    // added rows when it next does a local write. Before the fix, restamp_head
    // blindly stamped the post-write HEAD as fresh, hiding the external row;
    // now it detects the pre-write HEAD drift and marks the cache stale so the
    // next read rebuilds and surfaces every committed row.
    #[test]
    fn local_write_after_external_commit_does_not_hide_pulled_rows() {
        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_path = dir.path().join(".aida").join("cache.db");
        std::fs::create_dir_all(&store_root).unwrap();
        // A REAL git repo so HEAD advances on each commit — the staleness key
        // (and thus the restamp_head guard) is a no-op without one. trace:TASK-712
        crate::git_ops::init(&store_root).unwrap();
        crate::git_ops::configure_user(&store_root, "Test", "test@example.com").unwrap();

        // Long-lived backend: add one row, cache fresh at HEAD-A.
        let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();
        backend
            .add_requirement(sample_req("FR-1-001", "a"))
            .unwrap();
        assert_eq!(backend.cache().requirement_count().unwrap(), 1);

        // External writer (a second backend on the same store, no shared cache)
        // commits another row, advancing the store HEAD to HEAD-B underneath the
        // long-lived backend. Stands in for an external `git pull`.
        {
            let external = GitBackend::new(&store_root).unwrap();
            external
                .add_requirement(sample_req("FR-1-002", "b (external)"))
                .unwrap();
        }

        // Long-lived backend does a LOCAL write (HEAD advances to HEAD-C).
        backend
            .add_requirement(sample_req("FR-1-003", "c"))
            .unwrap();

        // A subsequent read must surface ALL THREE rows — the external one is
        // not silently dropped. (ensure_cache_fresh rebuilds because restamp_head
        // cleared the recorded SHA when it saw the pre-write HEAD had drifted.)
        let all = backend.list_summaries(&ListFilter::default()).unwrap();
        assert_eq!(all.len(), 3, "external row must not be hidden, got {all:?}");
    }

    #[test]
    fn rebuild_recovers_from_dropped_cache() {
        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_path = dir.path().join(".aida").join("cache.db");
        std::fs::create_dir_all(&store_root).unwrap();

        let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();
        backend
            .add_requirement(sample_req("FR-1-001", "a"))
            .unwrap();
        backend
            .add_requirement(sample_req("FR-1-002", "b"))
            .unwrap();
        assert_eq!(backend.cache().requirement_count().unwrap(), 2);

        // Drop the cache file entirely; rebuild restores it.
        drop(backend);
        std::fs::remove_file(&cache_path).unwrap();

        let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();
        // Open triggered ensure_cache_fresh, which detected stale and rebuilt.
        assert_eq!(backend.cache().requirement_count().unwrap(), 2);
    }
}
