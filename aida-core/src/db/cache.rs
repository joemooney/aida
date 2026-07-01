//! SQLite cache projection for the git-canonical store.
//!
//! Per EPIC-1-001 / docs/plans/2026-05-02-git-canonical-storage.md:
//! the git orphan store is the source of truth. This module maintains a
//! rebuildable SQLite read cache used for fast list/filter/search queries.
//! It is never authoritative — drop and rebuild any time.
//!
//! trace:EPIC-1-001 | ai:claude

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use uuid::Uuid;

use crate::models::{Relationship, RelationshipType, Requirement, RequirementsStore};
use std::collections::{HashMap, HashSet};

/// Lightweight projection of a Requirement, sourced from the cache rather
/// than from canonical YAML. Contains just the fields needed for list /
/// filter / search views — heavy fields (history, comments, relationships,
/// custom_fields, etc.) live in the YAML file and require a full record
/// fetch via `GitBackend::get_requirement` if needed.
#[derive(Debug, Clone)]
pub struct RequirementSummary {
    pub id: Uuid,
    pub spec_id: Option<String>,
    pub agreed_id: Option<String>,
    pub title: String,
    pub description: String,
    pub status: String,
    pub priority: String,
    pub owner: String,
    /// Team-member this spec is assigned to; None when unassigned. trace:STORY-639 | ai:claude
    pub assignee: Option<String>,
    pub feature: String,
    pub req_type: String,
    pub tags: Vec<String>,
    pub created_at: String,
    pub modified_at: String,
    pub archived: bool,
    /// ISO RFC3339 timestamp; None when not archived. trace:STORY-441 | ai:claude
    pub archived_at: Option<String>,
    /// Whether the spec carries the deferred view-flag. trace:STORY-584 | ai:claude
    pub deferred: bool,
    /// ISO RFC3339 timestamp; None when not deferred. trace:STORY-584 | ai:claude
    pub deferred_at: Option<String>,
    /// Free-text revisit trigger; None when not set. trace:STORY-584 | ai:claude
    pub deferred_until: Option<String>,
    /// Inbound edge count — deterministic local centrality. trace:STORY-632 | ai:claude
    pub in_degree: u32,
    /// Outbound edge count. trace:STORY-632 | ai:claude
    pub out_degree: u32,
    /// Type-weighted combined heft score (inbound + outbound). trace:STORY-632 | ai:claude
    pub heft: u32,
    /// Whether this spec has an incomplete BlockedBy edge (a blocker that hasn't
    /// reached Completed, or a dangling blocker). Projected into the cache so
    /// `aida list --blocked` / the `blocked` JSON field read the cache instead of
    /// a full backend.load() — same rebuildable-projection contract as the
    /// degree fields. trace:TASK-902 | ai:claude
    pub blocked: bool,
    pub yaml_path: String,
}

/// STORY-632: deterministic local graph-centrality of a single spec, derived
/// from the relationship graph and materialized in the cache (never in YAML).
///
/// `in_degree` and `out_degree` are tracked SEPARATELY on purpose — they mean
/// different things: high inbound = foundational / load-bearing heft (specs
/// that reference or depend on this one); high outbound = coupling / complexity
/// (this spec reaching out to many others, often just a big epic). v1 is RAW
/// LOCAL degree (direct edges only — not transitive, no PageRank).
///
/// `heft` is a type-weighted combined score: each inbound + outbound edge
/// contributes `edge_weight(rel_type)` (see the static lookup table). A
/// `blocked-by`/`blocks` edge carries more heft than a bare `references`.
/// trace:STORY-632 | ai:claude
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Degrees {
    /// Count of inbound edges (other specs whose relationships target this one).
    pub in_degree: u32,
    /// Count of this spec's own outbound edges.
    pub out_degree: u32,
    /// Type-weighted combined score over inbound + outbound edges.
    pub heft: u32,
}

/// Static `RelationshipType -> weight` lookup for the type-weighted heft score
/// (STORY-632). NOT a field on `Relationship` (the operator resolved the fork on
/// 2026-06-15: type-weighting is a small table, not per-edge state). A
/// blocks/blocked-by edge is the heaviest signal of structural importance; a
/// duplicate carries none. trace:STORY-632 | ai:claude
pub fn edge_weight(rel_type: &RelationshipType) -> u32 {
    match rel_type {
        RelationshipType::BlockedBy | RelationshipType::Blocks => 3,
        RelationshipType::Parent | RelationshipType::Child => 2,
        RelationshipType::Verifies | RelationshipType::VerifiedBy => 1,
        RelationshipType::References => 1,
        RelationshipType::Duplicate => 0,
        RelationshipType::Custom(_) => 1,
    }
}

/// Compute per-spec in/out degree + type-weighted heft for every requirement in
/// `store`, from the relationship graph. v1 = raw LOCAL degree (direct edges
/// only). Each requirement's outbound edges (`req.relationships`) contribute to
/// its own out_degree, and to the in_degree of each edge's target. Heft sums
/// `edge_weight` over both the inbound and outbound edges incident to a spec.
///
/// Returns a map keyed by requirement UUID. Specs with no edges are absent from
/// the map (callers default to `Degrees::default()` == all-zero).
/// trace:STORY-632 | ai:claude
pub fn compute_degrees(store: &RequirementsStore) -> HashMap<Uuid, Degrees> {
    let mut degrees: HashMap<Uuid, Degrees> = HashMap::new();
    for req in &store.requirements {
        for rel in &req.relationships {
            let w = edge_weight(&rel.rel_type);
            // Outbound edge from `req`.
            let src = degrees.entry(req.id).or_default();
            src.out_degree += 1;
            src.heft += w;
            // Inbound edge to the target.
            let dst = degrees.entry(rel.target_id).or_default();
            dst.in_degree += 1;
            dst.heft += w;
        }
    }
    degrees
}

/// Out-degree-only computation for a single spec's own relationships, used by
/// the single-row write-through path (`upsert_requirement`) which has no view of
/// the wider graph. The inbound axis of an upserted row is preserved from the
/// prior cache row (a neighbor's edge change still requires a full
/// `aida cache rebuild` to re-derive — same rebuildable-projection contract as
/// the rest of the cache). trace:STORY-632 | ai:claude
fn own_outbound(relationships: &[Relationship]) -> (u32, u32) {
    let mut out_degree = 0u32;
    let mut heft = 0u32;
    for rel in relationships {
        out_degree += 1;
        heft += edge_weight(&rel.rel_type);
    }
    (out_degree, heft)
}

/// TASK-902: compute the set of requirement UUIDs that have an incomplete
/// BlockedBy edge, over the WHOLE store. A spec is "blocked" when any of its
/// BlockedBy targets hasn't reached Completed (or is a dangling edge) — exactly
/// `pickability::blocked_by_incomplete`, the same predicate the CLI's
/// `--blocked` overlay used to run inline against a full `backend.load()`.
/// Computing it once over the loaded store and materializing it into the cache
/// lets `aida list --blocked` read the cache like every other filter.
///
/// Like degree/heft, this is authoritative only after a full rebuild: it depends
/// on each blocker's status, so a status flip on a blocker changes its
/// dependents' blocked flag — captured on the next rebuild, same
/// rebuildable-projection contract. trace:TASK-902 | ai:claude
pub fn compute_blocked(store: &RequirementsStore) -> HashSet<Uuid> {
    store
        .requirements
        .iter()
        .filter(|req| crate::pickability::blocked_by_incomplete(req, store))
        .map(|req| req.id)
        .collect()
}

/// Compute each EPIC's derived (read-only rollup) status over the WHOLE store,
/// returned as a map of epic UUID -> the cache's `status` string form (the same
/// `format!("{:?}", status)` projection `insert_one` uses, so the override is
/// byte-compatible with the existing `idx_cache_status` filter).
///
/// Only epics whose rollup yields a status are present; an epic with no children
/// or whose only children are Rejected is absent (the caller keeps the stored
/// status). Like degree/heft/blocked this is a whole-graph fact, authoritative
/// after a full rebuild.
// trace:BUG-626 | ai:claude
pub fn compute_epic_statuses(store: &RequirementsStore) -> HashMap<Uuid, String> {
    store
        .requirements
        .iter()
        .filter(|req| req.req_type == crate::models::RequirementType::Epic)
        .filter_map(|req| {
            crate::rollup::derive_epic_status(store, req.id)
                .map(|status| (req.id, format!("{status:?}")))
        })
        .collect()
}

/// Three-way filter for the archive axis. STORY-441 introduces archive as
/// a view-level flag distinct from status — `aida list` defaults to
/// `NonArchivedOnly`, `--archived` flips to `ArchivedOnly`, `--all` is
/// `Both`. trace:STORY-441 | ai:claude
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFilter {
    /// Default: archived rows are hidden from the result.
    #[default]
    NonArchivedOnly,
    /// Only archived rows are returned.
    ArchivedOnly,
    /// Archived and non-archived rows are both returned (the
    /// everything-escape-hatch).
    Both,
}

/// Three-way filter for the defer axis. STORY-584 introduces defer as a
/// view-level flag parallel to archive — `aida list` defaults to
/// `NonDeferredOnly`, `--deferred` flips to `DeferredOnly`, `--all` is `Both`.
/// `NonDeferredOnly` additionally honors the legacy `deferred:*` parking tags
/// (a spec carrying any such tag is treated as deferred for view purposes even
/// when the flag is unset), bridging pre-flag tags onto the new tier.
/// trace:STORY-584 | ai:claude
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum DeferFilter {
    /// Default: deferred rows (flag set OR carrying a `deferred:*` tag) are
    /// hidden from the result.
    #[default]
    NonDeferredOnly,
    /// Only deferred rows are returned (flag set OR carrying a `deferred:*` tag).
    DeferredOnly,
    /// Deferred and non-deferred rows are both returned (the
    /// everything-escape-hatch).
    Both,
}

/// SQL fragment matching specs that carry a legacy `deferred:*` parking tag.
/// Tags are stored as a JSON array of strings, so a tag like
/// `deferred:stabilization-first` appears in the column as the substring
/// `"deferred:`. trace:STORY-584 | ai:claude
const DEFER_TAG_LIKE: &str = r#"tags_json LIKE '%"deferred:%'"#;

/// Ordering for cache list queries. STORY-632 adds `Heft` so
/// `aida list --sort heft` surfaces the most-connected (load-bearing) specs
/// first. trace:STORY-632 | ai:claude
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    /// Default: freshest-first by modified_at.
    #[default]
    ModifiedDesc,
    /// Most-connected first by the type-weighted heft score (ties broken by
    /// modified_at DESC).
    HeftDesc,
}

/// Filter passed to cache list queries. All fields are AND'd together;
/// each field's match semantics are documented inline.
#[derive(Debug, Default, Clone)]
pub struct ListFilter {
    /// Case-insensitive equality match on the cache's status string.
    pub status: Option<String>,
    /// Case-insensitive equality match on req_type (matches the Debug
    /// formatting used during projection — e.g. "Functional", "Bug").
    pub req_type: Option<String>,
    /// Case-insensitive equality match on priority (High / Medium / Low).
    /// trace:TASK-1-107 | ai:claude — was silently dropped before; the
    /// CLI accepted --priority but the cache query never received it,
    /// so `aida list --priority low --type task` was equivalent to
    /// `--type task` alone.
    pub priority: Option<String>,
    /// Exact match on owner (handle).
    pub owner: Option<String>,
    /// Exact match on assignee (team-member handle). Powers `aida list --mine`
    /// and `--assigned <user>`. trace:STORY-639 | ai:claude
    pub assignee: Option<String>,
    /// Additional assignee handles that resolve to the SAME canonical person as
    /// `assignee` (TASK-845's person-alias map). When non-empty, the assignee
    /// filter matches `assignee` OR any of these aliases, so `--mine` /
    /// `--assigned <user>` surfaces specs assigned under any of a person's
    /// cross-host owner strings. Empty = the plain single-handle match.
    // trace:TASK-845 | ai:claude
    pub assignee_aliases: Vec<String>,
    /// Exact match on EITHER owner OR assignee (handle). Powers `aida list
    /// --user <name>` / `me` / `user:<name>` — broader than the owner-only or
    /// assignee-only filters. trace:STORY-662 | ai:claude
    pub owner_or_assignee: Option<String>,
    /// Exact match on feature.
    pub feature: Option<String>,
    /// All listed tags must be present on the requirement (AND, not OR).
    /// Empty = no tag filter. Match is exact-string against the JSON array
    /// stored in the cache via LIKE '%"<tag>"%'.
    /// trace:TASK-1-021 | ai:claude
    pub tags: Vec<String>,
    /// Archive axis — see `ArchiveFilter`. Default `NonArchivedOnly` means
    /// archived rows are filtered out. trace:STORY-441 | ai:claude
    pub archive: ArchiveFilter,
    /// Defer axis — see `DeferFilter`. Default `NonDeferredOnly` means deferred
    /// rows (flag set OR `deferred:*`-tagged) are filtered out.
    /// trace:STORY-584 | ai:claude
    pub defer: DeferFilter,
    /// Result ordering. Default `ModifiedDesc`. trace:STORY-632 | ai:claude
    pub sort: SortOrder,
    /// When `Some(true)`, restrict to specs whose cached `blocked` flag is set
    /// (an incomplete BlockedBy edge); `Some(false)` restricts to unblocked.
    /// `None` (default) applies no blocked filter. trace:TASK-902 | ai:claude
    pub blocked: Option<bool>,
    /// Optional cap on returned rows (after ordering).
    pub limit: Option<usize>,
}

/// Escape a user-supplied search string for FTS5 MATCH.
///
/// FTS5 treats `-`, `:`, `*`, `^`, `(`, `)`, `"` as syntax. A bare query like
/// `PR-9` parses as `PR:9` (column:value) and errors with `no such column: 9`.
/// Wrapping each whitespace-separated token in double quotes turns it into a
/// phrase match; quoted tokens are joined by space (implicit AND) so multi-word
/// queries still behave as expected.
///
/// Empty input yields empty output (FTS5 treats `MATCH ''` as match-nothing,
/// which is the desired behavior for an empty query).
/// trace:BUG-77 | ai:claude
pub(crate) fn escape_fts5_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|tok| format!("\"{}\"", tok.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Strip space/hyphen/underscore and lowercase. Mirrors the normalization
/// in `Requirement::set_status_from_str` so the user-typed filter value
/// matches the cache's stored Debug form regardless of casing or word-break
/// punctuation. trace:BUG-1-025 | ai:claude
fn normalize_status_filter(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_'))
        .collect::<String>()
        .to_lowercase()
}

const SCHEMA_SQL: &str = include_str!("cache_schema.sql");
// STORY-441: bumped to "2" when `archived_at` column was added. The cache
// is rebuildable from git so a version mismatch on first read forces a
// transparent rebuild via `CachedGitBackend::ensure_fresh`.
// STORY-476: bumped to "3" when the `external_refs` FTS column was added so
// external issue refs become searchable.
// STORY-584: bumped to "4" when the `deferred` / `deferred_at` / `deferred_until`
// columns were added (view-flag parallel to archived).
// STORY-632: bumped to "5" when the `in_degree` / `out_degree` / `heft`
// centrality columns were added (computed-on-rebuild from the relationship
// graph, never stored in YAML).
// trace:STORY-639 | ai:claude — bumped to 6 for the `assignee` column.
// TASK-902: bumped to "7" when the `blocked` column was added (computed-on-rebuild
// from the BlockedBy graph, never stored in YAML — parallel to in_degree/heft).
// trace:TASK-902 | ai:claude
// TASK-955: bumped to "8" when the `hierarchy_edges` parent->child edge table was
// added (computed-on-rebuild from the relationship graph, never stored in YAML),
// so `aida list --parent <id> --recursive` can walk the transitive subtree with a
// WITH RECURSIVE query instead of a full backend.load().
// trace:TASK-955 | ai:claude
// TASK-1074: bumped to "9" when `hierarchy_edges` switched from the per-endpoint
// convention-B normalization to the shared rank-oriented rule
// (`graph_walk::oriented_hierarchy_edges`), so the `descendant_ids` closure that
// `aida focus` reads agrees with `aida graph --tree`. Existing caches rebuild on
// next read to re-orient their edges. trace:TASK-1074 | ai:claude
const SCHEMA_VERSION: &str = "9";

const META_KEY_SCHEMA_VERSION: &str = "schema_version";
const META_KEY_SOURCE_HEAD_SHA: &str = "source_head_sha";
const META_KEY_BUILT_AT: &str = "built_at";
const DEFAULT_CACHE_RETRY_DELAYS_MS: &[u64] = &[100, 200, 400, 800, 1600, 3200, 6400, 12800];

// BUG-681: a SHORT bounded retry ladder used only on advisory, skippable read
// paths (the per-turn `aida awaiting --notice` hook). The default ladder above
// sums to ~25s worst case; under lock contention that blows past Claude Code's
// 5s UserPromptSubmit hook timeout and the hook is killed. When fast-fail mode
// is armed for the current thread, a lock-contended open/read gives up after
// ~150ms so the notice degrades to empty (prints nothing) instead of blocking a
// prompt. Scoped to the notice path ONLY — normal reads/writes keep the full
// resilient ladder. trace:BUG-681
const FAST_FAIL_CACHE_RETRY_DELAYS_MS: &[u64] = &[50, 100];

thread_local! {
    // Armed by the `aida awaiting --notice` path before the cache is opened, so
    // BOTH the connection open and the summary reads honor the short ladder.
    // trace:BUG-681
    static FAST_FAIL_CACHE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Arm/disarm fast-fail cache mode for the CURRENT THREAD, returning the prior
/// value so a caller can restore it. When armed, the cache retry ladder collapses
/// to a short bounded set of delays (~150ms total) so a lock-contended cache
/// open/read fails fast and the caller can degrade gracefully instead of blocking
/// on the full exponential backoff. Scoped to advisory, skippable read paths —
/// currently the per-turn `aida awaiting --notice` hook, which must never block a
/// Claude Code prompt. Does NOT change timing for any normal read/write.
// trace:BUG-681 | ai:claude
pub fn set_fast_fail_cache(on: bool) -> bool {
    FAST_FAIL_CACHE.with(|c| c.replace(on))
}

/// Whether fast-fail cache mode is currently armed on this thread.
// trace:BUG-681 | ai:claude
pub fn fast_fail_cache_enabled() -> bool {
    FAST_FAIL_CACHE.with(|c| c.get())
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct CacheLockInfo {
    pub pid: u32,
    pub command: String,
    pub started_at: String,
    pub user: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl CacheLockInfo {
    fn current() -> Self {
        let command = std::env::args().collect::<Vec<_>>().join(" ");
        Self {
            pid: std::process::id(),
            command,
            started_at: chrono::Utc::now().to_rfc3339(),
            user: current_user(),
            session_id: std::env::var("AIDA_SESSION_ID")
                .ok()
                .filter(|s| !s.is_empty()),
        }
    }

    pub fn started_at_utc(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        chrono::DateTime::parse_from_rfc3339(&self.started_at)
            .ok()
            .map(|dt| dt.with_timezone(&chrono::Utc))
    }
}

pub fn cache_lock_info_path(cache_path: &Path) -> PathBuf {
    cache_path.with_file_name(format!(
        "{}.lock-info",
        cache_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("cache.db")
    ))
}

pub fn read_cache_lock_info(cache_path: &Path) -> Result<Option<CacheLockInfo>> {
    let path = cache_lock_info_path(cache_path);
    if !path.exists() {
        return Ok(None);
    }
    let body = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read cache lock info at {}", path.display()))?;
    let info = serde_json::from_str(&body)
        .with_context(|| format!("Failed to parse cache lock info at {}", path.display()))?;
    Ok(Some(info))
}

/// True when the cache write-lock is currently held by a DIFFERENT, still-alive
/// process (it is mid rebuild/write). Read paths consult this so they serve the
/// last-good committed snapshot instead of contending for the write lock through
/// the ~25s retry ladder (BUG-664). A lock-info from THIS process, or from a
/// dead pid (crashed writer), returns false — so a stale lock never wedges
/// readers and a single process never defers to itself.
// trace:BUG-664
pub fn foreign_writer_holds_lock(cache_path: &Path) -> bool {
    match read_cache_lock_info(cache_path) {
        Ok(Some(info)) => info.pid != std::process::id() && pid_is_alive(info.pid),
        _ => false,
    }
}

/// True when `pid` is a live process. Unix uses `kill(pid, 0)` (probes
/// existence/permission, sends no signal); other platforms conservatively
/// return true.
// trace:BUG-664
fn pid_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only probes existence/permission; it sends nothing.
        let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if rc == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

fn open_connection_with_retry(path: &Path) -> Result<Connection> {
    match open_connection_inner(path) {
        Ok(conn) => Ok(conn),
        // BUG-683: a corrupt / non-sqlite `.aida/cache.db` is a no-escape
        // dead-end — every cache-backed read fails, and even `aida cache
        // rebuild` re-opens the same corrupt file first and fails. The cache is
        // gitignored and rebuildable-by-design (EPIC-1-001), so a file that
        // exists but isn't a valid sqlite db is never authoritative: delete it
        // (plus any -wal/-shm sidecars) and recreate an empty db, then let the
        // caller's create+migrate path rebuild from git on the next read. This
        // is TRUE auto-heal. Scoped to the CORRUPTION class ONLY — a transient
        // busy/lock error (the BUG-681 fast-fail path) and permissions errors
        // are NOT self-healed here; they keep their existing behavior.
        Err(err) if is_sqlite_corruption_error(&err) => {
            remove_corrupt_cache_files(path);
            // Re-open the now-absent file: sqlite creates a fresh empty db, the
            // WAL pragma succeeds, and Cache::open applies the schema. If the
            // recreate ALSO fails (e.g. the file couldn't be removed — a
            // permissions problem masquerading past the corruption class), fall
            // back to an actionable message rather than the misleading
            // WAL-mode wording.
            open_connection_inner(path).map_err(|second| {
                if is_sqlite_corruption_error(&second) {
                    anyhow::anyhow!(
                        "cache database is unreadable (corrupt?) — delete it to \
                         auto-rebuild: rm {}",
                        path.display()
                    )
                } else {
                    second
                }
            })
        }
        Err(err) => Err(err),
    }
}

/// Single open attempt (with the lock-retry ladder). Opens the connection,
/// disables rusqlite's built-in busy handler (AIDA owns retry timing), and
/// switches the journal to WAL. Separated from `open_connection_with_retry` so
/// the corruption self-heal (BUG-683) can retry the whole open after deleting a
/// corrupt file.
// trace:STORY-580 | ai:codex
fn open_connection_inner(path: &Path) -> Result<Connection> {
    with_cache_retry(path, "open cache", || {
        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open cache at {:?}", path))?;
        // AIDA owns retry/backoff timing; rusqlite's default busy handler can
        // otherwise block inside a single attempt and hide the lock holder.
        conn.busy_timeout(Duration::from_millis(0))?;
        // trace:STORY-580 | ai:codex
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .context("Failed to enable WAL journal mode for cache")?;
        if !journal_mode.eq_ignore_ascii_case("wal") {
            anyhow::bail!("cache did not enter WAL journal mode: {journal_mode}");
        }
        Ok(conn)
    })
}

/// True when the error chain carries a sqlite result code from the CORRUPTION
/// class — the file exists but isn't a valid sqlite database. `NotADatabase`
/// (SQLITE_NOTADB, 26) is the "header is not a sqlite header" case that a
/// non-sqlite / truncated file produces; `DatabaseCorrupt` (SQLITE_CORRUPT, 11)
/// is a malformed on-disk image. Deliberately EXCLUDES `DatabaseBusy` /
/// `DatabaseLocked` (transient — the BUG-681 fast-fail path handles those) and
/// permissions / I/O errors (not the cache's to auto-delete).
// trace:BUG-683
fn is_sqlite_corruption_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<rusqlite::Error>()
            .is_some_and(|sqlite| match sqlite {
                rusqlite::Error::SqliteFailure(code, _) => matches!(
                    code.code,
                    rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt
                ),
                _ => false,
            })
    })
}

/// Delete a corrupt cache file and its WAL/SHM sidecars so the next open
/// recreates a fresh, empty database. Best-effort: a failed removal falls
/// through and the re-open surfaces the actionable message.
// trace:BUG-683
fn remove_corrupt_cache_files(path: &Path) {
    let _ = std::fs::remove_file(path);
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{}", path.display(), suffix));
        let _ = std::fs::remove_file(sidecar);
    }
}

fn with_cache_write<T, F>(cache_path: &Path, action: &str, f: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    write_cache_lock_info(cache_path)?;
    let result = with_cache_retry(cache_path, action, f);
    remove_cache_lock_info(cache_path);
    result
}

fn with_cache_retry<T, F>(cache_path: &Path, action: &str, mut f: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    let delays = cache_retry_delays();
    let mut attempts = 0usize;
    loop {
        match f() {
            Ok(value) => return Ok(value),
            Err(err) if is_sqlite_lock_error(&err) && attempts < delays.len() => {
                std::thread::sleep(delays[attempts]);
                attempts += 1;
            }
            Err(err) if is_sqlite_lock_error(&err) => {
                return Err(enrich_cache_lock_error(cache_path, action, err));
            }
            Err(err) => return Err(err),
        }
    }
}

fn cache_retry_delays() -> Vec<Duration> {
    // BUG-681: on the advisory notice path, collapse to the short bounded ladder
    // so a lock-contended open/read fails fast (~150ms) rather than blocking on
    // the full ~25s exponential backoff. An explicit AIDA_CACHE_RETRY_COUNT=0
    // still wins (disables retries entirely) for tests that simulate a hard lock.
    if fast_fail_cache_enabled() {
        let disabled = std::env::var("AIDA_CACHE_RETRY_COUNT")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .map(|c| c == 0)
            .unwrap_or(false);
        if disabled {
            return Vec::new();
        }
        return FAST_FAIL_CACHE_RETRY_DELAYS_MS
            .iter()
            .map(|&ms| Duration::from_millis(ms))
            .collect();
    }
    let count = std::env::var("AIDA_CACHE_RETRY_COUNT")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_CACHE_RETRY_DELAYS_MS.len());
    if count == 0 {
        return Vec::new();
    }
    let env_delays = std::env::var("AIDA_CACHE_RETRY_MS")
        .ok()
        .map(|raw| {
            raw.split(',')
                .filter_map(|part| part.trim().parse::<u64>().ok())
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_CACHE_RETRY_DELAYS_MS.to_vec());
    (0..count)
        .map(|idx| {
            let ms = env_delays
                .get(idx)
                .copied()
                .unwrap_or_else(|| *env_delays.last().unwrap_or(&500));
            Duration::from_millis(ms)
        })
        .collect()
}

fn is_sqlite_lock_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<rusqlite::Error>()
            .is_some_and(|sqlite| match sqlite {
                rusqlite::Error::SqliteFailure(code, _) => matches!(
                    code.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ),
                _ => false,
            })
    })
}

fn enrich_cache_lock_error(cache_path: &Path, action: &str, err: anyhow::Error) -> anyhow::Error {
    match read_cache_lock_info(cache_path) {
        Ok(Some(info)) if info.pid != std::process::id() => anyhow::anyhow!(
            "database is locked while trying to {action} by pid={} ({}) held since {} ({} ago). \
             Try again or check that process. If it's stuck, run `aida doctor heal stale-locks`.\ncaused by: {}",
            info.pid,
            if info.command.trim().is_empty() {
                "unknown command"
            } else {
                info.command.as_str()
            },
            info.started_at,
            humanize_cache_lock_age(&info),
            err
        ),
        _ => anyhow::anyhow!(
            "database is locked while trying to {action}. \
             Try again. If this persists, run `aida doctor heal stale-locks`.\ncaused by: {}",
            err
        ),
    }
}

fn humanize_cache_lock_age(info: &CacheLockInfo) -> String {
    let Some(started_at) = info.started_at_utc() else {
        return "unknown age".to_string();
    };
    let secs = chrono::Utc::now()
        .signed_duration_since(started_at)
        .num_seconds()
        .max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn write_cache_lock_info(cache_path: &Path) -> Result<()> {
    use std::io::Write;

    let path = cache_lock_info_path(cache_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create cache lock-info parent {}",
                parent.display()
            )
        })?;
    }
    let body = serde_json::to_string_pretty(&CacheLockInfo::current())?;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => file
            .write_all(body.as_bytes())
            .with_context(|| format!("Failed to write cache lock info at {}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(err)
            .with_context(|| format!("Failed to write cache lock info at {}", path.display())),
    }
}

fn remove_cache_lock_info(cache_path: &Path) {
    let path = cache_lock_info_path(cache_path);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(info) = serde_json::from_str::<CacheLockInfo>(&body) else {
        return;
    };
    if info.pid == std::process::id() {
        let _ = std::fs::remove_file(path);
    }
}

fn current_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

pub struct Cache {
    conn: Mutex<Connection>,
    path: PathBuf,
    lock_info_path: PathBuf,
}

impl Cache {
    /// Open or create the cache at `path`. The schema is applied on every open
    /// (idempotent — `CREATE TABLE IF NOT EXISTS`).
    ///
    /// STORY-441: when the on-disk `schema_version` meta key is older than
    /// `SCHEMA_VERSION`, the cache tables are dropped before re-applying the
    /// schema. `CREATE TABLE IF NOT EXISTS` doesn't add new columns to an
    /// existing table; dropping forces a clean rebuild from git on the next
    /// stale-check. The cache is rebuildable by definition, so this is safe.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create cache parent dir: {:?}", parent))?;
        }
        let conn = open_connection_with_retry(&path)?;
        // Check the recorded schema version BEFORE applying the schema —
        // if the table doesn't exist yet, the meta read silently returns
        // None which falls through to "no migration needed".
        let on_disk_version: Option<String> = conn
            .query_row(
                "SELECT value FROM cache_meta WHERE key = ?1",
                params![META_KEY_SCHEMA_VERSION],
                |row| row.get::<_, String>(0),
            )
            .ok();
        // BUG-485 / BUG-627: trigger a migration drop on EITHER a stamped-version
        // mismatch OR a structural drift in the ACTUAL on-disk schema. The
        // version stamp alone is not enough: a cache can carry the current
        // `schema_version` yet still have an old `requirements_fts` /
        // `requirements_cache` table (e.g. a column added to the projection while
        // the version bump was missed, or a partially-applied / torn rebuild). In
        // that state the version check never fires again — a one-way trap — and
        // the read/write hard-errors with `table requirements_cache has no column
        // named blocked`. BUG-627 specifically: concurrent worktree-isolated
        // agents on different-SHA binaries sharing one `.aida/cache.db` can leave
        // a column missing while the version meta still says "current". Detecting
        // the missing column structurally (substrate-as-bouncer — verify the real
        // columns, don't trust the meta) makes the cache self-heal: drop +
        // rebuild-from-git on next read, regardless of the version stamp or
        // HEAD-SHA freshness.
        let version_mismatch = on_disk_version
            .as_deref()
            .map(|v| v != SCHEMA_VERSION)
            .unwrap_or(false);
        let schema_drifted = fts_schema_drifted(&conn) || cache_schema_drifted(&conn);
        // BUG-664: a PURE READER must not take the cache write-lock on open. The
        // old open unconditionally re-applied the schema AND re-stamped the
        // schema-version meta on every open — both write transactions. With
        // `busy_timeout=0` + the retry ladder, a reader opening a healthy cache
        // while another process held the write-lock for a rebuild blocked +
        // retried for ~25s (the `aida status` telemetry spikes). The schema is
        // all `CREATE … IF NOT EXISTS`, so once the tables exist and the stamped
        // version matches, NOTHING needs writing — gate every write on an actual
        // create/migrate so the steady-state open is read-only. WAL then lets the
        // reader serve the last-good committed snapshot with zero contention.
        let tables_present = cache_tables_present(&conn);
        let needs_schema_apply = version_mismatch || schema_drifted || !tables_present;
        if version_mismatch || schema_drifted {
            // Drop the cache tables — the next stale-check will rebuild
            // from git. `cache_meta` survives so the source HEAD SHA
            // tracking continues to work after the rebuild stamps it.
            with_cache_write(&path, "drop cache tables for schema migration", || {
                conn.execute_batch(
                    "DROP TABLE IF EXISTS requirements_cache;
                     DROP TABLE IF EXISTS requirements_fts;",
                )
                .context("Failed to drop cache tables for schema migration")
            })?;
        }
        if needs_schema_apply {
            with_cache_write(&path, "apply cache schema", || {
                conn.execute_batch(SCHEMA_SQL)
                    .context("Failed to apply cache schema")
            })?;
        }
        let cache = Cache {
            conn: Mutex::new(conn),
            lock_info_path: cache_lock_info_path(&path),
            path,
        };
        // Only stamp the schema version when it is not already current — an
        // unconditional `INSERT … ON CONFLICT DO UPDATE` always takes the write
        // lock (BUG-664). A fresh cache (None) or a migrated one (mismatch) needs
        // the stamp; a current cache must not write here.
        if on_disk_version.as_deref() != Some(SCHEMA_VERSION) {
            cache.set_meta(META_KEY_SCHEMA_VERSION, SCHEMA_VERSION)?;
        }
        // After a schema-version bump (or a structural-drift drop, BUG-485)
        // the head SHA is no longer valid for the (now-empty) cache tables —
        // delete it so `is_stale` returns true (None → stale) and the next
        // read triggers a rebuild.
        if version_mismatch || schema_drifted {
            let conn = cache.conn.lock().unwrap();
            conn.execute(
                "DELETE FROM cache_meta WHERE key = ?1",
                params![META_KEY_SOURCE_HEAD_SHA],
            )?;
        }
        Ok(cache)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn lock_info_path(&self) -> &Path {
        &self.lock_info_path
    }

    // ------------------------------------------------------------------ meta

    fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        with_cache_write(&self.path, "set cache metadata", || {
            conn.execute(
                "INSERT INTO cache_meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
            Ok(())
        })?;
        Ok(())
    }

    fn get_meta(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let r = conn
            .query_row(
                "SELECT value FROM cache_meta WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .ok();
        Ok(r)
    }

    pub fn source_head_sha(&self) -> Result<Option<String>> {
        self.get_meta(META_KEY_SOURCE_HEAD_SHA)
    }

    pub fn set_source_head_sha(&self, sha: &str) -> Result<()> {
        self.set_meta(META_KEY_SOURCE_HEAD_SHA, sha)
    }

    pub fn built_at(&self) -> Result<Option<String>> {
        self.get_meta(META_KEY_BUILT_AT)
    }

    /// Cache is stale when the recorded source HEAD SHA differs from the
    /// store's actual current HEAD. Missing recorded SHA also = stale.
    pub fn is_stale(&self, current_head_sha: &str) -> Result<bool> {
        match self.source_head_sha()? {
            Some(s) => Ok(s != current_head_sha),
            None => Ok(true),
        }
    }

    // ------------------------------------------------------------- maintenance

    /// Wipe all cached rows. Schema and meta survive. Use before a rebuild.
    pub fn truncate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        with_cache_write(&self.path, "truncate cache", || {
            conn.execute_batch("DELETE FROM requirements_cache; DELETE FROM requirements_fts;")?;
            Ok(())
        })?;
        Ok(())
    }

    /// Rebuild the cache from a fully-loaded RequirementsStore (typically
    /// produced by `GitBackend::load`). Stamps the source HEAD SHA so future
    /// stale checks work.
    pub fn rebuild_from_store(
        &self,
        store: &RequirementsStore,
        source_head_sha: &str,
    ) -> Result<usize> {
        // STORY-632: degree/heft is a pure function of the WHOLE relationship
        // graph, so compute it once over the full store before the row inserts.
        // trace:STORY-632 | ai:claude
        let degrees = compute_degrees(store);
        // TASK-902: blocked is likewise a whole-graph fact (depends on each
        // BlockedBy target's status), so compute the blocked set once here and
        // project it per row. This is the authoritative recompute on rebuild.
        // trace:TASK-902 | ai:claude
        let blocked = compute_blocked(store);
        // BUG-626: an EPIC's status is a read-only rollup of its children, the
        // same whole-graph fact pattern as `blocked` above — compute it once
        // over the full store here so the projected cache `status` column is the
        // derived value. Only epics whose rollup yields a status are overridden;
        // an epic with no non-rejected children keeps its stored status (the
        // derivation returns None). trace:BUG-626 | ai:claude
        let epic_status = compute_epic_statuses(store);
        // TASK-1074: materialize the ONE shared subtree substrate — every
        // hierarchy edge oriented parent->child by the same rank rule
        // `graph_walk::subtree_ids` walks — so the `descendant_ids` CTE and
        // `aida graph --tree` agree on membership. Computed once over the whole
        // store (both endpoints' types available) rather than per-req, so the
        // orientation is authoritative after a rebuild. trace:TASK-1074 | ai:claude
        let hierarchy_edges = crate::graph_walk::oriented_hierarchy_edges(store);
        let count = {
            let conn = self.conn.lock().unwrap();
            with_cache_write(&self.path, "rebuild cache", || {
                let tx = conn.unchecked_transaction()?;
                tx.execute_batch(
                    "DELETE FROM requirements_cache; DELETE FROM requirements_fts; DELETE FROM hierarchy_edges;",
                )?;
                let mut count = 0usize;
                for req in &store.requirements {
                    let d = degrees.get(&req.id).copied().unwrap_or_default();
                    let status_override = epic_status.get(&req.id).map(String::as_str);
                    insert_one(&tx, req, d, blocked.contains(&req.id), status_override)?;
                    count += 1;
                }
                for (parent, child) in &hierarchy_edges {
                    tx.execute(
                        "INSERT OR IGNORE INTO hierarchy_edges (parent_id, child_id) VALUES (?1, ?2)",
                        params![parent.to_string(), child.to_string()],
                    )?;
                }
                tx.commit()?;
                Ok(count)
            })?
        };
        self.set_source_head_sha(source_head_sha)?;
        self.set_meta(META_KEY_BUILT_AT, &chrono::Utc::now().to_rfc3339())?;
        Ok(count)
    }

    /// Single-row upsert called after a write-through git mutation succeeds.
    ///
    /// STORY-632: the edited row's out_degree + heft are recomputed from its own
    /// relationships (cheap, no graph view needed). The inbound axis is
    /// PRESERVED from the prior cache row — a single-row write can't re-derive a
    /// neighbor's in_degree, so an edge add/remove leaves the *other* endpoint's
    /// inbound count stale until the next full `aida cache rebuild`. This matches
    /// the rebuildable-projection contract: centrality is authoritative after a
    /// rebuild; per-write upserts keep the touched row's own outbound axis fresh.
    /// trace:STORY-632 | ai:claude
    ///
    /// TASK-902: the `blocked` flag is the same shape — the edited row's OWN
    /// blocked state is recomputed from its BlockedBy targets' statuses as
    /// recorded in the cache (cheap, one indexed lookup per blocker), so the
    /// touched row stays fresh. A status flip on a blocker that should flip its
    /// *dependents'* blocked flags is NOT propagated here (a single-row write has
    /// no view of who depends on it) — that's recovered on the next full rebuild,
    /// the same contract as the inbound-degree axis. trace:TASK-902 | ai:claude
    pub fn upsert_requirement(&self, req: &Requirement) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        with_cache_write(&self.path, "upsert cached requirement", || {
            // Preserve the inbound axis recorded by the last full rebuild.
            let prior_in: u32 = conn
                .query_row(
                    "SELECT in_degree FROM requirements_cache WHERE id = ?1",
                    params![req.id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .ok()
                .map(|v| v.max(0) as u32)
                .unwrap_or(0);
            let (out_degree, out_heft) = own_outbound(&req.relationships);
            // Heft over inbound edges isn't recoverable from a single row's
            // state, so approximate the inbound contribution as 1-per-edge
            // (the modal weight) plus the exact outbound heft. The next rebuild
            // replaces this with the exact type-weighted value.
            let degrees = Degrees {
                in_degree: prior_in,
                out_degree,
                heft: out_heft + prior_in,
            };
            // TASK-902: recompute THIS row's blocked flag from its BlockedBy
            // targets' cached statuses. Mirrors pickability::blocked_by_incomplete:
            // any BlockedBy target that isn't Completed (or is unresolvable in the
            // cache) leaves the row blocked. trace:TASK-902 | ai:claude
            let blocked = blocked_from_cache(&conn, req);
            // BUG-626: if the upserted row is an EPIC, its status is the derived
            // rollup of its children. The epic's own outbound Parent/Child edges
            // name its children, so we can roll up from the children's CACHED
            // statuses without a whole-store load. A non-epic projects its stored
            // status (None). Propagating a child's status flip UP to its parent
            // epic is handled by the backend wrapper (it has the inner store to
            // re-read the epic's siblings); here we only freshen the epic's own
            // row when the epic itself is the thing being written.
            // trace:BUG-626 | ai:claude
            let epic_override = epic_status_override_from_cache(&conn, req);
            delete_one_uncommitted(&conn, &req.id)?;
            insert_one(&conn, req, degrees, blocked, epic_override.as_deref())?;
            // TASK-955: refresh THIS spec's own outbound hierarchy edges. Like
            // the inbound-degree axis, an edge recorded on the OTHER endpoint
            // (a child carrying `Parent -> this`) is only re-derived on a full
            // rebuild — same rebuildable-projection contract. delete_one_uncommitted
            // already cleared this row's outbound edges before the re-insert.
            // trace:TASK-955 | ai:claude
            insert_edges(&conn, req)?;
            Ok(())
        })?;
        Ok(())
    }

    /// Recompute and re-stamp a single EPIC's derived status column from the
    /// statuses in `rollup` (the epic's children, as the caller resolved them —
    /// typically by loading the epic + its children from the inner git store).
    /// Updates ONLY the `status` column of the epic's existing cache row; a
    /// no-op if the epic has no cache row yet. This is the child-status-flip
    /// propagation the single-row upsert can't do on its own (it has no view of
    /// the epic's other children).
    // trace:BUG-626 | ai:claude
    pub fn recompute_epic_status(
        &self,
        epic_id: &Uuid,
        rollup: &crate::graph_walk::StatusRollup,
    ) -> Result<()> {
        let Some(derived) = crate::rollup::derive_epic_status_from_rollup(rollup) else {
            // Only-rejected (or otherwise indeterminate) children: keep the
            // stored status already in the row.
            return Ok(());
        };
        let status_str = format!("{derived:?}");
        let conn = self.conn.lock().unwrap();
        with_cache_write(&self.path, "recompute epic status", || {
            conn.execute(
                "UPDATE requirements_cache SET status = ?1 WHERE id = ?2",
                params![status_str, epic_id.to_string()],
            )?;
            Ok(())
        })?;
        Ok(())
    }

    /// Single-row delete called after a write-through git delete succeeds.
    pub fn delete_requirement(&self, id: &Uuid) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        with_cache_write(&self.path, "delete cached requirement", || {
            delete_one_uncommitted(&conn, id)
        })
    }

    // ----------------------------------------------------------------- query

    /// Return a filtered list of summaries. Uses indexed columns for filter
    /// pushdown and orders by modified_at DESC so callers see freshest
    /// requirements first.
    pub fn list_summaries(&self, filter: &ListFilter) -> Result<Vec<RequirementSummary>> {
        let mut sql = String::from(
            "SELECT id, spec_id, agreed_id, title, description, status, priority,
                    owner, feature, req_type, tags_json, created_at, modified_at,
                    archived, archived_at, deferred, deferred_at, deferred_until,
                    in_degree, out_degree, heft, yaml_path, assignee, blocked
             FROM requirements_cache WHERE 1=1",
        );
        let mut args: Vec<String> = Vec::new();

        // STORY-441: three-way archive filter, replaces the previous
        // `include_archived` bool. trace:STORY-441 | ai:claude
        match filter.archive {
            ArchiveFilter::NonArchivedOnly => sql.push_str(" AND archived = 0"),
            ArchiveFilter::ArchivedOnly => sql.push_str(" AND archived = 1"),
            ArchiveFilter::Both => {} // no archive clause
        }

        // STORY-584: three-way defer filter, parallel to the archive axis.
        // "Deferred" = the `deferred` flag is set OR the spec carries a legacy
        // `deferred:*` parking tag (honor-both migration). trace:STORY-584 | ai:claude
        match filter.defer {
            DeferFilter::NonDeferredOnly => {
                sql.push_str(&format!(" AND deferred = 0 AND NOT ({DEFER_TAG_LIKE})"));
            }
            DeferFilter::DeferredOnly => {
                sql.push_str(&format!(" AND (deferred = 1 OR {DEFER_TAG_LIKE})"));
            }
            DeferFilter::Both => {} // no defer clause
        }
        if let Some(s) = &filter.status {
            // The cache stores RequirementStatus's Debug form (e.g.
            // "InProgress" — no space/hyphen). Users type "in-progress",
            // "In Progress", "InProgress", etc. Strip space/hyphen/underscore
            // from both sides and lowercase so every variant matches.
            // trace:BUG-1-025 | ai:claude
            //
            // TASK-0415: a comma-separated status spec is a logical OR within
            // the status axis (`draft,approved` → Draft OR Approved). Each
            // token is normalized and the set is OR'd in a single grouped
            // clause so it still composes with the other filters via AND.
            // A single value (no comma) reduces to the original equality.
            // trace:TASK-0415
            let normalized: Vec<String> = s
                .split(',')
                .map(normalize_status_filter)
                .filter(|t| !t.is_empty())
                .collect();
            if !normalized.is_empty() {
                let clause =
                    "LOWER(REPLACE(REPLACE(REPLACE(status, ' ', ''), '-', ''), '_', '')) = ?";
                let ored = vec![clause; normalized.len()].join(" OR ");
                sql.push_str(&format!(" AND ({})", ored));
                args.extend(normalized);
            }
        }
        if let Some(t) = &filter.req_type {
            sql.push_str(" AND LOWER(req_type) = LOWER(?)");
            args.push(t.clone());
        }
        // trace:TASK-1-107 | ai:claude — priority filter that was
        // missing before.
        if let Some(p) = &filter.priority {
            sql.push_str(" AND LOWER(priority) = LOWER(?)");
            args.push(p.clone());
        }
        // trace:TASK-951 | ai:claude — owner/assignee matching is case-insensitive
        // (`LOWER(...) = LOWER(?)`) so `Joe` matches a spec owned by `joe`. The
        // stored value keeps its original casing; only the COMPARISON folds.
        if let Some(o) = &filter.owner {
            sql.push_str(" AND LOWER(owner) = LOWER(?)");
            args.push(o.clone());
        }
        // trace:STORY-639 trace:TASK-951 trace:TASK-845 | ai:claude — the assignee
        // match folds case (TASK-951) and ORs in the person's other aliases
        // (TASK-845) so `--mine` / `--assigned` surfaces specs assigned under any
        // of one human's cross-host owner strings.
        if let Some(a) = &filter.assignee {
            if filter.assignee_aliases.is_empty() {
                sql.push_str(" AND LOWER(assignee) = LOWER(?)");
                args.push(a.clone());
            } else {
                let mut handles = vec![a.clone()];
                handles.extend(filter.assignee_aliases.iter().cloned());
                let placeholders = handles
                    .iter()
                    .map(|_| "LOWER(?)")
                    .collect::<Vec<_>>()
                    .join(", ");
                sql.push_str(&format!(" AND LOWER(assignee) IN ({placeholders})"));
                args.extend(handles);
            }
        }
        // trace:STORY-662 trace:TASK-951 | ai:claude — `--user <name>` / `me` /
        // `user:<name>` matches owner OR assignee, so a person sees specs they own
        // OR are on; the match folds case (TASK-951).
        if let Some(u) = &filter.owner_or_assignee {
            sql.push_str(" AND (LOWER(owner) = LOWER(?) OR LOWER(assignee) = LOWER(?))");
            args.push(u.clone());
            args.push(u.clone());
        }
        if let Some(f) = &filter.feature {
            sql.push_str(" AND feature = ?");
            args.push(f.clone());
        }
        // TASK-902: blocked axis pushed down to the indexed cache column so
        // `aida list --blocked` filters without a full backend.load().
        // trace:TASK-902 | ai:claude
        if let Some(b) = filter.blocked {
            sql.push_str(if b {
                " AND blocked = 1"
            } else {
                " AND blocked = 0"
            });
        }
        // trace:TASK-1-021 | ai:claude
        // tags_json is a JSON array text column; bracket each tag with quotes
        // to avoid `foo` matching `foobar` mid-string.
        //
        // TASK-527: a trailing `*` (`aida:queue:*`) is a prefix-glob — match any
        // tag whose JSON-quoted value starts with the literal prefix, plus the
        // bare prefix without its trailing `:` (so `aida:queue:*` also matches an
        // exact `aida:queue`). The opening `"` anchors the prefix to a JSON
        // string boundary so it can't match mid-value. trace:TASK-527 | ai:claude
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        for tag in &filter.tags {
            if let Some(prefix) = tag.strip_suffix('*') {
                let bare = prefix.strip_suffix(':').unwrap_or(prefix);
                sql.push_str(" AND (tags_json LIKE ? OR tags_json LIKE ?)");
                args.push(format!("%\"{}%", esc(prefix))); // "aida:queue:...
                args.push(format!("%\"{}\"%", esc(bare))); // exact "aida:queue"
            } else {
                sql.push_str(" AND tags_json LIKE ?");
                args.push(format!("%\"{}\"%", esc(tag)));
            }
        }

        match filter.sort {
            SortOrder::ModifiedDesc => sql.push_str(" ORDER BY modified_at DESC"),
            // trace:STORY-632 | ai:claude
            SortOrder::HeftDesc => sql.push_str(" ORDER BY heft DESC, modified_at DESC"),
        }
        if let Some(n) = filter.limit {
            sql.push_str(&format!(" LIMIT {}", n));
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            args.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt
            .query_map(param_refs.as_slice(), row_to_summary)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// FTS5 full-text search over spec_id, agreed_id, title, description.
    /// Uses MATCH semantics — pass an FTS5 query expression (a bare word
    /// works for prefix-tolerant token matching). `archive` follows the
    /// same axis as `list_summaries` — STORY-441.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        archive: ArchiveFilter,
        defer: DeferFilter,
    ) -> Result<Vec<RequirementSummary>> {
        let escaped = escape_fts5_query(query);
        // Empty query (or whitespace-only) → FTS5 rejects an empty MATCH
        // expression. Return no rows instead of erroring. trace:BUG-77 | ai:claude
        if escaped.is_empty() {
            return Ok(Vec::new());
        }
        let archive_clause = match archive {
            ArchiveFilter::NonArchivedOnly => " AND c.archived = 0",
            ArchiveFilter::ArchivedOnly => " AND c.archived = 1",
            ArchiveFilter::Both => "",
        };
        // STORY-584: defer axis, parallel to archive. Honors the `deferred`
        // flag and legacy `deferred:*` tags. trace:STORY-584 | ai:claude
        let defer_clause = match defer {
            DeferFilter::NonDeferredOnly => {
                format!(" AND c.deferred = 0 AND NOT (c.{DEFER_TAG_LIKE})")
            }
            DeferFilter::DeferredOnly => format!(" AND (c.deferred = 1 OR c.{DEFER_TAG_LIKE})"),
            DeferFilter::Both => String::new(),
        };
        let conn = self.conn.lock().unwrap();
        // FTS5 requires the MATCH clause to use the bare table name, not an
        // alias — hence no `f` alias on requirements_fts.
        let sql = format!(
            "SELECT c.id, c.spec_id, c.agreed_id, c.title, c.description,
                          c.status, c.priority, c.owner, c.feature, c.req_type,
                          c.tags_json, c.created_at, c.modified_at, c.archived,
                          c.archived_at, c.deferred, c.deferred_at, c.deferred_until,
                          c.in_degree, c.out_degree, c.heft, c.yaml_path, c.assignee,
                          c.blocked
                   FROM requirements_fts
                   JOIN requirements_cache c ON c.id = requirements_fts.id
                   WHERE requirements_fts MATCH ?{archive_clause}{defer_clause}
                   ORDER BY rank
                   LIMIT ?"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params![escaped, limit as i64], row_to_summary)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    // ----------------------------------------------------------------- stats

    pub fn requirement_count(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM requirements_cache", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// STORY-632: read the cached in/out degree + heft for a single spec by
    /// UUID. Returns `Degrees::default()` (all-zero) when the spec isn't in the
    /// cache yet. trace:STORY-632 | ai:claude
    pub fn degrees_for_id(&self, id: &Uuid) -> Result<Degrees> {
        let conn = self.conn.lock().unwrap();
        let d = conn
            .query_row(
                "SELECT in_degree, out_degree, heft FROM requirements_cache WHERE id = ?1",
                params![id.to_string()],
                |row| {
                    Ok(Degrees {
                        in_degree: row.get::<_, i64>(0)?.max(0) as u32,
                        out_degree: row.get::<_, i64>(1)?.max(0) as u32,
                        heft: row.get::<_, i64>(2)?.max(0) as u32,
                    })
                },
            )
            .ok()
            .unwrap_or_default();
        Ok(d)
    }

    /// Resolve a UUID to its stable `spec_id` via the cache, so a uuid-keyed
    /// lookup on the write path can load the ONE matching YAML object instead of
    /// full-scanning every object file (`find_by_uuid` is O(n) over ~2.5k
    /// YAMLs). The uuid→spec_id mapping is invariant (spec_ids are stable and
    /// never reused), so a row built by an older rebuild is still correct;
    /// callers fall back to the authoritative full scan on a cache miss. Returns
    /// `Ok(None)` when the uuid isn't in the cache (stale/never-ingested).
    // trace:BUG-634 | ai:claude
    pub fn spec_id_for_uuid(&self, id: &Uuid) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let spec_id = conn
            .query_row(
                "SELECT spec_id FROM requirements_cache WHERE id = ?1",
                params![id.to_string()],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        Ok(spec_id)
    }

    /// Resolve a stable spec_id back to its UUID using the cached row. Used by
    /// the incremental cache update to turn a DELETED object file's spec_id
    /// (parsed from its path) into the UUID `delete_requirement` keys on.
    /// `Ok(None)` when no cache row carries that spec_id — the row is already
    /// absent, so the delete is a no-op. Comparison is case-insensitive to match
    /// `canonical_spec_id` (stored spec_ids are upper-cased).
    // trace:BUG-636
    pub fn uuid_for_spec_id(&self, spec_id: &str) -> Result<Option<Uuid>> {
        let conn = self.conn.lock().unwrap();
        let id_str: Option<String> = conn
            .query_row(
                "SELECT id FROM requirements_cache WHERE spec_id = ?1 COLLATE NOCASE",
                params![spec_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match id_str {
            Some(s) => Ok(Uuid::parse_str(&s).ok()),
            None => Ok(None),
        }
    }

    /// The FULL transitive descendant id-set of `root` — the root itself plus
    /// every spec reachable by following parent->child edges, of any depth —
    /// read from the materialized `hierarchy_edges` table with a single
    /// `WITH RECURSIVE` query (no `backend.load()`, so `aida list --recursive`
    /// stays cache-fast). The included root lets the caller apply the existing
    /// list filters to the whole subtree closure. The CTE's UNION (not UNION ALL)
    /// dedups, so a diamond / cycle in the edge set terminates rather than
    /// looping.
    // trace:TASK-955 | ai:claude
    pub fn descendant_ids(&self, root: &Uuid) -> Result<HashSet<Uuid>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "WITH RECURSIVE subtree(id) AS (
                 SELECT ?1
                 UNION
                 SELECT e.child_id
                   FROM hierarchy_edges e
                   JOIN subtree s ON e.parent_id = s.id
             )
             SELECT id FROM subtree",
        )?;
        let rows = stmt
            .query_map(params![root.to_string()], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        let ids = rows
            .into_iter()
            .filter_map(|s| Uuid::parse_str(&s).ok())
            .collect();
        Ok(ids)
    }
}

// --------------------------------------------------------------- schema drift

/// Columns the current `requirements_fts` projection must expose. Adding a
/// column here (and to `cache_schema.sql` + `insert_one`) makes existing
/// caches self-heal on next open even if the SCHEMA_VERSION bump is missed.
// trace:BUG-485
const FTS_REQUIRED_COLUMNS: &[&str] = &[
    "id",
    "spec_id",
    "agreed_id",
    "title",
    "description",
    "external_refs",
];

/// Columns the current `requirements_cache` table must expose. Mirror of
/// `cache_schema.sql`'s `CREATE TABLE requirements_cache` column list. Adding a
/// column to the schema (and to `insert_one`) means adding it here too, so an
/// existing cache built by an older-schema binary self-heals on next open even
/// if the `SCHEMA_VERSION` bump was missed.
//
// The operator hit `table requirements_cache has no column named blocked`
// while `aida cache status` reported FRESH. `blocked` was added without the
// version stamp distinguishing a v7-without-blocked cache from a v7-with-blocked
// one — and concurrent worktree-isolated agents running different-SHA binaries
// against the SHARED `.aida/cache.db` can leave the table at an inconsistent
// schema (a torn/racy rebuild) even when the stamp is current. The version-meta
// check alone can't catch that. Verifying the ACTUAL columns on open
// (substrate-as-bouncer — don't trust the meta) makes a column drift self-heal
// instead of hard-erroring.
// trace:BUG-627 trace:TASK-902
const CACHE_REQUIRED_COLUMNS: &[&str] = &[
    "id",
    "spec_id",
    "agreed_id",
    "title",
    "description",
    "status",
    "priority",
    "owner",
    "assignee",
    "feature",
    "req_type",
    "tags_json",
    "created_at",
    "modified_at",
    "archived",
    "archived_at",
    "deferred",
    "deferred_at",
    "deferred_until",
    "in_degree",
    "out_degree",
    "heft",
    "blocked",
    "yaml_path",
];

/// True when the `requirements_cache` table already exists on disk. A read-only
/// `sqlite_master` lookup (no write lock). Used by `open()` to skip the schema
/// apply on a healthy cache so a pure reader never takes the write lock.
// trace:BUG-664
fn cache_tables_present(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'requirements_cache'",
        [],
        |_| Ok(()),
    )
    .optional()
    .unwrap_or(None)
    .is_some()
}

/// Returns true when the on-disk `requirements_fts` table exists but is missing
/// one or more of the columns the current binary writes. A drifted FTS table
/// would make `insert_one`'s write hard-error (`table requirements_fts has no
/// column named external_refs`); detecting it structurally lets `open()` drop +
/// force a rebuild from git regardless of the stamped schema version (BUG-485).
///
/// When the table does not exist yet (fresh cache) this returns false — the
/// schema apply will create it correctly and no migration is needed.
// trace:BUG-485
fn fts_schema_drifted(conn: &Connection) -> bool {
    // `PRAGMA table_info` works on FTS5 virtual tables and lists the
    // user-visible columns. If the table is absent the pragma yields no rows.
    let cols: Vec<String> = match conn.prepare("PRAGMA table_info(requirements_fts)") {
        Ok(mut stmt) => match stmt.query_map([], |row| row.get::<_, String>(1)) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => return false,
        },
        Err(_) => return false,
    };
    if cols.is_empty() {
        // Table doesn't exist yet — schema apply will create it fresh.
        return false;
    }
    FTS_REQUIRED_COLUMNS
        .iter()
        .any(|required| !cols.iter().any(|c| c == required))
}

/// Returns true when the on-disk `requirements_cache` table exists but is
/// missing one or more of the columns the current binary writes. A drifted
/// table makes reads/writes hard-error (`table requirements_cache has no column
/// named blocked`); detecting it structurally lets `open()` drop + force a
/// rebuild from git regardless of the stamped schema version.
///
/// When the table does not exist yet (fresh cache) this returns false — the
/// schema apply will create it correctly and no migration is needed.
// trace:BUG-627
fn cache_schema_drifted(conn: &Connection) -> bool {
    let cols: Vec<String> = match conn.prepare("PRAGMA table_info(requirements_cache)") {
        Ok(mut stmt) => match stmt.query_map([], |row| row.get::<_, String>(1)) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => return false,
        },
        Err(_) => return false,
    };
    if cols.is_empty() {
        // Table doesn't exist yet — schema apply will create it fresh.
        return false;
    }
    CACHE_REQUIRED_COLUMNS
        .iter()
        .any(|required| !cols.iter().any(|c| c == required))
}

// ---------------------------------------------------------------- row helpers

/// TASK-902: compute one requirement's `blocked` flag from the BlockedBy targets'
/// statuses as currently recorded in the cache — the single-row analog of
/// `pickability::blocked_by_incomplete`. A BlockedBy target whose cached status
/// is anything other than `Completed`, OR that has no cache row at all (dangling /
/// not-yet-projected), leaves the row blocked (defensive: don't silently
/// un-block). Used by the write-through upsert path which has no whole-store
/// view. trace:TASK-902 | ai:claude
fn blocked_from_cache(conn: &Connection, req: &Requirement) -> bool {
    req.relationships
        .iter()
        .filter(|rel| matches!(rel.rel_type, RelationshipType::BlockedBy))
        .any(|rel| {
            let target_status: Option<String> = conn
                .query_row(
                    "SELECT status FROM requirements_cache WHERE id = ?1",
                    params![rel.target_id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .ok();
            match target_status {
                // Resolvable blocker that hasn't reached Completed → blocked.
                Some(status) => status != "Completed",
                // Unresolvable in the cache → treat as unsatisfied.
                None => true,
            }
        })
}

/// When `req` is an EPIC, derive its rollup status from its children's CACHED
/// statuses (the epic's own outbound `Parent`/`Child` edges name the children —
/// same union the tree walk uses). Returns the cache `status` string form, or
/// `None` for a non-epic / an epic whose rollup is indeterminate (only-rejected
/// children → keep stored status). Used by the single-row upsert so an epic's
/// own row stays fresh when the epic itself is written. A child resolvable in
/// the cache contributes its status; an unresolvable child edge contributes
/// nothing (skipped), matching `graph_walk::status_rollup`.
// trace:BUG-626 | ai:claude
fn epic_status_override_from_cache(conn: &Connection, req: &Requirement) -> Option<String> {
    if req.req_type != crate::models::RequirementType::Epic {
        return None;
    }
    let mut rollup = crate::graph_walk::StatusRollup::default();
    for rel in &req.relationships {
        if !matches!(
            rel.rel_type,
            RelationshipType::Parent | RelationshipType::Child
        ) {
            continue;
        }
        let child_status: Option<String> = conn
            .query_row(
                "SELECT status FROM requirements_cache WHERE id = ?1",
                params![rel.target_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .ok();
        if let Some(status) = child_status {
            tally_status_str(&mut rollup, &status);
        }
    }
    crate::rollup::derive_epic_status_from_rollup(&rollup).map(|s| format!("{s:?}"))
}

/// Fold a cache `status` string (the `format!("{:?}", status)` form, or a
/// custom status) into a [`StatusRollup`] bucket. Mirrors the match in
/// `graph_walk::status_rollup`. An unrecognized (custom) status counts toward
/// `remaining` — it isn't a terminal/in-flight signal.
// trace:BUG-626 | ai:claude
fn tally_status_str(r: &mut crate::graph_walk::StatusRollup, status: &str) {
    r.total += 1;
    match status {
        "Completed" => r.completed += 1,
        "Done" => r.done += 1,
        "InProgress" => r.in_progress += 1,
        "NeedsAttention" => r.shelved += 1,
        "Rejected" => r.rejected += 1,
        // Draft / Approved / Planned / any custom status → not yet started.
        _ => r.remaining += 1,
    }
}

/// Project a single requirement row into the cache.
///
/// `status_override`: when `Some`, the projected `status` column is the override
/// string instead of the stored status. This is how the EPIC-rollup derived
/// status (computed over the whole child subtree at rebuild time, the same
/// whole-graph-fact pattern as `compute_blocked`) lands in the cache so the
/// cache-backed `aida list` filters and displays the derived epic status without
/// re-loading the full store. `None` projects the stored status verbatim.
// trace:BUG-626 | ai:claude
fn insert_one(
    conn: &Connection,
    req: &Requirement,
    degrees: Degrees,
    blocked: bool,
    status_override: Option<&str>,
) -> Result<()> {
    let yaml_path = yaml_path_for(req);
    let tags_json = serde_json::to_string(&req.tags).unwrap_or_else(|_| "[]".into());
    let archived = if req.archived { 1 } else { 0 };
    // trace:STORY-441 | ai:claude
    let archived_at = req.archived_at.map(|dt| dt.to_rfc3339());
    // trace:STORY-584 | ai:claude
    let deferred = if req.deferred { 1 } else { 0 };
    let deferred_at = req.deferred_at.map(|dt| dt.to_rfc3339());
    let deferred_until = req.deferred_until.clone();
    let req_type_str = format!("{:?}", req.req_type);
    // trace:BUG-626 | ai:claude — an epic's status is a read-only rollup of its
    // children; the override (when present) is the derived value.
    let status_str = status_override.map(str::to_string).unwrap_or_else(|| {
        req.custom_status
            .clone()
            .unwrap_or_else(|| format!("{:?}", req.status))
    });
    let priority_str = req
        .custom_priority
        .clone()
        .unwrap_or_else(|| format!("{:?}", req.priority));

    conn.execute(
        "INSERT INTO requirements_cache (
            id, spec_id, agreed_id, title, description, status, priority,
            owner, feature, req_type, tags_json, created_at, modified_at,
            archived, archived_at, deferred, deferred_at, deferred_until,
            in_degree, out_degree, heft, blocked, yaml_path, assignee
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
        params![
            req.id.to_string(),
            req.spec_id,
            req.agreed_id,
            req.title,
            req.description,
            status_str,
            priority_str,
            req.owner,
            req.feature,
            req_type_str,
            tags_json,
            req.created_at.to_rfc3339(),
            req.modified_at.to_rfc3339(),
            archived,
            archived_at,
            deferred,
            deferred_at,
            deferred_until,
            degrees.in_degree,
            degrees.out_degree,
            degrees.heft,
            // trace:TASK-902 | ai:claude
            if blocked { 1 } else { 0 },
            yaml_path,
            // trace:STORY-639 | ai:claude
            req.assignee,
        ],
    )?;

    // STORY-476: index external issue refs (space-joined) so
    // `aida search "linear:LIN-123"` finds the spec.
    let external_refs = req.external_refs.join(" ");
    conn.execute(
        "INSERT INTO requirements_fts (id, spec_id, agreed_id, title, description, external_refs)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            req.id.to_string(),
            req.spec_id.clone().unwrap_or_default(),
            req.agreed_id.clone().unwrap_or_default(),
            req.title,
            req.description,
            external_refs,
        ],
    )?;
    Ok(())
}

fn row_to_summary(row: &rusqlite::Row) -> rusqlite::Result<RequirementSummary> {
    let id_str: String = row.get(0)?;
    let id = Uuid::parse_str(&id_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let tags_json: String = row.get(10)?;
    let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
    let archived_int: i64 = row.get(13)?;
    let deferred_int: i64 = row.get(15)?;
    Ok(RequirementSummary {
        id,
        spec_id: row.get(1)?,
        agreed_id: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        status: row.get(5)?,
        priority: row.get(6)?,
        owner: row.get(7)?,
        feature: row.get(8)?,
        req_type: row.get(9)?,
        tags,
        created_at: row.get(11)?,
        modified_at: row.get(12)?,
        archived: archived_int != 0,
        // trace:STORY-441 | ai:claude — column index 14, nullable.
        archived_at: row.get(14)?,
        // trace:STORY-584 | ai:claude — columns 15/16/17.
        deferred: deferred_int != 0,
        deferred_at: row.get(16)?,
        deferred_until: row.get(17)?,
        // trace:STORY-632 | ai:claude — columns 18/19/20 (stored as INTEGER).
        in_degree: row.get::<_, i64>(18)?.max(0) as u32,
        out_degree: row.get::<_, i64>(19)?.max(0) as u32,
        heft: row.get::<_, i64>(20)?.max(0) as u32,
        yaml_path: row.get(21)?,
        // trace:STORY-639 | ai:claude — column index 22, nullable.
        assignee: row.get(22)?,
        // trace:TASK-902 | ai:claude — column index 23, INTEGER 0/1.
        blocked: row.get::<_, i64>(23)? != 0,
    })
}

fn delete_one_uncommitted(conn: &Connection, id: &Uuid) -> Result<()> {
    let id_str = id.to_string();
    conn.execute(
        "DELETE FROM requirements_cache WHERE id = ?1",
        params![id_str],
    )?;
    conn.execute(
        "DELETE FROM requirements_fts WHERE id = ?1",
        params![id_str],
    )?;
    // TASK-955: clear the OUTBOUND hierarchy edges this row contributed (the
    // ones where it is the parent, plus the parent->this rows it authored as a
    // child via a `Parent` edge). insert_edges re-derives them from the fresh
    // record. Edges authored by OTHER specs that target this id survive — they
    // belong to that endpoint's row and are refreshed when it is rewritten / on
    // the next full rebuild. trace:TASK-955 | ai:claude
    conn.execute(
        "DELETE FROM hierarchy_edges WHERE parent_id = ?1 OR child_id = ?1",
        params![id_str],
    )?;
    Ok(())
}

/// Rank of a cache-stored `req_type` string (the `format!("{:?}", …)` Debug
/// form `insert_one` writes) — mirrors
/// [`crate::models::RequirementType::hierarchy_rank`] without a parse. An
/// unknown/absent type defaults to child-rank (2) so a dangling parent-ref does
/// not invert an edge.
// trace:TASK-1074 | ai:claude
fn rank_from_cache_type(s: &str) -> u8 {
    match s {
        "Epic" => 0,
        "Story" => 1,
        _ => 2,
    }
}

/// Write a single spec's parent->child hierarchy edges into the
/// `hierarchy_edges` table, oriented by the same shared rank rule the full
/// rebuild uses (`graph_walk::orient_hierarchy_edge`). The target's rank is read
/// from its cache row (child-rank if it is not yet cached — the full rebuild
/// re-derives the authoritative orientation from both endpoints' types, the same
/// authoritative-after-rebuild contract as in_degree/blocked). Idempotent via
/// the (parent_id, child_id) primary key (`INSERT OR IGNORE`).
// trace:TASK-955 trace:TASK-1074 | ai:claude
fn insert_edges(conn: &Connection, req: &Requirement) -> Result<()> {
    let src_rank = req.req_type.hierarchy_rank();
    for rel in &req.relationships {
        let tgt_rank = conn
            .query_row(
                "SELECT req_type FROM requirements_cache WHERE id = ?1",
                params![rel.target_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .map(|s| rank_from_cache_type(&s))
            .unwrap_or(2);
        if let Some((parent, child)) = crate::graph_walk::orient_hierarchy_edge(
            req.id,
            src_rank,
            &rel.rel_type,
            rel.target_id,
            tgt_rank,
        ) {
            conn.execute(
                "INSERT OR IGNORE INTO hierarchy_edges (parent_id, child_id) VALUES (?1, ?2)",
                params![parent.to_string(), child.to_string()],
            )?;
        }
    }
    Ok(())
}

/// Compute the relative YAML path inside the git store for a requirement.
/// Mirrors the layout used by `GitBackend` (objects/<TYPE>/<shard>/<spec_id>.yaml).
fn yaml_path_for(req: &Requirement) -> String {
    let prefix = req
        .spec_id
        .as_deref()
        .and_then(|s| s.split('-').next())
        .unwrap_or("SPEC");
    let spec_id = req.spec_id.clone().unwrap_or_else(|| req.id.to_string());
    format!("objects/{}/000/{}.yaml", prefix, spec_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{RequirementPriority, RequirementStatus, RequirementType};
    use std::sync::{Mutex as StdMutex, OnceLock};
    use tempfile::tempdir;

    fn env_lock() -> &'static StdMutex<()> {
        static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| StdMutex::new(()))
    }

    fn sample_req(spec_id: &str, title: &str) -> Requirement {
        let mut r = Requirement::new(title.into(), "desc".into());
        r.spec_id = Some(spec_id.into());
        r.owner = "joe".into();
        r.feature = "storage".into();
        r.tags.insert("a".into());
        r.tags.insert("b".into());
        r
    }

    fn rel(rel_type: RelationshipType, target: Uuid) -> Relationship {
        Relationship {
            rel_type,
            target_id: target,
            created_at: None,
            created_by: None,
        }
    }

    /// STORY-632: a synthetic star graph — one hub with N inbound + M outbound
    /// edges — exercises the separate-axis degree computation and the
    /// type-weighted heft score.
    #[test]
    fn compute_degrees_counts_inbound_and_outbound_separately() {
        // Hub HUB-1 has 2 outbound edges (→ OUT-1, → OUT-2) and 3 inbound
        // edges (IN-1, IN-2, IN-3 each point at it).
        let hub = sample_req("HUB-1", "hub");
        let out1 = sample_req("OUT-1", "out1");
        let out2 = sample_req("OUT-2", "out2");
        let in1 = sample_req("IN-1", "in1");
        let in2 = sample_req("IN-2", "in2");
        let in3 = sample_req("IN-3", "in3");

        let (hub_id, out1_id, out2_id) = (hub.id, out1.id, out2.id);

        let mut hub = hub;
        hub.relationships
            .push(rel(RelationshipType::Blocks, out1_id)); // weight 3
        hub.relationships
            .push(rel(RelationshipType::References, out2_id)); // weight 1

        let mut in1 = in1;
        in1.relationships
            .push(rel(RelationshipType::BlockedBy, hub_id)); // weight 3
        let mut in2 = in2;
        in2.relationships
            .push(rel(RelationshipType::Parent, hub_id)); // weight 2
        let mut in3 = in3;
        in3.relationships
            .push(rel(RelationshipType::References, hub_id)); // weight 1

        let mut store = RequirementsStore::new();
        store.requirements.extend([hub, out1, out2, in1, in2, in3]);

        let degrees = compute_degrees(&store);
        let hub_d = degrees.get(&hub_id).copied().unwrap();
        assert_eq!(hub_d.in_degree, 3, "3 specs point at the hub");
        assert_eq!(hub_d.out_degree, 2, "hub has 2 outbound edges");
        // heft = outbound (3 + 1) + inbound (3 + 2 + 1) = 10
        assert_eq!(hub_d.heft, 10);

        // A pure leaf target of an outbound edge gets in_degree 1, out 0.
        let out1_d = degrees.get(&out1_id).copied().unwrap();
        assert_eq!(out1_d.in_degree, 1);
        assert_eq!(out1_d.out_degree, 0);
        assert_eq!(out1_d.heft, 3); // the inbound Blocks edge, weight 3
    }

    /// TASK-902: the cache-projected `blocked` flag matches the edge-walk
    /// result. A fixture with BlockedBy chains — an incomplete blocker, a
    /// Completed blocker, a dangling blocker, and an unblocked spec — is
    /// rebuilt into the cache; every row's cached `blocked` flag must equal
    /// both `compute_blocked` and `pickability::blocked_by_incomplete` over the
    /// full store (correctness: same WHICH-is-blocked set, just cache-fast).
    #[test]
    fn cache_blocked_flag_matches_edge_walk() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        // lone: no edges → never blocked.
        let lone = sample_req("FR-1-001", "lone");
        // blocker_open: an Approved spec used as a live blocker.
        let mut blocker_open = sample_req("FR-1-002", "blocker-open");
        blocker_open.status = RequirementStatus::Approved;
        // blocker_done: a Completed spec — blocking it does NOT block.
        let mut blocker_done = sample_req("FR-1-003", "blocker-done");
        blocker_done.status = RequirementStatus::Completed;

        let (open_id, done_id) = (blocker_open.id, blocker_done.id);
        // A target id with no requirement in the store → dangling edge.
        let dangling_id = Uuid::new_v4();

        // dep_open: blocked by an incomplete blocker → BLOCKED.
        let mut dep_open = sample_req("FR-1-010", "dep-open");
        dep_open
            .relationships
            .push(rel(RelationshipType::BlockedBy, open_id));
        // dep_done: blocked only by a Completed blocker → NOT blocked.
        let mut dep_done = sample_req("FR-1-011", "dep-done");
        dep_done
            .relationships
            .push(rel(RelationshipType::BlockedBy, done_id));
        // dep_mixed: blocked by both a Completed and an incomplete blocker →
        // BLOCKED (any incomplete blocker blocks).
        let mut dep_mixed = sample_req("FR-1-012", "dep-mixed");
        dep_mixed
            .relationships
            .push(rel(RelationshipType::BlockedBy, done_id));
        dep_mixed
            .relationships
            .push(rel(RelationshipType::BlockedBy, open_id));
        // dep_dangling: blocked by an unresolvable target → BLOCKED (defensive).
        let mut dep_dangling = sample_req("FR-1-013", "dep-dangling");
        dep_dangling
            .relationships
            .push(rel(RelationshipType::BlockedBy, dangling_id));
        // dep_blocks_only: carries a Blocks edge (not BlockedBy) → NOT blocked.
        let mut dep_blocks_only = sample_req("FR-1-014", "dep-blocks-only");
        dep_blocks_only
            .relationships
            .push(rel(RelationshipType::Blocks, open_id));

        let mut store = RequirementsStore::new();
        store.requirements.extend([
            lone,
            blocker_open,
            blocker_done,
            dep_open,
            dep_done,
            dep_mixed,
            dep_dangling,
            dep_blocks_only,
        ]);

        cache.rebuild_from_store(&store, "head").unwrap();

        // Ground truth: the edge-walk over the full store.
        let expected = compute_blocked(&store);

        // Read every row's cached blocked flag (Both = ignore archive/defer).
        let filter = ListFilter {
            archive: ArchiveFilter::Both,
            defer: DeferFilter::Both,
            ..Default::default()
        };
        let rows = cache.list_summaries(&filter).unwrap();
        assert_eq!(rows.len(), store.requirements.len());

        for row in &rows {
            let want = expected.contains(&row.id);
            assert_eq!(
                row.blocked, want,
                "cache blocked flag for {:?} disagrees with compute_blocked",
                row.spec_id
            );
            // And cross-check directly against the pickability predicate.
            let req = store.requirements.iter().find(|r| r.id == row.id).unwrap();
            assert_eq!(
                row.blocked,
                crate::pickability::blocked_by_incomplete(req, &store),
                "cache blocked flag for {:?} disagrees with blocked_by_incomplete",
                row.spec_id
            );
        }

        // The blocked-axis filter pushes down to the cache: only the three
        // genuinely-blocked specs come back.
        let blocked_filter = ListFilter {
            archive: ArchiveFilter::Both,
            defer: DeferFilter::Both,
            blocked: Some(true),
            ..Default::default()
        };
        let blocked_rows = cache.list_summaries(&blocked_filter).unwrap();
        let blocked_specs: std::collections::HashSet<String> = blocked_rows
            .iter()
            .filter_map(|r| r.spec_id.clone())
            .collect();
        assert_eq!(
            blocked_specs,
            ["FR-1-010", "FR-1-012", "FR-1-013"]
                .into_iter()
                .map(String::from)
                .collect::<std::collections::HashSet<_>>(),
        );
    }

    /// STORY-632: the static type-weight table matches the operator-agreed
    /// values (BlockedBy/Blocks=3, Parent/Child=2, Verifies/References/Custom=1,
    /// Duplicate=0).
    #[test]
    fn edge_weight_table_matches_agreed_values() {
        assert_eq!(edge_weight(&RelationshipType::BlockedBy), 3);
        assert_eq!(edge_weight(&RelationshipType::Blocks), 3);
        assert_eq!(edge_weight(&RelationshipType::Parent), 2);
        assert_eq!(edge_weight(&RelationshipType::Child), 2);
        assert_eq!(edge_weight(&RelationshipType::Verifies), 1);
        assert_eq!(edge_weight(&RelationshipType::VerifiedBy), 1);
        assert_eq!(edge_weight(&RelationshipType::References), 1);
        assert_eq!(edge_weight(&RelationshipType::Duplicate), 0);
        assert_eq!(edge_weight(&RelationshipType::Custom("x".into())), 1);
    }

    /// STORY-632: the cache round-trips the new in/out degree + heft columns
    /// across a rebuild, and `degrees_for_id` reads them back.
    #[test]
    fn cache_roundtrips_degree_columns() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut a = sample_req("FR-1-001", "alpha");
        let b = sample_req("FR-1-002", "beta");
        let (a_id, b_id) = (a.id, b.id);
        // a --Blocks--> b  (weight 3). a: out 1, b: in 1.
        a.relationships.push(rel(RelationshipType::Blocks, b_id));

        let mut store = RequirementsStore::new();
        store.requirements.push(a);
        store.requirements.push(b);
        cache.rebuild_from_store(&store, "head").unwrap();

        let a_d = cache.degrees_for_id(&a_id).unwrap();
        assert_eq!(a_d.out_degree, 1);
        assert_eq!(a_d.in_degree, 0);
        assert_eq!(a_d.heft, 3);

        let b_d = cache.degrees_for_id(&b_id).unwrap();
        assert_eq!(b_d.in_degree, 1);
        assert_eq!(b_d.out_degree, 0);
        assert_eq!(b_d.heft, 3);

        // Unknown id → all-zero default.
        assert_eq!(
            cache.degrees_for_id(&Uuid::nil()).unwrap(),
            Degrees::default()
        );
    }

    /// STORY-632: `SortOrder::HeftDesc` orders the most-connected spec first.
    #[test]
    fn list_summaries_sorts_by_heft() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut hub = sample_req("HUB-1", "hub");
        let leaf = sample_req("LEAF-1", "leaf");
        let lonely = sample_req("LONE-1", "lonely");
        hub.relationships
            .push(rel(RelationshipType::Blocks, leaf.id)); // hub heft 3, leaf heft 3

        let mut store = RequirementsStore::new();
        store.requirements.push(hub);
        store.requirements.push(leaf);
        store.requirements.push(lonely); // heft 0
        cache.rebuild_from_store(&store, "head").unwrap();

        let rows = cache
            .list_summaries(&ListFilter {
                sort: SortOrder::HeftDesc,
                ..Default::default()
            })
            .unwrap();
        // Lonely (heft 0) must be last; the two heft-3 specs lead.
        assert_eq!(rows.last().unwrap().spec_id.as_deref(), Some("LONE-1"));
        assert!(rows[0].heft >= rows[2].heft);
    }

    #[test]
    fn roundtrip_rebuild_and_upsert() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        // Initially stale (no recorded SHA).
        assert!(cache.is_stale("anysha").unwrap());

        let mut store = RequirementsStore::new();
        store.requirements.push(sample_req("FR-1-001", "first"));
        store.requirements.push(sample_req("FR-1-002", "second"));

        let n = cache.rebuild_from_store(&store, "headsha1").unwrap();
        assert_eq!(n, 2);
        assert_eq!(cache.requirement_count().unwrap(), 2);
        assert!(!cache.is_stale("headsha1").unwrap());
        assert!(cache.is_stale("headsha2").unwrap());

        // Upsert: replace title of an existing requirement.
        let mut updated = store.requirements[0].clone();
        updated.title = "first updated".into();
        cache.upsert_requirement(&updated).unwrap();
        assert_eq!(cache.requirement_count().unwrap(), 2);

        // Delete: remove the second requirement.
        cache.delete_requirement(&store.requirements[1].id).unwrap();
        assert_eq!(cache.requirement_count().unwrap(), 1);
    }

    #[test]
    fn list_summaries_filters_and_orders() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        let mut a = sample_req("FR-1-001", "alpha");
        a.owner = "joe".into();
        let mut b = sample_req("FR-1-002", "beta");
        b.owner = "spock".into();
        let mut c = sample_req("BUG-1-003", "gamma");
        c.owner = "joe".into();
        c.archived = true;
        store.requirements.push(a);
        store.requirements.push(b);
        store.requirements.push(c);

        cache.rebuild_from_store(&store, "head").unwrap();

        // Default filter: skip archived → 2 rows.
        let all = cache.list_summaries(&ListFilter::default()).unwrap();
        assert_eq!(all.len(), 2);

        // Filter by owner.
        let mine = cache
            .list_summaries(&ListFilter {
                owner: Some("joe".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].spec_id.as_deref(), Some("FR-1-001"));

        // ArchiveFilter::Both returns the BUG too.
        let everything = cache
            .list_summaries(&ListFilter {
                archive: ArchiveFilter::Both,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(everything.len(), 3);
    }

    /// STORY-639: the assignee column round-trips through the cache and the
    /// `assignee` filter narrows to specs assigned to that user (the substrate
    /// behind `aida list --mine` / `--assigned`). trace:STORY-639 | ai:claude
    #[test]
    fn list_summaries_filter_by_assignee() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        let mut a = sample_req("FR-1-001", "alpha");
        a.assignee = Some("alice".into());
        let mut b = sample_req("FR-1-002", "beta");
        b.assignee = Some("bob".into());
        let c = sample_req("FR-1-003", "gamma"); // unassigned
        store.requirements.push(a);
        store.requirements.push(b);
        store.requirements.push(c);

        cache.rebuild_from_store(&store, "head").unwrap();

        // The assignee column round-trips (None for the unassigned row).
        let all = cache.list_summaries(&ListFilter::default()).unwrap();
        let by_id = |id: &str| {
            all.iter()
                .find(|r| r.spec_id.as_deref() == Some(id))
                .unwrap()
        };
        assert_eq!(by_id("FR-1-001").assignee.as_deref(), Some("alice"));
        assert_eq!(by_id("FR-1-003").assignee, None);

        // Filter to alice's work only.
        let alices = cache
            .list_summaries(&ListFilter {
                assignee: Some("alice".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(alices.len(), 1);
        assert_eq!(alices[0].spec_id.as_deref(), Some("FR-1-001"));

        // An unassigned filter target matches nothing.
        let nobody = cache
            .list_summaries(&ListFilter {
                assignee: Some("carol".into()),
                ..Default::default()
            })
            .unwrap();
        assert!(nobody.is_empty());
    }

    /// STORY-662: the `owner_or_assignee` filter (behind `aida list --user
    /// <name>` / `me` / `user:<name>`) matches a spec the person OWNS or is
    /// ASSIGNED — broader than the owner-only or assignee-only filters.
    /// trace:STORY-662 | ai:claude
    #[test]
    fn list_summaries_filter_by_owner_or_assignee() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        // owned by joe (sample_req default), unassigned.
        let owned = sample_req("FR-1-001", "owned-by-joe");
        // owned by spock, ASSIGNED to joe.
        let mut assigned = sample_req("FR-1-002", "assigned-to-joe");
        assigned.owner = "spock".into();
        assigned.assignee = Some("joe".into());
        // neither owned by nor assigned to joe.
        let mut neither = sample_req("FR-1-003", "spocks");
        neither.owner = "spock".into();
        neither.assignee = Some("uhura".into());
        store.requirements.push(owned);
        store.requirements.push(assigned);
        store.requirements.push(neither);

        cache.rebuild_from_store(&store, "head").unwrap();

        // joe's view: the owned one AND the assigned-to-joe one, not spock's.
        let joes = cache
            .list_summaries(&ListFilter {
                owner_or_assignee: Some("joe".into()),
                ..Default::default()
            })
            .unwrap();
        let mut ids: Vec<&str> = joes.iter().filter_map(|r| r.spec_id.as_deref()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["FR-1-001", "FR-1-002"]);

        // A handle that neither owns nor is assigned anything matches nothing.
        let nobody = cache
            .list_summaries(&ListFilter {
                owner_or_assignee: Some("carol".into()),
                ..Default::default()
            })
            .unwrap();
        assert!(nobody.is_empty());
    }

    // TASK-845: the assignee filter ORs in the canonical person's aliases, so
    // `--mine` / `--assigned` surfaces specs assigned under ANY of one human's
    // cross-host owner strings. Empty `assignee_aliases` keeps the plain match.
    // trace:TASK-845 | ai:claude
    #[test]
    fn list_summaries_assignee_filter_matches_person_aliases() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        let mut a = sample_req("FR-1-001", "assigned-to-joe");
        a.assignee = Some("joe".into());
        let mut b = sample_req("FR-1-002", "assigned-to-alias");
        b.assignee = Some("joe.mooney@gmail.com".into());
        let mut c = sample_req("FR-1-003", "assigned-to-stranger");
        c.assignee = Some("uhura".into());
        store.requirements.push(a);
        store.requirements.push(b);
        store.requirements.push(c);
        cache.rebuild_from_store(&store, "head").unwrap();

        // Plain assignee filter (no aliases) → only the exact handle.
        let plain = cache
            .list_summaries(&ListFilter {
                assignee: Some("joe".into()),
                ..Default::default()
            })
            .unwrap();
        let ids: Vec<&str> = plain.iter().filter_map(|r| r.spec_id.as_deref()).collect();
        assert_eq!(ids, vec!["FR-1-001"], "no aliases → single-handle match");

        // With the person's aliases expanded → both joe and the linked alias.
        let mut joes = cache
            .list_summaries(&ListFilter {
                assignee: Some("joe".into()),
                assignee_aliases: vec!["joe.mooney@gmail.com".into()],
                ..Default::default()
            })
            .unwrap();
        joes.sort_by(|x, y| x.spec_id.cmp(&y.spec_id));
        let ids: Vec<&str> = joes.iter().filter_map(|r| r.spec_id.as_deref()).collect();
        assert_eq!(
            ids,
            vec!["FR-1-001", "FR-1-002"],
            "alias-expanded filter spans both owner strings, never the stranger"
        );
    }

    /// TASK-527: `--tags aida:queue:*` prefix-glob matches every tag under the
    /// `aida:queue:` surface plus an exact bare `aida:queue`, and never a
    /// sibling surface; exact match still works.
    #[test]
    fn list_summaries_tag_prefix_glob() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        let mut work = sample_req("TASK-1", "work");
        work.tags.insert("aida:queue:work".into());
        let mut list = sample_req("TASK-2", "list");
        list.tags.insert("aida:queue:list".into());
        let mut bare = sample_req("TASK-3", "bare");
        bare.tags.insert("aida:queue".into());
        let mut status = sample_req("TASK-4", "status");
        status.tags.insert("aida:status".into());
        store.requirements.extend([work, list, bare, status]);
        cache.rebuild_from_store(&store, "head").unwrap();

        let glob = cache
            .list_summaries(&ListFilter {
                tags: vec!["aida:queue:*".into()],
                ..Default::default()
            })
            .unwrap();
        // work + list (prefix) + bare (exact via glob) = 3; aida:status excluded.
        assert_eq!(glob.len(), 3, "prefix-glob over the queue surface");

        let exact = cache
            .list_summaries(&ListFilter {
                tags: vec!["aida:status".into()],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(exact.len(), 1, "exact match unaffected");
    }

    /// trace:TASK-132 | ai:codex
    #[test]
    fn list_summaries_combines_status_type_and_priority_with_and() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        let mut exact = sample_req("TASK-132-001", "approved low task");
        exact.status = RequirementStatus::Approved;
        exact.priority = RequirementPriority::Low;
        exact.req_type = RequirementType::Task;

        let mut wrong_priority = sample_req("TASK-132-002", "approved medium task");
        wrong_priority.status = RequirementStatus::Approved;
        wrong_priority.priority = RequirementPriority::Medium;
        wrong_priority.req_type = RequirementType::Task;

        let mut wrong_status = sample_req("TASK-132-003", "planned low task");
        wrong_status.status = RequirementStatus::Planned;
        wrong_status.priority = RequirementPriority::Low;
        wrong_status.req_type = RequirementType::Task;

        let mut wrong_type = sample_req("BUG-132-004", "approved low bug");
        wrong_type.status = RequirementStatus::Approved;
        wrong_type.priority = RequirementPriority::Low;
        wrong_type.req_type = RequirementType::Bug;

        store.requirements.push(exact);
        store.requirements.push(wrong_priority);
        store.requirements.push(wrong_status);
        store.requirements.push(wrong_type);
        cache.rebuild_from_store(&store, "head").unwrap();

        let rows = cache
            .list_summaries(&ListFilter {
                status: Some("approved".into()),
                req_type: Some("Task".into()),
                priority: Some("low".into()),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].spec_id.as_deref(), Some("TASK-132-001"));
    }

    /// trace:TASK-0415 — a comma-separated status spec is a logical OR within
    /// the status axis, and still composes with other filters via AND.
    #[test]
    fn list_summaries_status_comma_is_or() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        let mut draft = sample_req("TASK-415-001", "draft task");
        draft.status = RequirementStatus::Draft;
        let mut approved = sample_req("TASK-415-002", "approved task");
        approved.status = RequirementStatus::Approved;
        let mut planned = sample_req("TASK-415-003", "planned task");
        planned.status = RequirementStatus::Planned;
        store.requirements.push(draft);
        store.requirements.push(approved);
        store.requirements.push(planned);
        cache.rebuild_from_store(&store, "head").unwrap();

        // Draft OR Approved → two rows, Planned excluded.
        let rows = cache
            .list_summaries(&ListFilter {
                status: Some("Draft,Approved".into()),
                ..Default::default()
            })
            .unwrap();
        let mut ids: Vec<_> = rows.iter().filter_map(|r| r.spec_id.clone()).collect();
        ids.sort();
        assert_eq!(ids, vec!["TASK-415-001", "TASK-415-002"]);

        // A single value still behaves like equality (backward compat).
        let one = cache
            .list_summaries(&ListFilter {
                status: Some("Planned".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].spec_id.as_deref(), Some("TASK-415-003"));
    }

    /// trace:STORY-441 | ai:claude
    #[test]
    fn archive_filter_non_archived_only_excludes_archived() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        let active = sample_req("FR-1-001", "active");
        let mut archived = sample_req("FR-1-002", "archived");
        archived.archived = true;
        archived.archived_at = Some(chrono::Utc::now());
        store.requirements.push(active);
        store.requirements.push(archived);
        cache.rebuild_from_store(&store, "head").unwrap();

        let rows = cache
            .list_summaries(&ListFilter {
                archive: ArchiveFilter::NonArchivedOnly,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "active");
    }

    /// trace:STORY-441 | ai:claude
    #[test]
    fn archive_filter_archived_only_returns_only_archived() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        let active = sample_req("FR-1-001", "active");
        let mut archived = sample_req("FR-1-002", "archived");
        archived.archived = true;
        archived.archived_at = Some(chrono::Utc::now());
        store.requirements.push(active);
        store.requirements.push(archived);
        cache.rebuild_from_store(&store, "head").unwrap();

        let rows = cache
            .list_summaries(&ListFilter {
                archive: ArchiveFilter::ArchivedOnly,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "archived");
        assert!(rows[0].archived);
        assert!(rows[0].archived_at.is_some());
    }

    /// trace:STORY-441 | ai:claude
    #[test]
    fn archive_filter_both_returns_everything() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        let active = sample_req("FR-1-001", "active");
        let mut archived = sample_req("FR-1-002", "archived");
        archived.archived = true;
        archived.archived_at = Some(chrono::Utc::now());
        store.requirements.push(active);
        store.requirements.push(archived);
        cache.rebuild_from_store(&store, "head").unwrap();

        let rows = cache
            .list_summaries(&ListFilter {
                archive: ArchiveFilter::Both,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 2);
    }

    /// trace:STORY-441 | ai:claude
    #[test]
    fn archived_at_round_trips_through_cache() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        let mut req = sample_req("FR-1-001", "archived");
        req.archived = true;
        let ts = chrono::Utc::now();
        req.archived_at = Some(ts);
        store.requirements.push(req);
        cache.rebuild_from_store(&store, "head").unwrap();

        let rows = cache
            .list_summaries(&ListFilter {
                archive: ArchiveFilter::Both,
                ..Default::default()
            })
            .unwrap();
        let row = rows.into_iter().next().expect("at least one row");
        let raw = row.archived_at.expect("archived_at should be set");
        // Parses back as the same instant (round-trips to millisecond
        // resolution at minimum — RFC3339 keeps full precision).
        let parsed = chrono::DateTime::parse_from_rfc3339(&raw).unwrap();
        assert_eq!(
            parsed.with_timezone(&chrono::Utc).timestamp_millis(),
            ts.timestamp_millis()
        );
    }

    /// trace:STORY-584 | ai:claude
    #[test]
    fn defer_filter_non_deferred_only_excludes_deferred() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        let active = sample_req("FR-1-001", "active");
        let mut deferred = sample_req("FR-1-002", "deferred");
        deferred.deferred = true;
        deferred.deferred_at = Some(chrono::Utc::now());
        deferred.deferred_until = Some("when the shelf grows".into());
        store.requirements.push(active);
        store.requirements.push(deferred);
        cache.rebuild_from_store(&store, "head").unwrap();

        let rows = cache
            .list_summaries(&ListFilter {
                defer: DeferFilter::NonDeferredOnly,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "active");
    }

    /// The legacy `deferred:*` parking tag is honored by the default view even
    /// when the flag is unset (honor-both migration). trace:STORY-584 | ai:claude
    #[test]
    fn defer_filter_honors_legacy_deferred_tag() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        let active = sample_req("FR-1-001", "active");
        let mut tagged = sample_req("FR-1-002", "tag-deferred");
        tagged.tags.insert("deferred:stabilization-first".into());
        store.requirements.push(active);
        store.requirements.push(tagged);
        cache.rebuild_from_store(&store, "head").unwrap();

        // Default hides the tag-deferred spec...
        let active_rows = cache
            .list_summaries(&ListFilter {
                defer: DeferFilter::NonDeferredOnly,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(active_rows.len(), 1);
        assert_eq!(active_rows[0].title, "active");

        // ...and DeferredOnly surfaces it despite the unset flag.
        let deferred_rows = cache
            .list_summaries(&ListFilter {
                defer: DeferFilter::DeferredOnly,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(deferred_rows.len(), 1);
        assert_eq!(deferred_rows[0].title, "tag-deferred");
        assert!(!deferred_rows[0].deferred); // flag unset; matched by tag
    }

    /// trace:STORY-584 | ai:claude
    #[test]
    fn defer_filter_deferred_only_returns_flag_and_tag_deferred() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        store.requirements.push(sample_req("FR-1-001", "active"));
        let mut flagged = sample_req("FR-1-002", "flag-deferred");
        flagged.deferred = true;
        let mut tagged = sample_req("FR-1-003", "tag-deferred");
        tagged.tags.insert("deferred:on-demand".into());
        store.requirements.push(flagged);
        store.requirements.push(tagged);
        cache.rebuild_from_store(&store, "head").unwrap();

        let rows = cache
            .list_summaries(&ListFilter {
                defer: DeferFilter::DeferredOnly,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.title != "active"));
    }

    /// trace:STORY-584 | ai:claude
    #[test]
    fn defer_filter_both_returns_everything() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        store.requirements.push(sample_req("FR-1-001", "active"));
        let mut deferred = sample_req("FR-1-002", "deferred");
        deferred.deferred = true;
        store.requirements.push(deferred);
        cache.rebuild_from_store(&store, "head").unwrap();

        let rows = cache
            .list_summaries(&ListFilter {
                defer: DeferFilter::Both,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 2);
    }

    /// The defer axis is orthogonal to the archive axis — each filters
    /// independently. trace:STORY-584 | ai:claude
    #[test]
    fn defer_and_archive_axes_compose_independently() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        store.requirements.push(sample_req("FR-1-001", "active"));
        let mut deferred = sample_req("FR-1-002", "deferred");
        deferred.deferred = true;
        let mut archived = sample_req("FR-1-003", "archived");
        archived.archived = true;
        store.requirements.push(deferred);
        store.requirements.push(archived);
        cache.rebuild_from_store(&store, "head").unwrap();

        // Default both-axes view: only the active row.
        let rows = cache
            .list_summaries(&ListFilter {
                archive: ArchiveFilter::NonArchivedOnly,
                defer: DeferFilter::NonDeferredOnly,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "active");
    }

    /// deferred_at + deferred_until survive the cache round-trip.
    /// trace:STORY-584 | ai:claude
    #[test]
    fn deferred_fields_round_trip_through_cache() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        let mut req = sample_req("FR-1-001", "deferred");
        req.deferred = true;
        let ts = chrono::Utc::now();
        req.deferred_at = Some(ts);
        req.deferred_until = Some("when a slice verb ships".into());
        store.requirements.push(req);
        cache.rebuild_from_store(&store, "head").unwrap();

        let rows = cache
            .list_summaries(&ListFilter {
                defer: DeferFilter::Both,
                ..Default::default()
            })
            .unwrap();
        let row = rows.into_iter().next().expect("at least one row");
        assert!(row.deferred);
        assert_eq!(
            row.deferred_until.as_deref(),
            Some("when a slice verb ships")
        );
        let raw = row.deferred_at.expect("deferred_at should be set");
        let parsed = chrono::DateTime::parse_from_rfc3339(&raw).unwrap();
        assert_eq!(
            parsed.with_timezone(&chrono::Utc).timestamp_millis(),
            ts.timestamp_millis()
        );
    }

    #[test]
    fn search_uses_fts5() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        store
            .requirements
            .push(sample_req("FR-1-001", "git canonical storage"));
        store
            .requirements
            .push(sample_req("FR-1-002", "react dashboard"));
        store
            .requirements
            .push(sample_req("FR-1-003", "canonical readme"));

        cache.rebuild_from_store(&store, "head").unwrap();

        let hits = cache
            .search(
                "canonical",
                10,
                ArchiveFilter::NonArchivedOnly,
                DeferFilter::NonDeferredOnly,
            )
            .unwrap();
        assert_eq!(hits.len(), 2);
        let titles: Vec<_> = hits.iter().map(|h| h.title.as_str()).collect();
        assert!(titles.contains(&"git canonical storage"));
        assert!(titles.contains(&"canonical readme"));
    }

    #[test]
    fn search_handles_spec_id_shaped_queries() {
        // trace:BUG-77 | ai:claude
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        let mut a = sample_req("PR-9", "PR-9 thing");
        a.spec_id = Some("PR-9".into());
        store.requirements.push(a);
        store.requirements.push(sample_req("FR-1-002", "unrelated"));

        cache.rebuild_from_store(&store, "head").unwrap();

        // Each of these used to error with `no such column: 9` (or similar).
        for q in &["PR-9", "EPIC-20", "STORY-86", "BUG:78", "*foo", "(a)"] {
            let _ = cache
                .search(
                    q,
                    10,
                    ArchiveFilter::NonArchivedOnly,
                    DeferFilter::NonDeferredOnly,
                )
                .unwrap_or_else(|e| {
                    panic!("search({:?}) errored: {}", q, e);
                });
        }

        // Multi-word still does implicit-AND across tokens.
        let mut s = RequirementsStore::new();
        s.requirements
            .push(sample_req("FR-2-001", "alpha beta gamma"));
        s.requirements.push(sample_req("FR-2-002", "alpha delta"));
        let dir2 = tempdir().unwrap();
        let c2 = Cache::open(dir2.path().join("cache.db")).unwrap();
        c2.rebuild_from_store(&s, "head").unwrap();
        let hits = c2
            .search(
                "alpha beta",
                10,
                ArchiveFilter::NonArchivedOnly,
                DeferFilter::NonDeferredOnly,
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "alpha beta gamma");

        // Empty query: well-formed (returns nothing rather than panicking).
        assert!(cache
            .search(
                "",
                10,
                ArchiveFilter::NonArchivedOnly,
                DeferFilter::NonDeferredOnly
            )
            .is_ok());
        assert!(cache
            .search(
                "   ",
                10,
                ArchiveFilter::NonArchivedOnly,
                DeferFilter::NonDeferredOnly
            )
            .is_ok());
    }

    #[test]
    fn search_finds_spec_by_external_ref() {
        // STORY-476: external issue refs are indexed in FTS so a spec is
        // findable by its linear:/jira:/github: pointer.
        // trace:STORY-476 | ai:claude
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut store = RequirementsStore::new();
        let mut a = sample_req("STORY-476", "external refs");
        a.external_refs = vec!["linear:LIN-123".into(), "github:owner/repo#7".into()];
        store.requirements.push(a);
        store.requirements.push(sample_req("FR-1-002", "unrelated"));

        cache.rebuild_from_store(&store, "head").unwrap();

        let hits = cache
            .search(
                "LIN-123",
                10,
                ArchiveFilter::NonArchivedOnly,
                DeferFilter::NonDeferredOnly,
            )
            .unwrap();
        assert_eq!(
            hits.len(),
            1,
            "should find the spec carrying linear:LIN-123"
        );
        assert_eq!(hits[0].spec_id.as_deref(), Some("STORY-476"));

        // The other ref's token is searchable too.
        let hits2 = cache
            .search(
                "owner",
                10,
                ArchiveFilter::NonArchivedOnly,
                DeferFilter::NonDeferredOnly,
            )
            .unwrap();
        assert_eq!(hits2.len(), 1);
        assert_eq!(hits2[0].spec_id.as_deref(), Some("STORY-476"));
    }

    #[test]
    fn old_schema_fts_cache_self_heals_on_open() {
        // BUG-485: a cache whose `requirements_fts` predates the
        // `external_refs` column must drop + rebuild on open rather than
        // hard-erroring on the next write with `table requirements_fts has no
        // column named external_refs`. Worst-case: the on-disk schema_version
        // already matches the current SCHEMA_VERSION (the bump was missed /
        // the FTS table drifted), so a version-stamp check alone would NOT
        // fire — the structural drift detector must.
        // trace:BUG-485
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("cache.db");

        // Hand-build an OLD-schema cache: the FTS table is the pre-STORY-476
        // 5-column shape (no `external_refs`), but the meta version is stamped
        // to the CURRENT value to prove the trap a pure version-check leaves.
        {
            let conn = Connection::open(&cache_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE cache_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE VIRTUAL TABLE requirements_fts USING fts5(
                     id UNINDEXED, spec_id, agreed_id, title, description,
                     tokenize = 'porter unicode61'
                 );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO cache_meta (key, value) VALUES (?1, ?2)",
                params![META_KEY_SCHEMA_VERSION, SCHEMA_VERSION],
            )
            .unwrap();
            // Stamp a head SHA so a naive open would think the cache is fresh.
            conn.execute(
                "INSERT INTO cache_meta (key, value) VALUES (?1, ?2)",
                params![META_KEY_SOURCE_HEAD_SHA, "stalehead"],
            )
            .unwrap();
        }

        // Open with the current binary — must self-heal (drop drifted table)
        // and clear the stamped head SHA so the next read rebuilds from git.
        let cache = Cache::open(&cache_path).unwrap();
        assert!(
            cache.source_head_sha().unwrap().is_none(),
            "self-heal should invalidate the recorded head SHA so a rebuild fires"
        );

        // A write that uses the new `external_refs` FTS column must now
        // succeed (would have hard-errored against the old table).
        let mut store = RequirementsStore::new();
        let mut a = sample_req("BUG-485", "fts drift heals");
        a.external_refs = vec!["linear:LIN-485".into()];
        store.requirements.push(a);
        cache
            .rebuild_from_store(&store, "newhead")
            .expect("rebuild against the healed FTS schema must succeed");

        // And the external ref is searchable, proving the column is live.
        let hits = cache
            .search(
                "LIN-485",
                10,
                ArchiveFilter::NonArchivedOnly,
                DeferFilter::NonDeferredOnly,
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].spec_id.as_deref(), Some("BUG-485"));
    }

    #[test]
    fn fts_schema_drift_detects_missing_column_only() {
        // trace:BUG-485 — the detector must fire on a drifted (old) FTS table
        // and stay quiet on a current one and on a fresh (absent) one.
        let dir = tempdir().unwrap();

        // Absent table → not drifted (fresh apply will create it).
        let fresh = Connection::open(dir.path().join("fresh.db")).unwrap();
        assert!(!fts_schema_drifted(&fresh));

        // Old 5-column FTS table → drifted.
        let old = Connection::open(dir.path().join("old.db")).unwrap();
        old.execute_batch(
            "CREATE VIRTUAL TABLE requirements_fts USING fts5(
                 id UNINDEXED, spec_id, agreed_id, title, description
             );",
        )
        .unwrap();
        assert!(fts_schema_drifted(&old));

        // Current schema → not drifted (no spurious rebuild on normal runs).
        let current = Connection::open(dir.path().join("current.db")).unwrap();
        current.execute_batch(SCHEMA_SQL).unwrap();
        assert!(!fts_schema_drifted(&current));
    }

    #[test]
    fn old_schema_cache_missing_column_self_heals_on_open() {
        // BUG-627: a cache whose `requirements_cache` table predates the
        // `blocked` column (TASK-902) must drop + rebuild on open rather than
        // hard-erroring with `table requirements_cache has no column named
        // blocked`. Worst-case (the operator's actual hit): the on-disk
        // schema_version already matches the CURRENT SCHEMA_VERSION and the head
        // SHA is stamped — `aida cache status` reports FRESH — yet the table is
        // missing the column because a concurrent older-schema binary built it
        // (or a torn rebuild). A version-stamp + HEAD-SHA check alone would NOT
        // fire here; the structural column-existence detector must.
        // trace:BUG-627
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("cache.db");

        // Hand-build an OLD-schema cache: the requirements_cache table is the
        // pre-TASK-902 shape (no `blocked` column), but the meta version is
        // stamped to the CURRENT value and a head SHA is recorded to prove the
        // FRESH-but-broken trap a pure version/SHA check leaves.
        {
            let conn = Connection::open(&cache_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE cache_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE requirements_cache (
                     id TEXT PRIMARY KEY NOT NULL,
                     spec_id TEXT,
                     agreed_id TEXT,
                     title TEXT NOT NULL,
                     description TEXT NOT NULL DEFAULT '',
                     status TEXT NOT NULL,
                     priority TEXT NOT NULL,
                     owner TEXT NOT NULL DEFAULT '',
                     assignee TEXT,
                     feature TEXT NOT NULL DEFAULT '',
                     req_type TEXT NOT NULL,
                     tags_json TEXT NOT NULL DEFAULT '[]',
                     created_at TEXT NOT NULL,
                     modified_at TEXT NOT NULL,
                     archived INTEGER NOT NULL DEFAULT 0,
                     archived_at TEXT,
                     deferred INTEGER NOT NULL DEFAULT 0,
                     deferred_at TEXT,
                     deferred_until TEXT,
                     in_degree INTEGER NOT NULL DEFAULT 0,
                     out_degree INTEGER NOT NULL DEFAULT 0,
                     heft INTEGER NOT NULL DEFAULT 0,
                     -- NOTE: no `blocked` column — pre-TASK-902 shape.
                     yaml_path TEXT NOT NULL
                 );
                 CREATE VIRTUAL TABLE requirements_fts USING fts5(
                     id UNINDEXED, spec_id, agreed_id, title, description, external_refs,
                     tokenize = 'porter unicode61'
                 );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO cache_meta (key, value) VALUES (?1, ?2)",
                params![META_KEY_SCHEMA_VERSION, SCHEMA_VERSION],
            )
            .unwrap();
            // Stamp a head SHA so a naive open would think the cache is fresh.
            conn.execute(
                "INSERT INTO cache_meta (key, value) VALUES (?1, ?2)",
                params![META_KEY_SOURCE_HEAD_SHA, "stalehead"],
            )
            .unwrap();
        }

        // Open with the current binary — must self-heal (drop the drifted table)
        // and clear the stamped head SHA so the next read rebuilds from git.
        let cache = Cache::open(&cache_path).unwrap();
        assert!(
            cache.source_head_sha().unwrap().is_none(),
            "self-heal should invalidate the recorded head SHA so a rebuild fires"
        );

        // A rebuild that writes the `blocked` column must now succeed (would have
        // hard-errored against the old table). This is the `insert_one` write
        // path the operator's `aida show`/`list` exercised.
        let mut store = RequirementsStore::new();
        store
            .requirements
            .push(sample_req("BUG-627", "blocked drift heals"));
        cache
            .rebuild_from_store(&store, "newhead")
            .expect("rebuild against the healed cache schema must succeed");

        // And a query touching the `blocked` column must succeed, not error.
        let rows = cache
            .list_summaries(&ListFilter::default())
            .expect("list reading the `blocked` column must succeed after self-heal");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].spec_id.as_deref(), Some("BUG-627"));
    }

    #[test]
    fn cache_schema_drift_detects_missing_column_only() {
        // trace:BUG-627 — the detector must fire on a drifted (old) cache table
        // and stay quiet on a current one and on a fresh (absent) one.
        let dir = tempdir().unwrap();

        // Absent table → not drifted (fresh apply will create it).
        let fresh = Connection::open(dir.path().join("fresh.db")).unwrap();
        assert!(!cache_schema_drifted(&fresh));

        // Old table missing the `blocked` column → drifted.
        let old = Connection::open(dir.path().join("old.db")).unwrap();
        old.execute_batch(
            "CREATE TABLE requirements_cache (
                 id TEXT PRIMARY KEY NOT NULL,
                 spec_id TEXT,
                 agreed_id TEXT,
                 title TEXT NOT NULL,
                 description TEXT NOT NULL DEFAULT '',
                 status TEXT NOT NULL,
                 priority TEXT NOT NULL,
                 owner TEXT NOT NULL DEFAULT '',
                 assignee TEXT,
                 feature TEXT NOT NULL DEFAULT '',
                 req_type TEXT NOT NULL,
                 tags_json TEXT NOT NULL DEFAULT '[]',
                 created_at TEXT NOT NULL,
                 modified_at TEXT NOT NULL,
                 archived INTEGER NOT NULL DEFAULT 0,
                 archived_at TEXT,
                 deferred INTEGER NOT NULL DEFAULT 0,
                 deferred_at TEXT,
                 deferred_until TEXT,
                 in_degree INTEGER NOT NULL DEFAULT 0,
                 out_degree INTEGER NOT NULL DEFAULT 0,
                 heft INTEGER NOT NULL DEFAULT 0,
                 yaml_path TEXT NOT NULL
             );",
        )
        .unwrap();
        assert!(cache_schema_drifted(&old));

        // Current schema → not drifted (no spurious rebuild on normal runs).
        let current = Connection::open(dir.path().join("current.db")).unwrap();
        current.execute_batch(SCHEMA_SQL).unwrap();
        assert!(!cache_schema_drifted(&current));
    }

    #[test]
    fn escape_fts5_query_quotes_each_token() {
        // trace:BUG-77 | ai:claude
        assert_eq!(escape_fts5_query("PR-9"), "\"PR-9\"");
        assert_eq!(escape_fts5_query("EPIC-20"), "\"EPIC-20\"");
        assert_eq!(escape_fts5_query("alpha beta"), "\"alpha\" \"beta\"");
        assert_eq!(escape_fts5_query(""), "");
        assert_eq!(escape_fts5_query("   "), "");
        // Embedded double-quote → doubled.
        assert_eq!(escape_fts5_query(r#"a"b"#), "\"a\"\"b\"");
    }

    #[test]
    fn yaml_path_layout_matches_git_backend() {
        let r = sample_req("FR-1-001", "x");
        assert_eq!(yaml_path_for(&r), "objects/FR/000/FR-1-001.yaml");

        let mut r2 = sample_req("BUG-7", "y");
        r2.spec_id = Some("BUG-7".into());
        assert_eq!(yaml_path_for(&r2), "objects/BUG/000/BUG-7.yaml");
    }

    #[test]
    fn cache_open_enables_wal_and_keeps_busy_timeout_zero() {
        // trace:STORY-580 | ai:codex
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();
        let conn = cache.conn.lock().unwrap();

        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        let busy_timeout_ms: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();

        assert_eq!(journal_mode.to_lowercase(), "wal");
        assert_eq!(busy_timeout_ms, 0);
    }

    // BUG-664: re-opening a HEALTHY (current-schema) cache must not take the
    // write lock — so a pure reader never blocks behind a writer's lock for the
    // ~25s retry ladder. We hold an exclusive write transaction on a separate
    // connection and assert the re-open still completes promptly. A regression
    // (the old unconditional schema-apply / version re-stamp) would block on the
    // held write lock and the open would not return within the timeout.
    #[test]
    fn open_on_healthy_cache_does_not_block_behind_write_lock() {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("cache.db");
        // First open establishes the current schema + version stamp.
        drop(Cache::open(&cache_path).unwrap());

        // Hold an exclusive (write) transaction on a separate connection.
        let holder = Connection::open(&cache_path).unwrap();
        holder.busy_timeout(Duration::from_millis(0)).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();

        let p = cache_path.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let outcome = Cache::open(&p).map(|_| ()).map_err(|e| e.to_string());
            let _ = tx.send(outcome);
        });
        let result = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("re-opening a healthy cache must not block behind a held write lock (BUG-664)");
        result.expect("re-open of a current-schema cache should succeed without writing");

        holder.execute_batch("ROLLBACK").unwrap();
    }

    // BUG-664: read paths consult `foreign_writer_holds_lock` to decide whether
    // to serve the last-good snapshot instead of contending for the write lock.
    #[test]
    fn foreign_writer_holds_lock_only_for_live_foreign_pid() {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("cache.db");
        let lock_path = cache_lock_info_path(&cache_path);

        // No lock-info file → no holder.
        assert!(!foreign_writer_holds_lock(&cache_path));

        let write_lock = |pid: u32| {
            let info = CacheLockInfo {
                pid,
                command: "test-writer".to_string(),
                started_at: chrono::Utc::now().to_rfc3339(),
                user: "test".to_string(),
                session_id: None,
            };
            std::fs::write(&lock_path, serde_json::to_string(&info).unwrap()).unwrap();
        };

        // This process holds it → not "foreign", must never defer to itself.
        write_lock(std::process::id());
        assert!(!foreign_writer_holds_lock(&cache_path));

        // A dead pid (crashed writer) → not held, so it never wedges readers.
        write_lock(0x7fff_fffe);
        assert!(!foreign_writer_holds_lock(&cache_path));

        // A live, foreign pid (init, pid 1, always alive on unix) → held.
        #[cfg(unix)]
        {
            write_lock(1);
            assert!(foreign_writer_holds_lock(&cache_path));
        }
    }

    #[test]
    fn cache_reader_sees_last_committed_snapshot_during_write() {
        // trace:STORY-580 | ai:codex
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("cache.db");
        let writer_cache = Cache::open(&cache_path).unwrap();
        let reader_cache = Cache::open(&cache_path).unwrap();

        let mut store = RequirementsStore::new();
        store
            .requirements
            .push(sample_req("STORY-580-A", "committed"));
        writer_cache.rebuild_from_store(&store, "head").unwrap();

        let writer = Connection::open(&cache_path).unwrap();
        writer.busy_timeout(Duration::from_millis(0)).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        writer
            .execute(
                "UPDATE requirements_cache SET title = ?1 WHERE spec_id = ?2",
                params!["uncommitted", "STORY-580-A"],
            )
            .unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = reader_cache
                .list_summaries(&ListFilter::default())
                .map(|rows| rows.into_iter().map(|row| row.title).collect::<Vec<_>>());
            tx.send(result).unwrap();
        });

        let titles = rx
            .recv_timeout(Duration::from_millis(500))
            .expect("reader should not block behind an uncommitted WAL writer")
            .unwrap();
        assert_eq!(titles, vec!["committed"]);

        writer.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn default_cache_retry_budget_is_patient_enough_for_schema_contention() {
        let _guard = env_lock().lock().unwrap_or_else(|err| err.into_inner());
        std::env::remove_var("AIDA_CACHE_RETRY_COUNT");
        std::env::remove_var("AIDA_CACHE_RETRY_MS");

        let delays = cache_retry_delays()
            .into_iter()
            .map(|duration| duration.as_millis())
            .collect::<Vec<_>>();

        assert_eq!(delays, vec![100, 200, 400, 800, 1600, 3200, 6400, 12800]);
        assert!(
            delays.iter().sum::<u128>() >= 10_000,
            "default retry budget should cover 10-15s production cache locks"
        );
    }

    #[test]
    fn cache_retry_count_zero_keeps_empty_retry_budget() {
        let _guard = env_lock().lock().unwrap_or_else(|err| err.into_inner());
        std::env::set_var("AIDA_CACHE_RETRY_COUNT", "0");
        std::env::remove_var("AIDA_CACHE_RETRY_MS");

        assert!(cache_retry_delays().is_empty());

        std::env::remove_var("AIDA_CACHE_RETRY_COUNT");
    }

    // BUG-681: on the advisory `aida awaiting --notice` path, the retry ladder
    // must collapse to a short bounded set so a lock-contended open/read bails in
    // ~150ms instead of the full ~25s backoff that blows past Claude Code's 5s
    // UserPromptSubmit hook timeout. Normal (unarmed) reads keep the full ladder.
    #[test]
    fn fast_fail_cache_mode_uses_short_bounded_ladder() {
        let _guard = env_lock().lock().unwrap_or_else(|err| err.into_inner());
        std::env::remove_var("AIDA_CACHE_RETRY_COUNT");
        std::env::remove_var("AIDA_CACHE_RETRY_MS");

        // Off by default → the full, patient production ladder.
        assert!(!fast_fail_cache_enabled());
        let full: u128 = cache_retry_delays().iter().map(|d| d.as_millis()).sum();
        assert!(full >= 10_000, "unarmed reads keep the resilient ladder");

        // Armed → a short bounded ladder that bails well under the 5s hook budget.
        let prior = set_fast_fail_cache(true);
        assert!(fast_fail_cache_enabled());
        let fast = cache_retry_delays()
            .into_iter()
            .map(|d| d.as_millis())
            .collect::<Vec<_>>();
        assert_eq!(fast, vec![50, 100]);
        assert!(
            fast.iter().sum::<u128>() < 1_000,
            "notice-path ladder must bail far under the 5s hook timeout"
        );

        // An explicit AIDA_CACHE_RETRY_COUNT=0 still disables retries entirely.
        std::env::set_var("AIDA_CACHE_RETRY_COUNT", "0");
        assert!(cache_retry_delays().is_empty());
        std::env::remove_var("AIDA_CACHE_RETRY_COUNT");

        // Restore the thread-local so a reused test thread is unaffected.
        set_fast_fail_cache(prior);
        assert!(!fast_fail_cache_enabled());
    }

    // BUG-681: end-to-end — with fast-fail armed, a lock-contended cache
    // read/open must GIVE UP quickly rather than block on the full retry budget.
    // We drive the shared retry driver (`with_cache_retry`) with a closure that
    // ALWAYS reports SQLITE_BUSY, simulating a writer that never releases the
    // lock. Without the fix this spins ~25s (well past the 5s hook timeout); with
    // it, the notice-path short ladder gives up in a fraction of a second.
    #[test]
    fn fast_fail_cache_retry_bails_fast_on_persistent_lock() {
        let _guard = env_lock().lock().unwrap_or_else(|err| err.into_inner());
        std::env::remove_var("AIDA_CACHE_RETRY_COUNT");
        std::env::remove_var("AIDA_CACHE_RETRY_MS");

        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("cache.db");

        // A fresh SQLITE_BUSY error per attempt (rusqlite::Error is not Clone).
        let busy = || {
            anyhow::Error::new(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some("database is locked".to_string()),
            ))
        };

        let prior = set_fast_fail_cache(true);
        let started = std::time::Instant::now();
        let result: Result<()> =
            with_cache_retry(&cache_path, "simulated locked read", || Err(busy()));
        let elapsed = started.elapsed();
        set_fast_fail_cache(prior);

        let err = result.expect_err("a permanently-locked read must surface an error, not hang");
        assert!(
            err.to_string().contains("locked"),
            "the surfaced error should name the lock condition: {err}"
        );
        // Short ladder is 50ms + 100ms of sleeps; comfortably under a second and
        // an order of magnitude under both the ~25s full ladder and the 5s hook
        // timeout.
        assert!(
            elapsed < Duration::from_secs(1),
            "fast-fail retry must give up in ~150ms, took {elapsed:?}"
        );
    }

    #[test]
    fn cache_lock_info_round_trips_sidecar_path() {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("cache.db");
        write_cache_lock_info(&cache_path).unwrap();

        let info = read_cache_lock_info(&cache_path)
            .unwrap()
            .expect("lock info should exist");
        assert_eq!(info.pid, std::process::id());
        assert!(!info.started_at.is_empty());
        assert_eq!(
            cache_lock_info_path(&cache_path),
            dir.path().join("cache.db.lock-info")
        );

        remove_cache_lock_info(&cache_path);
        assert!(read_cache_lock_info(&cache_path).unwrap().is_none());
    }

    #[test]
    fn cache_lock_info_cleanup_preserves_other_process_sidecar() {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("cache.db");
        let path = cache_lock_info_path(&cache_path);
        let other = CacheLockInfo {
            pid: std::process::id().saturating_add(1),
            command: "other aida".to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            user: "tester".to_string(),
            session_id: None,
        };
        std::fs::write(&path, serde_json::to_string(&other).unwrap()).unwrap();

        remove_cache_lock_info(&cache_path);

        assert!(path.exists(), "must not remove another process's sidecar");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn cache_open_retries_sqlite_lock_then_succeeds() {
        let _guard = env_lock().lock().unwrap_or_else(|err| err.into_inner());
        std::env::set_var("AIDA_CACHE_RETRY_COUNT", "3");
        std::env::set_var("AIDA_CACHE_RETRY_MS", "50,200,500");

        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("cache.db");
        Cache::open(&cache_path).unwrap();
        let locker = Connection::open(&cache_path).unwrap();
        locker.execute_batch("BEGIN EXCLUSIVE").unwrap();

        let path_for_thread = cache_path.clone();
        let handle = std::thread::spawn(move || Cache::open(path_for_thread));
        std::thread::sleep(Duration::from_millis(100));
        locker.execute_batch("COMMIT").unwrap();

        let opened = handle.join().unwrap();
        if let Err(err) = opened {
            panic!("second cache open should wait for the lock: {err}");
        }
        assert!(
            !cache_lock_info_path(&cache_path).exists(),
            "successful write should remove sidecar"
        );

        std::env::remove_var("AIDA_CACHE_RETRY_COUNT");
        std::env::remove_var("AIDA_CACHE_RETRY_MS");
    }

    #[test]
    fn cache_retry_count_zero_fails_fast_with_lock_holder_hint() {
        let _guard = env_lock().lock().unwrap_or_else(|err| err.into_inner());
        std::env::set_var("AIDA_CACHE_RETRY_COUNT", "0");

        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("cache.db");
        let path = cache_lock_info_path(&cache_path);
        let other = CacheLockInfo {
            pid: std::process::id().saturating_add(1),
            command: "aida pr ship".to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            user: "tester".to_string(),
            session_id: None,
        };
        std::fs::write(&path, serde_json::to_string(&other).unwrap()).unwrap();

        let mut attempts = 0usize;
        let err = match with_cache_retry(&cache_path, "test cache write", || {
            attempts += 1;
            Err(anyhow::Error::new(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ErrorCode::DatabaseBusy,
                    extended_code: 5,
                },
                None,
            )))
        }) {
            Ok(()) => panic!("lock should fail without retries"),
            Err(err) => err,
        };
        assert_eq!(attempts, 1);
        let msg = err.to_string();
        assert!(msg.contains("database is locked"), "{msg}");
        assert!(msg.contains("pid="), "{msg}");
        assert!(msg.contains("aida doctor heal stale-locks"), "{msg}");
        remove_cache_lock_info(&cache_path);

        std::env::remove_var("AIDA_CACHE_RETRY_COUNT");
    }

    #[test]
    fn cache_retry_exhaustion_keeps_lock_holder_hint() {
        let _guard = env_lock().lock().unwrap_or_else(|err| err.into_inner());
        std::env::set_var("AIDA_CACHE_RETRY_COUNT", "2");
        std::env::set_var("AIDA_CACHE_RETRY_MS", "1");

        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("cache.db");
        let path = cache_lock_info_path(&cache_path);
        let other = CacheLockInfo {
            pid: std::process::id().saturating_add(1),
            command: "aida queue work".to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            user: "tester".to_string(),
            session_id: None,
        };
        std::fs::write(&path, serde_json::to_string(&other).unwrap()).unwrap();

        let mut attempts = 0usize;
        let err = match with_cache_retry(&cache_path, "apply cache schema", || {
            attempts += 1;
            Err(anyhow::Error::new(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ErrorCode::DatabaseLocked,
                    extended_code: 6,
                },
                None,
            )))
        }) {
            Ok(()) => panic!("lock should exhaust retries"),
            Err(err) => err,
        };

        assert_eq!(attempts, 3);
        let msg = err.to_string();
        assert!(msg.contains("database is locked"), "{msg}");
        assert!(msg.contains("pid="), "{msg}");
        assert!(msg.contains("aida queue work"), "{msg}");
        assert!(msg.contains("aida doctor heal stale-locks"), "{msg}");
        remove_cache_lock_info(&cache_path);

        std::env::remove_var("AIDA_CACHE_RETRY_COUNT");
        std::env::remove_var("AIDA_CACHE_RETRY_MS");
    }

    // The cache projects an EPIC's status as the read-only rollup of its
    // children, not the stored field. An epic stored as Draft whose child is In
    // Progress must read In Progress from the cache; a childless epic stored In
    // Progress must read Draft. trace:BUG-626 | ai:claude
    fn cached_status(cache: &Cache, id: Uuid) -> String {
        let rows = cache
            .list_summaries(&ListFilter {
                archive: ArchiveFilter::Both,
                defer: DeferFilter::Both,
                ..Default::default()
            })
            .unwrap();
        rows.into_iter()
            .find(|r| r.id == id)
            .map(|r| r.status)
            .unwrap()
    }

    #[test]
    fn cache_projects_epic_status_as_child_rollup() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        // Active epic: stored Draft, but a child is In Progress.
        let mut epic = sample_req("EPIC-1", "active epic");
        epic.req_type = RequirementType::Epic;
        epic.status = RequirementStatus::Draft;
        let mut child = sample_req("STORY-1", "child");
        child.status = RequirementStatus::InProgress;
        epic.relationships
            .push(rel(RelationshipType::Parent, child.id));
        child
            .relationships
            .push(rel(RelationshipType::Child, epic.id));

        // Childless epic: stored In Progress (the false-active drift) → Draft.
        let mut childless = sample_req("EPIC-2", "childless epic");
        childless.req_type = RequirementType::Epic;
        childless.status = RequirementStatus::InProgress;

        // Done epic: every child Completed → Completed.
        let mut done_epic = sample_req("EPIC-3", "done epic");
        done_epic.req_type = RequirementType::Epic;
        done_epic.status = RequirementStatus::Draft;
        let mut done_child = sample_req("STORY-2", "done child");
        done_child.status = RequirementStatus::Completed;
        done_epic
            .relationships
            .push(rel(RelationshipType::Parent, done_child.id));

        let (epic_id, childless_id, done_id) = (epic.id, childless.id, done_epic.id);

        let mut store = RequirementsStore::new();
        store
            .requirements
            .extend([epic, child, childless, done_epic, done_child]);

        cache.rebuild_from_store(&store, "head").unwrap();

        assert_eq!(cached_status(&cache, epic_id), "InProgress");
        assert_eq!(cached_status(&cache, childless_id), "Draft");
        assert_eq!(cached_status(&cache, done_id), "Completed");
    }

    // BUG-628: archived is a view flag, not a status — the cache-projected epic
    // rollup must count archived-Completed children. A fully-shipped epic whose
    // Completed children were archived previously projected Draft (the archived
    // children appeared excluded); it must project Completed. trace:BUG-628
    #[test]
    fn cache_rollup_counts_archived_completed_children() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        // Epic stored Draft; both children Completed-but-archived.
        let mut epic = sample_req("EPIC-1", "shipped epic, children archived");
        epic.req_type = RequirementType::Epic;
        epic.status = RequirementStatus::Draft;

        let mut c1 = sample_req("STORY-1", "archived completed child 1");
        c1.status = RequirementStatus::Completed;
        c1.archived = true;
        let mut c2 = sample_req("STORY-2", "archived completed child 2");
        c2.status = RequirementStatus::Completed;
        c2.archived = true;

        epic.relationships
            .push(rel(RelationshipType::Parent, c1.id));
        epic.relationships
            .push(rel(RelationshipType::Parent, c2.id));
        c1.relationships.push(rel(RelationshipType::Child, epic.id));
        c2.relationships.push(rel(RelationshipType::Child, epic.id));

        let epic_id = epic.id;
        let mut store = RequirementsStore::new();
        store.requirements.extend([epic, c1, c2]);

        cache.rebuild_from_store(&store, "head").unwrap();

        // Before BUG-628 this read "Draft" — the archived children were treated
        // as absent from the rollup.
        assert_eq!(
            cached_status(&cache, epic_id),
            "Completed",
            "an epic whose only children are Completed-but-archived projects Completed"
        );
    }

    #[test]
    fn upsert_refreshes_epic_own_row_from_cached_children() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        // Epic stored Draft with one remaining child → derived Draft.
        let mut epic = sample_req("EPIC-9", "epic");
        epic.req_type = RequirementType::Epic;
        epic.status = RequirementStatus::Draft;
        let mut child = sample_req("STORY-9", "child");
        child.status = RequirementStatus::Approved;
        epic.relationships
            .push(rel(RelationshipType::Parent, child.id));

        let (epic_id, child_id) = (epic.id, child.id);
        let mut store = RequirementsStore::new();
        store.requirements.extend([epic.clone(), child.clone()]);
        cache.rebuild_from_store(&store, "head").unwrap();
        assert_eq!(cached_status(&cache, epic_id), "Draft");

        // The child starts; the child's own cache row reflects it, then an
        // epic-self upsert re-derives the epic's row from the now-In-Progress
        // child read from the cache.
        let mut child = child;
        child.status = RequirementStatus::InProgress;
        cache.upsert_requirement(&child).unwrap();
        assert_eq!(cached_status(&cache, child_id), "InProgress");
        cache.upsert_requirement(&epic).unwrap();
        assert_eq!(cached_status(&cache, epic_id), "InProgress");
    }

    // TASK-955: a 3-level hierarchy (epic -> story -> task) where --parent shows
    // only the direct child but --recursive (descendant_ids) shows the whole
    // subtree. Mirrors the edge convention this store actually uses: a `Parent`
    // rel_type on a node points at that node's CHILD. trace:TASK-955 | ai:claude
    #[test]
    fn descendant_ids_walks_transitive_subtree() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut epic = sample_req("EPIC-1", "epic");
        epic.req_type = RequirementType::Epic;
        let mut story = sample_req("STORY-1", "story");
        story.req_type = RequirementType::Story;
        let mut task = sample_req("TASK-1", "task");
        task.req_type = RequirementType::Task;
        // A sibling of the epic with no link — must NOT appear in the subtree.
        let mut unrelated = sample_req("TASK-2", "unrelated");
        unrelated.req_type = RequirementType::Task;

        let (epic_id, story_id, task_id, unrelated_id) = (epic.id, story.id, task.id, unrelated.id);

        // Parent edges point DOWN at the child (this store's convention).
        epic.relationships
            .push(rel(RelationshipType::Parent, story_id));
        story
            .relationships
            .push(rel(RelationshipType::Parent, task_id));

        let mut store = RequirementsStore::new();
        store.requirements.extend([epic, story, task, unrelated]);
        cache.rebuild_from_store(&store, "head").unwrap();

        // Direct children of the epic: just the story (the STORY-62 filter).
        // The transitive subtree: epic + story + task (the root is included).
        let subtree = cache.descendant_ids(&epic_id).unwrap();
        assert!(subtree.contains(&epic_id), "subtree includes the root epic");
        assert!(subtree.contains(&story_id), "subtree includes the story");
        assert!(
            subtree.contains(&task_id),
            "subtree includes the transitively-nested task"
        );
        assert!(
            !subtree.contains(&unrelated_id),
            "an unlinked sibling is NOT in the subtree"
        );
        assert_eq!(subtree.len(), 3, "exactly epic + story + task");

        // Walking from the story finds story + task but not the epic above it.
        let story_subtree = cache.descendant_ids(&story_id).unwrap();
        assert!(story_subtree.contains(&story_id));
        assert!(story_subtree.contains(&task_id));
        assert!(
            !story_subtree.contains(&epic_id),
            "descendant walk does not climb to the parent"
        );
        assert_eq!(story_subtree.len(), 2);

        // A leaf's subtree is just itself.
        let leaf_subtree = cache.descendant_ids(&task_id).unwrap();
        assert_eq!(leaf_subtree.len(), 1);
        assert!(leaf_subtree.contains(&task_id));
    }

    // TASK-955: the child may record the hierarchy edge instead of the parent
    // (a `Child` rel_type on a node points UP at its parent). descendant_ids
    // must reach the child whichever endpoint authored the edge — the same
    // either-endpoint union `aida graph --tree` walks (BUG-448). trace:TASK-955
    #[test]
    fn descendant_ids_handles_child_authored_edge() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut epic = sample_req("EPIC-1", "epic");
        epic.req_type = RequirementType::Epic;
        let mut story = sample_req("STORY-1", "story");
        story.req_type = RequirementType::Story;
        let (epic_id, story_id) = (epic.id, story.id);

        // The CHILD records the edge pointing UP at its parent (Child -> parent).
        story
            .relationships
            .push(rel(RelationshipType::Child, epic_id));

        let mut store = RequirementsStore::new();
        store.requirements.extend([epic, story]);
        cache.rebuild_from_store(&store, "head").unwrap();

        let subtree = cache.descendant_ids(&epic_id).unwrap();
        assert!(
            subtree.contains(&story_id),
            "child-authored edge still places the story under the epic"
        );
        assert_eq!(subtree.len(), 2);
    }

    // TASK-1074: the EPIC-54 discrepancy — `aida focus`'s `descendant_ids` closure
    // and `aida graph --tree`'s `graph_walk::subtree_ids` must reach the SAME
    // subtree. The pathology: a story that is a child of the epic AND has a
    // SAME-RANK second parent (another story) OUTSIDE the epic. The old agnostic
    // tree walk leaked the second parent in (44 vs 43); the shared rank-oriented
    // rule excludes it. This test proves the cache CTE and the in-memory closure
    // now AGREE on that exact shape. trace:TASK-1074 | ai:claude
    #[test]
    fn descendant_ids_agrees_with_subtree_ids_on_same_rank_second_parent() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path().join("cache.db")).unwrap();

        let mut epic = sample_req("EPIC-54", "epic");
        epic.req_type = RequirementType::Epic;
        let mut child = sample_req("STORY-699", "real child");
        child.req_type = RequirementType::Story;
        let mut second_parent = sample_req("STORY-698", "same-rank second parent");
        second_parent.req_type = RequirementType::Story;

        // epic --Parent--> child ; child --Child--> epic (convention B, both ends).
        epic.relationships
            .push(rel(RelationshipType::Parent, child.id));
        child
            .relationships
            .push(rel(RelationshipType::Child, epic.id));
        // second_parent --Parent--> child ; child --Child--> second_parent
        // (the 698<->699 shape: same STORY rank, so type gives no signal and the
        // rel_type places 698 ABOVE 699 — a parent, not a descendant of the epic).
        second_parent
            .relationships
            .push(rel(RelationshipType::Parent, child.id));
        child
            .relationships
            .push(rel(RelationshipType::Child, second_parent.id));

        let (epic_id, child_id, second_parent_id) = (epic.id, child.id, second_parent.id);
        let mut store = RequirementsStore::new();
        store.requirements.extend([epic, child, second_parent]);
        cache.rebuild_from_store(&store, "head").unwrap();

        // Cache CTE closure (includes the root); in-memory closure (excludes root).
        let desc = cache.descendant_ids(&epic_id).unwrap();
        let sub: std::collections::HashSet<Uuid> =
            crate::graph_walk::subtree_ids(&store, epic_id, None)
                .nodes
                .into_iter()
                .collect();

        assert!(desc.contains(&child_id), "real child in the cache closure");
        assert!(sub.contains(&child_id), "real child in the graph closure");
        assert!(
            !desc.contains(&second_parent_id),
            "same-rank second parent is NOT a descendant (cache)"
        );
        assert!(
            !sub.contains(&second_parent_id),
            "same-rank second parent is NOT a descendant (graph)"
        );
        // Exact agreement: descendant_ids minus the root == subtree_ids nodes.
        let desc_no_root: std::collections::HashSet<Uuid> =
            desc.into_iter().filter(|id| *id != epic_id).collect();
        assert_eq!(
            desc_no_root, sub,
            "aida focus and aida graph --tree agree on subtree membership"
        );
    }

    // BUG-683: a corrupt / non-sqlite `.aida/cache.db` must NOT dead-end every
    // read. Opening it self-heals — the corrupt file is deleted, a fresh empty
    // db is recreated, and the read succeeds (rebuild-from-git happens on the
    // next stale-check because the head SHA is absent).
    #[test]
    fn open_self_heals_a_corrupt_cache_file() {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("cache.db");
        // Write non-sqlite bytes — sqlite reports SQLITE_NOTADB (the corruption
        // class) when it tries to read the header via the WAL pragma.
        std::fs::write(&cache_path, b"this is not a sqlite database at all\n").unwrap();

        // Open self-heals instead of erroring: the file is replaced and the
        // cache is usable (a query against the fresh schema succeeds).
        let cache = Cache::open(&cache_path)
            .expect("a corrupt cache file must self-heal on open, not dead-end");
        let mut store = RequirementsStore::new();
        store.requirements.push(sample_req("BUG-683-A", "healed"));
        cache
            .rebuild_from_store(&store, "head")
            .expect("the healed cache is a normal, writable db");

        // The on-disk file is now a valid sqlite db (starts with the header).
        let header = std::fs::read(&cache_path).unwrap();
        assert!(
            header.starts_with(b"SQLite format 3\0"),
            "the healed file is a real sqlite database"
        );
    }

    // BUG-683: `aida cache rebuild` must recover from a corrupt starting file
    // too — it opens the cache (via CachedGitBackend) before rebuilding, so the
    // corrupt file has to self-heal at open. We exercise the same open path a
    // rebuild would take.
    #[test]
    fn cache_open_then_rebuild_recovers_from_corrupt_file() {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("cache.db");
        std::fs::write(&cache_path, &[0u8; 4096]).unwrap(); // zeroed, non-sqlite

        let cache = Cache::open(&cache_path).expect("open self-heals the corrupt file");
        let mut store = RequirementsStore::new();
        store
            .requirements
            .push(sample_req("BUG-683-B", "rebuilt after corruption"));
        let n = cache
            .rebuild_from_store(&store, "head")
            .expect("rebuild succeeds on the healed cache");
        assert_eq!(n, 1, "the rebuilt cache holds the store's one requirement");
    }

    // BUG-683: the self-heal is scoped to the CORRUPTION class ONLY. A transient
    // busy/lock error must NOT be misclassified as corruption (never delete the
    // file for a lock). `is_sqlite_corruption_error` is the gate — assert it
    // rejects the lock codes and accepts the corruption codes.
    #[test]
    fn corruption_detection_excludes_transient_lock_errors() {
        let lock = anyhow::Error::from(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            Some("database is locked".to_string()),
        ));
        assert!(
            !is_sqlite_corruption_error(&lock),
            "a busy/lock error is transient, NOT corruption — must not trigger a delete"
        );
        assert!(
            is_sqlite_lock_error(&lock),
            "the lock error stays classified as a lock (BUG-681 fast-fail path)"
        );

        let not_a_db = anyhow::Error::from(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_NOTADB),
            Some("file is not a database".to_string()),
        ));
        assert!(
            is_sqlite_corruption_error(&not_a_db),
            "SQLITE_NOTADB is the corruption class — self-heal applies"
        );
        assert!(
            !is_sqlite_lock_error(&not_a_db),
            "corruption is not a lock error"
        );

        let corrupt = anyhow::Error::from(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
            Some("malformed image".to_string()),
        ));
        assert!(
            is_sqlite_corruption_error(&corrupt),
            "SQLITE_CORRUPT is the corruption class — self-heal applies"
        );
    }

    // BUG-683: a lock-held cache must keep failing as a LOCK (not be deleted as
    // corruption). Hold an exclusive write lock, disable the retry ladder, and
    // assert the file survives and the error is the lock message — never the
    // corruption self-heal.
    #[test]
    fn locked_cache_is_not_treated_as_corruption() {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("cache.db");
        // Establish a healthy cache first.
        drop(Cache::open(&cache_path).unwrap());

        // Hold an exclusive write transaction on a separate connection so the
        // cache is under lock contention while we re-open it.
        let holder = Connection::open(&cache_path).unwrap();
        holder.busy_timeout(Duration::from_millis(0)).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        // Re-open while the lock is held. Whatever the outcome (WAL lets the
        // read through, or a lock surfaces), the corruption guard must NOT fire:
        // the file must survive untouched. (No env-var fiddling — that would
        // race sibling tests; the classification is asserted directly by
        // `corruption_detection_excludes_transient_lock_errors`.)
        let _ = open_connection_with_retry(&cache_path);
        holder.execute_batch("ROLLBACK").unwrap();

        assert!(
            cache_path.exists(),
            "a lock-contended open must NEVER delete the cache file (BUG-683 corruption-only scope)"
        );
        // And the file is still a valid sqlite db (never replaced).
        let header = std::fs::read(&cache_path).unwrap();
        assert!(
            header.starts_with(b"SQLite format 3\0"),
            "the healthy cache file was not clobbered by a lock event"
        );
    }
}
