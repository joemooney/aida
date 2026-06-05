//! SQLite cache projection for the git-canonical store.
//!
//! Per EPIC-1-001 / docs/plans/2026-05-02-git-canonical-storage.md:
//! the git orphan store is the source of truth. This module maintains a
//! rebuildable SQLite read cache used for fast list/filter/search queries.
//! It is never authoritative — drop and rebuild any time.
//!
//! trace:EPIC-1-001 | ai:claude

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use uuid::Uuid;

use crate::models::{Requirement, RequirementsStore};

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
    pub feature: String,
    pub req_type: String,
    pub tags: Vec<String>,
    pub created_at: String,
    pub modified_at: String,
    pub archived: bool,
    /// ISO RFC3339 timestamp; None when not archived. trace:STORY-441 | ai:claude
    pub archived_at: Option<String>,
    pub yaml_path: String,
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
    /// Optional cap on returned rows (after ordering by modified_at DESC).
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
const SCHEMA_VERSION: &str = "2";

const META_KEY_SCHEMA_VERSION: &str = "schema_version";
const META_KEY_SOURCE_HEAD_SHA: &str = "source_head_sha";
const META_KEY_BUILT_AT: &str = "built_at";
const DEFAULT_CACHE_RETRY_DELAYS_MS: &[u64] = &[100, 200, 400, 800, 1600, 3200, 6400, 12800];

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

fn open_connection_with_retry(path: &Path) -> Result<Connection> {
    with_cache_retry(path, "open cache", || {
        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open cache at {:?}", path))?;
        // AIDA owns retry/backoff timing; rusqlite's default busy handler can
        // otherwise block inside a single attempt and hide the lock holder.
        conn.busy_timeout(Duration::from_millis(0))?;
        Ok(conn)
    })
}

fn with_cache_write<T, F>(cache_path: &Path, action: &str, mut f: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    write_cache_lock_info(cache_path)?;
    let result = with_cache_retry(cache_path, action, || f());
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
        if let Some(v) = &on_disk_version {
            if v != SCHEMA_VERSION {
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
        }
        with_cache_write(&path, "apply cache schema", || {
            conn.execute_batch(SCHEMA_SQL)
                .context("Failed to apply cache schema")
        })?;
        let cache = Cache {
            conn: Mutex::new(conn),
            lock_info_path: cache_lock_info_path(&path),
            path,
        };
        cache.set_meta(META_KEY_SCHEMA_VERSION, SCHEMA_VERSION)?;
        // After a schema-version bump, the head SHA is no longer valid for
        // the (now-empty) cache tables — delete it so `is_stale` returns
        // true (None → stale) and the next read triggers a rebuild.
        if let Some(v) = on_disk_version {
            if v != SCHEMA_VERSION {
                let conn = cache.conn.lock().unwrap();
                conn.execute(
                    "DELETE FROM cache_meta WHERE key = ?1",
                    params![META_KEY_SOURCE_HEAD_SHA],
                )?;
            }
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
        let count = {
            let conn = self.conn.lock().unwrap();
            with_cache_write(&self.path, "rebuild cache", || {
                let tx = conn.unchecked_transaction()?;
                tx.execute_batch("DELETE FROM requirements_cache; DELETE FROM requirements_fts;")?;
                let mut count = 0usize;
                for req in &store.requirements {
                    insert_one(&tx, req)?;
                    count += 1;
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
    pub fn upsert_requirement(&self, req: &Requirement) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        with_cache_write(&self.path, "upsert cached requirement", || {
            delete_one_uncommitted(&conn, &req.id)?;
            insert_one(&conn, req)?;
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
                    archived, archived_at, yaml_path
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
        if let Some(s) = &filter.status {
            // The cache stores RequirementStatus's Debug form (e.g.
            // "InProgress" — no space/hyphen). Users type "in-progress",
            // "In Progress", "InProgress", etc. Strip space/hyphen/underscore
            // from both sides and lowercase so every variant matches.
            // trace:BUG-1-025 | ai:claude
            sql.push_str(
                " AND LOWER(REPLACE(REPLACE(REPLACE(status, ' ', ''), '-', ''), '_', '')) = ?",
            );
            args.push(normalize_status_filter(s));
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
        if let Some(o) = &filter.owner {
            sql.push_str(" AND owner = ?");
            args.push(o.clone());
        }
        if let Some(f) = &filter.feature {
            sql.push_str(" AND feature = ?");
            args.push(f.clone());
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

        sql.push_str(" ORDER BY modified_at DESC");
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
        let conn = self.conn.lock().unwrap();
        // FTS5 requires the MATCH clause to use the bare table name, not an
        // alias — hence no `f` alias on requirements_fts.
        let sql = format!(
            "SELECT c.id, c.spec_id, c.agreed_id, c.title, c.description,
                          c.status, c.priority, c.owner, c.feature, c.req_type,
                          c.tags_json, c.created_at, c.modified_at, c.archived,
                          c.archived_at, c.yaml_path
                   FROM requirements_fts
                   JOIN requirements_cache c ON c.id = requirements_fts.id
                   WHERE requirements_fts MATCH ?{archive_clause}
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
}

// ---------------------------------------------------------------- row helpers

fn insert_one(conn: &Connection, req: &Requirement) -> Result<()> {
    let yaml_path = yaml_path_for(req);
    let tags_json = serde_json::to_string(&req.tags).unwrap_or_else(|_| "[]".into());
    let archived = if req.archived { 1 } else { 0 };
    // trace:STORY-441 | ai:claude
    let archived_at = req.archived_at.map(|dt| dt.to_rfc3339());
    let req_type_str = format!("{:?}", req.req_type);
    let status_str = req
        .custom_status
        .clone()
        .unwrap_or_else(|| format!("{:?}", req.status));
    let priority_str = req
        .custom_priority
        .clone()
        .unwrap_or_else(|| format!("{:?}", req.priority));

    conn.execute(
        "INSERT INTO requirements_cache (
            id, spec_id, agreed_id, title, description, status, priority,
            owner, feature, req_type, tags_json, created_at, modified_at,
            archived, archived_at, yaml_path
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
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
            yaml_path,
        ],
    )?;

    conn.execute(
        "INSERT INTO requirements_fts (id, spec_id, agreed_id, title, description)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            req.id.to_string(),
            req.spec_id.clone().unwrap_or_default(),
            req.agreed_id.clone().unwrap_or_default(),
            req.title,
            req.description,
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
        yaml_path: row.get(15)?,
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
            .search("canonical", 10, ArchiveFilter::NonArchivedOnly)
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
                .search(q, 10, ArchiveFilter::NonArchivedOnly)
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
            .search("alpha beta", 10, ArchiveFilter::NonArchivedOnly)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "alpha beta gamma");

        // Empty query: well-formed (returns nothing rather than panicking).
        assert!(cache.search("", 10, ArchiveFilter::NonArchivedOnly).is_ok());
        assert!(cache
            .search("   ", 10, ArchiveFilter::NonArchivedOnly)
            .is_ok());
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
}
