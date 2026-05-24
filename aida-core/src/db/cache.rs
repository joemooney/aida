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

pub struct Cache {
    conn: Mutex<Connection>,
    path: PathBuf,
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
        let conn = Connection::open(&path)
            .with_context(|| format!("Failed to open cache at {:?}", path))?;
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
                conn.execute_batch(
                    "DROP TABLE IF EXISTS requirements_cache;
                     DROP TABLE IF EXISTS requirements_fts;",
                )
                .context("Failed to drop cache tables for schema migration")?;
            }
        }
        conn.execute_batch(SCHEMA_SQL)
            .context("Failed to apply cache schema")?;
        let cache = Cache {
            conn: Mutex::new(conn),
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

    // ------------------------------------------------------------------ meta

    fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO cache_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
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
        conn.execute_batch("DELETE FROM requirements_cache; DELETE FROM requirements_fts;")?;
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
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch("DELETE FROM requirements_cache; DELETE FROM requirements_fts;")?;
            let mut count = 0usize;
            for req in &store.requirements {
                insert_one(&tx, req)?;
                count += 1;
            }
            tx.commit()?;
            count
        };
        self.set_source_head_sha(source_head_sha)?;
        self.set_meta(META_KEY_BUILT_AT, &chrono::Utc::now().to_rfc3339())?;
        Ok(count)
    }

    /// Single-row upsert called after a write-through git mutation succeeds.
    pub fn upsert_requirement(&self, req: &Requirement) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        delete_one_uncommitted(&conn, &req.id)?;
        insert_one(&conn, req)?;
        Ok(())
    }

    /// Single-row delete called after a write-through git delete succeeds.
    pub fn delete_requirement(&self, id: &Uuid) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        delete_one_uncommitted(&conn, id)
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
        for tag in &filter.tags {
            sql.push_str(" AND tags_json LIKE ?");
            args.push(format!(
                "%\"{}\"%",
                tag.replace('\\', "\\\\").replace('"', "\\\"")
            ));
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
    use tempfile::tempdir;

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
}
