//! Durable retained-work foundation. No command, observer, or launch admission is
//! enabled here. Callers take the ownership lock BEFORE the canonical store lock;
//! canonical identity is written first, then the wait. A missing half retains
//! ownership as recovery uncertainty, never as abandoned work.
// trace:STORY-2661 | ai:codex
use aida_core::{DecisionRequest, ParkBinding};
use anyhow::{ensure, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use uuid::Uuid;

const LIMIT: u64 = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WaitState {
    ParkRequested,
    Parked,
    ResumeReady,
    Resuming,
    BlockedAuthority,
    DeliveryUnknown,
    PostExitRecoveryRequired,
    RecoveryRequired,
    Released,
}

/// Fields are data references, never reusable credentials. Terminal release is
/// explicit; delivery acknowledgement alone leaves retained ownership intact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ParkRecord {
    pub version: u32,
    pub binding: ParkBinding,
    pub spec_uuid: Uuid,
    pub question_id: Uuid,
    pub question_hash: String,
    pub owner_role: String,
    pub lease_id: String,
    pub run_id: String,
    pub phase: String,
    pub vendor: String,
    pub vendor_session_id: String,
    pub capture_source: String,
    pub launch_generation: u64,
    pub harness_version: Option<String>,
    pub seat_path: PathBuf,
    pub worktree_identity: String,
    pub branch: String,
    pub head_sha: String,
    pub posture: String,
    pub grant_reference: String,
    pub requester_identity: String,
    pub state: WaitState,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub answer_hash: Option<String>,
    pub answer_event: Option<String>,
    pub recovery_reason: Option<String>,
}

pub(crate) fn question_hash(question: &DecisionRequest) -> Result<String> {
    // Asked/answered timestamps, answer and binding are deliberately excluded.
    let bytes = serde_json::to_vec(&(
        &question.question,
        &question.choices,
        question.recommended,
        &question.rationale,
    ))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(crate) fn answer_hash(question: &DecisionRequest) -> Result<Option<String>> {
    let Some(choice) = question.answered else {
        return Ok(None);
    };
    ensure!(choice < question.choices.len(), "invalid answer index");
    let bytes = serde_json::to_vec(&(choice, &question.note, question.answered_at))?;
    Ok(Some(format!("{:x}", Sha256::digest(bytes))))
}

/// Invoke only while holding the canonical writer lock; persist the migrated
/// identity before writing any runtime record. Repeating migration is stable.
pub(crate) fn migrate_question_identity(question: &mut DecisionRequest) -> Result<Uuid> {
    ensure!(
        question.park_binding.is_none() || question.question_id.is_some(),
        "bound question missing identity requires recovery"
    );
    Ok(*question.question_id.get_or_insert_with(Uuid::new_v4))
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Ownership {
    Unbound,
    Retained,
    RecoveryRequired(String),
    Released,
}

/// Pure restart reconciliation. Neither missing evidence nor a changed answer
/// can release a seat, dispatch an answer, or repair canonical data by guessing.
pub(crate) fn reconcile(
    spec_uuid: Uuid,
    question: Option<&DecisionRequest>,
    record: Option<&ParkRecord>,
) -> Ownership {
    let binding = question.and_then(|q| q.park_binding.as_ref());
    if binding.is_none() && record.is_none() {
        return Ownership::Unbound;
    }
    let (Some(question), Some(binding), Some(record)) = (question, binding, record) else {
        return Ownership::RecoveryRequired("incomplete canonical/runtime binding".into());
    };
    if binding.version != 1
        || record.version != 1
        || binding.owner_node_id.is_empty()
        || record.binding != *binding
        || record.spec_uuid != spec_uuid
        || question.question_id != Some(record.question_id)
        || question_hash(question).ok().as_ref() != Some(&record.question_hash)
    {
        return Ownership::RecoveryRequired("conflicting question/wait identity".into());
    }
    if let Some(expected) = &record.answer_hash {
        if answer_hash(question).ok().flatten().as_ref() != Some(expected) {
            return Ownership::RecoveryRequired("answer changed after observation".into());
        }
    }
    if record.state == WaitState::Released {
        Ownership::Released
    } else {
        Ownership::Retained
    }
}

/// Pinned private directory and permanent per-wait sidecar. Opening never waits
/// on FIFOs and lock acquisition is bounded (busy returns an error).
pub(crate) struct WaitLock {
    directory: File,
    _lock: File,
    wait_id: Uuid,
}
impl WaitLock {
    pub(crate) fn acquire(directory: &Path, wait_id: Uuid) -> Result<Self> {
        Self::acquire_with_parent_sync(directory, wait_id, File::sync_all)
    }

    fn acquire_with_parent_sync(
        directory: &Path,
        wait_id: Uuid,
        sync_parent: impl FnOnce(&File) -> std::io::Result<()>,
    ) -> Result<Self> {
        // Resolve each directory component relative to a pinned handle. Checking
        // only the final component would still follow a symlinked .aida parent.
        ensure!(
            directory.is_absolute(),
            "park runtime path must be absolute"
        );
        let mut parent = File::open("/")?;
        let components: Vec<_> = directory.components().collect();
        let mut sync_parent = Some(sync_parent);
        for (index, component) in components.iter().enumerate().skip(1) {
            let std::path::Component::Normal(name) = component else {
                anyhow::bail!("invalid park runtime path component");
            };
            let path = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(name);
            if index + 1 == components.len() {
                match std::fs::DirBuilder::new().mode(0o700).create(&path) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                    Err(e) => return Err(e.into()),
                }
                // Every acquirer must persist the entry: an earlier creator may
                // have failed before syncing this pinned parent.
                sync_parent.take().unwrap()(&parent)?;
            }
            parent = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
                .open(path)?;
        }
        let directory = parent;
        let meta = directory.metadata()?;
        ensure!(
            meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o777 == 0o700,
            "unsafe park runtime directory"
        );
        let path = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
        let lock = Self::open(&path.join(format!("{wait_id}.lock")), true)?;
        lock.try_lock_exclusive()?;
        Ok(Self {
            directory,
            _lock: lock,
            wait_id,
        })
    }
    /// Migrate a legacy question through the canonical single-spec writer.
    /// The per-wait ownership lock is already held; the backend acquires the
    /// store lock and reloads the target before assigning a stable identity.
    pub(crate) fn migrate_legacy_question(
        &self,
        backend: &aida_core::CachedGitBackend,
        target: &aida_core::Requirement,
    ) -> Result<DecisionRequest> {
        use aida_core::DatabaseBackend;
        let mut outcome = None;
        backend.update_spec_atomically(target, |current| {
            outcome = Some((|| -> Result<DecisionRequest> {
                let question = current
                    .decision_request
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("canonical question missing"))?;
                if let Some(binding) = &question.park_binding {
                    ensure!(
                        binding.wait_id == self.wait_id,
                        "question belongs to another wait"
                    );
                }
                migrate_question_identity(question)?;
                Ok(question.clone())
            })());
        })?;
        outcome.ok_or_else(|| anyhow::anyhow!("canonical spec missing"))?
    }

    fn path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.directory.as_raw_fd()))
    }
    fn open(path: &Path, create: bool) -> Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(create)
            .create(create)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        let meta = file.metadata()?;
        ensure!(
            meta.is_file()
                && meta.nlink() == 1
                && meta.mode() & 0o777 == 0o600
                && meta.uid() == unsafe { libc::geteuid() },
            "unsafe park runtime file"
        );
        Ok(file)
    }
    pub(crate) fn read(&self) -> Result<Option<ParkRecord>> {
        let path = self.path().join(format!("{}.json", self.wait_id));
        let file = match Self::open(&path, false) {
            Ok(file) => file,
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(None)
            }
            Err(e) => return Err(e),
        };
        ensure!(file.metadata()?.len() <= LIMIT, "oversized park record");
        let mut bytes = Vec::new();
        file.take(LIMIT + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() as u64 <= LIMIT, "oversized park record");
        let record: ParkRecord = serde_json::from_slice(&bytes)?;
        ensure!(
            record.version == 1
                && record.binding.version == 1
                && record.binding.wait_id == self.wait_id,
            "unsupported or mismatched park record"
        );
        Ok(Some(record))
    }
    pub(crate) fn write(&self, record: &ParkRecord) -> Result<()> {
        ensure!(
            record.version == 1
                && record.binding.version == 1
                && record.binding.wait_id == self.wait_id,
            "unsupported or mismatched park record"
        );
        // Refuse an unsafe existing destination rather than replacing it.
        if let Some(previous) = self.read()? {
            ensure!(
                previous.binding == record.binding
                    && previous.spec_uuid == record.spec_uuid
                    && previous.question_id == record.question_id
                    && previous.question_hash == record.question_hash
                    && previous.seat_path == record.seat_path
                    && previous.worktree_identity == record.worktree_identity
                    && previous.vendor_session_id == record.vendor_session_id,
                "cannot replace retained wait identity"
            );
            ensure!(
                previous.state != WaitState::Released || record.state == WaitState::Released,
                "cannot resurrect released ownership"
            );
        }
        let bytes = serde_json::to_vec(record)?;
        ensure!(bytes.len() as u64 <= LIMIT, "oversized park record");
        let temporary = self.path().join(format!("{}.tmp", Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        let result = (|| -> Result<()> {
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(
                &temporary,
                self.path().join(format!("{}.json", self.wait_id)),
            )?;
            self.directory.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        result
    }
}

#[cfg(test)]
#[path = "tests/park_runtime_tests.rs"]
mod tests;
