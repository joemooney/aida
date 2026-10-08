//! Bounded append-only evidence in a predeclared, owner-held namespace.
//!
//! A chain is evidence storage, never authentication, a decision or a release
//! permit. The publisher must bind the plan into its immutable journal/COMMIT;
//! the callback pump must establish origin before using this writer. Recovery
//! cannot relabel installation verification as an authentic Git callback.
// trace:BUG-1808 | ai:codex

#![cfg(target_os = "linux")]

use super::durable::{DurableDirectory, DurableWriteError};
use super::records::ContentDigest;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io;
use uuid::Uuid;

const MAX_RECORDS: u32 = 4096;
const MAX_RECORD_BYTES: usize = 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ObservationKind {
    GitCallback,
    InstallVerification,
    PossibleExecution,
    SendOutcome,
    Cancellation,
    Settlement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ObservationPlan {
    pub namespace: Uuid,
    /// Exact immutable prepared step or decided publication digest.
    pub origin: ContentDigest,
    pub allowed_kinds: BTreeSet<ObservationKind>,
    pub max_records: u32,
    /// Encoded records, including framing/identity/hash overhead.
    pub max_bytes: u64,
}

impl ObservationPlan {
    fn validate(&self) -> Result<(), ObservationError> {
        if self.namespace.is_nil()
            || self.allowed_kinds.is_empty()
            || self.max_records == 0
            || self.max_records > MAX_RECORDS
            || self.max_bytes == 0
            || self.max_bytes > MAX_TOTAL_BYTES
        {
            return Err(ObservationError::Plan);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Observation {
    pub kind: ObservationKind,
    /// Already validated typed producer input, encoded by its owner. No
    /// callback admission or process fact is inferred by this byte container.
    pub payload: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    namespace: Uuid,
    origin: ContentDigest,
    sequence: u32,
    previous: Option<ContentDigest>,
    observation: Observation,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum ObservationError {
    #[error("publication observation plan is invalid")]
    Plan,
    #[error("publication observation exceeds the declared budget")]
    Budget,
    #[error("publication observation kind was not declared")]
    Kind,
    #[error("publication observation chain is inconsistent")]
    Chain,
    #[error("publication observation replay conflicts with retained evidence")]
    Replay,
    #[error("publication observation read failed: {0}")]
    Read(#[from] io::Error),
    #[error("publication observation encoding is invalid: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error("publication observation write failed: {0}")]
    Write(#[from] DurableWriteError),
}

pub(super) struct ObservationChain<'a> {
    directory: &'a DurableDirectory,
    plan: &'a ObservationPlan,
    records: Vec<(Observation, ContentDigest)>,
    bytes: u64,
    // A failed append must be reopened/revalidated, never retried from stale
    // in-memory state after a possibly visible rename.
    append_failed: bool,
}

/// Bind this exact checkpoint into the retained receipt. A valid shorter
/// prefix is not proof that all required terminal observations survived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ObservationCheckpoint {
    count: u32,
    bytes: u64,
    tip: Option<ContentDigest>,
}

impl<'a> ObservationChain<'a> {
    pub fn open(
        directory: &'a DurableDirectory,
        plan: &'a ObservationPlan,
    ) -> Result<Self, ObservationError> {
        plan.validate()?;
        let mut records = Vec::new();
        let mut total = 0u64;
        let mut gap = false;
        for sequence in 0..plan.max_records {
            let Some(bytes) = directory.read(&filename(plan, sequence), MAX_RECORD_BYTES)? else {
                gap = true;
                continue;
            };
            // Do not interpret a missing numbered record as end-of-chain if
            // any later declared slot exists. No unbounded directory scan.
            if gap {
                return Err(ObservationError::Chain);
            }
            total = total
                .checked_add(bytes.len() as u64)
                .ok_or(ObservationError::Budget)?;
            if total > plan.max_bytes {
                return Err(ObservationError::Budget);
            }
            let record: Record = serde_json::from_slice(&bytes)?;
            let previous = records
                .last()
                .map(|(_, hash): &(_, ContentDigest)| hash.clone());
            if record.version != 1
                || record.namespace != plan.namespace
                || record.origin != plan.origin
                || record.sequence != sequence
                || record.previous != previous
            {
                return Err(ObservationError::Chain);
            }
            if !plan.allowed_kinds.contains(&record.observation.kind) {
                return Err(ObservationError::Kind);
            }
            // Reconfirm potentially visible-but-unsynced evidence before it
            // can support an ACK or a retained receipt after recovery.
            directory.sync_entry(&filename(plan, sequence))?;
            records.push((record.observation, ContentDigest::of(&bytes)));
        }
        Ok(Self {
            directory,
            plan,
            records,
            bytes: total,
            append_failed: false,
        })
    }

    pub fn tip(&self) -> Option<&ContentDigest> {
        self.records.last().map(|(_, digest)| digest)
    }

    pub fn checkpoint(&self) -> ObservationCheckpoint {
        ObservationCheckpoint {
            count: self.records.len() as u32,
            bytes: self.bytes,
            tip: self.tip().cloned(),
        }
    }

    pub fn require_checkpoint(
        &self,
        expected: &ObservationCheckpoint,
    ) -> Result<(), ObservationError> {
        if &self.checkpoint() != expected {
            return Err(ObservationError::Chain);
        }
        Ok(())
    }

    /// Identical sequence replay is idempotent. Neither a new kind nor a new
    /// payload can replace an existing record, even with the same sequence.
    pub fn append(
        &mut self,
        sequence: u32,
        observation: Observation,
    ) -> Result<ContentDigest, ObservationError> {
        if self.append_failed {
            return Err(ObservationError::Chain);
        }
        if !self.plan.allowed_kinds.contains(&observation.kind) {
            return Err(ObservationError::Kind);
        }
        if let Some((existing, digest)) = self.records.get(sequence as usize) {
            if existing != &observation {
                return Err(ObservationError::Replay);
            }
            let retained = self
                .directory
                .read(&filename(self.plan, sequence), MAX_RECORD_BYTES)?
                .ok_or(ObservationError::Chain)?;
            if &ContentDigest::of(&retained) != digest {
                return Err(ObservationError::Chain);
            }
            self.directory.sync_entry(&filename(self.plan, sequence))?;
            return Ok(digest.clone());
        }
        if sequence as usize != self.records.len() {
            return Err(ObservationError::Chain);
        }
        if sequence >= self.plan.max_records || observation.payload.len() > MAX_RECORD_BYTES {
            return Err(ObservationError::Budget);
        }
        let record = Record {
            version: 1,
            namespace: self.plan.namespace,
            origin: self.plan.origin.clone(),
            sequence,
            previous: self.tip().cloned(),
            observation,
        };
        let bytes = serde_json::to_vec(&record)?;
        let total = self
            .bytes
            .checked_add(bytes.len() as u64)
            .ok_or(ObservationError::Budget)?;
        if bytes.len() > MAX_RECORD_BYTES || total > self.plan.max_bytes {
            return Err(ObservationError::Budget);
        }
        if let Err(error) = self
            .directory
            .create(&filename(self.plan, sequence), &bytes)
        {
            self.append_failed = true;
            return Err(error.into());
        }
        let digest = ContentDigest::of(&bytes);
        self.records.push((record.observation, digest.clone()));
        self.bytes = total;
        Ok(digest)
    }
}

fn filename(plan: &ObservationPlan, sequence: u32) -> String {
    format!("{}-{sequence:04}.json", plan.namespace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn plan() -> ObservationPlan {
        ObservationPlan {
            namespace: Uuid::new_v4(),
            origin: ContentDigest::of(b"immutable decision"),
            allowed_kinds: BTreeSet::from([ObservationKind::InstallVerification]),
            max_records: 4,
            max_bytes: 8192,
        }
    }

    fn observation(payload: &[u8]) -> Observation {
        Observation {
            kind: ObservationKind::InstallVerification,
            payload: payload.to_vec(),
        }
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn immutable_chain_reopens_and_identical_replay_does_not_append() {
        let root = tempfile::tempdir().unwrap();
        let dir = DurableDirectory::open_existing(root.path()).unwrap();
        let plan = plan();
        let mut chain = ObservationChain::open(&dir, &plan).unwrap();
        let first = chain.append(0, observation(b"a")).unwrap();
        assert_eq!(chain.append(0, observation(b"a")).unwrap(), first);
        assert!(matches!(
            chain.append(0, observation(b"b")),
            Err(ObservationError::Replay)
        ));
        let tip = chain.append(1, observation(b"b")).unwrap();
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
        drop(chain);
        let mut reopened = ObservationChain::open(&dir, &plan).unwrap();
        assert_eq!(reopened.tip(), Some(&tip));
        reopened.append(2, observation(b"c")).unwrap();
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn missing_middle_corruption_and_foreign_origin_block() {
        let root = tempfile::tempdir().unwrap();
        let dir = DurableDirectory::open_existing(root.path()).unwrap();
        let plan = plan();
        let mut chain = ObservationChain::open(&dir, &plan).unwrap();
        chain.append(0, observation(b"a")).unwrap();
        chain.append(1, observation(b"b")).unwrap();
        drop(chain);
        let path = root.path().join(filename(&plan, 0));
        let original = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(matches!(
            ObservationChain::open(&dir, &plan),
            Err(ObservationError::Chain)
        ));
        fs::write(&path, b"{").unwrap();
        assert!(matches!(
            ObservationChain::open(&dir, &plan),
            Err(ObservationError::Encoding(_))
        ));
        fs::write(&path, &original).unwrap();
        assert!(ObservationChain::open(&dir, &plan).is_ok());
        let mut foreign = plan.clone();
        foreign.origin = ContentDigest::of(b"another decision");
        assert!(matches!(
            ObservationChain::open(&dir, &foreign),
            Err(ObservationError::Chain)
        ));
        let mut changed: Record = serde_json::from_slice(&original).unwrap();
        changed.observation.payload = b"changed".to_vec();
        fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(matches!(
            ObservationChain::open(&dir, &plan),
            Err(ObservationError::Chain)
        ));
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn budget_kind_and_sequence_cannot_expand_authorization() {
        let root = tempfile::tempdir().unwrap();
        let dir = DurableDirectory::open_existing(root.path()).unwrap();
        let mut plan = plan();
        plan.max_records = 1;
        let mut chain = ObservationChain::open(&dir, &plan).unwrap();
        assert!(matches!(
            chain.append(1, observation(b"a")),
            Err(ObservationError::Chain)
        ));
        assert!(matches!(
            chain.append(
                0,
                Observation {
                    kind: ObservationKind::GitCallback,
                    payload: vec![]
                }
            ),
            Err(ObservationError::Kind)
        ));
        assert!(matches!(
            chain.append(0, observation(&vec![255; 8192])),
            Err(ObservationError::Budget)
        ));
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
        chain.append(0, observation(b"a")).unwrap();
        assert!(matches!(
            chain.append(1, observation(b"b")),
            Err(ObservationError::Budget)
        ));
        drop(chain);
        assert!(ObservationChain::open(&dir, &plan).is_ok());
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn retained_checkpoint_detects_lost_tail_and_append_fault_never_overwrites() {
        let root = tempfile::tempdir().unwrap();
        let dir = DurableDirectory::open_existing(root.path()).unwrap();
        let plan = plan();
        let mut chain = ObservationChain::open(&dir, &plan).unwrap();
        chain.append(0, observation(b"a")).unwrap();
        let checkpoint = chain.checkpoint();
        chain.require_checkpoint(&checkpoint).unwrap();
        fs::remove_file(root.path().join(filename(&plan, 0))).unwrap();
        assert!(matches!(
            chain.append(0, observation(b"a")),
            Err(ObservationError::Chain)
        ));
        drop(chain);
        let mut reopened = ObservationChain::open(&dir, &plan).unwrap();
        assert!(matches!(
            reopened.require_checkpoint(&checkpoint),
            Err(ObservationError::Chain)
        ));
        dir.create(&filename(&plan, 0), b"inconsistent occupied entry")
            .unwrap();
        assert!(matches!(
            reopened.append(0, observation(b"a")),
            Err(ObservationError::Write(_))
        ));
        assert!(matches!(
            reopened.append(0, observation(b"a")),
            Err(ObservationError::Chain)
        ));
        assert_eq!(
            dir.read(&filename(&plan, 0), 100).unwrap().unwrap(),
            b"inconsistent occupied entry"
        );
    }
}
