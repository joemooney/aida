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

use super::cache::{
    is_cache_schema_drift_error, ArchiveFilter, Cache, CacheTx, DeferFilter, ListFilter,
    RequirementSummary,
};
use super::git_backend::GitBackend;
use super::traits::{BackendType, DatabaseBackend, UpdateResult};
use crate::models::{QueueEntry, Requirement, RequirementsStore, User};

pub struct CachedGitBackend {
    inner: GitBackend,
    cache: Cache,
}

impl CachedGitBackend {
    /// Open an existing git store at `git_root` with a SQLite cache at
    /// `cache_path`. Missing caches are rebuilt; compatible stale caches use
    /// the single-flight read policy. Queries record their snapshot labels.
    pub fn open(git_root: &Path, cache_path: &Path) -> Result<Self> {
        let inner = GitBackend::new(git_root)?;
        Self::with_inner(inner, cache_path)
    }

    /// Wrap an already-configured GitBackend (e.g., one that was built with
    /// `.with_dispenser(...)`). The cache is opened or created at
    /// `cache_path` and brought through the single-flight reader protocol.
    pub fn with_inner(inner: GitBackend, cache_path: &Path) -> Result<Self> {
        let cache = Cache::open(cache_path)?;
        let backend = CachedGitBackend { inner, cache };
        // trace:TASK-1526 | ai:codex
        // Constructors share the tolerant protocol; mutations recheck strictly.
        backend.ensure_cache_fresh_for_read_with_schema_retry()?;
        Ok(backend)
    }

    /// Open the last committed cache snapshot without refreshing it.
    ///
    /// This is reserved for tightly bounded advisory paths such as
    /// `aida awaiting --notice`: refreshing a stale cache may require a full
    /// canonical-store scan, which violates those paths' latency contract.
    /// Callers must validate any cache-located record against the authoritative
    /// targeted YAML read and surface cache misses/errors when the snapshot is
    /// stale.
    // trace:BUG-1569 | ai:codex
    pub fn with_inner_cache_snapshot(inner: GitBackend, cache_path: &Path) -> Result<Self> {
        let cache = Cache::open(cache_path)?;
        Ok(CachedGitBackend { inner, cache })
    }

    /// Whether the unrefreshed cache snapshot differs from canonical HEAD.
    // trace:BUG-1569 | ai:codex
    pub fn cache_snapshot_is_stale(&self) -> Result<bool> {
        self.cache.is_stale(&self.current_head_sha())
    }

    /// Default cache location for a project's git store at `git_root`:
    /// `<project_root>/.aida/cache.db`. We never put the cache *inside* the
    /// store directory — that would pollute the orphan branch's worktree —
    /// so the probe starts at `git_root.parent()` and walks up.
    pub fn default_cache_path(git_root: &Path) -> PathBuf {
        Self::default_cache_path_with_roots(git_root, &crate::store_locate::real_temp_roots())
    }

    /// [`default_cache_path`], parameterized on the temp roots to guard
    /// against. Factored out so a test can plant an ambient `.aida` under a
    /// FAKE temp root (a plain tempdir standing in for "a temp root") and
    /// assert the walk-up refuses to adopt it — without mutating `TMPDIR` or
    /// touching the real, shared system temp dir.
    // trace:BUG-1598 | ai:claude
    fn default_cache_path_with_roots(git_root: &Path, temp_roots: &[PathBuf]) -> PathBuf {
        // Canonicalize the root set ONCE, before the loop — not on every
        // ancestor level.
        let canonical_roots = crate::store_locate::canonicalize_roots(temp_roots);
        let mut probe = match git_root.parent() {
            Some(p) => p.to_path_buf(),
            None => return git_root.with_extension("cache.db"),
        };
        for _ in 0..6 {
            // BUG-1598: never adopt a temp root itself as the project root.
            // A store rooted one or two levels under a temp dir (a
            // `mktemp -d`/`tempfile::tempdir()` fixture with no `.aida` of
            // its own) would otherwise walk straight into it and, if
            // anything else on the machine ever left a `.aida/` sitting
            // there, silently share/corrupt that cache instead of falling
            // through to its own sibling file below.
            if crate::store_locate::is_in_canonical_roots(&probe, &canonical_roots) {
                break;
            }
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
        self.with_cache_schema_retry("scan spec-id collisions", || {
            self.ensure_cache_fresh()?;
            self.cache.spec_id_collisions()
        })
    }

    /// Every object `id` resolves to (native `spec_id` or `agreed_id`), in the
    /// deterministic resolution order of `id_collisions::order_candidates`.
    /// Two or more entries = an ambiguous id; callers warn rather than let a
    /// silent pick stand. Uses the read-freshness check, so it stays as cheap
    /// as the lookup it guards.
    // trace:BUG-1535 | ai:claude
    pub fn id_candidates(&self, id: &str) -> Result<Vec<crate::id_collisions::IdCandidate>> {
        self.with_cache_schema_retry("resolve id candidates", || {
            self.tolerant_read(|cache| {
                let rows = cache.id_rows_for(id)?;
                Ok(crate::id_collisions::candidates_for_id(rows.iter(), id))
            })
        })
    }

    /// [`Self::id_candidates`] for a caller that acts on the answer: the cache
    /// is always brought to the store's HEAD first, even while another process
    /// holds the cache write lock, so a spec committed by another writer since
    /// the last refresh is among the candidates.
    // trace:BUG-1670 | ai:claude
    pub fn id_candidates_strict(&self, id: &str) -> Result<Vec<crate::id_collisions::IdCandidate>> {
        self.with_cache_schema_retry("resolve id candidates", || {
            self.ensure_cache_fresh()?;
            let rows = self.cache.id_rows_for(id)?;
            Ok(crate::id_collisions::candidates_for_id(rows.iter(), id))
        })
    }

    /// Resolve `id` for a WRITE — or any caller that must not act on a guess.
    /// A UUID resolves directly. A spec/agreed id naming more than one
    /// requirement returns an [`AmbiguousIdError`] (downcastable from the
    /// `anyhow::Error`) listing each candidate's unambiguous handle; exactly
    /// one candidate resolves as `get_requirement_by_spec_id` would.
    ///
    /// The candidate scan always refreshes a stale cache first (never serves
    /// the last snapshot because another process is writing the cache): a
    /// snapshot that misses a colliding spec another writer committed would
    /// report one candidate and let the write pick it silently. Read-only
    /// callers use [`Self::get_requirement_unambiguous_for_read`].
    ///
    /// If the cache cannot answer, the check falls back to the authoritative
    /// full scan rather than skipping it: an unchecked write is the bug.
    ///
    /// [`AmbiguousIdError`]: crate::id_collisions::AmbiguousIdError
    // trace:BUG-1535 | ai:claude
    pub fn get_requirement_unambiguous(&self, id: &str) -> Result<Option<Requirement>> {
        // trace:BUG-1670 | ai:claude
        self.resolve_unambiguous(id, true)
    }

    /// [`Self::get_requirement_unambiguous`] for a READ-ONLY caller (`aida
    /// show` and friends): the candidate scan may serve the last committed
    /// snapshot while another process holds the cache write lock, so the read
    /// stays as cheap as the lookup it guards. Never use it before a write.
    // trace:BUG-1670 | ai:claude
    pub fn get_requirement_unambiguous_for_read(&self, id: &str) -> Result<Option<Requirement>> {
        self.resolve_unambiguous(id, false)
    }

    // trace:BUG-1535 trace:BUG-1670 | ai:claude
    fn resolve_unambiguous(&self, id: &str, strict: bool) -> Result<Option<Requirement>> {
        if let Ok(uuid) = Uuid::parse_str(id.trim()) {
            return self.get_requirement(&uuid);
        }
        let scanned = if strict {
            self.id_candidates_strict(id)
        } else {
            self.id_candidates(id)
        };
        let candidates = match scanned {
            Ok(c) => c,
            Err(_) => {
                let store = self.inner.load()?;
                let rows: Vec<crate::id_collisions::IdRow> = store
                    .requirements
                    .iter()
                    .map(crate::id_collisions::IdRow::from)
                    .collect();
                crate::id_collisions::candidates_for_id(rows.iter(), id)
            }
        };
        if candidates.len() > 1 {
            // The cache said "ambiguous"; confirm against the canonical
            // objects before refusing, so a stale cache row (an object
            // rewritten under a new uuid, a deleted spec) never blocks a
            // write. One targeted read per candidate. The survivors are
            // the objects that, on disk, still answer to `id`.
            let objects_root = self.inner.path().join("objects");
            let mut seen_files = std::collections::HashSet::new();
            let mut rows: Vec<crate::id_collisions::IdRow> = Vec::new();
            for c in &candidates {
                let Some(file) = c
                    .spec_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    continue;
                };
                if !seen_files.insert(file.to_ascii_uppercase()) {
                    continue;
                }
                match crate::object_store::read_object(&objects_root, file) {
                    Ok(req) => rows.push(crate::id_collisions::IdRow::from(&req)),
                    // A file that is gone is a stale cache row: drop it.
                    Err(_)
                        if !crate::object_store::object_exists(&objects_root, file)
                            .unwrap_or(true) => {}
                    // PRIN-5: a file that EXISTS but cannot be read or parsed
                    // is absent evidence, not good evidence. It still counts
                    // as a candidate, so the write refuses instead of
                    // slipping through on the one readable object.
                    // trace:TASK-1468 | ai:claude
                    Err(_) => rows.push(crate::id_collisions::IdRow::from(c)),
                }
            }
            let confirmed = crate::id_collisions::candidates_for_id(rows.iter(), id);
            if confirmed.len() > 1 {
                return Err(crate::id_collisions::resolve_candidates(id, confirmed)
                    .unwrap_err()
                    .into());
            }
        }
        self.get_requirement_by_spec_id(id)
    }

    /// Every ambiguous id in the store, from the cache (one full-table read,
    /// grouped in Rust — no YAML load). Backs the `aida list` relabel.
    // trace:BUG-1535 | ai:claude
    pub fn id_collisions(&self) -> Result<Vec<crate::id_collisions::IdCollision>> {
        self.with_cache_schema_retry("scan id collisions", || {
            self.tolerant_read(|cache| {
                let rows = cache.id_rows_all()?;
                Ok(crate::id_collisions::find_id_collisions(rows.iter()))
            })
        })
    }

    /// Read the current git HEAD on the store branch. Empty string if not in a
    /// git repo (e.g., test fixture); stale check then collapses to "always
    /// fresh" which is fine for non-git scenarios.
    fn current_head_sha(&self) -> String {
        crate::git_ops::head_sha(self.inner.path()).unwrap_or_default()
    }

    /// Resolve one UUID using the cache as a locator and canonical YAML as truth.
    // trace:BUG-634 | ai:claude
    // trace:BUG-1678 | ai:codex
    fn get_requirement_targeted(&self, id: &Uuid) -> Result<Option<Requirement>> {
        self.requirement_reader()(id)
    }

    /// Create a reader for one read operation (for example, rendering a spec's
    /// relationships and blockers). Cache misses share a lazy UUID-to-path index,
    /// so even an empty or unreadable cache costs at most one full-store parse.
    /// Each result is read from canonical YAML and its UUID checked. Create a new
    /// reader after writes: the fallback file inventory is scoped to this reader.
    // trace:BUG-1678 | ai:codex
    pub fn requirement_reader(&self) -> impl Fn(&Uuid) -> Result<Option<Requirement>> + '_ {
        use std::collections::HashMap;
        use std::sync::OnceLock;

        let paths: OnceLock<Result<HashMap<Uuid, PathBuf>>> = OnceLock::new();
        let objects_root = self.inner.path().join("objects");
        move |id| {
            if let Ok(Some(spec_id)) = self.cache.spec_id_for_uuid(id) {
                // A missing filename must not trigger the agreed-id full scan.
                if let Ok(req) = crate::object_store::read_object(&objects_root, &spec_id) {
                    if req.id == *id {
                        return Ok(Some(req));
                    }
                }
            }
            let indexed = paths.get_or_init(|| {
                let mut indexed = HashMap::new();
                for (_, path) in crate::object_store::list_objects(&objects_root)? {
                    // Match find_by_uuid's tolerance for unreadable objects and
                    // its deterministic first-match behavior for duplicate UUIDs.
                    if let Ok(req) = crate::object_store::read_object_from_path(&path) {
                        indexed.entry(req.id).or_insert(path);
                    }
                }
                Ok(indexed)
            });
            let indexed = indexed.as_ref().map_err(|e| anyhow::anyhow!("{e:#}"))?;
            Ok(indexed.get(id).and_then(|path| {
                crate::object_store::read_object_from_path(path)
                    .ok()
                    .filter(|req| req.id == *id)
            }))
        }
    }

    /// The STORED status of `id`, from one targeted object read: the cache maps
    /// the uuid to its spec_id and only that YAML is read (the read is
    /// authoritative; the uuid is re-checked against it). `None` on any miss.
    /// Deliberately has NO full-store fallback, unlike
    /// [`Self::get_requirement_targeted`]: it backs the epic-rollup refresh on
    /// the read path, where an O(n) scan is exactly the unbounded cost
    /// BUG-1606 removed.
    // trace:BUG-1606 | ai:claude
    fn stored_status_targeted(&self, id: &Uuid) -> Option<crate::models::RequirementStatus> {
        self.stored_status_for_spec_id(id, self.cache.spec_id_for_uuid(id).ok().flatten())
    }

    /// The object-read half of [`Self::stored_status_targeted`], for a
    /// spec_id the caller already resolved (from the cache, or from the open
    /// incremental-refresh transaction).
    // trace:BUG-1606 trace:TASK-1515 | ai:claude
    fn stored_status_for_spec_id(
        &self,
        id: &Uuid,
        spec_id: Option<String>,
    ) -> Option<crate::models::RequirementStatus> {
        #[cfg(test)]
        super::cache_refresh::test_count(if super::cache::holding_write() {
            "epic_inside_txn"
        } else {
            "epic_before_txn"
        });
        let spec_id = spec_id?;
        // Read exactly that object file. Not `get_requirement_by_spec_id`: on
        // a missing file it falls through to an agreed_id scan of every object.
        let objects_root = self.inner.path().join("objects");
        let req = crate::object_store::read_object(&objects_root, &spec_id).ok()?;
        (req.id == *id).then_some(req.status)
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
        // trace:BUG-1752 | ai:codex
        if self.cache.is_read_only() {
            return Ok(());
        }
        // TASK-1515: an incremental refresh declines when HEAD moves while it
        // reads the changed objects (it cannot stamp one HEAD on rows that may
        // come from a later one). On a busy store that is common, so retry the
        // cheap incremental from the newly captured HEAD a bounded number of
        // times before paying for a full rebuild.
        // trace:TASK-1515 | ai:claude
        const INCREMENTAL_ATTEMPTS: usize = 3;
        let mut head = self.current_head_sha();
        for _ in 0..INCREMENTAL_ATTEMPTS {
            if !self.cache.is_stale(&head)? {
                return Ok(());
            }
            // Non-git fixture (empty HEAD): nothing to diff — full rebuild is
            // the only correct path (and is cheap, there are no commits).
            if head.is_empty() {
                break;
            }
            let Some(recorded) = self.cache.source_head_sha()? else {
                break;
            };
            if recorded.is_empty()
                || !crate::git_ops::is_ancestor(self.inner.path(), &recorded, &head)
                    .unwrap_or(false)
            {
                break;
            }
            match self.try_incremental_update(&recorded, &head) {
                Ok(true) => return Ok(()),
                // Ok(false): incremental declined. If HEAD moved during the
                // reads, retry from the new HEAD; otherwise (diff too large /
                // a row the diff named couldn't be read) fall through to a
                // full rebuild, which is always correct.
                Ok(false) => {
                    let now = self.current_head_sha();
                    if now == head {
                        break;
                    }
                    head = now;
                }
                // trace:TASK-1526 | ai:codex
                Err(e) if super::cache::is_cache_lock_error(&e) => return Err(e),
                // Other errors: fall back to the
                // full rebuild, which re-enters the lock retry ladder and
                // fails the command if the lock is still held.
                Err(e) => {
                    eprintln!("warning: incremental cache update failed ({e}); full rebuild");
                    break;
                }
            }
        }
        // TASK-1515: re-check staleness at a freshly captured HEAD before
        // paying for the rebuild. The rebuild derives its own stamp HEAD,
        // verified unchanged across its store load (`load_at_stable_head`),
        // so a commit landing mid-load can no longer strand a ghost or
        // reverted row that the incremental diff from an older stamp would
        // never see again.
        // trace:TASK-1515 | ai:claude
        // trace:BUG-1663 | ai:claude
        let head = self.current_head_sha();
        if !self.cache.is_stale(&head)? {
            return Ok(());
        }
        self.full_rebuild(true)
    }

    /// A reader that cannot refresh inline files the durable request and
    /// spawns the detached worker (STORY-1484 slice C). Returns `None` when
    /// the cache was brought current on the spot (`AIDA_CACHE_NO_DETACH`, or
    /// amendment A7's crash-loop fallback to the strict inline path), else
    /// the label to serve: `Requested` after a successful spawn, `Deferred`
    /// when the spawn itself failed (recorded, so backoff still engages).
    // trace:TASK-1527 | ai:claude
    fn file_and_spawn_refresh(&self) -> Result<Option<super::cache_refresh::RefreshState>> {
        use super::cache_refresh::RefreshState;
        use super::{refresh_request, refresh_worker};
        if refresh_worker::no_detach() {
            self.ensure_cache_fresh()?;
            return Ok(None);
        }
        let cache_path = self.cache.path();
        let head = self.current_head_sha();
        // trace:BUG-1778 | ai:antigravity
        let request = match refresh_request::file_request(cache_path, &head) {
            Ok(req) => req,
            Err(e) => {
                eprintln!("warning: failed to write refresh hint: {e}");
                return Ok(Some(RefreshState::Deferred));
            }
        };
        if request.spawning_suppressed(chrono::Utc::now()) {
            self.ensure_cache_fresh()?;
            return Ok(None);
        }
        match refresh_worker::spawn_detached_worker(self.inner.path(), cache_path) {
            Ok(_) => Ok(Some(RefreshState::Requested)),
            Err(e) => {
                if let Err(e2) = refresh_request::record_failed_attempt(
                    cache_path,
                    &format!("spawn failed: {e}"),
                ) {
                    eprintln!("warning: failed to record failed refresh attempt: {e2}");
                }
                Ok(Some(RefreshState::Deferred))
            }
        }
    }

    /// Single-flight reader protocol. Sidecars are diagnostics only.
    // trace:TASK-1526 | ai:codex
    pub fn freshen_for_read(
        &self,
        budget: super::cache_refresh::ReadBudget,
    ) -> Result<Option<super::cache_refresh::RefreshState>> {
        use super::cache_refresh::{self, RefreshLock, RefreshState};
        use std::time::Duration;
        loop {
            let head = self.current_head_sha();
            let migration = self.cache.recheck_migration();
            // trace:BUG-1752 | ai:codex
            // A read-only cache must serve its committed projection without rebuilding.
            if self.cache.is_read_only() && self.cache.has_usable_snapshot() {
                return Ok(None);
            }
            if migration && budget.0.is_zero() {
                return Err(cache_refresh::AdvisoryCacheUnavailable.into());
            }
            if !migration && !self.cache.is_stale(&head)? {
                return Ok(None);
            }
            match RefreshLock::try_acquire(self.cache.path()) {
                Err(_) => {
                    self.ensure_cache_fresh()?;
                    return Ok(None);
                }
                Ok(Some(guard)) => {
                    if guard.is_nested() {
                        return Ok(None);
                    }
                    // The lock winner always checks again after acquiring it.
                    if !self.cache.is_stale(&self.current_head_sha())? {
                        return Ok(None);
                    }
                    if migration || !self.cache.has_usable_snapshot() {
                        self.ensure_cache_fresh()?;
                        return Ok(None);
                    }
                    let head = self.current_head_sha();
                    let recorded = self.cache.source_head_sha()?;
                    if let Some(from) = recorded.filter(|s| !s.is_empty()) {
                        if !head.is_empty()
                            && crate::git_ops::is_ancestor(self.inner.path(), &from, &head)?
                        {
                            let attempt = super::cache::ReadRefreshAttempt::new();
                            let result = self.try_incremental_update(&from, &head);
                            drop(attempt);
                            match result {
                                Ok(true) => return Ok(None),
                                Ok(false) => {}
                                Err(e) if super::cache::is_cache_lock_error(&e) => {
                                    drop(guard);
                                    return Ok(if self.cache.is_stale(&self.current_head_sha())? {
                                        Some(RefreshState::WriterBusy)
                                    } else {
                                        None
                                    });
                                }
                                Err(e) => return Err(e),
                            }
                        }
                    }
                    if cache_refresh::read_policy().0 && !budget.0.is_zero() {
                        // One bounded attempt only: the pre-C inline rebuild a
                        // TTY reader performs must not park in the SQLite
                        // retry ladder behind a live writer (~25 s, then a
                        // hard error — past ADR-53's 5 s interaction
                        // ceiling). On write-lock contention this winner
                        // degrades to the same reader protocol as every
                        // other caller: file the durable request, spawn the
                        // detached worker, and serve the committed snapshot
                        // with a stale label. A free lock keeps the inline
                        // rebuild (and its fresh result) unchanged.
                        // trace:BUG-1674 | ai:claude
                        let attempt = super::cache::ReadRefreshAttempt::new();
                        let result = self.ensure_cache_fresh();
                        drop(attempt);
                        match result {
                            Ok(()) => return Ok(None),
                            Err(e) if !super::cache::is_cache_lock_error(&e) => return Err(e),
                            Err(_) => return self.file_and_spawn_refresh(),
                        }
                    }
                    // The request is filed while the flock is still held, so
                    // no reader ever observes a free lock with no pending
                    // request; the worker's acquire patience covers the gap
                    // until this guard drops at return (TASK-1527).
                    // trace:TASK-1527 | ai:claude
                    return self.file_and_spawn_refresh();
                }
                Ok(None) => {
                    let limit = if migration && !budget.0.is_zero() {
                        cache_refresh::migration_wait_limit()
                    } else {
                        budget.0
                    };
                    let remaining = cache_refresh::wait_remaining(limit);
                    if remaining.is_zero() || !cache_refresh::may_wait() {
                        if migration {
                            if budget.0.is_zero() {
                                return Err(cache_refresh::AdvisoryCacheUnavailable.into());
                            }
                            anyhow::bail!("the cache is being upgraded; re-run shortly");
                        }
                        // Final lock observation: never claim a departed holder.
                        let state = match RefreshLock::try_acquire(self.cache.path()) {
                            Ok(None) => RefreshState::WorkerRunning,
                            Ok(Some(_guard)) => {
                                // The holder departed without finishing. Hand
                                // the work to the detached worker rather than
                                // inheriting it past the read budget.
                                // trace:TASK-1527 | ai:claude
                                if !self.cache.is_stale(&self.current_head_sha())? {
                                    return Ok(None);
                                }
                                match self.file_and_spawn_refresh()? {
                                    None => return Ok(None),
                                    Some(state) => state,
                                }
                            }
                            Err(_) => {
                                self.ensure_cache_fresh()?;
                                return Ok(None);
                            }
                        };
                        return Ok(if self.cache.is_stale(&self.current_head_sha())? {
                            Some(state)
                        } else {
                            None
                        });
                    }
                    cache_refresh::wait_poll(remaining.min(Duration::from_millis(100)));
                    // A departed holder does not transfer its unfinished work to
                    // this loser or restart the wait with another backend; the
                    // detached worker inherits it instead (TASK-1527).
                    match RefreshLock::try_acquire(self.cache.path()) {
                        Ok(Some(_guard)) if !migration => {
                            if !self.cache.is_stale(&self.current_head_sha())? {
                                return Ok(None);
                            }
                            // trace:TASK-1527 | ai:claude
                            return self.file_and_spawn_refresh();
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Metadata and rows are read through the SAME pinned WAL snapshot.
    // trace:TASK-1526 | ai:codex
    fn tolerant_read<T>(&self, read: impl FnMut(&Cache) -> Result<T>) -> Result<T> {
        self.tolerant_read_with_budget(super::cache_refresh::read_policy().1, read)
    }

    // trace:TASK-1526 | ai:codex
    fn tolerant_read_with_budget<T>(
        &self,
        budget: super::cache_refresh::ReadBudget,
        mut read: impl FnMut(&Cache) -> Result<T>,
    ) -> Result<T> {
        use super::cache_refresh::{self, StaleServe};
        let mut schema_retries = 0;
        // A busy store can keep moving HEAD between refresh and pin. Wait only
        // for the normal read budget, then serve the pinned rows with a stale
        // label rather than failing a read.
        let repin_deadline = std::time::Instant::now() + budget.0;
        loop {
            let mut state = self.freshen_for_read(budget)?;
            let head = self.current_head_sha();
            let snapshot = match self.cache.read_snapshot() {
                Ok(snapshot) => snapshot,
                Err(err) if err.is::<super::cache::CacheSchemaChanged>() && schema_retries == 0 => {
                    // Re-enter the migration protocol, including its silent
                    // advisory exit and no-wait-under-write-lock rule.
                    schema_retries += 1;
                    continue;
                }
                Err(err) => return Err(err),
            };
            let cache_head = snapshot.source_head_sha()?;
            let built_at = snapshot.built_at()?;
            // HEAD may move between refresh and pinning. Re-enter the same
            // bounded operation, never attach stale:false to an older snapshot.
            if state.is_none()
                && cache_head.as_deref() != Some(head.as_str())
                && !cache_refresh::RefreshLock::held_by_current_thread(self.cache.path())
            {
                if self.cache.is_read_only() {
                    anyhow::bail!(super::cache::cache_read_only_guidance(self.cache.path()));
                }
                if std::time::Instant::now() >= repin_deadline {
                    state = Some(cache_refresh::RefreshState::Deferred);
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
            }
            let value = read(&snapshot)?;
            let state = if state == Some(cache_refresh::RefreshState::WorkerRunning) {
                match cache_refresh::RefreshLock::try_acquire(self.cache.path()) {
                    Ok(None) => state,
                    Ok(Some(_)) => Some(cache_refresh::RefreshState::Deferred),
                    Err(_) => {
                        drop(snapshot);
                        self.ensure_cache_fresh()?;
                        continue;
                    }
                }
            } else {
                state
            };
            let stale = state
                .filter(|_| cache_head.as_deref() != Some(head.as_str()))
                .map(|refreshing| StaleServe {
                    stale: true,
                    cache_head: cache_head.filter(|head| !head.is_empty()),
                    store_head: head,
                    built_at: built_at.filter(|time| !time.is_empty()),
                    refreshing,
                });
            cache_refresh::record_read(stale);
            return Ok(value);
        }
    }

    // trace:TASK-1526 | ai:codex
    fn ensure_cache_fresh_for_read(&self) -> Result<()> {
        // Opening a backend has not served any rows. Labels belong to the
        // pinned query snapshot, not an earlier constructor observation.
        self.freshen_for_read(super::cache_refresh::read_policy().1)
            .map(|_| ())
    }

    /// Load the whole store for a rebuild, returning the HEAD the rebuild
    /// must stamp: one verified unchanged across the load.
    ///
    /// The load reads the live tree, so a commit landing mid-load leaves the
    /// rows newer than a HEAD captured before it — and the one stamp that is
    /// provably consistent with the rows is a pre-load HEAD that equals the
    /// post-load HEAD. Stamping the post-load HEAD alone would be wrong in
    /// the other direction: rows read BEFORE a mid-load commit would carry
    /// its label and never be re-read. So on a mid-load HEAD move, retry the
    /// load (bounded, like TASK-1515's incremental retries).
    ///
    /// When every bounded attempt races a commit, give up and stamp the
    /// final attempt's PRE-load HEAD: older than (or equal to) everything
    /// that load read, so the next refresh re-reads whatever changed since
    /// it. The one shape that diff cannot see — a file changed during the
    /// load and reverted (or deleted) by a later commit — now needs the tree
    /// to change during every attempt, instead of once.
    // trace:BUG-1663 | ai:claude
    fn load_at_stable_head(&self) -> Result<(RequirementsStore, String)> {
        const STABLE_LOAD_ATTEMPTS: usize = 3;
        let mut head = self.current_head_sha();
        for attempt in 1..=STABLE_LOAD_ATTEMPTS {
            #[cfg(test)]
            tests::bug_1663_during_rebuild_load();
            let store = self
                .inner
                .load()
                .context("Failed to load git store for cache rebuild")?;
            let after = self.current_head_sha();
            if after == head || attempt == STABLE_LOAD_ATTEMPTS {
                return Ok((store, head));
            }
            head = after;
        }
        unreachable!("the final attempt returns above")
    }

    /// Full authoritative rebuild: load the whole store and re-project every
    /// row. The fallback whenever incremental can't be proven safe. The
    /// stamped HEAD is derived here, verified stable across the load.
    // trace:BUG-636
    // trace:BUG-1663 | ai:claude
    fn full_rebuild(&self, refresh_only: bool) -> Result<()> {
        #[cfg(test)]
        super::cache_refresh::test_count("full_rebuild");
        let (store, head) = self.load_at_stable_head()?;
        // Save-time reconciliation must rebuild even at the same HEAD: it
        // may have preserved unloaded objects, including uncommitted fixtures.
        // Only freshness-driven refreshes may skip an already committed refill.
        // trace:TASK-1526 | ai:codex
        if refresh_only {
            self.cache
                .rebuild_for_refresh(&store, &head, self.inner.path())?;
        } else {
            self.cache.rebuild_from_store(&store, &head)?;
        }
        Ok(())
    }

    /// Drop/recreate the cache projection and rebuild it from the authoritative
    /// git store after a cache operation discovers schema drift.
    // trace:BUG-1097 | ai:codex
    fn rebuild_after_cache_schema_drift(&self) -> Result<()> {
        // trace:BUG-1663 | ai:claude
        let (store, head) = self.load_at_stable_head()?;
        self.cache.rebuild_from_store_after_schema_drift(&store, &head)
            .map_err(|err| {
                if super::cache::is_cache_read_only_error(&err) || super::cache::is_cache_unwritable_error(&err) {
                    anyhow::anyhow!(
                        "the AIDA cache at {} is not writable, so it cannot be migrated or rebuilt (reads still work). This usually means a sandboxed agent is running in a linked worktree whose cache is symlinked into the main checkout. Grant write access to the main checkout's .aida directory (codex: --add-dir <main>/.aida) or run the command from the main checkout: {err}",
                        self.cache.path().display()
                    )
                } else { err }
            })?;
        Ok(())
    }

    /// Run one cache operation; if it fails with a missing-column/table schema
    /// drift error, rebuild the projection once in-process and retry.
    // trace:BUG-1097 | ai:codex
    fn with_cache_schema_retry<T, F>(&self, action: &str, mut f: F) -> Result<T>
    where
        F: FnMut() -> Result<T>,
    {
        match f() {
            Ok(value) => Ok(value),
            Err(err) if is_cache_schema_drift_error(&err) => {
                eprintln!(
                    "warning: cache schema drift while trying to {action}; rebuilding cache and retrying once: {err}"
                );
                self.rebuild_after_cache_schema_drift()?;
                f()
            }
            Err(err) => Err(err),
        }
    }

    // trace:BUG-1097 | ai:codex
    pub fn ensure_cache_fresh_with_schema_retry(&self) -> Result<()> {
        self.with_cache_schema_retry("freshen cache", || self.ensure_cache_fresh())
    }

    // trace:BUG-1097 | ai:codex
    fn ensure_cache_fresh_for_read_with_schema_retry(&self) -> Result<()> {
        self.with_cache_schema_retry("freshen cache for read", || {
            self.ensure_cache_fresh_for_read()
        })
    }

    // trace:BUG-1097 | ai:codex
    fn upsert_requirement_with_schema_retry(&self, req: &Requirement) -> Result<()> {
        self.with_cache_schema_retry("upsert cached requirement", || {
            self.cache.upsert_requirement(req)
        })
    }

    // trace:BUG-1097 | ai:codex
    fn delete_requirement_with_schema_retry(&self, id: &Uuid) -> Result<()> {
        self.with_cache_schema_retry("delete cached requirement", || {
            self.cache.delete_requirement(id)
        })
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
        #[cfg(test)]
        super::cache_refresh::test_count("incremental");
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
        // TASK-1515: read every changed object BEFORE taking the cache write
        // lock (the YAML reads are the slow part), then apply all the rows and
        // the head-SHA stamp in ONE transaction. Previously each row committed
        // on its own and the SHA was stamped last, so a concurrent reader could
        // see rows from `to` labelled `from`, and a decline part-way through
        // left the rows it had already committed behind.
        // trace:TASK-1515 | ai:claude
        enum Step {
            Upsert(Box<Requirement>),
            Delete(String),
        }
        let mut steps = Vec::with_capacity(changes.len());
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
                        Some(req) => steps.push(Step::Upsert(Box::new(req))),
                        None => {
                            // The diff says present at `to` but the worktree can't
                            // read it (HEAD moved underneath us, or a torn state).
                            // Don't guess — full rebuild. trace:BUG-636
                            return Ok(false);
                        }
                    }
                }
                ObjectChange::Deleted => steps.push(Step::Delete(spec_id.to_string())),
            }
        }
        // Resolve candidate ancestors before BEGIN IMMEDIATE. Newly introduced
        // hierarchy edges can still reveal an ancestor inside the transaction;
        // those misses retain the authoritative targeted read.
        // trace:TASK-1526 | ai:codex
        let mut epic_statuses = std::collections::HashMap::new();
        for step in &steps {
            if let Step::Upsert(req) = step {
                for id in self.cache.ancestor_epic_ids(&req.id)? {
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        epic_statuses.entry(id)
                    {
                        let spec_id = self.cache.spec_id_for_uuid(&id)?;
                        entry.insert(self.stored_status_for_spec_id(&id, spec_id));
                    }
                }
            }
        }
        // TASK-1515: the reads above came from the live worktree. If HEAD
        // moved while they ran, some rows may be from a later HEAD than `to`
        // and stamping `to` would mislabel them. Decline; the caller retries
        // the incremental from the new HEAD (bounded), then full-rebuilds at
        // a freshly captured HEAD.
        // trace:TASK-1515 | ai:claude
        #[cfg(test)]
        tests::task_1515_after_incremental_reads();
        #[cfg(test)]
        if super::cache_refresh::INCREMENTAL_ERROR.with(|c| c.get()) {
            anyhow::bail!("injected incremental failure");
        }
        if self.current_head_sha() != to {
            return Ok(false);
        }
        self.cache.apply_incremental(to, self.inner.path(), |tx| {
            for step in &steps {
                match step {
                    Step::Upsert(req) => {
                        tx.upsert_requirement(req)?;
                        // BUG-626 parity with the write path: a child's status
                        // / hierarchy change shifts its parent epic's rollup.
                        // BUG-1606: an ancestor epic the targeted read cannot
                        // resolve is a cache/store disagreement. Don't guess:
                        // decline (rolling back every row), and the caller does
                        // a full rebuild.
                        // trace:BUG-1606 | ai:claude
                        if !self.refresh_parent_epic_status_in(tx, req, &epic_statuses) {
                            return Ok(false);
                        }
                    }
                    Step::Delete(spec_id) => {
                        // Resolve the spec_id (from the path) to its uuid via the
                        // cache and drop the row. Absent already → harmless no-op.
                        if let Some(uuid) = tx.uuid_for_spec_id(spec_id)? {
                            tx.delete_requirement(&uuid)?;
                        }
                    }
                }
            }
            // Every changed row refreshed — `apply_incremental` stamps `to`
            // and commits it all together.
            Ok(true)
        })
    }

    /// [`Self::refresh_parent_epic_status`] inside the incremental refresh
    /// transaction: same ancestor walk, same targeted stored-status read, same
    /// fail-closed `false` on a miss, but every cache read and write goes
    /// through `tx` (the cache connection is held by the open transaction).
    ///
    /// Candidate epic statuses are resolved before the transaction. Only
    /// ancestors introduced by the incoming hierarchy need a targeted read here.
    // trace:TASK-1515 trace:BUG-1606 | ai:claude
    #[must_use]
    fn refresh_parent_epic_status_in(
        &self,
        tx: &CacheTx<'_>,
        req: &Requirement,
        preresolved: &std::collections::HashMap<Uuid, Option<crate::models::RequirementStatus>>,
    ) -> bool {
        let Ok(ancestor_epics) = tx.ancestor_epic_ids(&req.id) else {
            return false;
        };
        let mut all_resolved = true;
        for epic_id in ancestor_epics {
            // trace:TASK-1526 | ai:codex
            let stored = preresolved.get(&epic_id).cloned().unwrap_or_else(|| {
                let spec_id = tx.spec_id_for_uuid(&epic_id).ok().flatten();
                self.stored_status_for_spec_id(&epic_id, spec_id)
            });
            let Some(stored) = stored else {
                all_resolved = false;
                continue;
            };
            let _ = tx.recompute_epic_status_from_hierarchy(&epic_id, &stored);
        }
        all_resolved
    }

    /// Count rows through the same labelled snapshot as other tolerant reads.
    // trace:TASK-1526 | ai:codex
    pub fn requirement_count(&self) -> Result<usize> {
        self.with_cache_schema_retry("count cached requirements", || {
            self.tolerant_read(|cache| cache.requirement_count())
        })
    }

    /// Read summaries with an explicit wait budget. Zero-budget advisory
    /// readers defer compatible full rebuilds and refuse pending migrations.
    // trace:TASK-1526 | ai:codex
    pub fn list_summaries_with_budget(
        &self,
        filter: &ListFilter,
        budget: super::cache_refresh::ReadBudget,
    ) -> Result<Vec<RequirementSummary>> {
        self.with_cache_schema_retry("read bounded cache summaries", || {
            self.tolerant_read_with_budget(budget, |cache| cache.list_summaries(filter))
        })
    }

    /// Cache-backed filter pushdown with invocation-scoped freshness labels.
    /// A compatible stale snapshot may be returned under the reader policy.
    // trace:TASK-1526 | ai:codex
    pub fn list_summaries(&self, filter: &ListFilter) -> Result<Vec<RequirementSummary>> {
        self.with_cache_schema_retry("list cached summaries", || {
            self.tolerant_read(|cache| cache.list_summaries(filter))
        })
    }

    /// [`Self::list_summaries`] for a caller that deletes, prunes, files or
    /// gates on the rows: a stale cache is always brought to the store's HEAD
    /// first. Tolerant reads may serve a labelled committed snapshot; this
    /// strict entry point must never be weakened to that policy.
    // trace:BUG-1670 | ai:claude
    pub fn list_summaries_strict(&self, filter: &ListFilter) -> Result<Vec<RequirementSummary>> {
        self.with_cache_schema_retry("list cached summaries", || {
            self.ensure_cache_fresh()?;
            self.cache.list_summaries(filter)
        })
    }

    /// TASK-1065: count non-archived specs with a still-pending DecisionRequest,
    /// read from the `has_pending_decision` cache column. Cache-backed so the
    /// `aida status --full` decision-inbox count no longer needs a full
    /// `backend.load()`. The invocation collector describes the committed
    /// cache snapshot used for this count.
    // trace:TASK-1065 | ai:claude
    pub fn pending_decision_count(&self) -> Result<usize> {
        self.with_cache_schema_retry("count pending decisions", || {
            self.tolerant_read(|cache| cache.pending_decision_count())
        })
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
        self.with_cache_schema_retry("read cached degrees", || {
            self.tolerant_read(|cache| cache.degrees_for_id(id))
        })
    }

    /// The transitive descendant id-set of `root` (root + all parent->child
    /// descendants, any depth), read from the cache's materialized hierarchy
    /// edges via one WITH RECURSIVE query. Triggers a stale-check first so the
    /// edge set reflects the latest committed graph. Backs
    /// `aida list --parent <id> --recursive`.
    // trace:TASK-955 | ai:claude
    pub fn descendant_ids(&self, root: &Uuid) -> Result<std::collections::HashSet<Uuid>> {
        self.with_cache_schema_retry("read cached descendants", || {
            self.tolerant_read(|cache| cache.descendant_ids(root))
        })
    }

    /// [`Self::descendant_ids`] for a write-path gate: a stale cache is always
    /// brought to the store's HEAD first, so a child added (or re-parented) by
    /// another writer since the last refresh is classified correctly.
    // trace:BUG-1670 | ai:claude
    pub fn descendant_ids_strict(&self, root: &Uuid) -> Result<std::collections::HashSet<Uuid>> {
        self.with_cache_schema_retry("read cached descendants", || {
            self.ensure_cache_fresh()?;
            self.cache.descendant_ids(root)
        })
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
        self.with_cache_schema_retry("search cache", || {
            self.tolerant_read(|cache| cache.search(query, limit, archive, defer))
        })
    }

    /// Force a full cache rebuild, regardless of staleness. Used by the
    /// `aida cache rebuild` CLI command.
    pub fn rebuild_cache(&self) -> Result<usize> {
        // trace:BUG-1663 | ai:claude
        let (store, head) = self.load_at_stable_head()?;
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
        // TASK-1515: while a schema migration is pending, `source_head_sha()`
        // reads `None`, which the freshness match below would take for "just
        // rebuilt" and stamp the post-write HEAD on rows that never ingested
        // external commits. Raw `cache_meta` readers (and older binaries) do
        // not see the pending flag, so keep the on-disk head empty (stale)
        // until the migration rebuild stamps it. `refresh_epics_then_restamp`
        // routes through here, so it is covered too.
        // trace:TASK-1515 | ai:claude
        if self.cache.migration_pending() {
            let _ = self.cache.set_source_head_sha("");
            return;
        }
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
    ///
    /// BUG-768: each ancestor epic's STORED status is read back from the store
    /// (not from its cache row, which already holds a derived override) and
    /// handed to the recompute, so a human's `--force` close survives every
    /// child write. An unreadable epic falls back to `Draft` — a non-terminal
    /// value, i.e. "derive normally", the pre-BUG-768 behavior.
    ///
    /// BUG-1606: the stored status is read with ONE targeted object read
    /// (cache uuid -> spec_id -> that YAML, uuid re-checked), never the inner
    /// backend's by-uuid lookup, which parses every object file until it finds
    /// the epic. That scan ran once per ancestor epic for every changed row the
    /// read-path incremental catch-up replayed, so a plain `aida show` after any
    /// store commit parsed ~2x the whole store; on a busy spinning disk with a
    /// cold page cache it took minutes.
    ///
    /// Fail-closed (PRIN-5): a targeted miss means the cache and the store
    /// disagree about an epic, so nothing is guessed. That epic is left
    /// unrecomputed and this returns `false`; callers then force a full rebuild
    /// (the incremental catch-up declines; write paths mark the cache stale).
    /// `true` means every ancestor epic was re-derived from its stored status.
    // trace:BUG-626 trace:BUG-764 trace:BUG-768 | ai:claude
    // trace:BUG-1606 | ai:claude
    #[must_use]
    fn refresh_parent_epic_status(&self, req: &Requirement) -> bool {
        let Ok(ancestor_epics) = self.cache.ancestor_epic_ids(&req.id) else {
            return false;
        };
        let mut all_resolved = true;
        for epic_id in ancestor_epics {
            let Some(stored) = self.stored_status_targeted(&epic_id) else {
                all_resolved = false;
                continue;
            };
            let _ = self
                .cache
                .recompute_epic_status_from_hierarchy(&epic_id, &stored);
        }
        all_resolved
    }

    /// Write paths: refresh the ancestor epics, then either re-stamp the cache
    /// HEAD (every epic resolved) or mark the cache stale so the next read does
    /// an authoritative full rebuild (a targeted epic read missed).
    // trace:BUG-1606 | ai:claude
    fn refresh_epics_then_restamp(&self, reqs: &[&Requirement], pre_write_head: &str) {
        // TASK-1515: a pending migration keeps the on-disk head empty; see
        // `restamp_head`. Skip the epic rollups too: the next read rebuilds.
        // trace:TASK-1515 | ai:claude
        if self.cache.migration_pending() {
            let _ = self.cache.set_source_head_sha("");
            return;
        }
        let mut all_resolved = true;
        for req in reqs {
            all_resolved &= self.refresh_parent_epic_status(req);
        }
        if all_resolved {
            self.restamp_head(pre_write_head);
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
            if let Err(e) = self.upsert_requirement_with_schema_retry(req) {
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
            let reqs: Vec<&Requirement> = requirements.iter().collect();
            self.refresh_epics_then_restamp(&reqs, &pre_write_head);
        } else {
            let _ = self.cache.set_source_head_sha("");
        }
        Ok(n)
    }

    /// Write-through batched compare-and-swap (see
    /// [`GitBackend::bulk_update_atomically`]): every eligibility decision is
    /// taken on the object read inside the store write lock, then the specs
    /// actually written are upserted into the cache and the HEAD-SHA re-stamped
    /// once.
    ///
    /// The cache upserts run AFTER the store lock is released, exactly as
    /// `bulk_update` does: the lock closes the re-check/write window and
    /// nothing else, so other readers do not also wait out the cache writes.
    // trace:BUG-1671 | ai:claude
    pub fn bulk_update_atomically<F>(
        &self,
        targets: &[Requirement],
        commit_subject: &str,
        keep: F,
    ) -> Result<super::git_backend::BulkAtomicReport>
    where
        F: FnMut(&mut Requirement) -> bool,
    {
        // Capture HEAD BEFORE the write so restamp_head can tell our own commit
        // apart from an external pull that moved HEAD underneath us.
        let pre_write_head = self.current_head_sha();
        let report = self
            .inner
            .bulk_update_atomically(targets, commit_subject, keep)?;
        let mut cache_ok = true;
        for req in &report.written {
            if let Err(e) = self.upsert_requirement_with_schema_retry(req) {
                eprintln!(
                    "warning: cache upsert failed during bulk_update_atomically, \
                     cache marked stale: {}",
                    e
                );
                cache_ok = false;
                break;
            }
        }
        if cache_ok {
            let reqs: Vec<&Requirement> = report.written.iter().collect();
            self.refresh_epics_then_restamp(&reqs, &pre_write_head);
        } else {
            let _ = self.cache.set_source_head_sha("");
        }
        Ok(report)
    }

    /// Per-spec compare-and-swap with an optional commit subject (see
    /// [`GitBackend::update_spec_atomically_with_subject`]), then that one
    /// cache row. Holds the store write lock across both; never scans the
    /// store.
    // trace:TASK-1506 trace:BUG-1612 | ai:claude
    pub fn update_spec_atomically_with_subject<F>(
        &self,
        target: &Requirement,
        commit_subject: Option<&str>,
        update_fn: F,
    ) -> Result<Option<Requirement>>
    where
        F: FnOnce(&mut Requirement),
    {
        let _lock = self.inner.lock_store()?;
        let pre_write_head = self.current_head_sha();
        let Some(updated) =
            self.inner
                .update_spec_atomically_with_subject(target, commit_subject, update_fn)?
        else {
            return Ok(None);
        };
        if let Err(e) = self.upsert_requirement_with_schema_retry(&updated) {
            let _ = self.cache.set_source_head_sha("");
            eprintln!("warning: cache upsert failed, cache marked stale: {}", e);
        } else {
            self.refresh_epics_then_restamp(&[&updated], &pre_write_head);
        }
        Ok(Some(updated))
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

    // trace:TASK-1514 | ai:antigravity
    fn load_for_read(&self) -> Result<RequirementsStore> {
        self.ensure_cache_fresh_for_read_with_schema_retry()?;
        self.inner.load()
    }

    fn load(&self) -> Result<RequirementsStore> {
        // Phase 1: reads delegate to git. Phase 2 will switch list/search
        // to the cache.
        self.ensure_cache_fresh_with_schema_retry()?;
        self.inner.load()
    }

    fn save(&self, store: &RequirementsStore) -> Result<()> {
        // Bulk save: write through git, then full-rebuild the cache to
        // guarantee invariants (additions, modifications, deletions all
        // captured). Cheap enough for current scale.
        //
        // BUG-1612: the store lock is held across the cache rebuild, and when
        // the save kept objects the in-memory store never had (added by another
        // writer after the load), the cache is rebuilt from disk instead of
        // from `store`, which would otherwise hide them. The same holds when
        // it skipped specs that changed on disk after the load (the in-memory
        // copies are stale).
        // trace:BUG-1612 | ai:claude
        let _lock = self.inner.lock_store()?;
        let report = self.inner.save_reporting(store)?;
        let head = self.current_head_sha();
        if report.kept_unloaded.is_empty() && report.stale_untouched.is_empty() {
            self.with_cache_schema_retry("rebuild cache after save", || {
                self.cache.rebuild_from_store(store, &head)
            })?;
        } else {
            self.with_cache_schema_retry("rebuild cache after save", || self.full_rebuild(false))?;
        }
        Ok(())
    }

    /// Whole-store transaction written per spec, then the cache rows it
    /// touched. Holds the store write lock across both.
    // trace:BUG-1612 | ai:claude
    fn update_atomically<F>(&self, update_fn: F) -> Result<RequirementsStore>
    where
        F: FnOnce(&mut RequirementsStore),
    {
        let _lock = self.inner.lock_store()?;
        let pre_write_head = self.current_head_sha();
        let (store, summary) = self.inner.update_atomically_tracked(update_fn)?;
        if summary.written.is_empty() && summary.deleted.is_empty() && !summary.metadata_changed {
            return Ok(store);
        }
        if summary.metadata_changed || !summary.deleted.is_empty() {
            // Store-level fields or removals: re-project from the store this
            // transaction loaded and wrote under the lock.
            let head = self.current_head_sha();
            self.with_cache_schema_retry("rebuild cache after atomic update", || {
                self.cache.rebuild_from_store(&store, &head)
            })?;
            return Ok(store);
        }
        let mut cache_ok = true;
        for req in &summary.written {
            if let Err(e) = self.upsert_requirement_with_schema_retry(req) {
                eprintln!("warning: cache upsert failed, cache marked stale: {}", e);
                cache_ok = false;
                break;
            }
        }
        if cache_ok {
            let reqs: Vec<&Requirement> = summary.written.iter().collect();
            self.refresh_epics_then_restamp(&reqs, &pre_write_head);
        } else {
            let _ = self.cache.set_source_head_sha("");
        }
        Ok(store)
    }

    /// Per-spec compare-and-swap, then that one cache row. Holds the store
    /// write lock across both; never scans the store.
    // trace:BUG-1612 | ai:claude
    fn update_spec_atomically<F>(
        &self,
        target: &Requirement,
        update_fn: F,
    ) -> Result<Option<Requirement>>
    where
        F: FnOnce(&mut Requirement),
    {
        self.update_spec_atomically_with_subject(target, None, update_fn)
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

    // Route the trait's checked resolver to the cache-backed one above
    // (no full-store load). trace:TASK-1468 | ai:claude
    fn get_requirement_unambiguous(&self, id: &str) -> Result<Option<Requirement>> {
        CachedGitBackend::get_requirement_unambiguous(self, id)
    }

    // trace:BUG-1670 | ai:claude
    fn get_requirement_unambiguous_for_read(&self, id: &str) -> Result<Option<Requirement>> {
        CachedGitBackend::get_requirement_unambiguous_for_read(self, id)
    }

    fn list_requirements_for_read(&self, include_archived: bool) -> Result<Vec<Requirement>> {
        self.ensure_cache_fresh_for_read_with_schema_retry()?;
        self.inner.list_requirements(include_archived)
    }

    fn list_requirements(&self, include_archived: bool) -> Result<Vec<Requirement>> {
        self.ensure_cache_fresh_with_schema_retry()?;
        self.inner.list_requirements(include_archived)
    }

    fn add_requirement(&self, requirement: Requirement) -> Result<Requirement> {
        // trace:TASK-712 — capture HEAD before the write (see restamp_head).
        let pre_write_head = self.current_head_sha();
        let added = self.inner.add_requirement(requirement)?;
        if let Err(e) = self.upsert_requirement_with_schema_retry(&added) {
            // Cache write failure is non-fatal — mark stale by clearing the
            // recorded SHA so the next read triggers a rebuild.
            let _ = self.cache.set_source_head_sha("");
            eprintln!("warning: cache upsert failed, cache marked stale: {}", e);
        } else {
            // BUG-626: a new child shifts its parent epic's rollup status.
            self.refresh_epics_then_restamp(&[&added], &pre_write_head);
        }
        Ok(added)
    }

    fn update_requirement(&self, requirement: &Requirement) -> Result<()> {
        // trace:TASK-712 — capture HEAD before the write (see restamp_head).
        let pre_write_head = self.current_head_sha();
        self.inner.update_requirement(requirement)?;
        if let Err(e) = self.upsert_requirement_with_schema_retry(requirement) {
            let _ = self.cache.set_source_head_sha("");
            eprintln!("warning: cache upsert failed, cache marked stale: {}", e);
        } else {
            // BUG-626: a child's status (or hierarchy edge) change shifts its
            // parent epic's rollup status — refresh the parent epic's row.
            self.refresh_epics_then_restamp(&[requirement], &pre_write_head);
        }
        Ok(())
    }

    fn update_requirement_versioned(&self, requirement: &Requirement) -> Result<UpdateResult> {
        // trace:TASK-712 — capture HEAD before the write (see restamp_head).
        let pre_write_head = self.current_head_sha();
        let result = self.inner.update_requirement_versioned(requirement)?;
        if matches!(result, UpdateResult::Success) {
            if let Err(e) = self.upsert_requirement_with_schema_retry(requirement) {
                let _ = self.cache.set_source_head_sha("");
                eprintln!("warning: cache upsert failed, cache marked stale: {}", e);
            } else {
                // BUG-626: refresh the parent epic's rollup status. trace:BUG-626
                self.refresh_epics_then_restamp(&[requirement], &pre_write_head);
            }
        }
        Ok(result)
    }

    fn delete_requirement(&self, id: &Uuid) -> Result<()> {
        // trace:TASK-712 — capture HEAD before the write (see restamp_head).
        let pre_write_head = self.current_head_sha();
        self.inner.delete_requirement(id)?;
        if let Err(e) = self.delete_requirement_with_schema_retry(id) {
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

    // trace:BUG-1671 | ai:claude
    fn queue_remove_many_if(
        &self,
        user_id: &str,
        ids: &[Uuid],
        still_dead: &dyn Fn(&Uuid) -> bool,
    ) -> Result<Vec<QueueEntry>> {
        self.inner.queue_remove_many_if(user_id, ids, still_dead)
    }

    // trace:BUG-1671 | ai:claude
    fn queue_remove_for_role_if(
        &self,
        user_id: &str,
        requirement_id: &Uuid,
        role: Option<&str>,
        still_dead: &dyn Fn(&Uuid) -> bool,
    ) -> Result<bool> {
        self.inner
            .queue_remove_for_role_if(user_id, requirement_id, role, still_dead)
    }
}

#[cfg(test)]
mod tests {
    use super::super::cache::is_cache_lock_error;
    use super::*;
    use tempfile::tempdir;

    fn sample_req(spec_id: &str, title: &str) -> Requirement {
        let mut r = Requirement::new(title.into(), "desc".into());
        r.spec_id = Some(spec_id.into());
        r
    }

    /// BUG-1598: the direct test at the fix site — proves
    /// `default_cache_path` itself refuses to adopt a temp root, not just
    /// its callers. The store sits directly under a FAKE temp root (a plain
    /// tempdir injected as the guarded root, standing in for "a temp root"
    /// like `/tmp`) that holds a planted `.aida/`; without the guard, the
    /// walk-up would find that `.aida` one hop up from `store.parent()` and
    /// return `<fake_temp_root>/.aida/cache.db`. With the guard, it must
    /// stop before adopting it and fall back to the sibling-file path
    /// instead. No env mutation — the fake root is injected directly.
    // trace:BUG-1598 | ai:claude
    #[test]
    fn default_cache_path_never_adopts_a_temp_root_ancestor() {
        let fake_temp_root = tempdir().unwrap();
        std::fs::create_dir_all(fake_temp_root.path().join(".aida")).unwrap();

        let git_root = fake_temp_root.path().join("store");
        std::fs::create_dir_all(&git_root).unwrap();

        let roots = vec![fake_temp_root.path().to_path_buf()];
        let resolved = CachedGitBackend::default_cache_path_with_roots(&git_root, &roots);
        assert_eq!(
            resolved,
            git_root.with_extension("cache.db"),
            "must fall back to the sibling-file cache path, not adopt the fake temp root's .aida"
        );

        // Sanity check: WITHOUT the guard (empty roots list), the same
        // fixture DOES adopt the planted `.aida` — proving the guard, not
        // some other difference, is what redirects the result above.
        let unguarded = CachedGitBackend::default_cache_path_with_roots(&git_root, &[]);
        assert_eq!(
            unguarded,
            fake_temp_root.path().join(".aida").join("cache.db"),
            "fixture must be adoptable when nothing is guarded, or this test proves nothing"
        );
    }

    // trace:BUG-1678 | ai:codex
    #[test]
    fn targeted_reader_shares_one_scan_for_stale_cache_targets() {
        use crate::object_store::{self, OBJECT_LIST_COUNT};

        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        std::fs::create_dir_all(&store_root).unwrap();
        let backend = CachedGitBackend::open(&store_root, &dir.path().join("cache.db")).unwrap();
        let objects = store_root.join("objects");
        let targets: Vec<_> = (1..=8)
            .map(|n| sample_req(&format!("TASK-{n}"), &format!("Target {n}")))
            .collect();
        for target in &targets {
            object_store::write_object(&objects, target).unwrap();
        }
        // One good cache hit; one row points to a missing file; another points
        // to a different UUID. The remaining targets are absent from the cache.
        backend.cache.upsert_requirement(&targets[0]).unwrap();
        let mut missing_file = targets[1].clone();
        missing_file.spec_id = Some("TASK-99".into());
        backend.cache.upsert_requirement(&missing_file).unwrap();
        let mut wrong_uuid = targets[2].clone();
        // Use a distinct existing file to avoid the cache's spec-id uniqueness.
        let other = sample_req("TASK-98", "Different UUID");
        object_store::write_object(&objects, &other).unwrap();
        wrong_uuid.spec_id = other.spec_id;
        backend.cache.upsert_requirement(&wrong_uuid).unwrap();

        OBJECT_LIST_COUNT.with(|c| c.set(0));
        let read = backend.requirement_reader();
        assert_eq!(
            read(&targets[0].id).unwrap().unwrap().title,
            targets[0].title
        );
        assert_eq!(OBJECT_LIST_COUNT.with(|c| c.get()), 0);
        // Render relationships, then blockers (including duplicate/missing IDs).
        for _ in 0..2 {
            for target in &targets {
                let found = read(&target.id).unwrap().unwrap();
                assert_eq!(found.id, target.id);
                assert_eq!(found.title, target.title);
            }
            assert!(read(&Uuid::now_v7()).unwrap().is_none());
        }
        assert_eq!(OBJECT_LIST_COUNT.with(|c| c.get()), 1);

        // The index only locates files; it does not freeze record contents.
        let mut updated = targets[7].clone();
        updated.title = "Updated canonical title".into();
        object_store::write_object(&objects, &updated).unwrap();
        assert_eq!(read(&updated.id).unwrap().unwrap().title, updated.title);
        assert_eq!(OBJECT_LIST_COUNT.with(|c| c.get()), 1);
        // A new operation can discover files added since the previous scan.
        let added = sample_req("TASK-100", "New target");
        object_store::write_object(&objects, &added).unwrap();
        assert_eq!(
            backend.get_requirement(&added.id).unwrap().unwrap().id,
            added.id
        );
    }

    // trace:BUG-1678 | ai:codex
    #[test]
    fn targeted_reader_shares_one_scan_when_cache_is_unreadable() {
        use crate::object_store::{self, OBJECT_LIST_COUNT};

        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        std::fs::create_dir_all(&store_root).unwrap();
        let cache_path = dir.path().join("cache.db");
        let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();
        let targets = [sample_req("TASK-1", "One"), sample_req("TASK-2", "Two")];
        for target in &targets {
            object_store::write_object(&store_root.join("objects"), target).unwrap();
        }
        rusqlite::Connection::open(&cache_path)
            .unwrap()
            .execute_batch("DROP TABLE requirements_cache")
            .unwrap();
        OBJECT_LIST_COUNT.with(|c| c.set(0));
        let read = backend.requirement_reader();
        for target in &targets {
            assert_eq!(read(&target.id).unwrap().unwrap().title, target.title);
        }
        assert!(read(&Uuid::now_v7()).unwrap().is_none());
        assert_eq!(OBJECT_LIST_COUNT.with(|c| c.get()), 1);
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

    #[test]
    fn write_path_rebuilds_and_retries_after_open_cache_schema_drift() {
        use crate::models::{Relationship, RelationshipType, RequirementType};
        use rusqlite::Connection;

        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_path = dir.path().join(".aida").join("cache.db");
        std::fs::create_dir_all(&store_root).unwrap();
        let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();

        let mut epic = sample_req("EPIC-1097", "epic");
        epic.req_type = RequirementType::Epic;
        let epic_id = epic.id;
        backend.add_requirement(epic).unwrap();

        let mut child = sample_req("BUG-1097", "child");
        child.relationships.push(Relationship {
            rel_type: RelationshipType::Parent,
            target_id: epic_id,
            created_at: None,
            created_by: None,
        });
        let child_id = child.id;
        backend.add_requirement(child).unwrap();

        {
            let conn = Connection::open(&cache_path).unwrap();
            conn.execute_batch(
                "DROP TABLE hierarchy_edges;
                 CREATE TABLE hierarchy_edges (
                     parent_id TEXT NOT NULL,
                     child_id TEXT NOT NULL,
                     PRIMARY KEY (parent_id, child_id)
                 );",
            )
            .unwrap();
        }

        let mut edited = backend.get_requirement(&child_id).unwrap().unwrap();
        edited.title = "child edited through drifted cache".into();

        // The old hierarchy_edges table is missing author_id, so the first
        // cache upsert fails inside delete_one_uncommitted. The backend must
        // drop+rebuild the projection and retry in-process; the authoritative
        // git write must not surface as a raw sqlite error.
        // trace:BUG-1097 | ai:codex
        backend.update_requirement(&edited).unwrap();

        let descendants = backend.descendant_ids(&epic_id).unwrap();
        assert!(descendants.contains(&child_id));

        let conn = Connection::open(&cache_path).unwrap();
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(hierarchy_edges)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            cols.iter().any(|col| col == "author_id"),
            "schema-drift retry must restore hierarchy_edges.author_id"
        );
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

        // trace:TASK-1526 | ai:codex
        // Pre-C tolerant reads expose the incomplete snapshot honestly until
        // a strict refresh restores it. The write must not stamp it fresh.
        let scope = super::super::cache_refresh::CacheReadScope::new();
        backend.list_summaries(&ListFilter::default()).unwrap();
        assert_eq!(scope.metadata()["refreshing"], "deferred");
        assert!(backend.cache_snapshot_is_stale().unwrap());
        let all = backend
            .list_summaries_strict(&ListFilter::default())
            .unwrap();
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

    /// BUG-1606: a pure read (`aida show`'s path) that catches the cache up to
    /// an external commit touching an epic's child must NOT fall back to the
    /// by-uuid full-store scan to read the epic's stored status. That scan
    /// parsed every object once per ancestor epic per changed row, and on a
    /// busy disk with a cold page cache it made single `aida show` calls take
    /// minutes. The rollup result must be unchanged.
    // trace:BUG-1606 | ai:claude
    #[test]
    fn read_path_epic_rollup_refresh_never_scans_the_whole_store() {
        use crate::models::RequirementStatus;
        use crate::object_store::FULL_SCAN_COUNT;

        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_path = dir.path().join(".aida").join("cache.db");
        let (backend, epic_id, child_id) =
            epic_with_open_and_rejected_child(&store_root, &cache_path);

        // External raw write (another agent's `aida edit`): the cache is now
        // stale, and the next read replays it incrementally.
        {
            let external = GitBackend::new(&store_root).unwrap();
            let mut child = external
                .get_requirement_by_spec_id("STORY-1")
                .unwrap()
                .unwrap();
            assert_eq!(child.id, child_id);
            child.status = RequirementStatus::Completed;
            external.update_requirement(&child).unwrap();
        }

        FULL_SCAN_COUNT.with(|c| c.set(0));
        // The read `aida show` makes: the spec's cached degrees.
        backend.degrees(&child_id).unwrap();
        let scans = FULL_SCAN_COUNT.with(|c| c.get());
        assert_eq!(
            scans, 0,
            "the read-path epic rollup refresh must use a targeted read, not a full-store scan"
        );

        // Same rollup as before the fix: zero open children -> Completed.
        assert_eq!(cached_status(&backend, child_id), "Completed");
        assert_eq!(cached_status(&backend, epic_id), "Completed");
    }

    /// BUG-1606 (fail-closed, PRIN-5): when the targeted read of an ancestor
    /// epic misses (its cache row points at a file that no longer holds it),
    /// the incremental catch-up does not guess a status. It declines, so the
    /// caller does an authoritative full rebuild, and it never scans the store
    /// to look for the epic.
    // trace:BUG-1606 | ai:claude
    #[test]
    fn epic_targeted_miss_makes_incremental_catch_up_decline() {
        use crate::models::RequirementStatus;
        use crate::object_store::FULL_SCAN_COUNT;

        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_path = dir.path().join(".aida").join("cache.db");
        let (backend, epic_id, child_id) =
            epic_with_open_and_rejected_child(&store_root, &cache_path);
        let recorded = backend.cache().source_head_sha().unwrap().unwrap();

        // External commit to the child: the catch-up will replay it.
        {
            let external = GitBackend::new(&store_root).unwrap();
            let mut child = external
                .get_requirement_by_spec_id("STORY-1")
                .unwrap()
                .unwrap();
            child.status = RequirementStatus::Completed;
            external.update_requirement(&child).unwrap();
        }
        let head = crate::git_ops::head_sha(&store_root).unwrap();

        // Remove the epic's YAML behind the cache's back: the cache row still
        // maps its uuid to EPIC-1, but the targeted read now misses.
        let epic_file =
            store_root.join(crate::object_store::relative_object_path("EPIC-1").unwrap());
        std::fs::remove_file(&epic_file).unwrap();
        assert_eq!(backend.stored_status_targeted(&epic_id), None);
        assert!(backend.stored_status_targeted(&child_id).is_some());

        FULL_SCAN_COUNT.with(|c| c.set(0));
        let applied = backend.try_incremental_update(&recorded, &head).unwrap();
        assert!(
            !applied,
            "a missed epic read must make the catch-up decline (full rebuild), not guess"
        );
        assert_eq!(FULL_SCAN_COUNT.with(|c| c.get()), 0);
        // Declining leaves the recorded HEAD where it was, so the caller's
        // full rebuild is what brings the cache current.
        assert_eq!(
            backend.cache().source_head_sha().unwrap(),
            Some(recorded),
            "a declined catch-up must not stamp the new HEAD"
        );

        // The refresh reports the miss, which is what makes the write paths
        // mark the cache stale instead of re-stamping it.
        let child = backend
            .get_requirement_by_spec_id("STORY-1")
            .unwrap()
            .unwrap();
        assert!(!backend.refresh_parent_epic_status(&child));
    }

    // ---------------------------------------------------------------- BUG-768
    // The BUG-764 re-derivation must not reopen an epic a human force-closed.
    // Observed: EPIC-24 / EPIC-0428 were force-closed to Completed in the
    // store, and every overnight pull re-derived them back to in-progress
    // because a *Completed* intermediate child still carried stale Approved
    // grandchildren that the transitive rollup counts as open.

    /// Build a store holding a FORCE-CLOSED epic (stored `Completed`) that
    /// still carries one open (Approved) child — the observed shape. Returns
    /// the backend plus the (epic, child) ids.
    fn force_closed_epic_with_open_child(
        store_root: &Path,
        cache_path: &Path,
    ) -> (CachedGitBackend, Uuid, Uuid) {
        use crate::models::{Relationship, RelationshipType, RequirementStatus, RequirementType};

        std::fs::create_dir_all(store_root).unwrap();
        crate::git_ops::init(store_root).unwrap();
        crate::git_ops::configure_user(store_root, "Test", "test@example.com").unwrap();
        let backend = CachedGitBackend::open(store_root, cache_path).unwrap();

        let mut epic = sample_req("EPIC-1", "force-closed epic");
        epic.req_type = RequirementType::Epic;
        // The human's `aida edit EPIC-1 --status completed --force`.
        epic.status = RequirementStatus::Completed;
        let epic_id = epic.id;
        backend.add_requirement(epic).unwrap();

        let mut child = sample_req("TASK-1", "stale open child");
        child.status = RequirementStatus::Approved;
        child.relationships.push(Relationship {
            rel_type: RelationshipType::Parent,
            target_id: epic_id,
            created_at: None,
            created_by: None,
        });
        let child_id = child.id;
        backend.add_requirement(child).unwrap();

        (backend, epic_id, child_id)
    }

    // A force-closed epic stays Completed across BOTH re-derivation routes —
    // the direct write-through edit and the external raw-`GitBackend` write
    // the `aida pull` auto-bump uses — and across a full cache rebuild.
    // trace:BUG-768 | ai:claude
    #[test]
    fn force_closed_epic_is_not_reopened_by_rederivation() {
        use crate::models::RequirementStatus;

        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_path = dir.path().join(".aida").join("cache.db");
        let (backend, epic_id, child_id) =
            force_closed_epic_with_open_child(&store_root, &cache_path);

        // Adding the open child must not have reopened the epic.
        assert_eq!(
            cached_status(&backend, epic_id),
            "Completed",
            "adding an open child must not reopen a force-closed epic"
        );

        // Direct write-through child edit.
        let mut child = backend.get_requirement(&child_id).unwrap().unwrap();
        child.status = RequirementStatus::InProgress;
        backend.update_requirement(&child).unwrap();
        assert_eq!(
            cached_status(&backend, epic_id),
            "Completed",
            "a child edit must not reopen a force-closed epic"
        );

        // External raw-GitBackend write — the `aida pull` auto-bump shape,
        // replayed through the incremental cache catch-up.
        {
            let external = GitBackend::new(&store_root).unwrap();
            let mut child = external.get_requirement(&child_id).unwrap().unwrap();
            child.status = RequirementStatus::Approved;
            external.update_requirement(&child).unwrap();
        }
        assert_eq!(
            cached_status(&backend, epic_id),
            "Completed",
            "a pull-shaped external write must not reopen a force-closed epic"
        );

        // And the authoritative full rebuild agrees.
        backend.rebuild_cache().unwrap();
        assert_eq!(
            cached_status(&backend, epic_id),
            "Completed",
            "a full rebuild must not reopen a force-closed epic"
        );
    }

    // An epic whose children are ALL archived-completed stays terminal:
    // archived is a view flag, not a status (BUG-628), so archived children
    // still tally into the rollup rather than being filtered out into a
    // "no children" default. trace:BUG-768 | ai:claude
    #[test]
    fn epic_with_all_archived_completed_children_stays_completed() {
        use crate::models::{Relationship, RelationshipType, RequirementStatus, RequirementType};

        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_path = dir.path().join(".aida").join("cache.db");
        std::fs::create_dir_all(&store_root).unwrap();
        crate::git_ops::init(&store_root).unwrap();
        crate::git_ops::configure_user(&store_root, "Test", "test@example.com").unwrap();
        let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();

        // A NON-force-closed epic (stored InProgress) so the terminal status
        // under test is genuinely DERIVED from the archived children.
        let mut epic = sample_req("EPIC-1", "epic with an archived child set");
        epic.req_type = RequirementType::Epic;
        epic.status = RequirementStatus::InProgress;
        let epic_id = epic.id;
        backend.add_requirement(epic).unwrap();

        for (i, spec) in ["TASK-1", "TASK-2"].iter().enumerate() {
            let mut child = sample_req(spec, "archived completed child");
            child.status = RequirementStatus::Completed;
            child.archived = true;
            child.relationships.push(Relationship {
                rel_type: RelationshipType::Parent,
                target_id: epic_id,
                created_at: None,
                created_by: None,
            });
            backend.add_requirement(child).unwrap();
            assert_eq!(
                cached_status(&backend, epic_id),
                "Completed",
                "archived-completed child {i} must count toward the rollup"
            );
        }

        // The full rebuild derives the same value.
        backend.rebuild_cache().unwrap();
        assert_eq!(
            cached_status(&backend, epic_id),
            "Completed",
            "a rebuild must not drop archived children from the rollup"
        );
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

        // trace:TASK-1526 | ai:codex
        // The strict path still rebuilds non-ancestor snapshots. The pre-C
        // tolerant case is covered by the Deferred tests below.
        backend.ensure_cache_fresh().unwrap();
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

    // TASK-1468 / PRIN-5: during the on-disk re-check of a cache-reported
    // ambiguity, a candidate whose YAML EXISTS but cannot be parsed is absent
    // evidence and still counts, so the write refuses. A candidate whose file
    // is GONE is a stale cache row and drops out. trace:TASK-1468 | ai:claude
    #[test]
    fn unreadable_candidate_yaml_keeps_the_id_ambiguous_for_writes() {
        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_path = dir.path().join(".aida").join("cache.db");
        std::fs::create_dir_all(&store_root).unwrap();
        let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();

        let fixture = backend
            .add_requirement(sample_req("BUG-34", "fixture"))
            .unwrap();
        let mut real = sample_req("BUG-2-081", "real");
        real.agreed_id = Some("BUG-34".into());
        backend.add_requirement(real).unwrap();

        let err = backend.get_requirement_unambiguous("BUG-34").unwrap_err();
        assert!(err
            .downcast_ref::<crate::id_collisions::AmbiguousIdError>()
            .is_some());

        // Corrupt the agreed-id holder's YAML behind the cache's back.
        let objects_root = store_root.join("objects");
        let real_path = crate::object_store::object_path(&objects_root, "BUG-2-081").unwrap();
        std::fs::write(&real_path, "{{ not: yaml: at all").unwrap();
        let err = backend
            .get_requirement_unambiguous("BUG-34")
            .expect_err("an unreadable candidate must not let the write through");
        assert!(err
            .downcast_ref::<crate::id_collisions::AmbiguousIdError>()
            .is_some());
        // The trait entry point routes to the same checked resolver.
        assert!(DatabaseBackend::get_requirement_unambiguous(&backend, "BUG-34").is_err());

        // A candidate whose file is gone is a stale row: the id resolves.
        std::fs::remove_file(&real_path).unwrap();
        let only = backend
            .get_requirement_unambiguous("BUG-34")
            .unwrap()
            .unwrap();
        assert_eq!(only.id, fixture.id);
    }

    /// BUG-1612: the cached per-spec update is a targeted write-through: the
    /// row is refreshed without listing the store.
    // trace:BUG-1612 | ai:claude
    #[test]
    fn bug1612_cached_update_spec_atomically_refreshes_row_without_scan() {
        use crate::object_store::{FULL_SCAN_COUNT, OBJECT_LIST_COUNT};
        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_path = dir.path().join(".aida").join("cache.db");
        std::fs::create_dir_all(&store_root).unwrap();
        let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();
        let mut added = Vec::new();
        for i in 1..=4 {
            added.push(
                backend
                    .add_requirement(sample_req(&format!("TASK-{i}"), &format!("t{i}")))
                    .unwrap(),
            );
        }
        let target = added[1].clone();

        OBJECT_LIST_COUNT.with(|c| c.set(0));
        FULL_SCAN_COUNT.with(|c| c.set(0));
        backend
            .update_spec_atomically(&target, |r| {
                r.status = crate::models::RequirementStatus::Completed;
            })
            .unwrap()
            .unwrap();
        assert_eq!(OBJECT_LIST_COUNT.with(|c| c.get()), 0);
        assert_eq!(FULL_SCAN_COUNT.with(|c| c.get()), 0);
        assert_eq!(cached_status(&backend, target.id), "Completed");
    }

    /// BUG-1612: the cached whole-store transaction and save never delete a
    /// spec another writer added after the load, and the cache still shows it.
    // trace:BUG-1612 | ai:claude
    #[test]
    fn bug1612_cached_atomic_update_and_stale_save_keep_concurrent_add() {
        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_path = dir.path().join(".aida").join("cache.db");
        std::fs::create_dir_all(&store_root).unwrap();
        let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();
        let first = backend
            .add_requirement(sample_req("TASK-1", "one"))
            .unwrap();
        let stale = backend.load().unwrap();
        let objects = store_root.join("objects");

        let late = sample_req("TASK-2", "late");
        let late_id = late.id;
        backend
            .update_atomically(|s| {
                s.requirements[0].title = "one edited".into();
                crate::object_store::write_object(&objects, &late).unwrap();
            })
            .unwrap();
        assert!(crate::object_store::object_exists(&objects, "TASK-2").unwrap());
        assert_eq!(
            backend.get_requirement(&first.id).unwrap().unwrap().title,
            "one edited"
        );

        backend.save(&stale).unwrap();
        assert!(crate::object_store::object_exists(&objects, "TASK-2").unwrap());
        assert_eq!(cached_status(&backend, late_id), "Draft");
    }

    /// BUG-1612: the aida-server pattern: one store loaded once and saved
    /// after every mutation through a boxed backend. Every mutation lands on
    /// disk and in the cache.
    // trace:BUG-1612 | ai:claude
    #[test]
    fn bug1612_server_style_repeated_saves_all_land() {
        let dir = tempdir().unwrap();
        let store_root = dir.path().join("store");
        let cache_path = dir.path().join(".aida").join("cache.db");
        std::fs::create_dir_all(&store_root).unwrap();
        let backend: Box<dyn DatabaseBackend> =
            Box::new(CachedGitBackend::open(&store_root, &cache_path).unwrap());
        backend
            .add_requirement(sample_req("TASK-1", "one"))
            .unwrap();
        let mut store = backend.load().unwrap();
        let objects = store_root.join("objects");

        for (i, status) in ["Approved", "In Progress", "Completed"].iter().enumerate() {
            store.requirements[0].title = format!("edit {i}");
            store.requirements[0].set_status_from_str(status);
            backend.save(&store).unwrap();
            assert_eq!(
                crate::object_store::read_object(&objects, "TASK-1")
                    .unwrap()
                    .title,
                format!("edit {i}")
            );
        }
        let created = sample_req("TASK-2", "created");
        let created_id = created.id;
        store.requirements.push(created);
        backend.save(&store).unwrap();
        store.requirements[1].title = "created then edited".into();
        backend.save(&store).unwrap();
        store.requirements.retain(|r| r.id != created_id);
        backend.save(&store).unwrap();
        assert!(!crate::object_store::object_exists(&objects, "TASK-2").unwrap());
        assert_eq!(
            cached_status(&backend_as_cached(&*backend), store.requirements[0].id),
            "Completed"
        );
    }

    fn backend_as_cached(b: &dyn DatabaseBackend) -> CachedGitBackend {
        CachedGitBackend::open(
            b.path(),
            &b.path().parent().unwrap().join(".aida").join("cache.db"),
        )
        .unwrap()
    }

    /// BUG-1644 (SPIKE-90 case W): a reader in a sibling worktree whose
    /// `.aida/cache.db` symlinks to the main checkout's cache must see the
    /// main writer's live lock-info, and serve the last committed snapshot
    /// instead of entering the write ladder (~52 s, then failure, before the
    /// fix). The writer really holds `BEGIN IMMEDIATE` on the shared cache.
    // trace:BUG-1644 | ai:claude
    #[cfg(unix)]
    #[test]
    fn bug_1644_worktree_reader_serves_snapshot_behind_main_writer() {
        use rusqlite::Connection;

        let root = tempdir().unwrap();
        let store_root = root.path().join("main").join(".aida-store");
        let main_cache = root.path().join("main").join(".aida").join("cache.db");
        let wt_aida = root.path().join("wt-sibling").join(".aida");
        std::fs::create_dir_all(&store_root).unwrap();
        std::fs::create_dir_all(&wt_aida).unwrap();
        crate::git_ops::init(&store_root).unwrap();
        crate::git_ops::configure_user(&store_root, "Test", "test@example.com").unwrap();

        // Main checkout: build the cache with one committed row.
        {
            let main = CachedGitBackend::open(&store_root, &main_cache).unwrap();
            main.add_requirement(sample_req("BUG-1", "first")).unwrap();
        }
        let wt_cache = wt_aida.join("cache.db");
        for name in ["cache.db", "cache.db-shm", "cache.db-wal"] {
            let src = main_cache.with_file_name(name);
            if src.exists() {
                std::os::unix::fs::symlink(&src, wt_aida.join(name)).unwrap();
            }
        }

        // The store moves on without the cache (it is now stale) ...
        GitBackend::new(&store_root)
            .unwrap()
            .add_requirement(sample_req("BUG-2", "second"))
            .unwrap();

        // ... while a live foreign writer in the main checkout holds the write
        // lock and has recorded its sidecar beside the REAL cache file.
        let holder = Connection::open(&main_cache).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        let info = super::super::cache::CacheLockInfo {
            pid: 1,
            command: "main-checkout-writer".to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            user: "test".to_string(),
            ..Default::default()
        };
        std::fs::write(
            main_cache.with_file_name("cache.db.lock-info"),
            serde_json::to_string(&info).unwrap(),
        )
        .unwrap();

        assert!(
            CachedGitBackend::with_inner_cache_snapshot(
                GitBackend::new(&store_root).unwrap(),
                &wt_cache
            )
            .unwrap()
            .cache_snapshot_is_stale()
            .unwrap(),
            "fixture must leave the shared cache behind the store"
        );

        let started = std::time::Instant::now();
        let reader = CachedGitBackend::open(&store_root, &wt_cache)
            .expect("worktree reader must open behind the live writer");
        let rows = reader.list_summaries(&ListFilter::default()).unwrap();
        let elapsed = started.elapsed();

        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "worktree read entered the write ladder: {elapsed:?}"
        );
        assert_eq!(rows.len(), 1, "serves the last committed snapshot");
        holder.execute_batch("ROLLBACK").unwrap();
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    fn refresh_txn_rechecks_head_inside_txn() {
        let dir = tempdir().unwrap();
        let (backend, root, path) = task_1515_backend(dir.path());
        let old = backend.cache.source_head_sha().unwrap().unwrap();
        let target = task_1515_external_retitle(&root, "gen1");
        let newer = task_1515_external_retitle(&root, "gen2");
        backend.ensure_cache_fresh().unwrap();
        let before = task_1515_read(&rusqlite::Connection::open(&path).unwrap());
        // Simulate a second refresher arriving after a winner has committed.
        // Its stale application must not execute, even without a coherent flock.
        assert!(backend
            .cache
            .apply_incremental(&target, &root, |_| panic!("older refresh applied"))
            .unwrap());
        assert!(backend
            .cache
            .apply_incremental(&newer, &root, |_| panic!("duplicate refresh applied"))
            .unwrap());
        assert_eq!(
            before,
            task_1515_read(&rusqlite::Connection::open(&path).unwrap())
        );
        assert_ne!(old, newer);
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    fn second_process_skips_refill_after_migration_committed() {
        let dir = tempdir().unwrap();
        let (backend, root, path) = task_1515_backend(dir.path());
        let head = backend.current_head_sha();
        drop(backend);
        task_1515_mark_schema_older(&path);
        let first = Cache::open(&path).unwrap();
        let second = Cache::open(&path).unwrap();
        assert!(first.migration_pending() && second.migration_pending());
        let store = GitBackend::new(&root).unwrap().load().unwrap();
        first.rebuild_for_refresh(&store, &head, &root).unwrap();
        let raw = rusqlite::Connection::open(&path).unwrap();
        raw.execute_batch("CREATE TRIGGER no_refill BEFORE DELETE ON requirements_cache BEGIN SELECT RAISE(ABORT, 'redundant refill'); END;").unwrap();
        second.rebuild_for_refresh(&store, &head, &root).unwrap();
        assert!(!second.migration_pending());
        assert_eq!(second.source_head_sha().unwrap(), Some(head));
    }

    // ---------------------------------------------------------------- TASK-1515

    /// A long-lived backend over a fresh git store holding FR-1-001 and
    /// FR-1-002 (both titled `gen0`) plus FR-1-003 (`static`).
    // trace:TASK-1515 | ai:claude
    fn task_1515_backend(dir: &Path) -> (CachedGitBackend, PathBuf, PathBuf) {
        let store_root = dir.join("store");
        let cache_path = dir.join(".aida").join("cache.db");
        std::fs::create_dir_all(&store_root).unwrap();
        crate::git_ops::init(&store_root).unwrap();
        crate::git_ops::configure_user(&store_root, "Test", "test@example.com").unwrap();
        let backend = CachedGitBackend::open(&store_root, &cache_path).unwrap();
        for (id, title) in [
            ("FR-1-001", "gen0"),
            ("FR-1-002", "gen0"),
            ("FR-1-003", "static"),
        ] {
            backend.add_requirement(sample_req(id, title)).unwrap();
        }
        (backend, store_root, cache_path)
    }

    // trace:BUG-1752 | ai:codex
    #[cfg(unix)]
    #[test]
    fn unreadable_cache_uses_private_fallback_and_serves_store_rows() {
        use std::os::unix::fs::PermissionsExt;
        let mut env =
            crate::test_env::EnvVarsGuard::snapshot(&["AIDA_CACHE_FALLBACK_DIR", "XDG_CACHE_HOME"]);
        env.unset_key("AIDA_CACHE_FALLBACK_DIR");
        let dir = tempdir().unwrap();
        let (backend, store_root, project_cache_path) = task_1515_backend(dir.path());
        drop(backend);
        let locked_dir = dir.path().join("linked-cache-target");
        std::fs::create_dir_all(&locked_dir).unwrap();
        let cache_path = locked_dir.join("cache.db");
        std::fs::rename(&project_cache_path, &cache_path).unwrap();
        std::os::unix::fs::symlink(&cache_path, &project_cache_path).unwrap();
        let connection = rusqlite::Connection::open(&cache_path).unwrap();
        connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        drop(connection);
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", cache_path.display(), suffix));
        }
        let cache_dir = cache_path.parent().unwrap();
        let original = std::fs::metadata(cache_dir).unwrap().permissions();
        struct Restore(PathBuf, std::fs::Permissions);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(&self.0, self.1.clone());
            }
        }
        let _restore = Restore(cache_dir.to_path_buf(), original.clone());
        std::fs::set_permissions(
            cache_dir,
            std::fs::Permissions::from_mode(original.mode() & !0o222),
        )
        .unwrap();
        let unused_fallback_home = dir.path().join("fallback-home");
        env.set_key("XDG_CACHE_HOME", unused_fallback_home.as_os_str());
        let (tx, rx) = std::sync::mpsc::channel();
        let thread_cache_path = project_cache_path.clone();
        std::thread::spawn(move || {
            let result = CachedGitBackend::open(&store_root, &thread_cache_path)
                .unwrap()
                .list_summaries(&ListFilter::default())
                .unwrap()
                .len();
            let _ = tx.send(result);
        });
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("read exceeded thirty second bound");
        assert_eq!(result, 3);
        let fallback_parent = project_cache_path.parent().unwrap().join("cache-fallback");
        let fallback_root = std::fs::read_dir(&fallback_parent)
            .unwrap()
            .next()
            .expect("location one was not created")
            .unwrap()
            .path();
        assert!(fallback_root.join("cache.db").exists());
        assert!(
            !unused_fallback_home.exists(),
            "unused location two must not be created"
        );
    }

    // trace:BUG-1752 | ai:codex
    #[cfg(unix)]
    #[test]
    fn unreadable_cache_declines_when_configured_fallback_is_unavailable() {
        use std::os::unix::fs::PermissionsExt;
        let mut env = crate::test_env::EnvVarsGuard::snapshot(&["AIDA_CACHE_FALLBACK_DIR"]);
        let dir = tempdir().unwrap();
        let (backend, store_root, project_cache_path) = task_1515_backend(dir.path());
        drop(backend);
        let locked_dir = dir.path().join("linked-cache-target");
        std::fs::create_dir_all(&locked_dir).unwrap();
        let cache_path = locked_dir.join("cache.db");
        std::fs::rename(&project_cache_path, &cache_path).unwrap();
        std::os::unix::fs::symlink(&cache_path, &project_cache_path).unwrap();
        let connection = rusqlite::Connection::open(&cache_path).unwrap();
        connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        drop(connection);
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", cache_path.display(), suffix));
        }
        let cache_dir = cache_path.parent().unwrap();
        let original = std::fs::metadata(cache_dir).unwrap().permissions();
        struct Restore(PathBuf, std::fs::Permissions);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(&self.0, self.1.clone());
            }
        }
        let _restore = Restore(cache_dir.to_path_buf(), original.clone());
        std::fs::set_permissions(
            cache_dir,
            std::fs::Permissions::from_mode(original.mode() & !0o222),
        )
        .unwrap();
        let unavailable = dir.path().join("fallback-is-a-file");
        std::fs::write(&unavailable, b"block directory creation").unwrap();
        env.set_key("AIDA_CACHE_FALLBACK_DIR", unavailable.as_os_str());
        let (tx, rx) = std::sync::mpsc::channel();
        let thread_cache_path = project_cache_path.clone();
        std::thread::spawn(move || {
            let result = CachedGitBackend::open(&store_root, &thread_cache_path)
                .and_then(|backend| backend.list_summaries(&ListFilter::default()).map(|_| ()))
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(result);
        });
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("unavailable fallback check exceeded five second bound");
        let message = result.expect_err("unreadable cache unexpectedly opened");
        assert!(
            message.contains(&project_cache_path.display().to_string()),
            "{message}"
        );
        assert!(
            message.contains("Grant write access")
                || message.contains("run the command from the main checkout"),
            "{message}"
        );
    }

    /// External writer: retitle FR-1-001 and FR-1-002 in ONE store commit.
    // trace:TASK-1515 | ai:claude
    fn task_1515_external_retitle(store_root: &Path, title: &str) -> String {
        let ext = GitBackend::new(store_root).unwrap();
        let reqs: Vec<Requirement> = ["FR-1-001", "FR-1-002"]
            .iter()
            .map(|id| {
                let mut r = ext.get_requirement_by_spec_id(id).unwrap().unwrap();
                r.title = title.into();
                r
            })
            .collect();
        let before = crate::git_ops::head_sha(store_root).unwrap();
        ext.bulk_update(&reqs, &format!("retitle {title}")).unwrap();
        let after = crate::git_ops::head_sha(store_root).unwrap();
        assert_ne!(before, after, "retitle {title} must make a store commit");
        after
    }

    /// One read transaction on a separate connection: the recorded head and
    /// the titles of FR-1-001 and FR-1-002.
    // trace:TASK-1515 | ai:claude
    fn task_1515_read(conn: &rusqlite::Connection) -> (Option<String>, Vec<String>) {
        conn.execute_batch("BEGIN").unwrap();
        let sha: Option<String> = conn
            .query_row(
                "SELECT value FROM cache_meta WHERE key = 'source_head_sha'",
                [],
                |row| row.get(0),
            )
            .ok();
        let mut stmt = conn
            .prepare(
                "SELECT title FROM requirements_cache
                  WHERE spec_id IN ('FR-1-001', 'FR-1-002') ORDER BY spec_id",
            )
            .unwrap();
        let titles = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        drop(stmt);
        conn.execute_batch("COMMIT").unwrap();
        (sha, titles)
    }

    // TASK-1515: the incremental refresh applies every changed row and the
    // head stamp in ONE transaction. When the stamp fails, no row of the
    // refresh may remain (the pre-fix code committed each row on its own and
    // stamped last).
    // trace:TASK-1515 | ai:claude
    #[test]
    fn task_1515_incremental_rows_roll_back_when_the_stamp_fails() {
        let dir = tempdir().unwrap();
        let (backend, store_root, cache_path) = task_1515_backend(dir.path());
        let from = backend.cache().source_head_sha().unwrap().unwrap();
        let to = task_1515_external_retitle(&store_root, "gen1");
        let raw = rusqlite::Connection::open(&cache_path).unwrap();
        raw.execute_batch(
            "CREATE TRIGGER t1515 BEFORE UPDATE ON cache_meta
               WHEN NEW.key = 'source_head_sha'
               BEGIN SELECT RAISE(ABORT, 'task-1515 injected stamp failure'); END;",
        )
        .unwrap();

        let err = backend.try_incremental_update(&from, &to).unwrap_err();
        assert!(format!("{err:#}").contains("injected"), "{err:#}");
        assert_eq!(
            task_1515_read(&raw),
            (Some(from.clone()), vec!["gen0".into(), "gen0".into()]),
            "a failed refresh must leave the cache exactly at its previous head"
        );

        raw.execute_batch("DROP TRIGGER t1515;").unwrap();
        assert!(backend.try_incremental_update(&from, &to).unwrap());
        assert_eq!(
            task_1515_read(&raw),
            (Some(to), vec!["gen1".into(), "gen1".into()])
        );
    }

    // TASK-1515: a second connection reading while incremental refreshes
    // commit must see each row set together with the head it belongs to:
    // never rows from HEAD n labelled HEAD n-1, never half a refresh.
    // trace:TASK-1515 | ai:claude
    #[test]
    fn task_1515_concurrent_reader_sees_whole_incremental_refreshes() {
        use std::collections::HashMap;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};

        let dir = tempdir().unwrap();
        let (backend, store_root, cache_path) = task_1515_backend(dir.path());
        let built_at = backend.cache().built_at().unwrap();
        let head0 = backend.cache().source_head_sha().unwrap().unwrap();
        let generation: Arc<Mutex<HashMap<String, usize>>> =
            Arc::new(Mutex::new(HashMap::from([(head0, 0)])));
        let stop = Arc::new(AtomicBool::new(false));
        let reads = Arc::new(AtomicUsize::new(0));
        let reader = {
            let (generation, stop, reads) = (generation.clone(), stop.clone(), reads.clone());
            let cache_path = cache_path.clone();
            std::thread::spawn(move || {
                let conn = rusqlite::Connection::open(&cache_path).unwrap();
                while !stop.load(Ordering::SeqCst) {
                    let (sha, titles) = task_1515_read(&conn);
                    let sha = sha.expect("the cache always carries a head here");
                    let n = *generation
                        .lock()
                        .unwrap()
                        .get(&sha)
                        .unwrap_or_else(|| panic!("unknown head {sha}"));
                    let want = format!("gen{n}");
                    assert_eq!(titles, vec![want.clone(), want], "head {sha}");
                    reads.fetch_add(1, Ordering::SeqCst);
                }
            })
        };
        for n in 1..=8 {
            let title = format!("gen{n}");
            // Register the head before the refresh can stamp it.
            let head = task_1515_external_retitle(&store_root, &title);
            generation.lock().unwrap().insert(head.clone(), n);
            // A read-path freshen takes the incremental route.
            let rows = row_snapshot(&backend);
            assert!(rows.iter().any(|r| r.0 == "FR-1-001" && r.1 == title));
            assert_eq!(
                backend.cache().source_head_sha().unwrap().as_deref(),
                Some(head.as_str())
            );
        }
        stop.store(true, Ordering::SeqCst);
        reader
            .join()
            .expect("reader saw a torn incremental refresh");
        assert!(reads.load(Ordering::SeqCst) > 0);
        assert_eq!(
            backend.cache().built_at().unwrap(),
            built_at,
            "every refresh above was incremental, not a full rebuild"
        );
    }

    /// The on-disk `source_head_sha`, read on a separate connection (what raw
    /// `cache_meta` readers and older binaries see).
    // trace:TASK-1515 | ai:claude
    fn task_1515_raw_head(cache_path: &Path) -> Option<String> {
        rusqlite::Connection::open(cache_path)
            .unwrap()
            .query_row(
                "SELECT value FROM cache_meta WHERE key = 'source_head_sha'",
                [],
                |row| row.get(0),
            )
            .ok()
    }

    // TASK-1515 review finding 1: while a schema migration is pending, a
    // write must not stamp the post-write HEAD on disk (the rows never
    // ingested external commits). It leaves the recorded head empty.
    // trace:TASK-1515 | ai:claude
    #[test]
    fn task_1515_write_while_migration_pending_leaves_head_empty() {
        let dir = tempdir().unwrap();
        let (backend, store_root, cache_path) = task_1515_backend(dir.path());
        drop(backend);
        // An external commit the cache never ingests.
        task_1515_external_retitle(&store_root, "generation-1");
        {
            let raw = rusqlite::Connection::open(&cache_path).unwrap();
            let current: String = raw
                .query_row(
                    "SELECT value FROM cache_meta WHERE key = 'schema_version'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let older = (current.parse::<i64>().unwrap() - 1).to_string();
            raw.execute(
                "UPDATE cache_meta SET value = ?1 WHERE key = 'schema_version'",
                rusqlite::params![older],
            )
            .unwrap();
        }
        // Open without a refresh, so the migration stays pending.
        let writer = CachedGitBackend::with_inner_cache_snapshot(
            GitBackend::new(&store_root).unwrap(),
            &cache_path,
        )
        .unwrap();
        assert!(writer.cache().migration_pending());
        assert!(task_1515_raw_head(&cache_path).is_some_and(|s| !s.is_empty()));

        writer
            .add_requirement(sample_req("FR-1-004", "written"))
            .unwrap();
        assert_eq!(
            task_1515_raw_head(&cache_path).as_deref(),
            Some(""),
            "a write during a pending migration must leave the head stale"
        );
        let mut r = writer
            .get_requirement_by_spec_id("FR-1-003")
            .unwrap()
            .unwrap();
        r.title = "edited".into();
        writer.update_requirement(&r).unwrap();
        assert_eq!(task_1515_raw_head(&cache_path).as_deref(), Some(""));
    }

    // TASK-1515 review finding 4: if HEAD moved past `to` while the changed
    // objects were being read, the incremental refresh declines rather than
    // stamping `to` on rows that may come from a later HEAD.
    // trace:TASK-1515 | ai:claude
    #[test]
    fn task_1515_incremental_declines_when_head_moved_during_reads() {
        let dir = tempdir().unwrap();
        let (backend, store_root, cache_path) = task_1515_backend(dir.path());
        let from = backend.cache().source_head_sha().unwrap().unwrap();
        let to = task_1515_external_retitle(&store_root, "gen1");
        task_1515_external_retitle(&store_root, "generation-2");

        assert!(!backend.try_incremental_update(&from, &to).unwrap());
        let raw = rusqlite::Connection::open(&cache_path).unwrap();
        assert_eq!(
            task_1515_read(&raw),
            (Some(from), vec!["gen0".into(), "gen0".into()]),
            "a declined refresh leaves the cache untouched"
        );
    }

    thread_local! {
        /// Test hook run inside `try_incremental_update` after the pre-lock
        /// object reads and before the HEAD re-check (thread-local, so
        /// parallel tests never see each other's hook).
        // trace:TASK-1515 | ai:claude
        static TASK_1515_AFTER_READS: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
            std::cell::RefCell::new(None);
    }

    /// Called from `try_incremental_update` under `cfg(test)`.
    // trace:TASK-1515 | ai:claude
    pub(super) fn task_1515_after_incremental_reads() {
        let hook = TASK_1515_AFTER_READS.with(|h| h.borrow_mut().take());
        if let Some(mut f) = hook {
            f();
            TASK_1515_AFTER_READS.with(|h| {
                let mut slot = h.borrow_mut();
                if slot.is_none() {
                    *slot = Some(f);
                }
            });
        }
    }

    /// Install a hook that makes an external store commit (retitling
    /// FR-1-001 and FR-1-002 to `hookN`) on each of the first `moves`
    /// incremental attempts. Returns the attempt counter.
    // trace:TASK-1515 | ai:claude
    fn task_1515_move_head_during_reads(
        store_root: &Path,
        moves: usize,
    ) -> std::rc::Rc<std::cell::Cell<usize>> {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let counter = calls.clone();
        let root = store_root.to_path_buf();
        TASK_1515_AFTER_READS.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                let n = counter.get() + 1;
                counter.set(n);
                if n <= moves {
                    task_1515_external_retitle(&root, &format!("hook{n}"));
                }
            }));
        });
        calls
    }

    // trace:TASK-1515 | ai:claude
    fn task_1515_clear_hook() {
        TASK_1515_AFTER_READS.with(|h| *h.borrow_mut() = None);
    }

    // TASK-1515 round-2 finding 2: when HEAD moves during the incremental
    // reads, the refresh retries the incremental from the new HEAD instead of
    // full-rebuilding, and the stamp is the HEAD the rows came from.
    // trace:TASK-1515 | ai:claude
    #[test]
    fn task_1515_head_move_during_reads_retries_incremental_from_new_head() {
        let dir = tempdir().unwrap();
        let (backend, store_root, cache_path) = task_1515_backend(dir.path());
        let built_at = backend.cache().built_at().unwrap();
        task_1515_external_retitle(&store_root, "gen1");
        let calls = task_1515_move_head_during_reads(&store_root, 1);

        let result = backend.ensure_cache_fresh();
        task_1515_clear_hook();
        result.unwrap();

        assert_eq!(calls.get(), 2, "one declined attempt, one retry");
        let head = crate::git_ops::head_sha(&store_root).unwrap();
        let raw = rusqlite::Connection::open(&cache_path).unwrap();
        assert_eq!(
            task_1515_read(&raw),
            (Some(head), vec!["hook1".into(), "hook1".into()]),
            "the stamp is the HEAD the rows were read at"
        );
        assert_eq!(
            backend.cache().built_at().unwrap(),
            built_at,
            "the retry stayed incremental"
        );
    }

    // TASK-1515 round-2 finding 2: when HEAD keeps moving through every
    // bounded incremental attempt, the full rebuild stamps a HEAD captured
    // right before its load (the current one), not the HEAD captured before
    // the attempts, which would label newer rows with an older SHA.
    // trace:TASK-1515 | ai:claude
    #[test]
    fn task_1515_head_moving_every_attempt_full_rebuilds_at_current_head() {
        let dir = tempdir().unwrap();
        let (backend, store_root, cache_path) = task_1515_backend(dir.path());
        let first = task_1515_external_retitle(&store_root, "gen1");
        let calls = task_1515_move_head_during_reads(&store_root, usize::MAX);

        let result = backend.ensure_cache_fresh();
        task_1515_clear_hook();
        result.unwrap();

        let n = calls.get();
        assert!(n >= 2, "the incremental was retried: {n}");
        let head = crate::git_ops::head_sha(&store_root).unwrap();
        assert_ne!(head, first);
        let raw = rusqlite::Connection::open(&cache_path).unwrap();
        let last = format!("hook{n}");
        assert_eq!(
            task_1515_read(&raw),
            (Some(head), vec![last.clone(), last]),
            "the rebuild stamps the HEAD its rows were loaded at"
        );
    }

    // ---------------------------------------------------------------- BUG-1663

    thread_local! {
        /// Test hook run inside the rebuild's stable-head load loop, after
        /// the pre-load HEAD capture and before the store load — the window
        /// a mid-load external commit lands in (thread-local, so parallel
        /// tests never see each other's hook).
        // trace:BUG-1663 | ai:claude
        static BUG_1663_DURING_REBUILD_LOAD: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
            std::cell::RefCell::new(None);
    }

    /// Called from `load_at_stable_head` under `cfg(test)`.
    // trace:BUG-1663 | ai:claude
    pub(super) fn bug_1663_during_rebuild_load() {
        let hook = BUG_1663_DURING_REBUILD_LOAD.with(|h| h.borrow_mut().take());
        if let Some(mut f) = hook {
            f();
            BUG_1663_DURING_REBUILD_LOAD.with(|h| {
                let mut slot = h.borrow_mut();
                if slot.is_none() {
                    *slot = Some(f);
                }
            });
        }
    }

    // trace:BUG-1663 | ai:claude
    fn bug_1663_clear_hook() {
        BUG_1663_DURING_REBUILD_LOAD.with(|h| *h.borrow_mut() = None);
    }

    /// Raw row probe on a separate connection: no backend read, so it can
    /// never trigger the freshen it is trying to observe the absence of.
    // trace:BUG-1663 | ai:claude
    fn bug_1663_raw_row_count(cache_path: &Path, spec_id: &str) -> i64 {
        rusqlite::Connection::open(cache_path)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM requirements_cache WHERE spec_id = ?1",
                [spec_id],
                |row| row.get(0),
            )
            .unwrap()
    }

    // BUG-1663: a full rebuild used to stamp the HEAD captured BEFORE its
    // store load. A spec committed while the load ran was still projected
    // into the cache (the load reads the live tree), but its commit came
    // after the stamp; when a LATER commit deleted the spec again before any
    // refresh ran, the incremental diff from that stamp saw the file absent
    // at both endpoints — no net change — so the ghost row survived every
    // refresh until a manual full rebuild. The rebuild now re-checks HEAD
    // after the load and retries, so its stamp matches the tree the rows
    // came from and the delete is visible to the next diff.
    // trace:BUG-1663 | ai:claude
    #[test]
    fn bug_1663_spec_committed_mid_rebuild_load_then_deleted_leaves_no_ghost_row() {
        let dir = tempdir().unwrap();
        let (backend, store_root, cache_path) = task_1515_backend(dir.path());
        // Erase the recorded head so the next freshen must take the
        // full-rebuild path (nothing to diff from).
        backend.cache().set_source_head_sha("").unwrap();

        // While the rebuild's load runs, an external writer commits FR-1-004.
        let ghost: std::rc::Rc<std::cell::RefCell<Option<Requirement>>> = Default::default();
        {
            let root = store_root.clone();
            let ghost = ghost.clone();
            BUG_1663_DURING_REBUILD_LOAD.with(|h| {
                *h.borrow_mut() = Some(Box::new(move || {
                    if ghost.borrow().is_some() {
                        return; // one mid-load commit; retries see a stable tree
                    }
                    let before = crate::git_ops::head_sha(&root).unwrap();
                    let ext = GitBackend::new(&root).unwrap();
                    let req = ext
                        .add_requirement(sample_req("FR-1-004", "ghost"))
                        .unwrap();
                    assert_ne!(
                        crate::git_ops::head_sha(&root).unwrap(),
                        before,
                        "the mid-load add must land as a store commit"
                    );
                    *ghost.borrow_mut() = Some(req);
                }));
            });
        }
        let result = backend.ensure_cache_fresh();
        bug_1663_clear_hook();
        result.unwrap();
        let ghost = ghost
            .borrow_mut()
            .take()
            .expect("the hook must have run during the rebuild load");
        // The mid-load spec IS in the projection (the load read the live
        // tree), and the stamp must match that tree, not the pre-load HEAD.
        assert_eq!(
            bug_1663_raw_row_count(&cache_path, "FR-1-004"),
            1,
            "the rebuild load must have seen the mid-load commit"
        );
        let built_at = backend.cache().built_at().unwrap();

        // A later commit deletes the spec again, BEFORE any refresh runs.
        GitBackend::new(&store_root)
            .unwrap()
            .delete_requirement(&ghost.id)
            .unwrap();

        // The next refresh must converge: no ghost row.
        backend.ensure_cache_fresh().unwrap();
        assert_eq!(
            bug_1663_raw_row_count(&cache_path, "FR-1-004"),
            0,
            "a spec deleted after the rebuild must not survive as a ghost row"
        );
        assert_eq!(
            backend.cache().source_head_sha().unwrap().as_deref(),
            Some(crate::git_ops::head_sha(&store_root).unwrap().as_str())
        );
        assert_eq!(
            backend.cache().built_at().unwrap(),
            built_at,
            "the convergence must come from the incremental diff, not another full rebuild"
        );
    }

    // BUG-1663: the stable-head load retry is bounded. When an external
    // writer lands a commit during EVERY load attempt, the rebuild gives up
    // after the last one and stamps that attempt's PRE-load HEAD — older
    // than (or equal to) every row it loaded, so the next refresh still
    // re-reads the trailing commits — rather than looping forever or
    // stamping a HEAD newer than rows it read before a mid-load commit.
    // trace:BUG-1663 | ai:claude
    #[test]
    fn bug_1663_rebuild_load_retry_is_bounded_and_stamps_the_last_preload_head() {
        let dir = tempdir().unwrap();
        let (backend, store_root, cache_path) = task_1515_backend(dir.path());
        backend.cache().set_source_head_sha("").unwrap();

        let shas: std::rc::Rc<std::cell::RefCell<Vec<String>>> = Default::default();
        {
            let root = store_root.clone();
            let shas = shas.clone();
            BUG_1663_DURING_REBUILD_LOAD.with(|h| {
                *h.borrow_mut() = Some(Box::new(move || {
                    let n = shas.borrow().len() + 1;
                    let sha = task_1515_external_retitle(&root, &format!("load{n}"));
                    shas.borrow_mut().push(sha);
                }));
            });
        }
        let result = backend.ensure_cache_fresh();
        bug_1663_clear_hook();
        result.unwrap();

        let shas = shas.borrow().clone();
        assert_eq!(shas.len(), 3, "one mid-load commit per bounded attempt");
        assert_eq!(
            backend.cache().source_head_sha().unwrap().as_deref(),
            Some(shas[1].as_str()),
            "an exhausted retry stamps the final attempt's pre-load HEAD"
        );

        // Honestly stale, so the next refresh converges on the real HEAD.
        backend.ensure_cache_fresh().unwrap();
        let raw = rusqlite::Connection::open(&cache_path).unwrap();
        assert_eq!(
            task_1515_read(&raw),
            (Some(shas[2].clone()), vec!["load3".into(), "load3".into()])
        );
    }

    /// A live (pid 1) foreign writer's lock sidecar beside `cache_path`,
    /// without holding the SQLite lock.
    // trace:TASK-1515 | ai:claude
    fn task_1515_write_foreign_sidecar(cache_path: &Path) {
        let info = super::super::cache::CacheLockInfo {
            pid: 1,
            command: "foreign-writer".to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            user: "test".to_string(),
            ..Default::default()
        };
        std::fs::write(
            cache_path.with_file_name("cache.db.lock-info"),
            serde_json::to_string(&info).unwrap(),
        )
        .unwrap();
    }

    // TASK-1515 round-2 finding 3: the foreign-writer shortcut must not serve
    // an OLD-schema snapshot while a migration is pending; the read takes
    // the strict path and rebuilds.
    // trace:TASK-1515 | ai:claude
    #[test]
    fn task_1515_foreign_writer_shortcut_skipped_while_migration_pending() {
        let dir = tempdir().unwrap();
        let (backend, store_root, cache_path) = task_1515_backend(dir.path());
        drop(backend);
        task_1515_external_retitle(&store_root, "gen1");
        task_1515_mark_schema_older(&cache_path);
        task_1515_write_foreign_sidecar(&cache_path);

        let reader = CachedGitBackend::with_inner_cache_snapshot(
            GitBackend::new(&store_root).unwrap(),
            &cache_path,
        )
        .unwrap();
        assert!(reader.cache().migration_pending());
        let rows = reader.list_summaries(&ListFilter::default()).unwrap();
        assert!(!reader.cache().migration_pending(), "the read rebuilt");
        let title = rows
            .iter()
            .find(|r| r.spec_id.as_deref() == Some("FR-1-001"))
            .map(|r| r.title.clone());
        assert_eq!(title.as_deref(), Some("gen1"));
        assert!(!reader.cache_snapshot_is_stale().unwrap());
    }

    /// Rewrite the on-disk schema version one lower, so the next open sees a
    /// pending migration.
    // trace:TASK-1515 | ai:claude
    fn task_1515_mark_schema_older(cache_path: &Path) {
        let raw = rusqlite::Connection::open(cache_path).unwrap();
        let current: String = raw
            .query_row(
                "SELECT value FROM cache_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let older = (current.parse::<i64>().unwrap() - 1).to_string();
        raw.execute(
            "UPDATE cache_meta SET value = ?1 WHERE key = 'schema_version'",
            rusqlite::params![older],
        )
        .unwrap();
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    fn reader_incremental_lock_error_does_not_full_rebuild() {
        use super::super::cache_refresh::*;
        for sidecar in [false, true] {
            let dir = tempdir().unwrap();
            let (backend, store_root, cache_path) = task_1515_backend(dir.path());
            let from = backend.cache().source_head_sha().unwrap().unwrap();
            let built_at = backend.cache().built_at().unwrap();
            task_1515_external_retitle(&store_root, "gen1");
            if sidecar {
                task_1515_write_foreign_sidecar(&cache_path);
            }
            let holder = rusqlite::Connection::open(&cache_path).unwrap();
            holder.execute_batch("BEGIN IMMEDIATE").unwrap();
            for constructor in [false, true] {
                let scope = CacheReadScope::new();
                test_counts();
                if constructor {
                    let opened = CachedGitBackend::open(&store_root, &cache_path).unwrap();
                    let counts = test_counts();
                    assert_eq!(counts.get("write_attempt"), Some(&1));
                    assert_eq!(counts.get("full_rebuild"), None);
                    assert!(!scope.touched(), "a constructor has not served rows");
                    assert_eq!(
                        opened.list_summaries(&ListFilter::default()).unwrap().len(),
                        3
                    );
                } else {
                    assert_eq!(
                        backend
                            .list_summaries(&ListFilter::default())
                            .unwrap()
                            .len(),
                        3
                    );
                }
                let counts = test_counts();
                assert_eq!(counts.get("write_attempt"), Some(&1));
                assert_eq!(counts.get("retry_sleep"), None);
                assert_eq!(counts.get("full_rebuild"), None);
                let m = scope.metadata();
                assert_eq!(m["refreshing"], "writer_busy");
                assert_eq!(m["cache_head"], from);
                assert_eq!(m["built_at"], serde_json::json!(built_at));
            }
            holder.execute_batch("ROLLBACK").unwrap();
            assert_eq!(
                task_1515_read(&holder),
                (Some(from), vec!["gen0".into(), "gen0".into()])
            );
            holder.execute_batch("ROLLBACK").ok();
        }
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    fn writer_busy_next_reader_retries_incremental_after_lock_release() {
        use super::super::cache_refresh::*;
        let dir = tempdir().unwrap();
        let (backend, store, path) = task_1515_backend(dir.path());
        let to = task_1515_external_retitle(&store, "gen1");
        let holder = rusqlite::Connection::open(&path).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        {
            let s = CacheReadScope::new();
            backend.list_summaries(&ListFilter::default()).unwrap();
            assert_eq!(s.metadata()["refreshing"], "writer_busy");
        }
        holder.execute_batch("ROLLBACK").unwrap();
        let s = CacheReadScope::new();
        test_counts();
        backend.list_summaries(&ListFilter::default()).unwrap();
        assert_eq!(s.metadata()["stale"], false);
        assert_eq!(backend.cache().source_head_sha().unwrap(), Some(to));
        assert_eq!(test_counts().get("incremental"), Some(&1));
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    fn strict_incremental_lock_error_propagates_without_full_rebuild() {
        use super::super::cache_refresh::*;
        let dir = tempdir().unwrap();
        let (backend, store, path) = task_1515_backend(dir.path());
        task_1515_external_retitle(&store, "gen1");
        let holder = rusqlite::Connection::open(&path).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        backend.list_summaries(&ListFilter::default()).unwrap();
        test_counts();
        let prev = super::super::cache::set_fast_fail_cache(true);
        let result = backend.ensure_cache_fresh();
        super::super::cache::set_fast_fail_cache(prev);
        assert!(is_cache_lock_error(&result.unwrap_err()));
        let counts = test_counts();
        assert!(counts["write_attempt"] > 1, "strict ladder restored");
        assert!(counts["retry_sleep"] > 0);
        assert_eq!(counts.get("full_rebuild"), None);
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    fn non_tty_full_rebuild_winner_files_request_and_serves_deferred_when_spawn_refused() {
        use super::super::cache_refresh::*;
        for stamp in ["", "missing", "non-ancestor", "large-diff", "decline"] {
            let dir = tempdir().unwrap();
            let (backend, store, path) = task_1515_backend(dir.path());
            task_1515_external_retitle(&store, "gen1");
            if stamp.is_empty() || stamp == "non-ancestor" {
                backend.cache().set_source_head_sha(stamp).unwrap();
            }
            if stamp == "missing" {
                rusqlite::Connection::open(&path)
                    .unwrap()
                    .execute("DELETE FROM cache_meta WHERE key='source_head_sha'", [])
                    .unwrap();
            }
            if stamp == "large-diff" {
                for n in 0..501 {
                    crate::object_store::write_object(
                        &store.join("objects"),
                        &sample_req(&format!("TASK-{n}"), "bulk"),
                    )
                    .unwrap();
                }
                assert!(std::process::Command::new("git")
                    .args(["add", "."])
                    .current_dir(&store)
                    .status()
                    .unwrap()
                    .success());
                assert!(std::process::Command::new("git")
                    .args(["commit", "-qm", "bulk"])
                    .current_dir(&store)
                    .status()
                    .unwrap()
                    .success());
            }
            if stamp == "decline" {
                std::fs::remove_file(
                    crate::object_store::object_path(&store.join("objects"), "FR-1-001").unwrap(),
                )
                .unwrap();
            }
            // A real unrelated commit is a readable non-ancestor head.
            if stamp == "non-ancestor" {
                let other = dir.path().join("other");
                std::fs::create_dir(&other).unwrap();
                crate::git_ops::init(&other).unwrap();
                crate::git_ops::configure_user(&other, "T", "t@t.test").unwrap();
                std::fs::write(other.join("file"), "other").unwrap();
                std::process::Command::new("git")
                    .args(["add", "."])
                    .current_dir(&other)
                    .output()
                    .unwrap();
                std::process::Command::new("git")
                    .args(["commit", "-m", "other"])
                    .current_dir(&other)
                    .output()
                    .unwrap();
                std::process::Command::new("git")
                    .arg("fetch")
                    .arg(&other)
                    .arg("HEAD")
                    .current_dir(&store)
                    .output()
                    .unwrap();
                backend
                    .cache()
                    .set_source_head_sha(&crate::git_ops::head_sha(&other).unwrap())
                    .unwrap();
            }
            let scope = CacheReadScope::new();
            scope.configure(false, Some(ReadBudget(std::time::Duration::ZERO)));
            test_counts();
            let start = std::time::Instant::now();
            let rows = backend.list_summaries(&ListFilter::default()).unwrap();
            assert!(start.elapsed() < std::time::Duration::from_secs(2));
            assert_eq!(rows.len(), 3);
            assert!(rows.iter().any(|r| r.title == "gen0"));
            assert_eq!(scope.metadata()["refreshing"], "deferred");
            let counts = test_counts();
            assert_eq!(counts.get("full_rebuild"), None);
            assert_eq!(counts.get("write_attempt"), None);
            // TASK-1527: the winner now files the durable request before
            // serving stale; the cfg(test) spawn refusal is recorded on it,
            // which is the Deferred-not-Requested evidence.
            // trace:TASK-1527 | ai:claude
            let request = super::super::refresh_request::load(&path)
                .expect("the deferred winner must leave a refresh request");
            assert_eq!(
                request.target_head,
                crate::git_ops::head_sha(&store).unwrap()
            );
            assert!(request
                .last_error()
                .is_some_and(|e| e.contains("spawn failed")));
        }
    }

    /// Amendment A7's named test — the READER half: three failed attempts
    /// recorded on the request stop the spawning, and the reader takes
    /// today's strict inline path instead of serving stale forever. The
    /// failures here are recorded by the cfg(test) spawn refusal; the other
    /// two recorders feed the same counter and are pinned separately (the
    /// reaper in `refresh_worker::tests::reaper_records_abnormal_exits_and_
    /// ignores_clean_ones`, the worker's own error path in `cache_cmd`'s
    /// integration tests).
    // trace:TASK-1527 | ai:claude
    #[test]
    fn worker_crash_loop_stops_spawning() {
        use super::super::cache_refresh::*;
        use super::super::refresh_request;
        let dir = tempdir().unwrap();
        let (backend, store, path) = task_1515_backend(dir.path());
        task_1515_external_retitle(&store, "gen1");
        backend.cache().set_source_head_sha("").unwrap();
        // Three stale reads: each files the request and records the refused
        // spawn (cfg(test) never forks), serving stale honestly each time.
        for _ in 0..3 {
            let scope = CacheReadScope::new();
            scope.configure(false, Some(ReadBudget(std::time::Duration::ZERO)));
            backend.list_summaries(&ListFilter::default()).unwrap();
            assert_eq!(scope.metadata()["refreshing"], "deferred");
        }
        let failures = refresh_request::load(&path).unwrap();
        assert_eq!(
            failures
                .attempts
                .iter()
                .filter(|a| a.error.is_some())
                .count(),
            3
        );
        assert!(failures.spawning_suppressed(chrono::Utc::now()));
        // The fourth reader must stop spawning and refresh strictly inline.
        test_counts();
        let scope = CacheReadScope::new();
        scope.configure(false, Some(ReadBudget(std::time::Duration::ZERO)));
        let rows = backend.list_summaries(&ListFilter::default()).unwrap();
        assert!(
            rows.iter().any(|r| r.title == "gen1"),
            "served CURRENT data"
        );
        assert!(
            scope.stale().is_none(),
            "an inline refresh is not a stale serve"
        );
        assert_eq!(
            refresh_request::load(&path)
                .unwrap()
                .attempts
                .iter()
                .filter(|a| a.error.is_some())
                .count(),
            3,
            "the suppressed reader must not record a fourth attempt"
        );
    }

    /// A successful spawn labels the read `requested` and leaves the request
    /// targeting the head the worker must reach.
    // trace:TASK-1527 | ai:claude
    #[test]
    fn deferred_winner_with_a_live_spawn_serves_requested() {
        use super::super::cache_refresh::*;
        use super::super::{refresh_request, refresh_worker};
        let dir = tempdir().unwrap();
        let (backend, store, path) = task_1515_backend(dir.path());
        task_1515_external_retitle(&store, "gen1");
        backend.cache().set_source_head_sha("").unwrap();
        refresh_worker::SPAWN_LOG.with(|log| *log.borrow_mut() = Some(Vec::new()));
        let scope = CacheReadScope::new();
        scope.configure(false, Some(ReadBudget(std::time::Duration::ZERO)));
        backend.list_summaries(&ListFilter::default()).unwrap();
        assert_eq!(scope.metadata()["refreshing"], "requested");
        let spawned = refresh_worker::SPAWN_LOG.with(|log| log.borrow_mut().take().unwrap());
        assert_eq!(spawned.len(), 1, "exactly one worker spawn per stale read");
        let request = refresh_request::load(&path).unwrap();
        assert_eq!(
            request.target_head,
            crate::git_ops::head_sha(&store).unwrap()
        );
        assert!(request.last_error().is_none());
    }

    // trace:BUG-1778 trace:BUG-1777 | ai:codex
    #[test]
    fn failed_hint_write_degrades_to_deferred() {
        use super::super::cache_refresh::*;
        use super::super::refresh_request;
        use super::super::refresh_worker;
        let dir = tempdir().unwrap();
        let (backend, store, path) = task_1515_backend(dir.path());
        task_1515_external_retitle(&store, "gen1");
        backend.cache().set_source_head_sha("").unwrap();
        refresh_worker::SPAWN_LOG.with(|log| *log.borrow_mut() = Some(Vec::new()));

        // A directory at the request-file path makes the atomic rename fail
        // consistently on Unix and Windows; readonly directory permissions
        // do not prevent writes on Windows.
        let request_path = refresh_request::refresh_request_path(&path);
        std::fs::create_dir(&request_path).unwrap();

        let scope = CacheReadScope::new();
        scope.configure(false, Some(ReadBudget(std::time::Duration::ZERO)));

        let result = backend.list_summaries(&ListFilter::default());

        std::fs::remove_dir(&request_path).unwrap();

        assert!(result.is_ok(), "read must not fail when hint write fails");
        assert_eq!(scope.metadata()["refreshing"], "deferred");
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    fn deferred_full_rebuild_repeats_honestly_until_strict_refresh() {
        use super::super::cache_refresh::*;
        let dir = tempdir().unwrap();
        let (backend, store, _) = task_1515_backend(dir.path());
        task_1515_external_retitle(&store, "gen1");
        backend.cache().set_source_head_sha("").unwrap();
        for _ in 0..2 {
            let scope = CacheReadScope::new();
            backend.list_summaries(&ListFilter::default()).unwrap();
            assert_eq!(scope.metadata()["refreshing"], "deferred");
        }
        backend.rebuild_cache().unwrap();
        let scope = CacheReadScope::new();
        backend.list_summaries(&ListFilter::default()).unwrap();
        assert_eq!(scope.metadata()["stale"], false);
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    fn tty_full_rebuild_winner_remains_strict_before_c() {
        use super::super::cache_refresh::*;
        let dir = tempdir().unwrap();
        let (backend, store, _) = task_1515_backend(dir.path());
        task_1515_external_retitle(&store, "gen1");
        backend.cache().set_source_head_sha("").unwrap();
        let scope = CacheReadScope::new();
        scope.configure(true, None);
        test_counts();
        backend.list_summaries(&ListFilter::default()).unwrap();
        assert_eq!(scope.metadata()["stale"], false);
        assert_eq!(test_counts().get("full_rebuild"), Some(&1));
    }

    /// BUG-1674's ceiling shape: a TTY reader whose stale cache needs a FULL
    /// rebuild while a live foreign writer holds the SQLite write lock. The
    /// inline rebuild gets exactly one bounded attempt — never the ~25 s
    /// retry ladder followed by a hard error — and the winner degrades to
    /// the reader protocol: durable request filed, labelled committed
    /// snapshot served, exit success.
    // trace:BUG-1674 trace:BUG-1674.ac93280e | ai:claude
    #[test]
    fn tty_full_rebuild_winner_bounded_behind_live_writer() {
        use super::super::cache_refresh::*;
        use super::super::refresh_request;
        let dir = tempdir().unwrap();
        let (backend, store, path) = task_1515_backend(dir.path());
        task_1515_external_retitle(&store, "gen1");
        backend.cache().set_source_head_sha("").unwrap();
        let holder = rusqlite::Connection::open(&path).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        let scope = CacheReadScope::new();
        scope.configure(true, None);
        test_counts();
        let start = std::time::Instant::now();
        let rows = backend.list_summaries(&ListFilter::default()).unwrap();
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "a contended TTY read must stay bounded, took {:?}",
            start.elapsed()
        );
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter().any(|r| r.title == "gen0"),
            "served the committed snapshot"
        );
        // cfg(test) refuses the detached spawn, so the filed request reports
        // Deferred; outside tests this is Requested with a live worker.
        assert_eq!(scope.metadata()["refreshing"], "deferred");
        assert_eq!(scope.metadata()["stale"], true);
        let counts = test_counts();
        assert_eq!(counts.get("full_rebuild"), Some(&1), "one attempt");
        assert_eq!(counts.get("retry_sleep"), None, "never entered the ladder");
        let request = refresh_request::load(&path)
            .expect("the contended winner must leave a durable refresh request");
        assert_eq!(
            request.target_head,
            crate::git_ops::head_sha(&store).unwrap()
        );
        holder.execute_batch("ROLLBACK").unwrap();
        // With the writer gone the next strict refresh restores freshness.
        backend.rebuild_cache().unwrap();
        let scope = CacheReadScope::new();
        let rows = backend.list_summaries(&ListFilter::default()).unwrap();
        assert_eq!(scope.metadata()["stale"], false);
        assert!(rows.iter().any(|r| r.title == "gen1"));
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    fn flock_unsupported_falls_back_to_strict_refresh() {
        use super::super::cache_refresh::*;
        let dir = tempdir().unwrap();
        let (backend, store, _) = task_1515_backend(dir.path());
        task_1515_external_retitle(&store, "gen1");
        backend.cache().set_source_head_sha("").unwrap();
        let scope = CacheReadScope::new();
        UNSUPPORTED.with(|c| c.set(true));
        test_counts();
        let result = backend.list_summaries(&ListFilter::default());
        UNSUPPORTED.with(|c| c.set(false));
        result.unwrap();
        assert_eq!(scope.metadata()["stale"], false);
        assert_eq!(test_counts().get("full_rebuild"), Some(&1));
    }
    // trace:TASK-1526 | ai:codex
    #[test]
    fn single_flight_four_readers_one_refresh() {
        use super::super::cache_refresh::*;
        let dir = tempdir().unwrap();
        let (_, store, path) = task_1515_backend(dir.path());
        task_1515_external_retitle(&store, "gen1");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let (store, path, barrier) = (store.clone(), path.clone(), barrier.clone());
                std::thread::spawn(move || {
                    let scope = CacheReadScope::new();
                    // The property under test is single flight, not latency: exactly one
                    // reader refreshes and the other three wait for that refresh rather
                    // than serving stale rows. With the default 1500ms ReadBudget the
                    // waiters abandon the wait on a loaded host, serve pre-refresh rows,
                    // and fail the freshness assertions below for a reason that has
                    // nothing to do with single flight (BUG-1729 AC1). A generous
                    // per-thread budget makes those assertions load-independent while
                    // still bounding the test if the refresh never lands. Each reader
                    // thread owns its own scope, so this configures only itself.
                    scope.configure(false, Some(ReadBudget(std::time::Duration::from_secs(60))));
                    let backend = CachedGitBackend::with_inner_cache_snapshot(
                        GitBackend::new(&store).unwrap(),
                        &path,
                    )
                    .unwrap();
                    barrier.wait();
                    test_counts();
                    let rows = backend.list_summaries(&ListFilter::default()).unwrap();
                    assert!(rows.iter().any(|r| r.title == "gen1"));
                    assert_eq!(scope.metadata()["stale"], false);
                    test_counts().get("incremental").copied().unwrap_or(0)
                })
            })
            .collect();
        assert_eq!(
            threads
                .into_iter()
                .map(|t| t.join().unwrap())
                .sum::<usize>(),
            1
        );
    }

    /// What the holder thread reports back: the attempt count it needed to take
    /// the flock, or why it gave up.
    // trace:BUG-1736 | ai:claude
    type HeldReady = std::result::Result<usize, String>;

    /// Hold the refresh flock for `path` on another thread until told to stop.
    ///
    /// Not a bare `try_acquire`: unlike the fresh-tempdir acquires in
    /// `cache_refresh`'s own tests, production code has already opened and
    /// closed descriptors to *this* cache's refresh sidecar by the time the
    /// fixture returns, so a sibling test thread that forks during one of those
    /// windows leaves its child an inherited descriptor to the same open file
    /// description. The child keeps the flock until it reaches `exec`, and this
    /// acquire then fails with `Ok(None)` while no test logically holds the
    /// lock. That is not hypothetical: this helper's acquire panicked on
    /// ubuntu-latest in run 36702526804.
    // trace:BUG-1736 | ai:claude
    fn held_refresh(path: PathBuf) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
        let (stop_tx, thread, _attempts) =
            held_refresh_within(path, super::super::cache_refresh::FORK_WINDOW_BOUND);
        (stop_tx, thread)
    }

    /// `held_refresh` with the acquire bound supplied by the caller, and the
    /// holder's attempt count returned.
    // trace:BUG-1736 | ai:claude
    fn held_refresh_within(
        path: PathBuf,
        bound: std::time::Duration,
    ) -> (
        std::sync::mpsc::Sender<()>,
        std::thread::JoinHandle<()>,
        usize,
    ) {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<HeldReady>();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let acquired =
                super::super::cache_refresh::acquire_past_fork_window_within(&path, bound);
            let _lock = match acquired {
                Ok(Some((lock, attempts))) => {
                    if ready_tx.send(Ok(attempts)).is_err() {
                        return;
                    }
                    lock
                }
                Ok(None) => {
                    let _ = ready_tx.send(Err(format!(
                        "the refresh flock for {} was still held after {bound:?}; a sibling \
                         thread's forked child may hold an inherited descriptor to it — see \
                         BUG-1729. {}",
                        path.display(),
                        super::super::cache_refresh::RefreshLock::diagnose_unavailable(&path),
                    )));
                    return;
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(format!(
                        "acquiring the refresh flock for {} errored: {e}",
                        path.display()
                    )));
                    return;
                }
            };
            stop_rx.recv().unwrap();
        });
        let (attempts, thread) = await_holder_ready(ready_rx, thread);
        (stop_tx, thread, attempts)
    }

    /// Wait for the holder to arm the ready channel, and attribute a failure to
    /// the holder rather than to this line.
    ///
    /// A bare `ready_rx.recv().unwrap()` turns any panic on the holder thread
    /// into `RecvError` reported at the *waiter's* location, which names the
    /// wrong line and discards the holder's message entirely. Joining and
    /// re-raising the holder's payload puts the real panic — with the real line
    /// — in the test output.
    // trace:BUG-1736 | ai:claude
    fn await_holder_ready(
        ready_rx: std::sync::mpsc::Receiver<HeldReady>,
        thread: std::thread::JoinHandle<()>,
    ) -> (usize, std::thread::JoinHandle<()>) {
        match ready_rx.recv() {
            Ok(Ok(attempts)) => (attempts, thread),
            Ok(Err(why)) => panic!("{why}"),
            Err(std::sync::mpsc::RecvError) => match thread.join() {
                Err(payload) => std::panic::resume_unwind(payload),
                Ok(()) => panic!(
                    "the refresh holder exited without arming the ready channel and \
                     without panicking"
                ),
            },
        }
    }

    /// The defective site, driven through the failure that panicked it in CI.
    ///
    /// A foreign thread holds the flock across `held_refresh_within`'s first
    /// attempt, so the holder it spawns must lose that attempt and win a later
    /// one. Before the fix this was `try_acquire().unwrap().unwrap()` and the
    /// `Ok(None)` panicked the holder thread outright.
    // trace:BUG-1736 | ai:claude
    #[test]
    fn held_refresh_waits_out_a_contended_flock_instead_of_panicking() {
        let dir = tempdir().unwrap();
        let (_backend, _store, path) = task_1515_backend(dir.path());
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let p = path.clone();
        let foreign = std::thread::spawn(move || {
            let lock = super::super::cache_refresh::acquire_past_fork_window(&p)
                .unwrap()
                .expect("the foreign holder could not take the flock at all");
            ready_tx.send(()).unwrap();
            let _ = release_rx.recv();
            drop(lock);
        });
        ready_rx
            .recv()
            .expect("the foreign holder died before taking the flock");
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(120));
            let _ = release_tx.send(());
        });

        let (stop, thread, attempts) =
            held_refresh_within(path, super::super::cache_refresh::FORK_WINDOW_BOUND);
        assert!(
            attempts > 1,
            "the holder took the flock on attempt {attempts}, so it never had to \
             wait: the foreign holder was not contending and this test proves nothing"
        );
        stop.send(()).unwrap();
        thread.join().unwrap();
        releaser.join().unwrap();
        foreign.join().unwrap();
    }

    /// A panic on the holder thread must be reported as the holder's panic.
    ///
    /// `ready_rx.recv().unwrap()` reports `RecvError` at the *waiter's* line and
    /// discards the holder's message, which is how the CI failure this bug was
    /// filed from named the wrong location. The holder's deliberate panic below
    /// prints one expected line to the suite's stderr; it identifies itself.
    // trace:BUG-1736 | ai:claude
    #[test]
    fn a_dead_holder_is_reported_as_its_own_panic_not_the_waiters_recverror() {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<HeldReady>();
        let thread = std::thread::spawn(move || {
            let _keep_the_sender_alive_until_the_unwind = ready_tx;
            panic!("BUG-1736 expected panic: holder died before arming the ready channel");
        });

        let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            await_holder_ready(ready_rx, thread)
        }))
        .err()
        .expect("the waiter returned although the holder never armed the channel");
        let msg = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("<non-string panic payload>")
            .to_string();

        assert!(
            msg.contains("holder died before arming the ready channel"),
            "the waiter raised {msg:?} instead of re-raising the holder's own payload"
        );
        assert!(
            !msg.contains("RecvError"),
            "the waiter reported its own RecvError rather than the holder's panic: {msg:?}"
        );
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    fn reader_path_never_calls_with_cache_write() {
        use super::super::cache_refresh::*;
        let dir = tempdir().unwrap();
        let (backend, store, path) = task_1515_backend(dir.path());
        task_1515_external_retitle(&store, "gen1");
        let (stop, thread) = held_refresh(path);
        let scope = CacheReadScope::new();
        scope.configure(
            false,
            Some(ReadBudget(std::time::Duration::from_millis(20))),
        );
        test_counts();
        backend.list_summaries(&ListFilter::default()).unwrap();
        assert_eq!(scope.metadata()["refreshing"], "worker_running");
        assert_eq!(test_counts().get("write_attempt"), None);
        stop.send(()).unwrap();
        thread.join().unwrap();
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    fn lock_winner_rechecks_is_stale_and_skips_work() {
        use super::super::cache_refresh::*;
        let dir = tempdir().unwrap();
        let (backend, store, path) = task_1515_backend(dir.path());
        task_1515_external_retitle(&store, "gen1");
        let writer =
            CachedGitBackend::with_inner_cache_snapshot(GitBackend::new(&store).unwrap(), &path)
                .unwrap();
        BEFORE_ACQUIRE.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                writer.ensure_cache_fresh().unwrap();
                test_counts();
            }))
        });
        let scope = CacheReadScope::new();
        backend.list_summaries(&ListFilter::default()).unwrap();
        assert!(
            test_counts().is_empty(),
            "winner must double-check after another refresher committed"
        );
        assert_eq!(scope.metadata()["stale"], false);
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    #[cfg(unix)]
    fn refresh_holder_exits_without_refresh_returns_deferred() {
        use super::super::cache_refresh::*;
        use std::io::{BufRead, Write};
        for crash in [false, true] {
            let dir = tempdir().unwrap();
            let (backend, store, path) = task_1515_backend(dir.path());
            task_1515_external_retitle(&store, "gen1");
            let lock_path = super::super::cache_lock::cache_sidecar_path(&path, "refresh.lock");
            let mut child = std::process::Command::new("python3").args(["-c", "import fcntl,sys; f=open(sys.argv[1],'w'); fcntl.flock(f,fcntl.LOCK_EX); print('ready',flush=True); sys.stdin.readline()"]).arg(lock_path)
                .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(child.stdout.take().unwrap())
                .read_line(&mut line)
                .unwrap();
            assert_eq!(line.trim(), "ready");
            let thread = std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(30));
                if crash {
                    child.kill().unwrap();
                } else {
                    child.stdin.as_mut().unwrap().write_all(b"\n").unwrap();
                }
                child.wait().unwrap();
            });
            let scope = CacheReadScope::new();
            let start = std::time::Instant::now();
            backend.list_summaries(&ListFilter::default()).unwrap();
            thread.join().unwrap();
            assert_eq!(scope.metadata()["refreshing"], "deferred");
            assert!(start.elapsed() < std::time::Duration::from_secs(1));
        }
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    fn stale_rows_and_label_metadata_share_snapshot() {
        use super::super::cache_refresh::*;
        let dir = tempdir().unwrap();
        let (backend, store, path) = task_1515_backend(dir.path());
        let from = backend.cache().source_head_sha().unwrap();
        let built = backend.cache().built_at().unwrap();
        task_1515_external_retitle(&store, "gen1");
        let (stop, thread) = held_refresh(path.clone());
        let scope = CacheReadScope::new();
        scope.configure(false, Some(ReadBudget(std::time::Duration::ZERO)));
        let rows = backend
            .tolerant_read(|snapshot| {
                // Commit after the reader pinned metadata, before it reads rows.
                let writer = CachedGitBackend::with_inner_cache_snapshot(
                    GitBackend::new(&store).unwrap(),
                    &path,
                )
                .unwrap();
                writer.rebuild_cache().unwrap();
                snapshot.list_summaries(&ListFilter::default())
            })
            .unwrap();
        assert!(rows.iter().any(|r| r.title == "gen0"));
        assert_eq!(scope.metadata()["cache_head"], serde_json::json!(from));
        assert_eq!(scope.metadata()["built_at"], serde_json::json!(built));
        assert_eq!(scope.metadata()["stale"], true);
        stop.send(()).unwrap();
        thread.join().unwrap();
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    fn migration_pending_reader_never_serves_old_schema() {
        use super::super::cache_refresh::*;
        // The shipped window is 15s. Spending it here puts 15 wall-clock
        // seconds in every suite run and leaves the upper bound a race against
        // the runner's scheduler, so pin the constant and measure a short
        // injected window instead. trace:TASK-1526 | ai:claude
        assert_eq!(MIGRATION_WAIT, std::time::Duration::from_secs(15));
        set_migration_wait_limit(Some(std::time::Duration::from_millis(400)));
        let dir = tempdir().unwrap();
        let (backend, store, path) = task_1515_backend(dir.path());
        drop(backend);
        task_1515_mark_schema_older(&path);
        let backend =
            CachedGitBackend::with_inner_cache_snapshot(GitBackend::new(&store).unwrap(), &path)
                .unwrap();
        // Even without a holder, an advisory invocation cannot do a migration.
        test_counts();
        assert!(backend
            .list_summaries_with_budget(
                &ListFilter::default(),
                ReadBudget(std::time::Duration::ZERO)
            )
            .unwrap_err()
            .is::<AdvisoryCacheUnavailable>());
        assert_eq!(test_counts().get("full_rebuild"), None);
        let (stop, thread) = held_refresh(path.clone());
        let scope = CacheReadScope::new();
        scope.configure(false, Some(ReadBudget(std::time::Duration::ZERO)));
        assert!(backend
            .list_summaries(&ListFilter::default())
            .unwrap_err()
            .to_string()
            .contains("being upgraded"));
        assert!(!scope.touched());
        drop(scope);
        let scope = CacheReadScope::new();
        let start = std::time::Instant::now();
        assert!(backend
            .list_summaries(&ListFilter::default())
            .unwrap_err()
            .to_string()
            .contains("being upgraded"));
        // Bounded by the injected window, not unbounded and not instant. The
        // ceiling is loose on purpose: it proves the wait ended, and a tight
        // one only measures how contended the runner was.
        assert!(start.elapsed() >= std::time::Duration::from_millis(400));
        assert!(start.elapsed() < std::time::Duration::from_secs(4));
        assert!(!scope.touched());
        stop.send(()).unwrap();
        thread.join().unwrap();
        // "Re-run shortly" means a NEW invocation, which gets its own wait
        // budget. Re-using the scope whose deadline the wait above already
        // spent leaves the migrating read zero tolerance for any transient
        // contention on the refresh flock, and it then fails instead of
        // migrating -- the shape this test failed with in CI. Hold the flock
        // briefly across the re-run so a spent budget cannot pass.
        // trace:TASK-1526 | ai:claude
        drop(scope);
        let _scope = CacheReadScope::new();
        set_migration_wait_limit(Some(std::time::Duration::from_secs(10)));
        let (release, holder) = held_refresh(path);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let _ = release.send(());
        });
        backend.list_summaries(&ListFilter::default()).unwrap();
        holder.join().unwrap();
        assert!(!backend.cache().migration_pending());
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    fn refresh_wait_refuses_while_store_or_sqlite_write_lock_held() {
        use super::super::cache_refresh::*;
        let dir = tempdir().unwrap();
        let (backend, store, path) = task_1515_backend(dir.path());
        task_1515_external_retitle(&store, "gen1");
        let (stop, thread) = held_refresh(path.clone());
        let _guard = super::super::store_lock::acquire(&store).unwrap();
        assert!(!may_wait());
        let start = std::time::Instant::now();
        backend.list_summaries(&ListFilter::default()).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_millis(500));
        drop(_guard);
        let other =
            CachedGitBackend::with_inner_cache_snapshot(GitBackend::new(&store).unwrap(), &path)
                .unwrap();
        let head = backend.current_head_sha();
        backend
            .cache
            .apply_incremental(&head, &store, |_| {
                assert!(!may_wait());
                let start = std::time::Instant::now();
                assert_eq!(
                    other.list_summaries(&ListFilter::default()).unwrap().len(),
                    3
                );
                assert!(start.elapsed() < std::time::Duration::from_millis(500));
                Ok(false)
            })
            .unwrap();
        // Release the holder like every other single-flight test in this file;
        // leaving it parked leaks the thread and drops its stop channel at an
        // arbitrary point. trace:TASK-1526 | ai:claude
        stop.send(()).unwrap();
        thread.join().unwrap();
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    fn head_recapture_after_incremental_err_break_stamps_current_head() {
        let dir = tempdir().unwrap();
        let (backend, store, path) = task_1515_backend(dir.path());
        task_1515_external_retitle(&store, "gen1");
        task_1515_move_head_during_reads(&store, 1);
        super::super::cache_refresh::INCREMENTAL_ERROR.with(|c| c.set(true));
        let result = backend.ensure_cache_fresh();
        super::super::cache_refresh::INCREMENTAL_ERROR.with(|c| c.set(false));
        task_1515_clear_hook();
        result.unwrap();
        assert_eq!(
            task_1515_read(&rusqlite::Connection::open(path).unwrap()),
            (
                Some(crate::git_ops::head_sha(&store).unwrap()),
                vec!["hook1".into(), "hook1".into()]
            )
        );
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    fn epic_status_preresolved_before_incremental_txn() {
        use super::super::cache_refresh::*;
        let dir = tempdir().unwrap();
        let store = dir.path().join("store");
        let path = dir.path().join("cache.db");
        let (backend, _, child) = force_closed_epic_with_open_child(&store, &path);
        let mut req = backend.get_requirement(&child).unwrap().unwrap();
        req.title = "changed".into();
        GitBackend::new(&store)
            .unwrap()
            .update_requirement(&req)
            .unwrap();
        test_counts();
        backend.ensure_cache_fresh().unwrap();
        let counts = test_counts();
        assert!(counts.get("epic_before_txn").copied().unwrap_or(0) > 0);
        assert_eq!(counts.get("epic_inside_txn"), None);
    }
    // trace:TASK-1526 | ai:codex
    #[test]
    fn nested_freshen_in_lock_holder_does_not_wait() {
        use super::super::cache_refresh::*;
        let dir = tempdir().unwrap();
        let (backend, store, path) = task_1515_backend(dir.path());
        task_1515_external_retitle(&store, "gen1");
        let scope = CacheReadScope::new();
        // Not a bare `try_acquire`, for the reason given on `held_refresh`: the
        // fixture has already driven production acquires against this sidecar,
        // so an inherited descriptor from a sibling thread's fork can make this
        // return `Ok(None)` with nothing logically holding the lock.
        // trace:BUG-1736 | ai:claude
        let _lock = acquire_past_fork_window(&path).unwrap().unwrap();
        test_counts();
        let start = std::time::Instant::now();
        backend.list_summaries(&ListFilter::default()).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_millis(500));
        assert_eq!(scope.metadata()["stale"], false);
        assert!(test_counts().is_empty());
    }

    // trace:TASK-1526 trace:BUG-1777 | ai:codex
    #[test]
    fn loser_budget_is_shared_across_backend_opens() {
        use super::super::cache_refresh::*;
        let dir = tempdir().unwrap();
        let (backend, store, path) = task_1515_backend(dir.path());
        task_1515_external_retitle(&store, "gen1");
        let (stop, thread) = held_refresh(path.clone());
        let scope = CacheReadScope::new();
        scope.configure(
            false,
            Some(ReadBudget(std::time::Duration::from_millis(300))),
        );
        backend.list_summaries(&ListFilter::default()).unwrap();
        let second = CachedGitBackend::open(&store, &path).unwrap();
        let start = std::time::Instant::now();
        second.list_summaries(&ListFilter::default()).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_millis(250));
        assert_eq!(scope.metadata()["refreshing"], "worker_running");
        stop.send(()).unwrap();
        thread.join().unwrap();
    }
    // trace:TASK-1526 | ai:codex
    #[test]
    fn already_open_schema_change_repairs_or_advisory_exits() {
        use super::super::cache_refresh::*;
        let dir = tempdir().unwrap();
        let (backend, _, path) = task_1515_backend(dir.path());
        rusqlite::Connection::open(path)
            .unwrap()
            .execute_batch("DROP TABLE requirements_fts")
            .unwrap();
        let scope = CacheReadScope::new();
        test_counts();
        let err = backend
            .list_summaries_with_budget(
                &ListFilter::default(),
                ReadBudget(std::time::Duration::ZERO),
            )
            .unwrap_err();
        assert!(err.is::<AdvisoryCacheUnavailable>());
        assert!(!scope.touched());
        assert_eq!(test_counts().get("write_attempt"), None);
        assert_eq!(
            backend
                .list_summaries(&ListFilter::default())
                .unwrap()
                .len(),
            3
        );
        assert_eq!(scope.metadata()["stale"], false);
    }
}
