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
        // BUG-664: the constructor is on the hot path of every read command
        // (`aida status`, `aida list`). Use the read-tolerant freshness check so
        // a reader serves the last-good snapshot when another process is already
        // rebuilding the cache, instead of contending for the write lock through
        // the ~25s retry ladder. Writers re-check freshness strictly on their own
        // write paths, and the SHA-based stale detection is unchanged.
        backend.ensure_cache_fresh_for_read()?;
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

    /// Cache-backed spec-id collision scan for the `aida add` hot path
    /// (BUG-701). Brings the cache up to the store's current HEAD first — so a
    /// collision that a just-completed pre-allocation pull introduced is visible
    /// — then runs the indexed group-by. In the steady state (cache already
    /// fresh, the common case) this is a sub-millisecond query instead of the
    /// old O(n) full-store `GitBackend::load()` scan; on the rare divergent-pull
    /// it pays a cache refresh that also serves the subsequent reads. Returns
    /// `(spec_id, uuid, title)` for every claimant of a collided spec_id.
    // trace:BUG-701 | ai:claude
    pub fn spec_id_collisions(&self) -> Result<Vec<(String, Uuid, String)>> {
        self.ensure_cache_fresh()?;
        self.cache.spec_id_collisions()
    }

    /// Read the current git HEAD on the store branch. Empty string if not in a
    /// git repo (e.g., test fixture); stale check then collapses to "always
    /// fresh" which is fine for non-git scenarios.
    fn current_head_sha(&self) -> String {
        crate::git_ops::head_sha(self.inner.path()).unwrap_or_default()
    }

    /// Resolve a UUID to its `Requirement` WITHOUT the O(n) `find_by_uuid` scan
    /// over every object file. The cache holds the stable uuid→spec_id mapping,
    /// so we resolve the spec_id from the cache then read the ONE matching YAML
    /// object (the read is authoritative — the cache only tells us which file).
    /// On any cache miss, a spec_id/uuid mismatch (defends against a torn cache
    /// row), or an unreadable cache, we fall back to the inner full scan so
    /// correctness never depends on cache freshness. The uuid→spec_id mapping is
    /// invariant (spec_ids are stable and never reused), so even a stale cache
    /// row resolves to the right file.
    // trace:BUG-634 | ai:claude
    fn get_requirement_targeted(&self, id: &Uuid) -> Result<Option<Requirement>> {
        if let Ok(Some(spec_id)) = self.cache.spec_id_for_uuid(id) {
            if let Ok(Some(req)) = self.inner.get_requirement_by_spec_id(&spec_id) {
                if req.id == *id {
                    return Ok(Some(req));
                }
            }
        }
        // Cache miss / mismatch / unreadable — fall back to the authoritative
        // (but O(n)) scan rather than risk a wrong not-found.
        self.inner.get_requirement(id)
    }

    /// If the cache is stale (or missing source SHA), bring it up to the
    /// store's current HEAD. Cheap when fresh — just a meta lookup + string
    /// compare.
    ///
    /// When the recorded cache HEAD is a known ancestor of the new HEAD (a
    /// normal fast-forward / merge advance), only the rows for the object files
    /// that changed in `recorded..head` are refreshed — instead of re-parsing
    /// and re-inserting all ~N objects on every HEAD move. In a busy multi-agent
    /// store HEAD moves constantly, so this is the dominant read/write wall-cost.
    /// We fall back to a full rebuild whenever incremental can't be proven safe
    /// (no recorded HEAD; recorded HEAD not an ancestor of head, i.e. the orphan
    /// branch was force-pushed/rebased; the diff is too large to beat a rebuild;
    /// or any error mid-update). Never produce a wrong cache: when in doubt, full
    /// rebuild.
    // trace:BUG-636 | ai:claude
    fn ensure_cache_fresh(&self) -> Result<()> {
        let head = self.current_head_sha();
        if !self.cache.is_stale(&head)? {
            return Ok(());
        }
        // Non-git fixture (empty HEAD): nothing to diff — full rebuild is the
        // only correct path (and is cheap, there are no commits).
        if !head.is_empty() {
            if let Some(recorded) = self.cache.source_head_sha()? {
                if !recorded.is_empty()
                    && crate::git_ops::is_ancestor(self.inner.path(), &recorded, &head)
                        .unwrap_or(false)
                {
                    match self.try_incremental_update(&recorded, &head) {
                        Ok(true) => return Ok(()),
                        // Ok(false): incremental declined (diff too large / a row
                        // the diff named couldn't be read) — fall through to a
                        // full rebuild, which is always correct.
                        Ok(false) => {}
                        Err(e) => {
                            eprintln!(
                                "warning: incremental cache update failed ({e}); full rebuild"
                            );
                        }
                    }
                }
            }
        }
        self.full_rebuild(&head)
    }

    /// Read-path freshness: like `ensure_cache_fresh`, but when the cache is
    /// stale AND another LIVE process currently holds the cache write-lock (it is
    /// mid rebuild/write), skip the rebuild and serve the last-good committed
    /// snapshot instead of contending for the write lock through the ~25s retry
    /// ladder (BUG-664). WAL guarantees that snapshot is consistent — a reader
    /// sees the last COMMITTED state, never a torn mid-rebuild read — and the
    /// foreign writer is itself bringing the cache current, so a momentary stale
    /// read is exactly the rebuildable-projection contract, not a correctness
    /// loss. Pure-read callers use this; writers keep the strict
    /// `ensure_cache_fresh` so the cache they write through is never weakened.
    // trace:BUG-664 | ai:claude
    fn ensure_cache_fresh_for_read(&self) -> Result<()> {
        let head = self.current_head_sha();
        if !self.cache.is_stale(&head)? {
            return Ok(());
        }
        // Stale, but if another live process is already rebuilding, don't pile
        // onto the write lock — read the prior consistent snapshot.
        if super::cache::foreign_writer_holds_lock(self.cache.path()) {
            return Ok(());
        }
        self.ensure_cache_fresh()
    }

    /// Full authoritative rebuild: load the whole store and re-project every
    /// row. The fallback whenever incremental can't be proven safe.
    // trace:BUG-636
    fn full_rebuild(&self, head: &str) -> Result<()> {
        let store = self
            .inner
            .load()
            .context("Failed to load git store for cache rebuild")?;
        self.cache.rebuild_from_store(&store, head)?;
        Ok(())
    }

    /// Refresh only the cache rows for the object files that changed between the
    /// recorded cache HEAD (`from`, a proven ancestor of `to`) and the new HEAD
    /// (`to`). Returns `Ok(true)` when the incremental update fully applied (cache
    /// stamped at `to`), `Ok(false)` when it declined (caller must full-rebuild),
    /// or `Err` on a git/diff failure (caller logs + full-rebuilds).
    ///
    /// Correctness: the single-YAML read (`get_requirement_by_spec_id` against the
    /// live worktree, which is checked out at `to`) is AUTHORITATIVE — the diff
    /// only tells us WHICH files to refresh, never their content. Each refreshed
    /// row goes through the same `upsert_requirement` + `refresh_parent_epic_status`
    /// the normal write-through path uses, so an incremental catch-up produces the
    /// same cache those commits would have produced had they been written through
    /// this backend. The whole-graph derived columns (a NEIGHBOR's `in_degree`,
    /// dependents' `blocked`, an epic rollup over a grandchild) follow the SAME
    /// rebuildable-projection contract as every single-row write — approximate
    /// between rebuilds, authoritative after `aida cache rebuild` — they are never
    /// WRONG about the set of rows or a row's own summary content.
    // trace:BUG-636
    fn try_incremental_update(&self, from: &str, to: &str) -> Result<bool> {
        use crate::git_ops::ObjectChange;

        let changes = crate::git_ops::changed_object_files(self.inner.path(), from, to)?;
        // Above this many changed files, a from-scratch rebuild (one batched
        // transaction, no per-row epic-rollup re-reads) is cheaper than replaying
        // upserts one at a time. The full store is ~thousands of objects; a few
        // hundred changed files is the crossover where incremental stops winning.
        const INCREMENTAL_MAX_FILES: usize = 500;
        if changes.len() > INCREMENTAL_MAX_FILES {
            return Ok(false);
        }
        for (kind, path) in &changes {
            // The spec_id is the file stem: objects/TYPE/000/<SPEC-ID>.yaml.
            let Some(spec_id) = path.file_stem().and_then(|s| s.to_str()) else {
                // Unparseable path — bail to the always-correct full rebuild.
                return Ok(false);
            };
            match kind {
                ObjectChange::Added | ObjectChange::Modified => {
                    // Authoritative read of the ONE object at the worktree HEAD.
                    match self.inner.get_requirement_by_spec_id(spec_id)? {
                        Some(req) => {
                            self.cache.upsert_requirement(&req)?;
                            // BUG-626 parity with the write path: a child's status
                            // / hierarchy change shifts its parent epic's rollup.
                            self.refresh_parent_epic_status(&req);
                        }
                        None => {
                            // The diff says present at `to` but the worktree can't
                            // read it (HEAD moved underneath us, or a torn state).
                            // Don't guess — full rebuild. trace:BUG-636
                            return Ok(false);
                        }
                    }
                }
                ObjectChange::Deleted => {
                    // Resolve the spec_id (from the path) to its uuid via the
                    // cache and drop the row. Absent already → harmless no-op.
                    if let Some(uuid) = self.cache.uuid_for_spec_id(spec_id)? {
                        self.cache.delete_requirement(&uuid)?;
                    }
                }
            }
        }
        // Every changed row refreshed — the cache now matches `to`.
        self.cache.set_source_head_sha(to)?;
        Ok(true)
    }

    /// Cache-backed list query with filter pushdown. Returns lightweight
    /// summaries — full Requirement records require a follow-up
    /// `get_requirement` call. Triggers a stale-check first so callers
    /// always see fresh data.
    pub fn list_summaries(&self, filter: &ListFilter) -> Result<Vec<RequirementSummary>> {
        self.ensure_cache_fresh_for_read()?;
        self.cache.list_summaries(filter)
    }

    /// TASK-1065: count non-archived specs with a still-pending DecisionRequest,
    /// read from the `has_pending_decision` cache column. Cache-backed so the
    /// `aida status --full` decision-inbox count no longer needs a full
    /// `backend.load()`. Triggers a stale-check first so the count reflects the
    /// latest committed store.
    // trace:TASK-1065 | ai:claude
    pub fn pending_decision_count(&self) -> Result<usize> {
        self.ensure_cache_fresh_for_read()?;
        self.cache.pending_decision_count()
    }

    /// TASK-1065: load ONLY the store metadata (name/title/description/features/
    /// id_config/…) — a single `metadata.yaml` read, NOT a scan of every object
    /// YAML. The returned `RequirementsStore` has an EMPTY `requirements` vec; it
    /// is the cheap metadata half of the `aida status --full` store that the rich
    /// status path needs (Project name + scaffolding preview) without paying for a
    /// full `backend.load()`.
    // trace:TASK-1065 | ai:claude
    pub fn load_metadata_only(&self) -> Result<RequirementsStore> {
        self.inner.load_metadata_only()
    }

    /// STORY-632: deterministic local graph-centrality (in/out degree + heft)
    /// for a single spec, read from the cache. Triggers a stale-check first so
    /// the inbound axis reflects the latest committed relationship graph (a
    /// HEAD change since the last rebuild forces a fresh full recompute).
    /// trace:STORY-632 | ai:claude
    pub fn degrees(&self, id: &Uuid) -> Result<crate::db::Degrees> {
        self.ensure_cache_fresh_for_read()?;
        self.cache.degrees_for_id(id)
    }

    /// The transitive descendant id-set of `root` (root + all parent->child
    /// descendants, any depth), read from the cache's materialized hierarchy
    /// edges via one WITH RECURSIVE query. Triggers a stale-check first so the
    /// edge set reflects the latest committed graph. Backs
    /// `aida list --parent <id> --recursive`.
    // trace:TASK-955 | ai:claude
    pub fn descendant_ids(&self, root: &Uuid) -> Result<std::collections::HashSet<Uuid>> {
        self.ensure_cache_fresh_for_read()?;
        self.cache.descendant_ids(root)
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
        self.ensure_cache_fresh_for_read()?;
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
    /// status flip (or a newly-added/removed child) must refresh the ancestor
    /// epics' cached status. The single-row cache upsert can't do this on its
    /// own — it has no view of the epic's other children. Best-effort: a failed
    /// lookup leaves the epic to be corrected on the next full rebuild (the
    /// rebuildable-projection contract).
    ///
    /// BUG-764: both legs of this used to read only ONE endpoint's edges — the
    /// ancestor scan read the written child's edges, but the child-set rollup
    /// read the epic's OWN outbound edges, which are empty in the common
    /// child-authored-edge shape (`aida add --parent` records the edge on the
    /// child). A child completing via the `aida pull` auto-bump (raw
    /// `GitBackend` writes, replayed through the incremental cache catch-up)
    /// then re-derived its epic from zero children, and the epic's cached
    /// status stuck. Now both legs walk the cache's materialized
    /// `hierarchy_edges` (both-endpoint, oriented — the same substrate the full
    /// rebuild and `descendant_ids` use): resolve every ancestor EPIC of the
    /// written row, then re-derive each from its transitive descendant set. As
    /// a bonus the rollup is now transitive, so a grandchild flip refreshes the
    /// top epic too, matching the rebuild's authoritative value.
    // trace:BUG-626 trace:BUG-764 | ai:claude
    fn refresh_parent_epic_status(&self, req: &Requirement) {
        let Ok(ancestor_epics) = self.cache.ancestor_epic_ids(&req.id) else {
            return;
        };
        for epic_id in ancestor_epics {
            let _ = self.cache.recompute_epic_status_from_hierarchy(&epic_id);
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
        // BUG-634: cache-resolve uuid→spec_id and read the one object, avoiding
        // the O(n) full scan on the write path (this method is on the hot path
        // via refresh_parent_epic_status and the CLI lease/ancestor walk).
        // trace:BUG-634 | ai:claude
        self.get_requirement_targeted(id)
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

    // trace:TASK-1052 | ai:claude
    fn queue_remove_many(&self, user_id: &str, ids: &[Uuid]) -> Result<Vec<QueueEntry>> {
        self.inner.queue_remove_many(user_id, ids)
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

    // ---------------------------------------------------------------- BUG-764
    // Epic rollup re-derivation when a child completes OUTSIDE the cached
    // backend — the `aida pull` auto-bump route writes flips through a raw
    // `GitBackend`, so the epic's cached status is only corrected when the next
    // read's incremental catch-up replays the changed child rows. The data uses
    // the CHILD-AUTHORED edge shape (the child records `Parent -> epic`; the
    // epic's record carries no hierarchy edge), which the old own-edges rollup
    // resolved to zero children. The epic also carries a Rejected sibling — the
    // observed stuck mix that used to derive InProgress forever.

    /// Cached status string for one row, from the cache's list projection.
    fn cached_status(b: &CachedGitBackend, id: Uuid) -> String {
        b.list_summaries(&ListFilter {
            archive: ArchiveFilter::Both,
            defer: DeferFilter::Both,
            ..Default::default()
        })
        .unwrap()
        .into_iter()
        .find(|r| r.id == id)
        .map(|r| r.status)
        .unwrap()
    }

    /// Build a real git store holding an epic with two child-authored-edge
    /// children: one open (Approved), one Rejected. Returns the backend plus
    /// the (epic, open child) ids.
    fn epic_with_open_and_rejected_child(
        store_root: &Path,
        cache_path: &Path,
    ) -> (CachedGitBackend, Uuid, Uuid) {
        use crate::models::{Relationship, RelationshipType, RequirementStatus, RequirementType};

        std::fs::create_dir_all(store_root).unwrap();
        crate::git_ops::init(store_root).unwrap();
        crate::git_ops::configure_user(store_root, "Test", "test@example.com").unwrap();
        let backend = CachedGitBackend::open(store_root, cache_path).unwrap();

        let mut epic = sample_req("EPIC-1", "epic");
        epic.req_type = RequirementType::Epic;
        epic.status = RequirementStatus::InProgress;
        let epic_id = epic.id;
        backend.add_requirement(epic).unwrap();

        let child_edge = || Relationship {
            rel_type: RelationshipType::Parent,
            target_id: epic_id,
            created_at: None,
            created_by: None,
        };
        let mut open_child = sample_req("STORY-1", "open child");
        open_child.status = RequirementStatus::Approved;
        open_child.relationships.push(child_edge());
        let child_id = open_child.id;
        backend.add_requirement(open_child).unwrap();

        let mut rejected = sample_req("STORY-2", "rejected sibling");
        rejected.status = RequirementStatus::Rejected;
        rejected.relationships.push(child_edge());
        backend.add_requirement(rejected).unwrap();

        // One open child + one rejected sibling → derived Draft (queued).
        assert_eq!(cached_status(&backend, epic_id), "Draft");
        (backend, epic_id, child_id)
    }

    // The auto-bump route: the last open child completes via a raw `GitBackend`
    // write (what `auto_bump_done_to_completed` does during `aida pull`); the
    // cached backend's next read must re-derive the epic to Completed — no
    // manual `edit --status --force` recovery. trace:BUG-764 | ai:claude
    #[test]
    fn epic_rollup_refreshes_after_external_auto_bump_completion() {
        use crate::models::RequirementStatus;

        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_path = dir.path().join(".aida").join("cache.db");
        let (backend, epic_id, child_id) =
            epic_with_open_and_rejected_child(&store_root, &cache_path);

        // External raw GitBackend flip — the auto-bump write shape.
        {
            let external = GitBackend::new(&store_root).unwrap();
            let mut child = external.get_requirement(&child_id).unwrap().unwrap();
            child.status = RequirementStatus::Completed;
            external.update_requirement(&child).unwrap();
        }

        // Next cache-backed read replays the flip and re-derives the epic:
        // zero open children (completed + rejected) → Completed.
        assert_eq!(cached_status(&backend, child_id), "Completed");
        assert_eq!(
            cached_status(&backend, epic_id),
            "Completed",
            "epic with zero open children must not stay stuck"
        );
    }

    // The direct-edit route: the same flip written THROUGH the cached backend
    // must refresh the epic's row synchronously. With the child-authored edge
    // shape the old own-edges rollup saw zero children and re-derived the epic
    // to Draft. trace:BUG-764 | ai:claude
    #[test]
    fn epic_rollup_refreshes_after_direct_child_edit() {
        use crate::models::RequirementStatus;

        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_path = dir.path().join(".aida").join("cache.db");
        let (backend, epic_id, child_id) =
            epic_with_open_and_rejected_child(&store_root, &cache_path);

        let mut child = backend.get_requirement(&child_id).unwrap().unwrap();
        child.status = RequirementStatus::Completed;
        backend.update_requirement(&child).unwrap();

        assert_eq!(cached_status(&backend, epic_id), "Completed");
    }

    // ---------------------------------------------------------------- BUG-636
    // Incremental cache update on a HEAD move: refresh only the changed rows
    // instead of a full delete-and-reinsert of every object. The tests below
    // pin the two correctness invariants — (1) an incremental update is
    // byte-equal to a full rebuild at the same HEAD for the row content it
    // owns, and (2) a non-ancestor HEAD (rewritten history) falls back to a
    // full rebuild — plus the git helpers they rely on. trace:BUG-636

    /// Snapshot the rows that matter for an incremental-vs-rebuild comparison:
    /// the authoritative summary content plus the projected derived columns.
    /// Sorted by spec_id for a deterministic compare.
    fn row_snapshot(b: &CachedGitBackend) -> Vec<(String, String, String, u32, u32, bool, bool)> {
        let mut rows: Vec<_> = b
            .list_summaries(&ListFilter {
                archive: ArchiveFilter::Both,
                defer: DeferFilter::Both,
                ..Default::default()
            })
            .unwrap()
            .into_iter()
            .map(|s| {
                (
                    s.spec_id.unwrap_or_default(),
                    s.title,
                    s.status,
                    s.in_degree,
                    s.out_degree,
                    s.blocked,
                    s.archived,
                )
            })
            .collect();
        rows.sort();
        rows
    }

    /// A long-lived backend whose cache is fresh at HEAD-A, then an external
    /// writer lands several commits (add + modify + delete) advancing HEAD to
    /// HEAD-B. The long-lived backend's next read must INCREMENTALLY update its
    /// cache and land on exactly the rows a from-scratch full rebuild at HEAD-B
    /// produces.
    // trace:BUG-636
    #[test]
    fn incremental_update_matches_full_rebuild() {
        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_a = dir.path().join(".aida").join("a.cache.db");
        let cache_fresh = dir.path().join(".aida").join("fresh.cache.db");
        std::fs::create_dir_all(&store_root).unwrap();
        crate::git_ops::init(&store_root).unwrap();
        crate::git_ops::configure_user(&store_root, "Test", "test@example.com").unwrap();

        // Long-lived backend A: three rows, cache fresh at HEAD-A.
        let backend = CachedGitBackend::open(&store_root, &cache_a).unwrap();
        backend
            .add_requirement(sample_req("FR-1-001", "a"))
            .unwrap();
        backend
            .add_requirement(sample_req("FR-1-002", "b"))
            .unwrap();
        backend
            .add_requirement(sample_req("FR-1-003", "c"))
            .unwrap();
        assert_eq!(backend.cache().requirement_count().unwrap(), 3);
        let recorded_before = backend.cache().source_head_sha().unwrap();

        // External writer advances HEAD with one of each change kind:
        // add FR-1-004, modify FR-1-002's title, delete FR-1-001.
        {
            let external = GitBackend::new(&store_root).unwrap();
            external
                .add_requirement(sample_req("FR-1-004", "d"))
                .unwrap();
            let mut r2 = external
                .get_requirement_by_spec_id("FR-1-002")
                .unwrap()
                .unwrap();
            r2.title = "b (modified externally)".into();
            external.update_requirement(&r2).unwrap();
            let r1 = external
                .get_requirement_by_spec_id("FR-1-001")
                .unwrap()
                .unwrap();
            external.delete_requirement(&r1.id).unwrap();
        }

        // Backend A reads → should take the INCREMENTAL path (recorded HEAD is
        // an ancestor of the new HEAD). The rows must match a full rebuild.
        let incremental_rows = row_snapshot(&backend);

        // The incremental update advanced the recorded SHA (it didn't just
        // clear it / leave it pinned at HEAD-A).
        let recorded_after = backend.cache().source_head_sha().unwrap();
        assert_ne!(
            recorded_before, recorded_after,
            "incremental update should advance the recorded HEAD"
        );
        assert_eq!(
            recorded_after,
            Some(crate::git_ops::head_sha(&store_root).unwrap()),
            "recorded HEAD must equal the store HEAD after an incremental update"
        );

        // Ground truth: a brand-new cache full-rebuilt at the same HEAD.
        let fresh = CachedGitBackend::open(&store_root, &cache_fresh).unwrap();
        let rebuild_rows = row_snapshot(&fresh);

        assert_eq!(
            incremental_rows, rebuild_rows,
            "incremental update must equal a full rebuild at the same HEAD"
        );
        // Spot-check the net effect: 001 gone, 002 retitled, 004 present.
        let titles: Vec<&str> = incremental_rows.iter().map(|r| r.1.as_str()).collect();
        assert!(!incremental_rows.iter().any(|r| r.0 == "FR-1-001"));
        assert!(titles.contains(&"b (modified externally)"));
        assert!(incremental_rows.iter().any(|r| r.0 == "FR-1-004"));
        assert_eq!(incremental_rows.len(), 3);
    }

    /// Each change kind refreshes the right row in isolation.
    // trace:BUG-636
    #[test]
    fn incremental_add_modify_delete_each_refresh_right_row() {
        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_path = dir.path().join(".aida").join("cache.db");
        std::fs::create_dir_all(&store_root).unwrap();
        crate::git_ops::init(&store_root).unwrap();
        crate::git_ops::configure_user(&store_root, "Test", "test@example.com").unwrap();

        let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();
        backend
            .add_requirement(sample_req("FR-1-001", "keep"))
            .unwrap();
        backend
            .add_requirement(sample_req("FR-1-002", "to-edit"))
            .unwrap();

        // ADD via external writer. (row_snapshot's list read triggers the
        // incremental freshen; requirement_count is a raw cache read and does
        // NOT freshen, so check it only after a read.)
        {
            let ext = GitBackend::new(&store_root).unwrap();
            ext.add_requirement(sample_req("FR-1-003", "added"))
                .unwrap();
        }
        let rows = row_snapshot(&backend);
        assert!(rows.iter().any(|r| r.0 == "FR-1-003" && r.1 == "added"));
        assert_eq!(backend.cache().requirement_count().unwrap(), 3);

        // MODIFY via external writer.
        {
            let ext = GitBackend::new(&store_root).unwrap();
            let mut r = ext.get_requirement_by_spec_id("FR-1-002").unwrap().unwrap();
            r.title = "edited".into();
            ext.update_requirement(&r).unwrap();
        }
        let rows = row_snapshot(&backend);
        assert!(rows.iter().any(|r| r.0 == "FR-1-002" && r.1 == "edited"));
        assert_eq!(backend.cache().requirement_count().unwrap(), 3);

        // DELETE via external writer.
        {
            let ext = GitBackend::new(&store_root).unwrap();
            let r = ext.get_requirement_by_spec_id("FR-1-001").unwrap().unwrap();
            ext.delete_requirement(&r.id).unwrap();
        }
        let rows = row_snapshot(&backend);
        assert!(!rows.iter().any(|r| r.0 == "FR-1-001"));
        assert_eq!(backend.cache().requirement_count().unwrap(), 2);
    }

    /// A rewritten orphan-branch history (the recorded HEAD is no longer an
    /// ancestor of the new HEAD) must fall back to a full rebuild rather than
    /// diff against an unreachable commit — and still produce a correct cache.
    // trace:BUG-636
    #[test]
    fn incremental_falls_back_on_non_ancestor() {
        use std::process::Command;
        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_path = dir.path().join(".aida").join("cache.db");
        std::fs::create_dir_all(&store_root).unwrap();
        crate::git_ops::init(&store_root).unwrap();
        crate::git_ops::configure_user(&store_root, "Test", "test@example.com").unwrap();

        let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();
        backend
            .add_requirement(sample_req("FR-1-001", "a"))
            .unwrap();
        let recorded = backend.cache().source_head_sha().unwrap().unwrap();

        // Rewrite history: `commit --amend` produces a SIBLING commit whose
        // parent is the old HEAD's parent — so the recorded HEAD is no longer
        // reachable from the new HEAD. Change the message so the amended commit
        // gets a DIFFERENT sha (an identical-tree/message/author amend is
        // byte-identical and git reuses the same hash).
        let amend = Command::new("git")
            .current_dir(&store_root)
            .args(["commit", "--amend", "-m", "rewritten root"])
            .output()
            .unwrap();
        assert!(amend.status.success(), "amend failed: {amend:?}");
        let amended = crate::git_ops::head_sha(&store_root).unwrap();
        assert_ne!(recorded, amended);
        assert!(
            !crate::git_ops::is_ancestor(&store_root, &recorded, &amended).unwrap(),
            "recorded HEAD must NOT be an ancestor of the amended HEAD"
        );

        // The read takes the non-ancestor fallback (full rebuild) and the cache
        // is still correct (the amend kept the same tree → one row).
        let rows = row_snapshot(&backend);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "FR-1-001");
        assert_eq!(
            backend.cache().source_head_sha().unwrap(),
            Some(amended),
            "fallback rebuild re-stamps the recorded HEAD to the new HEAD"
        );
    }

    /// Timing demonstration (ignored by default; run with
    /// `cargo test -p aida-core incremental_is_faster_than_full_rebuild -- --ignored --nocapture`).
    /// Builds a store of N specs, then compares a from-scratch full rebuild
    /// against a stale-by-one-commit incremental update. The incremental path
    /// parses ONE changed YAML versus all N.
    // trace:BUG-636
    #[test]
    #[ignore]
    fn incremental_is_faster_than_full_rebuild() {
        use std::time::Instant;
        const N: usize = 400;
        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        std::fs::create_dir_all(&store_root).unwrap();
        crate::git_ops::init(&store_root).unwrap();
        crate::git_ops::configure_user(&store_root, "Test", "test@example.com").unwrap();

        let backend = CachedGitBackend::open(&store_root, &dir.path().join("a.db")).unwrap();
        for i in 1..=N {
            backend
                .add_requirement(sample_req(&format!("FR-1-{i:04}"), &format!("r{i}")))
                .unwrap();
        }

        // Full rebuild: fresh cache opened at the current HEAD.
        let t0 = Instant::now();
        let fresh = CachedGitBackend::open(&store_root, &dir.path().join("b.db")).unwrap();
        let _ = fresh.list_summaries(&ListFilter::default()).unwrap();
        let full = t0.elapsed();

        // One more external commit, then an incremental update on `backend`
        // (recorded HEAD is an ancestor of the new HEAD → incremental path).
        {
            let ext = GitBackend::new(&store_root).unwrap();
            ext.add_requirement(sample_req("FR-1-9999", "new")).unwrap();
        }
        let t1 = Instant::now();
        let _ = backend.list_summaries(&ListFilter::default()).unwrap();
        let incr = t1.elapsed();

        println!("BUG-636 timing (N={N}): full_rebuild={full:?}  incremental_1commit={incr:?}");
        assert!(
            incr < full,
            "incremental ({incr:?}) should beat full rebuild ({full:?})"
        );
    }

    /// The diff helper classifies add / modify / delete over the objects tree.
    // trace:BUG-636
    #[test]
    fn changed_object_files_classifies_changes() {
        use crate::git_ops::ObjectChange;
        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_path = dir.path().join(".aida").join("cache.db");
        std::fs::create_dir_all(&store_root).unwrap();
        crate::git_ops::init(&store_root).unwrap();
        crate::git_ops::configure_user(&store_root, "Test", "test@example.com").unwrap();

        let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();
        backend
            .add_requirement(sample_req("FR-1-001", "a"))
            .unwrap();
        backend
            .add_requirement(sample_req("FR-1-002", "b"))
            .unwrap();
        let from = crate::git_ops::head_sha(&store_root).unwrap();

        // add FR-1-003, modify FR-1-001, delete FR-1-002.
        {
            let ext = GitBackend::new(&store_root).unwrap();
            ext.add_requirement(sample_req("FR-1-003", "c")).unwrap();
            let mut r = ext.get_requirement_by_spec_id("FR-1-001").unwrap().unwrap();
            r.title = "a2".into();
            ext.update_requirement(&r).unwrap();
            let d = ext.get_requirement_by_spec_id("FR-1-002").unwrap().unwrap();
            ext.delete_requirement(&d.id).unwrap();
        }
        let to = crate::git_ops::head_sha(&store_root).unwrap();

        let changes = crate::git_ops::changed_object_files(&store_root, &from, &to).unwrap();
        let kind_for = |spec: &str| -> Option<ObjectChange> {
            changes
                .iter()
                .find(|(_, p)| p.file_stem().and_then(|s| s.to_str()) == Some(spec))
                .map(|(k, _)| *k)
        };
        assert_eq!(kind_for("FR-1-003"), Some(ObjectChange::Added));
        assert_eq!(kind_for("FR-1-001"), Some(ObjectChange::Modified));
        assert_eq!(kind_for("FR-1-002"), Some(ObjectChange::Deleted));
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
