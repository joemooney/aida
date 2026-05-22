// trace:ARCH-distributed-dispenser | ai:claude
//! Sequence Dispenser — generates monotonically increasing sequence numbers
//! for object ID creation.
//!
//! The dispenser is a purely local concern. Once a Node ID is assigned,
//! sequence numbers require no network coordination. The dispenser interface
//! is stable; the backing implementation can be swapped (file → SQLite → daemon)
//! without touching callers.
//!
//! In centralized mode (node_id=0), the dispenser generates simple sequential
//! IDs like `FR-001`. In distributed mode, it generates node-namespaced IDs
//! like `FR-7-048`.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The operating mode for ID generation.
///
/// Node IDs were `u32` pre-EPIC-9; they're now `String` so personal
/// identities like `"JM"` are first-class. Numeric ids deserialize
/// back-compat as decimal strings (`"1"`, `"7"`).
/// trace:STORY-41 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdMode {
    /// Centralized mode: IDs are `{TYPE}-{SEQ}` (e.g., `FR-001`).
    /// Used when a central database is always available.
    Centralized,
    /// Distributed mode: IDs are `{TYPE}-{NODEID}-{SEQ}` (e.g., `FR-JM-048`
    /// or `FR-7-048`). Used for offline-capable, multi-node deployments.
    Distributed {
        #[serde(deserialize_with = "deserialize_node_id_for_idmode")]
        node_id: String,
    },
}

/// Local re-of the back-compat deserializer (so `IdMode::Distributed`
/// reads pre-EPIC-9 dispenser state where `node_id` was a u64).
/// trace:STORY-41 | ai:claude
fn deserialize_node_id_for_idmode<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Str(String),
        Num(u64),
    }
    Ok(match Repr::deserialize(deserializer)? {
        Repr::Str(s) => s,
        Repr::Num(n) => n.to_string(),
    })
}

/// Current state of the dispenser — all counters for a given node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispenserState {
    /// The operating mode
    pub mode: IdMode,
    /// Current sequence values per type prefix.
    /// The value is the last dispensed number; next will be value + 1.
    pub sequences: HashMap<String, u32>,
}

impl DispenserState {
    /// Create a new empty state for the given mode.
    pub fn new(mode: IdMode) -> Self {
        Self {
            mode,
            sequences: HashMap::new(),
        }
    }
}

/// The Dispenser trait — the stable interface for sequence number generation.
///
/// Implementations may be backed by:
/// - A TOML/YAML file with a lockfile (Phase 1)
/// - A SQLite table (Phase 2)
/// - A Unix socket daemon (Phase 3)
///
/// All implementations must guarantee:
/// 1. Monotonicity — `next()` never returns a value <= any previous return
/// 2. Persistence — values survive process restarts
/// 3. Thread safety — concurrent callers get distinct values
pub trait Dispenser: Send + Sync {
    /// Get the next sequence number for the given object type.
    /// Increments the counter atomically.
    fn next(&self, object_type: &str) -> Result<u32>;

    /// Peek at the next sequence number without incrementing.
    fn peek(&self, object_type: &str) -> Result<u32>;

    /// Get the full current state (all counters).
    fn state(&self) -> Result<DispenserState>;

    /// Format an object ID according to the current mode.
    ///
    /// - Centralized: `FR-001`
    /// - Distributed: `FR-7-001`
    fn format_id(&self, object_type: &str, seq: u32) -> Result<String> {
        let state = self.state()?;
        let digits = 3; // minimum padding
        match &state.mode {
            IdMode::Centralized => Ok(format!("{}-{:0>width$}", object_type, seq, width = digits)),
            IdMode::Distributed { node_id } => Ok(format!(
                "{}-{}-{:0>width$}",
                object_type,
                node_id,
                seq,
                width = digits
            )),
        }
    }

    /// Dispense a new sequence number and return the formatted ID.
    fn next_id(&self, object_type: &str) -> Result<String> {
        let seq = self.next(object_type)?;
        self.format_id(object_type, seq)
    }
}

/// A simple in-memory dispenser for testing and single-session use.
/// Not persistent — loses state on drop.
pub struct MemoryDispenser {
    state: std::sync::Mutex<DispenserState>,
}

impl MemoryDispenser {
    /// Create a new in-memory dispenser.
    pub fn new(mode: IdMode) -> Self {
        Self {
            state: std::sync::Mutex::new(DispenserState::new(mode)),
        }
    }

    /// Create with pre-loaded state (e.g., from a file).
    pub fn with_state(state: DispenserState) -> Self {
        Self {
            state: std::sync::Mutex::new(state),
        }
    }
}

impl Dispenser for MemoryDispenser {
    fn next(&self, object_type: &str) -> Result<u32> {
        let mut state = self.state.lock().unwrap();
        let counter = state
            .sequences
            .entry(object_type.to_uppercase())
            .or_insert(0);
        *counter += 1;
        Ok(*counter)
    }

    fn peek(&self, object_type: &str) -> Result<u32> {
        let state = self.state.lock().unwrap();
        let current = state
            .sequences
            .get(&object_type.to_uppercase())
            .copied()
            .unwrap_or(0);
        Ok(current + 1)
    }

    fn state(&self) -> Result<DispenserState> {
        let state = self.state.lock().unwrap();
        Ok(state.clone())
    }
}

/// File-backed dispenser using a TOML state file with advisory locking.
/// This is the Phase 1 implementation per the distributed architecture spec.
#[cfg(feature = "native")]
pub struct FileDispenser {
    path: std::path::PathBuf,
    mode: IdMode,
}

#[cfg(feature = "native")]
impl FileDispenser {
    /// Create or open a file-backed dispenser.
    /// The file will be created if it doesn't exist.
    pub fn open(path: std::path::PathBuf, mode: IdMode) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Create file if it doesn't exist
        if !path.exists() {
            let state = DispenserState::new(mode.clone());
            let content = toml::to_string_pretty(&state)?;
            // Atomic write: concurrent `aida add` from parallel sessions can
            // race the counter file — a torn write hands out duplicate
            // SPEC-IDs or blocks allocation entirely. trace:TASK-331 | ai:claude
            crate::write_atomic(&path, content)?;
        }
        Ok(Self { path, mode })
    }

    fn load_state(&self) -> Result<DispenserState> {
        // Reader can race a concurrent `aida add` writer mid-`write_atomic`;
        // on Windows that surfaces as a transient PermissionDenied/NotFound
        // from `CreateFile`. Retry through `read_atomic` so the dispenser
        // never spuriously fails to allocate. trace:TASK-346 | ai:claude
        let content = crate::read_atomic(&self.path)?;
        let state: DispenserState = toml::from_str(&content)?;
        Ok(state)
    }

    fn save_state(&self, state: &DispenserState) -> Result<()> {
        let content = toml::to_string_pretty(state)?;
        // Atomic write: the advisory lock in `next()` serializes writers, but
        // atomic rename additionally guarantees a reader (or a crash) never
        // sees a half-written counter file. trace:TASK-331 | ai:claude
        crate::write_atomic(&self.path, content)?;
        Ok(())
    }
}

#[cfg(feature = "native")]
impl Dispenser for FileDispenser {
    fn next(&self, object_type: &str) -> Result<u32> {
        use fs2::FileExt;
        use std::fs::OpenOptions;

        // Acquire advisory lock on the state file
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.path.with_extension("lock"))?;
        lock_file.lock_exclusive()?;

        let result = (|| {
            let mut state = self.load_state()?;
            let counter = state
                .sequences
                .entry(object_type.to_uppercase())
                .or_insert(0);
            *counter += 1;
            let value = *counter;
            self.save_state(&state)?;
            Ok(value)
        })();

        lock_file.unlock()?;
        result
    }

    fn peek(&self, object_type: &str) -> Result<u32> {
        let state = self.load_state()?;
        let current = state
            .sequences
            .get(&object_type.to_uppercase())
            .copied()
            .unwrap_or(0);
        Ok(current + 1)
    }

    fn state(&self) -> Result<DispenserState> {
        let mut state = self.load_state()?;
        state.mode = self.mode.clone();
        Ok(state)
    }
}

/// SQLite-backed dispenser using atomic UPSERT for sequence generation.
/// This is the Phase 2 implementation per the distributed architecture spec.
///
/// Uses a single `sequences` table with one row per (node_id, type).
/// SQLite's write serialization handles all concurrency — no external
/// lockfile needed. Natural fit when the local read model is also SQLite.
///
/// Schema:
/// ```sql
/// CREATE TABLE IF NOT EXISTS dispenser_sequences (
///     node_id INTEGER NOT NULL,
///     type_prefix TEXT NOT NULL,
///     next_val INTEGER NOT NULL DEFAULT 1,
///     PRIMARY KEY (node_id, type_prefix)
/// );
/// CREATE TABLE IF NOT EXISTS dispenser_meta (
///     key TEXT PRIMARY KEY,
///     value TEXT NOT NULL
/// );
/// ```
#[cfg(feature = "native")]
pub struct SqliteDispenser {
    conn: std::sync::Mutex<rusqlite::Connection>,
    mode: IdMode,
}

#[cfg(feature = "native")]
impl SqliteDispenser {
    /// Open or create a SQLite-backed dispenser.
    ///
    /// If `db_path` points to an existing database (e.g., the local read model),
    /// the dispenser tables are created alongside existing tables. If it doesn't
    /// exist, a new database is created.
    pub fn open(db_path: std::path::PathBuf, mode: IdMode) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = rusqlite::Connection::open(&db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;

        // Create tables if they don't exist
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS dispenser_sequences (
                node_id INTEGER NOT NULL,
                type_prefix TEXT NOT NULL,
                next_val INTEGER NOT NULL DEFAULT 1,
                PRIMARY KEY (node_id, type_prefix)
            );
            CREATE TABLE IF NOT EXISTS dispenser_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )?;

        // Store the mode
        let mode_json = serde_json::to_string(&mode)?;
        conn.execute(
            "INSERT OR REPLACE INTO dispenser_meta (key, value) VALUES ('mode', ?1)",
            rusqlite::params![mode_json],
        )?;

        Ok(Self {
            conn: std::sync::Mutex::new(conn),
            mode,
        })
    }

    /// Get the node_id for this dispenser ("0" for centralized).
    /// Pre-EPIC-9 stored this as i64; the column was migrated to TEXT
    /// when node ids became strings. Numeric ids continue to work via
    /// decimal-string repr (legacy dispenser.db rows pre-migration are
    /// out of scope — this backend is archived).
    /// trace:STORY-41 | ai:claude
    fn node_id(&self) -> String {
        match &self.mode {
            IdMode::Centralized => "0".to_string(),
            IdMode::Distributed { node_id } => node_id.clone(),
        }
    }
}

#[cfg(feature = "native")]
impl Dispenser for SqliteDispenser {
    fn next(&self, object_type: &str) -> Result<u32> {
        let conn = self.conn.lock().unwrap();
        let node_id = self.node_id();
        let type_upper = object_type.to_uppercase();

        // Atomic increment via UPSERT + RETURNING
        // SQLite 3.35+ supports RETURNING; fall back to two-step for older versions
        let result: Result<u32> = conn
            .query_row(
                "INSERT INTO dispenser_sequences (node_id, type_prefix, next_val)
                 VALUES (?1, ?2, 1)
                 ON CONFLICT (node_id, type_prefix)
                 DO UPDATE SET next_val = next_val + 1
                 RETURNING next_val",
                rusqlite::params![node_id.clone(), type_upper],
                |row| row.get(0),
            )
            .map_err(|e| anyhow::anyhow!("SQLite dispenser next() failed: {}", e));

        result
    }

    fn peek(&self, object_type: &str) -> Result<u32> {
        use rusqlite::OptionalExtension;

        let conn = self.conn.lock().unwrap();
        let node_id = self.node_id();
        let type_upper = object_type.to_uppercase();

        let current: Option<u32> = conn
            .query_row(
                "SELECT next_val FROM dispenser_sequences
                 WHERE node_id = ?1 AND type_prefix = ?2",
                rusqlite::params![node_id.clone(), type_upper],
                |row| row.get(0),
            )
            .optional()?;

        // If no row exists, next value will be 1
        Ok(current.map(|v| v + 1).unwrap_or(1))
    }

    fn state(&self) -> Result<DispenserState> {
        let conn = self.conn.lock().unwrap();
        let node_id = self.node_id();

        let mut stmt = conn
            .prepare("SELECT type_prefix, next_val FROM dispenser_sequences WHERE node_id = ?1")?;

        let mut sequences = HashMap::new();
        let rows = stmt.query_map(rusqlite::params![node_id.clone()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })?;

        for row in rows {
            let (prefix, val) = row?;
            sequences.insert(prefix, val);
        }

        Ok(DispenserState {
            mode: self.mode.clone(),
            sequences,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_dispenser_centralized() {
        let d = MemoryDispenser::new(IdMode::Centralized);

        assert_eq!(d.next("FR").unwrap(), 1);
        assert_eq!(d.next("FR").unwrap(), 2);
        assert_eq!(d.next("FR").unwrap(), 3);
        assert_eq!(d.next("FEAT").unwrap(), 1);
        assert_eq!(d.next("FR").unwrap(), 4);

        assert_eq!(d.next_id("FR").unwrap(), "FR-005");
        assert_eq!(d.next_id("FEAT").unwrap(), "FEAT-002");
    }

    #[test]
    fn test_memory_dispenser_distributed() {
        let d = MemoryDispenser::new(IdMode::Distributed {
            node_id: "7".to_string(),
        });

        assert_eq!(d.next_id("FR").unwrap(), "FR-7-001");
        assert_eq!(d.next_id("FR").unwrap(), "FR-7-002");
        assert_eq!(d.next_id("FEAT").unwrap(), "FEAT-7-001");
    }

    #[test]
    fn test_peek_does_not_increment() {
        let d = MemoryDispenser::new(IdMode::Centralized);

        assert_eq!(d.peek("FR").unwrap(), 1);
        assert_eq!(d.peek("FR").unwrap(), 1);
        assert_eq!(d.next("FR").unwrap(), 1);
        assert_eq!(d.peek("FR").unwrap(), 2);
    }

    #[test]
    fn test_state_snapshot() {
        let d = MemoryDispenser::new(IdMode::Distributed {
            node_id: "42".to_string(),
        });
        d.next("FR").unwrap();
        d.next("FR").unwrap();
        d.next("FEAT").unwrap();

        let state = d.state().unwrap();
        assert_eq!(
            state.mode,
            IdMode::Distributed {
                node_id: "42".to_string()
            }
        );
        assert_eq!(state.sequences.get("FR"), Some(&2));
        assert_eq!(state.sequences.get("FEAT"), Some(&1));
    }

    #[test]
    fn test_case_insensitive_type() {
        let d = MemoryDispenser::new(IdMode::Centralized);
        assert_eq!(d.next("fr").unwrap(), 1);
        assert_eq!(d.next("FR").unwrap(), 2);
        assert_eq!(d.next("Fr").unwrap(), 3);
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_file_dispenser() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dispenser.toml");

        let d = FileDispenser::open(
            path.clone(),
            IdMode::Distributed {
                node_id: "7".to_string(),
            },
        )
        .unwrap();

        assert_eq!(d.next("FR").unwrap(), 1);
        assert_eq!(d.next("FR").unwrap(), 2);
        assert_eq!(d.next_id("FR").unwrap(), "FR-7-003");

        // Reopen — state should persist
        let d2 = FileDispenser::open(
            path,
            IdMode::Distributed {
                node_id: "7".to_string(),
            },
        )
        .unwrap();
        assert_eq!(d2.next("FR").unwrap(), 4);
        assert_eq!(d2.peek("FEAT").unwrap(), 1);
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_sqlite_dispenser_basic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dispenser.db");

        let d = SqliteDispenser::open(
            path,
            IdMode::Distributed {
                node_id: "7".to_string(),
            },
        )
        .unwrap();

        assert_eq!(d.next("FR").unwrap(), 1);
        assert_eq!(d.next("FR").unwrap(), 2);
        assert_eq!(d.next("FR").unwrap(), 3);
        assert_eq!(d.next("FEAT").unwrap(), 1);
        assert_eq!(d.next("FR").unwrap(), 4);

        assert_eq!(d.next_id("FR").unwrap(), "FR-7-005");
        assert_eq!(d.next_id("FEAT").unwrap(), "FEAT-7-002");
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_sqlite_dispenser_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dispenser.db");

        // First session
        {
            let d = SqliteDispenser::open(
                path.clone(),
                IdMode::Distributed {
                    node_id: "3".to_string(),
                },
            )
            .unwrap();
            assert_eq!(d.next("FR").unwrap(), 1);
            assert_eq!(d.next("FR").unwrap(), 2);
            assert_eq!(d.next("BUG").unwrap(), 1);
        }

        // Second session — state persisted
        {
            let d = SqliteDispenser::open(
                path,
                IdMode::Distributed {
                    node_id: "3".to_string(),
                },
            )
            .unwrap();
            assert_eq!(d.next("FR").unwrap(), 3);
            assert_eq!(d.peek("BUG").unwrap(), 2);
            assert_eq!(d.next_id("BUG").unwrap(), "BUG-3-002");
        }
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_sqlite_dispenser_centralized() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dispenser.db");

        let d = SqliteDispenser::open(path, IdMode::Centralized).unwrap();

        assert_eq!(d.next_id("FR").unwrap(), "FR-001");
        assert_eq!(d.next_id("FR").unwrap(), "FR-002");
        assert_eq!(d.next_id("BUG").unwrap(), "BUG-001");
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_sqlite_dispenser_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dispenser.db");

        let d = SqliteDispenser::open(path, IdMode::Centralized).unwrap();
        assert_eq!(d.next("fr").unwrap(), 1);
        assert_eq!(d.next("FR").unwrap(), 2);
        assert_eq!(d.next("Fr").unwrap(), 3);
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_sqlite_dispenser_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dispenser.db");

        let d = SqliteDispenser::open(
            path,
            IdMode::Distributed {
                node_id: "42".to_string(),
            },
        )
        .unwrap();
        d.next("FR").unwrap();
        d.next("FR").unwrap();
        d.next("FEAT").unwrap();

        let state = d.state().unwrap();
        assert_eq!(
            state.mode,
            IdMode::Distributed {
                node_id: "42".to_string()
            }
        );
        assert_eq!(state.sequences.get("FR"), Some(&2));
        assert_eq!(state.sequences.get("FEAT"), Some(&1));
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_sqlite_dispenser_concurrent_threads() {
        use std::sync::Arc;
        use std::thread;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dispenser.db");

        let d = Arc::new(
            SqliteDispenser::open(
                path,
                IdMode::Distributed {
                    node_id: "1".to_string(),
                },
            )
            .unwrap(),
        );

        let mut handles = vec![];
        for _ in 0..10 {
            let d = Arc::clone(&d);
            handles.push(thread::spawn(move || d.next("FR").unwrap()));
        }

        let mut results: Vec<u32> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        results.sort();

        // All 10 values should be unique and sequential 1-10
        assert_eq!(results, (1..=10).collect::<Vec<u32>>());
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_sqlite_dispenser_coexists_with_other_tables() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.db");

        // Create a database with some other table first
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute(
                "CREATE TABLE other_data (id INTEGER PRIMARY KEY, value TEXT)",
                [],
            )
            .unwrap();
            conn.execute("INSERT INTO other_data (value) VALUES ('hello')", [])
                .unwrap();
        }

        // Open dispenser on the same database — should not interfere
        let d = SqliteDispenser::open(path.clone(), IdMode::Centralized).unwrap();
        assert_eq!(d.next("FR").unwrap(), 1);

        // Verify the other table is intact
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            let val: String = conn
                .query_row("SELECT value FROM other_data", [], |r| r.get(0))
                .unwrap();
            assert_eq!(val, "hello");
        }
    }

    // AC6 (TASK-331): concurrent-writer stress test on the dispenser counter
    // file. N threads each open their own FileDispenser on the SAME path and
    // pull a batch of IDs. The advisory lock serializes the writers and
    // write_atomic keeps each counter write torn-free — the invariant is
    // that every ID handed out is unique and contiguous. A torn counter
    // file would replay a value (duplicate SPEC-ID) or fail to parse.
    #[test]
    fn concurrent_file_dispensers_allocate_unique_ids() {
        use std::sync::Arc;
        const THREADS: usize = 8;
        const PER_THREAD: usize = 25;

        let dir = tempfile::tempdir().unwrap();
        let path = Arc::new(dir.path().join("dispenser.toml"));
        FileDispenser::open((*path).clone(), IdMode::Centralized).unwrap();

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let path = Arc::clone(&path);
                std::thread::spawn(move || {
                    let d = FileDispenser::open((*path).clone(), IdMode::Centralized).unwrap();
                    (0..PER_THREAD)
                        .map(|_| d.next("TASK").unwrap())
                        .collect::<Vec<u32>>()
                })
            })
            .collect();

        let mut ids: Vec<u32> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();
        let total = ids.len();
        assert_eq!(total, THREADS * PER_THREAD);
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            total,
            "dispenser handed out duplicate IDs under concurrency"
        );
        // Contiguous 1..=total — no torn state lost or replayed a counter.
        assert_eq!(*ids.last().unwrap(), total as u32);
        // The file still parses and the counter is consistent.
        let reopened = FileDispenser::open((*path).clone(), IdMode::Centralized).unwrap();
        assert_eq!(reopened.peek("TASK").unwrap(), total as u32 + 1);
    }
}
