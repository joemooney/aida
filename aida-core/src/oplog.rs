// trace:ARCH-distributed-oplog | ai:claude
//! Operation Log — append-only event log for conflict-free distributed sync.
//!
//! Inspired by git-bug's CRDT model: instead of syncing final state (which
//! requires conflict resolution), we sync append-only operation logs and
//! replay them deterministically to derive state.
//!
//! Each operation is a small, atomic change:
//! - CreateOp: new requirement
//! - SetTitleOp: change title
//! - SetStatusOp: change status
//! - AddCommentOp: add a comment
//! - etc.
//!
//! Operations carry a Lamport clock for causal ordering. When two operations
//! have the same clock value (concurrent), they're ordered by a deterministic
//! tiebreaker (operation hash).
//!
//! Current status: Foundation only. The full CRDT replay engine is future work.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A Lamport logical clock for causal ordering.
/// Incremented on every local operation, updated to max(local, remote) + 1
/// when receiving remote operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LamportClock(pub u64);

impl LamportClock {
    pub fn new() -> Self {
        Self(0)
    }

    /// Increment for a local operation.
    pub fn tick(&mut self) -> Self {
        self.0 += 1;
        *self
    }

    /// Update after receiving a remote clock value.
    pub fn receive(&mut self, remote: LamportClock) -> Self {
        self.0 = std::cmp::max(self.0, remote.0) + 1;
        *self
    }
}

impl Default for LamportClock {
    fn default() -> Self {
        Self::new()
    }
}

/// An operation on a requirement — the atomic unit of change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    /// Unique operation ID
    pub id: Uuid,
    /// The requirement this operation applies to
    pub target_id: Uuid,
    /// Who performed the operation
    pub author: String,
    /// Node that created this operation. String as of EPIC-9 / STORY-41
    /// (was u32; numeric ids deserialize as decimal strings for back-compat).
    #[serde(
        default = "default_node_id_str",
        deserialize_with = "deserialize_node_id_oplog"
    )]
    pub node_id: String,
    /// Lamport clock value at creation
    pub lamport: LamportClock,
    /// Wall clock timestamp
    pub timestamp: DateTime<Utc>,
    /// The actual change
    pub kind: OpKind,
}

/// The kind of operation — what changed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OpKind {
    /// Create a new requirement
    Create {
        title: String,
        description: String,
        req_type: String,
        status: String,
        priority: String,
    },
    /// Change the title
    SetTitle { title: String },
    /// Change the description
    SetDescription { description: String },
    /// Change the status
    SetStatus { status: String },
    /// Change the priority
    SetPriority { priority: String },
    /// Change the owner
    SetOwner { owner: String },
    /// Add a tag
    AddTag { tag: String },
    /// Remove a tag
    RemoveTag { tag: String },
    /// Add a comment
    AddComment {
        comment_id: Uuid,
        content: String,
        author: String,
    },
    /// Add a relationship
    AddRelationship { target_id: Uuid, rel_type: String },
    /// Remove a relationship
    RemoveRelationship { target_id: Uuid },
    /// Archive the requirement
    Archive,
    /// Unarchive the requirement
    Unarchive,
}

/// An operation log — append-only sequence of operations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpLog {
    /// All operations, in insertion order
    pub operations: Vec<Operation>,
    /// Current Lamport clock value
    pub clock: LamportClock,
    /// Node ID for this log. String as of STORY-41.
    #[serde(
        default = "default_node_id_str",
        deserialize_with = "deserialize_node_id_oplog"
    )]
    pub node_id: String,
}

fn default_node_id_str() -> String {
    "0".to_string()
}

/// Local back-compat deserializer (accepts u64 or String).
/// trace:STORY-41 | ai:claude
fn deserialize_node_id_oplog<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
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

impl OpLog {
    /// Create a new empty operation log for the given node.
    pub fn new(node_id: String) -> Self {
        Self {
            operations: Vec::new(),
            clock: LamportClock::new(),
            node_id,
        }
    }

    /// Append a new operation to the log.
    pub fn append(&mut self, target_id: Uuid, author: String, kind: OpKind) -> &Operation {
        let lamport = self.clock.tick();
        let op = Operation {
            id: Uuid::now_v7(),
            target_id,
            author,
            node_id: self.node_id.clone(),
            lamport,
            timestamp: Utc::now(),
            kind,
        };
        self.operations.push(op);
        self.operations.last().unwrap()
    }

    /// Merge operations from a remote log.
    /// Deduplicates by operation ID, updates the Lamport clock.
    pub fn merge(&mut self, remote: &OpLog) {
        let existing_ids: std::collections::HashSet<Uuid> =
            self.operations.iter().map(|op| op.id).collect();

        for op in &remote.operations {
            if !existing_ids.contains(&op.id) {
                self.clock.receive(op.lamport);
                self.operations.push(op.clone());
            }
        }

        // Sort by Lamport clock, then by operation ID for deterministic ordering
        self.operations
            .sort_by(|a, b| a.lamport.cmp(&b.lamport).then(a.id.cmp(&b.id)));
    }

    /// Get all operations for a specific requirement, in causal order.
    pub fn ops_for(&self, target_id: &Uuid) -> Vec<&Operation> {
        self.operations
            .iter()
            .filter(|op| &op.target_id == target_id)
            .collect()
    }

    /// Get operations since a specific Lamport clock value.
    pub fn ops_since(&self, since: LamportClock) -> Vec<&Operation> {
        self.operations
            .iter()
            .filter(|op| op.lamport > since)
            .collect()
    }

    /// Get the total number of operations.
    pub fn len(&self) -> usize {
        self.operations.len()
    }

    /// Check if the log is empty.
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    /// Save the operation log to a YAML file.
    #[cfg(feature = "native")]
    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        let content = serde_yaml::to_string(self)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Load the operation log from a YAML file.
    #[cfg(feature = "native")]
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        let log: Self = serde_yaml::from_str(&content)?;
        Ok(log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lamport_clock_tick() {
        let mut clock = LamportClock::new();
        assert_eq!(clock.tick(), LamportClock(1));
        assert_eq!(clock.tick(), LamportClock(2));
        assert_eq!(clock.tick(), LamportClock(3));
    }

    #[test]
    fn test_lamport_clock_receive() {
        let mut local = LamportClock(5);
        let remote = LamportClock(3);
        assert_eq!(local.receive(remote), LamportClock(6)); // max(5,3)+1

        let remote2 = LamportClock(10);
        assert_eq!(local.receive(remote2), LamportClock(11)); // max(6,10)+1
    }

    #[test]
    fn test_oplog_append() {
        let mut log = OpLog::new("7".to_string());
        let target = Uuid::now_v7();

        log.append(
            target,
            "joe".into(),
            OpKind::Create {
                title: "Test".into(),
                description: "Desc".into(),
                req_type: "functional".into(),
                status: "draft".into(),
                priority: "medium".into(),
            },
        );

        log.append(
            target,
            "joe".into(),
            OpKind::SetStatus {
                status: "approved".into(),
            },
        );

        assert_eq!(log.len(), 2);
        assert_eq!(log.operations[0].lamport, LamportClock(1));
        assert_eq!(log.operations[1].lamport, LamportClock(2));
    }

    #[test]
    fn test_oplog_merge_dedup() {
        let target = Uuid::now_v7();

        let mut log_a = OpLog::new("1".to_string());
        let op1 = log_a
            .append(
                target,
                "alice".into(),
                OpKind::SetTitle {
                    title: "Alice's title".into(),
                },
            )
            .clone();

        let mut log_b = OpLog::new("2".to_string());
        let _op2 = log_b
            .append(
                target,
                "bob".into(),
                OpKind::SetTitle {
                    title: "Bob's title".into(),
                },
            )
            .clone();

        // Manually add op1 to log_b to simulate it was already synced
        log_b.operations.push(op1.clone());

        // Merge log_b into log_a
        log_a.merge(&log_b);

        // Should have 2 unique operations (op1 was deduped)
        assert_eq!(log_a.len(), 2);
    }

    #[test]
    fn test_oplog_merge_deterministic_order() {
        let target = Uuid::now_v7();

        let mut log_a = OpLog::new("1".to_string());
        log_a.append(
            target,
            "alice".into(),
            OpKind::SetTitle {
                title: "From A".into(),
            },
        );

        let mut log_b = OpLog::new("2".to_string());
        log_b.append(
            target,
            "bob".into(),
            OpKind::SetTitle {
                title: "From B".into(),
            },
        );

        // Merge in both directions — should produce same order
        let mut merged_ab = log_a.clone();
        merged_ab.merge(&log_b);

        let mut merged_ba = log_b.clone();
        merged_ba.merge(&log_a);

        // Both should have same operations in same order
        assert_eq!(merged_ab.len(), merged_ba.len());
        for (a, b) in merged_ab.operations.iter().zip(merged_ba.operations.iter()) {
            assert_eq!(a.id, b.id);
        }
    }

    #[test]
    fn test_ops_for_target() {
        let mut log = OpLog::new("1".to_string());
        let target_a = Uuid::now_v7();
        let target_b = Uuid::now_v7();

        log.append(
            target_a,
            "joe".into(),
            OpKind::SetTitle { title: "A".into() },
        );
        log.append(
            target_b,
            "joe".into(),
            OpKind::SetTitle { title: "B".into() },
        );
        log.append(
            target_a,
            "joe".into(),
            OpKind::SetStatus {
                status: "done".into(),
            },
        );

        let ops_a = log.ops_for(&target_a);
        assert_eq!(ops_a.len(), 2);

        let ops_b = log.ops_for(&target_b);
        assert_eq!(ops_b.len(), 1);
    }

    #[test]
    fn test_ops_since() {
        let mut log = OpLog::new("1".to_string());
        let target = Uuid::now_v7();

        log.append(
            target,
            "joe".into(),
            OpKind::SetTitle { title: "v1".into() },
        );
        log.append(
            target,
            "joe".into(),
            OpKind::SetTitle { title: "v2".into() },
        );
        let checkpoint = log.clock;
        log.append(
            target,
            "joe".into(),
            OpKind::SetTitle { title: "v3".into() },
        );

        let since = log.ops_since(checkpoint);
        assert_eq!(since.len(), 1);
        match &since[0].kind {
            OpKind::SetTitle { title } => assert_eq!(title, "v3"),
            _ => panic!("wrong op kind"),
        }
    }

    #[cfg(feature = "native")]
    #[test]
    fn test_oplog_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oplog.yaml");

        let mut log = OpLog::new("7".to_string());
        let target = Uuid::now_v7();
        log.append(
            target,
            "joe".into(),
            OpKind::Create {
                title: "Test".into(),
                description: "Desc".into(),
                req_type: "functional".into(),
                status: "draft".into(),
                priority: "medium".into(),
            },
        );
        log.save(&path).unwrap();

        let loaded = OpLog::load(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.node_id, "7");
    }
}
