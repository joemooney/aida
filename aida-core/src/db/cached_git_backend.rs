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

use super::cache::{ArchiveFilter, Cache, ListFilter, RequirementSummary};
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
    fn restamp_head(&self) {
        let head = self.current_head_sha();
        if !head.is_empty() {
            let _ = self.cache.set_source_head_sha(&head);
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
            self.restamp_head();
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
        let added = self.inner.add_requirement(requirement)?;
        if let Err(e) = self.cache.upsert_requirement(&added) {
            // Cache write failure is non-fatal — mark stale by clearing the
            // recorded SHA so the next read triggers a rebuild.
            let _ = self.cache.set_source_head_sha("");
            eprintln!("warning: cache upsert failed, cache marked stale: {}", e);
        } else {
            self.restamp_head();
        }
        Ok(added)
    }

    fn update_requirement(&self, requirement: &Requirement) -> Result<()> {
        self.inner.update_requirement(requirement)?;
        if let Err(e) = self.cache.upsert_requirement(requirement) {
            let _ = self.cache.set_source_head_sha("");
            eprintln!("warning: cache upsert failed, cache marked stale: {}", e);
        } else {
            self.restamp_head();
        }
        Ok(())
    }

    fn update_requirement_versioned(&self, requirement: &Requirement) -> Result<UpdateResult> {
        let result = self.inner.update_requirement_versioned(requirement)?;
        if matches!(result, UpdateResult::Success) {
            if let Err(e) = self.cache.upsert_requirement(requirement) {
                let _ = self.cache.set_source_head_sha("");
                eprintln!("warning: cache upsert failed, cache marked stale: {}", e);
            } else {
                self.restamp_head();
            }
        }
        Ok(result)
    }

    fn delete_requirement(&self, id: &Uuid) -> Result<()> {
        self.inner.delete_requirement(id)?;
        if let Err(e) = self.cache.delete_requirement(id) {
            let _ = self.cache.set_source_head_sha("");
            eprintln!("warning: cache delete failed, cache marked stale: {}", e);
        } else {
            self.restamp_head();
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
