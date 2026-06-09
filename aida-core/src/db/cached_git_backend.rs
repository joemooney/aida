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

use super::cache::{ArchiveFilter, Cache, CacheRead, ListFilter, RequirementSummary};
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

    /// Fail-soft list (STORY-543 / L3). Under cache lock contention this does NOT
    /// hard-error: it returns best-effort rows plus a `degraded` flag so the
    /// caller can emit a staleness signal (stderr warning / `"stale": true`) and
    /// still exit 0. The stale-check rebuild is itself fail-soft — if the cache
    /// is locked while we try to refresh it, we proceed to read whatever the
    /// (possibly slightly stale) snapshot holds rather than blocking on the
    /// writer. trace:STORY-543
    pub fn list_summaries_soft(
        &self,
        filter: &ListFilter,
    ) -> Result<CacheRead<Vec<RequirementSummary>>> {
        let refresh_degraded = self.ensure_cache_fresh_soft();
        let mut read = self.cache.list_summaries_soft(filter)?;
        read.degraded = read.degraded || refresh_degraded;
        Ok(read)
    }

    /// Cache-backed FTS5 search across spec_id, agreed_id, title, description.
    /// `archive` controls the archive axis — STORY-441.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        archive: ArchiveFilter,
    ) -> Result<Vec<RequirementSummary>> {
        self.ensure_cache_fresh()?;
        self.cache.search(query, limit, archive)
    }

    /// Fail-soft FTS search (STORY-543 / L3). See `list_summaries_soft`.
    /// trace:STORY-543
    pub fn search_soft(
        &self,
        query: &str,
        limit: usize,
        archive: ArchiveFilter,
    ) -> Result<CacheRead<Vec<RequirementSummary>>> {
        let refresh_degraded = self.ensure_cache_fresh_soft();
        let mut read = self.cache.search_soft(query, limit, archive)?;
        read.degraded = read.degraded || refresh_degraded;
        Ok(read)
    }

    /// Fail-soft variant of `ensure_cache_fresh`: returns `true` (degraded) when
    /// the staleness rebuild could not run because the cache (or git store) was
    /// busy/locked, instead of propagating the error. The caller then reads the
    /// last-readable snapshot — possibly slightly stale, which is exactly the
    /// fail-soft contract. trace:STORY-543
    fn ensure_cache_fresh_soft(&self) -> bool {
        self.ensure_cache_fresh().is_err()
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

    fn queue_add(&self, entry: QueueEntry) -> Result<()> {
        self.inner.queue_add(entry)
    }

    fn queue_remove(&self, user_id: &str, requirement_id: &Uuid) -> Result<()> {
        self.inner.queue_remove(user_id, requirement_id)
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
