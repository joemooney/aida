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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdMode {
    /// Centralized mode: IDs are `{TYPE}-{SEQ}` (e.g., `FR-001`).
    /// Used when a central database is always available.
    Centralized,
    /// Distributed mode: IDs are `{TYPE}-{NODEID}-{SEQ}` (e.g., `FR-7-048`).
    /// Used for offline-capable, multi-node deployments.
    Distributed { node_id: u32 },
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
            IdMode::Centralized => {
                Ok(format!("{}-{:0>width$}", object_type, seq, width = digits))
            }
            IdMode::Distributed { node_id } => {
                Ok(format!(
                    "{}-{}-{:0>width$}",
                    object_type, node_id, seq,
                    width = digits
                ))
            }
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
            std::fs::write(&path, content)?;
        }
        Ok(Self { path, mode })
    }

    fn load_state(&self) -> Result<DispenserState> {
        let content = std::fs::read_to_string(&self.path)?;
        let state: DispenserState = toml::from_str(&content)?;
        Ok(state)
    }

    fn save_state(&self, state: &DispenserState) -> Result<()> {
        let content = toml::to_string_pretty(state)?;
        std::fs::write(&self.path, content)?;
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
        let d = MemoryDispenser::new(IdMode::Distributed { node_id: 7 });

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
        let d = MemoryDispenser::new(IdMode::Distributed { node_id: 42 });
        d.next("FR").unwrap();
        d.next("FR").unwrap();
        d.next("FEAT").unwrap();

        let state = d.state().unwrap();
        assert_eq!(state.mode, IdMode::Distributed { node_id: 42 });
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

        let d = FileDispenser::open(path.clone(), IdMode::Distributed { node_id: 7 }).unwrap();

        assert_eq!(d.next("FR").unwrap(), 1);
        assert_eq!(d.next("FR").unwrap(), 2);
        assert_eq!(d.next_id("FR").unwrap(), "FR-7-003");

        // Reopen — state should persist
        let d2 = FileDispenser::open(path, IdMode::Distributed { node_id: 7 }).unwrap();
        assert_eq!(d2.next("FR").unwrap(), 4);
        assert_eq!(d2.peek("FEAT").unwrap(), 1);
    }
}
