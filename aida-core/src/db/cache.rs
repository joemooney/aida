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

const SCHEMA_SQL: &str = include_str!("cache_schema.sql");
const SCHEMA_VERSION: &str = "1";

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
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create cache parent dir: {:?}", parent))?;
        }
        let conn = Connection::open(&path)
            .with_context(|| format!("Failed to open cache at {:?}", path))?;
        conn.execute_batch(SCHEMA_SQL)
            .context("Failed to apply cache schema")?;
        let cache = Cache {
            conn: Mutex::new(conn),
            path,
        };
        cache.set_meta(META_KEY_SCHEMA_VERSION, SCHEMA_VERSION)?;
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
            archived, yaml_path
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
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

fn delete_one_uncommitted(conn: &Connection, id: &Uuid) -> Result<()> {
    let id_str = id.to_string();
    conn.execute(
        "DELETE FROM requirements_cache WHERE id = ?1",
        params![id_str],
    )?;
    conn.execute("DELETE FROM requirements_fts WHERE id = ?1", params![id_str])?;
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
        cache
            .delete_requirement(&store.requirements[1].id)
            .unwrap();
        assert_eq!(cache.requirement_count().unwrap(), 1);
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
