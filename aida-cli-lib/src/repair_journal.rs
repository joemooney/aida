//! Shared host repair evidence. Watchers may retain a key, never a private copy.
//! Permanent sidecar lock -> bounded read -> atomic replacement; no lock across
//! child execution or mail delivery. Unknown spawn outcomes reserve the head.
//! trace:STORY-2710 | ai:codex
use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RepairKey {
    pub repository: PathBuf,
    pub pr: u64,
    pub original_head: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SpawnOutcome {
    Unknown,
    Started,
    ProvenDead,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Validation {
    pub tree: String,
    pub command: Vec<String>,
    pub success: bool,
    pub recorded_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RefusalNotice {
    pub reason: String,
    pub accepted_delivery: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RepairEntry {
    pub key: RepairKey,
    pub dispatched_by: String,
    pub reserved_at: String,
    pub lane_id: String,
    pub tree_path: PathBuf,
    pub branch: String,
    pub base: String,
    pub spawn: SpawnOutcome,
    // Immutable once host merge observation has been recorded. Never replaced
    // by a later (possibly fully staged) index scan.
    pub original_conflict_paths: Vec<String>,
    #[serde(default)]
    pub regeneration_success: Option<bool>,
    // Preparation restores outputs from this entry's immutable base/head inputs.
    // Missing evidence (including journals from older hosts) fails closed.
    #[serde(default)]
    pub preparation_success: Option<bool>,
    pub validation: Option<Validation>,
    pub refusal_notice: Option<RefusalNotice>,
}
impl RepairEntry {
    // A recorded failure survives adoption and staging retries. Only successful
    // preparation (and a full retry after generation failure) admits the lane.
    // The entry lookup binds this evidence to original head, base and tree.
    // trace:STORY-2710 | ai:codex
    pub fn require_prepared(&self) -> Result<()> {
        anyhow::ensure!(
            self.preparation_success == Some(true)
                && self.regeneration_success != Some(false)
                && !self.original_conflict_paths.is_empty(),
            "repair preparation did not succeed; resume pickup to retry regeneration before publication"
        );
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
struct Journal {
    version: u32,
    entries: Vec<RepairEntry>,
}
pub(crate) struct RepairGuard {
    _lock: File,
    path: PathBuf,
    journal: Journal,
}
impl RepairGuard {
    pub fn lock(main: &Path) -> Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let dir = main.join(".aida");
        std::fs::create_dir_all(&dir)?;
        anyhow::ensure!(
            !std::fs::symlink_metadata(&dir)?.file_type().is_symlink(),
            "repair journal directory is a symlink"
        );
        let path = dir.join("repair-journal.json");
        let lock = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(dir.join("repair-journal.lock"))?;
        anyhow::ensure!(
            lock.metadata()?.is_file(),
            "repair lock is not a regular file"
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match lock.try_lock_exclusive() {
                Ok(()) => break,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(e) => return Err(e).context("repair journal lock unavailable"),
            }
        }
        let journal = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
        {
            Ok(mut f) => {
                use std::io::Read;
                anyhow::ensure!(
                    f.metadata()?.is_file() && f.metadata()?.len() <= 4 * 1024 * 1024,
                    "invalid repair journal file"
                );
                let mut bytes = Vec::new();
                (&mut f).take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
                anyhow::ensure!(bytes.len() <= 4 * 1024 * 1024, "repair journal too large");
                serde_json::from_slice::<Journal>(&bytes).context("invalid repair journal")?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Journal {
                version: 1,
                entries: vec![],
            },
            Err(e) => return Err(e).context("cannot read repair journal"),
        };
        anyhow::ensure!(
            journal.version == 1,
            "unsupported repair journal version {}",
            journal.version
        );
        for (i, entry) in journal.entries.iter().enumerate() {
            anyhow::ensure!(
                entry.key.repository == main
                    && !journal.entries[..i].iter().any(|e| e.key == entry.key),
                "invalid or duplicate repair journal key"
            );
        }
        Ok(Self {
            _lock: lock,
            path,
            journal,
        })
    }
    pub fn reserve(&mut self, entry: RepairEntry) -> Result<()> {
        anyhow::ensure!(!self.journal.entries.iter().any(|e| e.key == entry.key && e.spawn != SpawnOutcome::ProvenDead), "repair head already reserved; unknown spawn outcomes require proof that the prior lane is dead");
        self.journal.entries.retain(|e| e.key != entry.key);
        self.journal.entries.push(entry);
        self.save()
    }
    pub fn record_paths(&mut self, key: &RepairKey, paths: &[String]) -> Result<()> {
        let entry = self
            .journal
            .entries
            .iter_mut()
            .find(|e| &e.key == key)
            .context("missing repair reservation")?;
        anyhow::ensure!(!paths.is_empty(), "empty original conflict paths");
        anyhow::ensure!(
            entry.original_conflict_paths.is_empty() || entry.original_conflict_paths == paths,
            "original conflict paths are immutable"
        );
        entry.original_conflict_paths = paths.to_vec();
        self.save()
    }
    /// Called only after session_start has verified orphan adoption. Repeating
    /// the transfer is harmless; a different lane never acquires the entry.
    pub fn transfer_adopted_lane(
        &mut self,
        tree: &Path,
        branch: &str,
        previous: &str,
        successor: &str,
    ) -> Result<()> {
        for entry in &mut self.journal.entries {
            if entry.tree_path == tree
                && entry.branch == branch
                && entry.lane_id == previous
                && entry.spawn != SpawnOutcome::ProvenDead
            {
                entry.lane_id = successor.to_owned();
            }
        }
        self.save()
    }
    pub fn record_preparation(&mut self, key: &RepairKey, success: bool) -> Result<()> {
        let entry = self
            .journal
            .entries
            .iter_mut()
            .find(|e| &e.key == key)
            .context("missing repair reservation")?;
        entry.preparation_success = Some(success);
        self.save()
    }
    pub fn record_regeneration(&mut self, key: &RepairKey, success: bool) -> Result<()> {
        let entry = self
            .journal
            .entries
            .iter_mut()
            .find(|e| &e.key == key)
            .context("missing repair reservation")?;
        entry.regeneration_success = Some(success);
        self.save()
    }
    pub fn for_tree(
        &self,
        tree: &Path,
        branch: &str,
        head: &str,
        base: &str,
    ) -> Result<&RepairEntry> {
        let mut entries = self.journal.entries.iter().filter(|e| {
            e.tree_path == tree
                && e.branch == branch
                && e.key.original_head == head
                && e.base == base
                && e.spawn != SpawnOutcome::ProvenDead
        });
        let entry = entries
            .next()
            .context("pending merge has no matching host repair journal evidence")?;
        anyhow::ensure!(
            entries.next().is_none(),
            "ambiguous host repair journal evidence"
        );
        Ok(entry)
    }
    fn save(&self) -> Result<()> {
        aida_core::fs_atomic::write_atomic(&self.path, serde_json::to_vec_pretty(&self.journal)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn story_2710_journal_retains_paths_and_unknown_reservation() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path();
        let key = RepairKey {
            repository: main.into(),
            pr: 3813,
            original_head: "a".repeat(40),
        };
        let entry = RepairEntry {
            key: key.clone(),
            dispatched_by: "direct-rework".into(),
            reserved_at: "now".into(),
            lane_id: "lease-1".into(),
            tree_path: main.join("seat"),
            branch: "pr".into(),
            base: "b".repeat(40),
            spawn: SpawnOutcome::Unknown,
            original_conflict_paths: vec![],
            regeneration_success: None,
            preparation_success: None,
            validation: None,
            refusal_notice: None,
        };
        let mut guard = RepairGuard::lock(main).unwrap();
        guard.reserve(entry.clone()).unwrap();
        assert!(guard.reserve(entry.clone()).is_err());
        guard.record_paths(&key, &["a\n.rs".into()]).unwrap();
        assert!(guard.record_paths(&key, &["other".into()]).is_err());
        drop(guard);
        let guard = RepairGuard::lock(main).unwrap();
        let loaded = guard
            .for_tree(&entry.tree_path, "pr", &key.original_head, &entry.base)
            .unwrap();
        assert_eq!(loaded.original_conflict_paths, ["a\n.rs"]);
        assert!(guard
            .for_tree(&entry.tree_path, "pr", "moved", &entry.base)
            .is_err());
        drop(guard);
        // Missing legacy evidence, recorded preparation failure, and a later
        // generation failure all fail closed across a journal reload.
        let mut guard = RepairGuard::lock(main).unwrap();
        assert!(guard
            .for_tree(&entry.tree_path, "pr", &key.original_head, &entry.base)
            .unwrap()
            .require_prepared()
            .is_err());
        guard.record_preparation(&key, true).unwrap();
        guard.record_regeneration(&key, false).unwrap();
        drop(guard);
        let mut guard = RepairGuard::lock(main).unwrap();
        assert!(guard
            .for_tree(&entry.tree_path, "pr", &key.original_head, &entry.base)
            .unwrap()
            .require_prepared()
            .is_err());
        guard.record_regeneration(&key, true).unwrap();
        guard
            .for_tree(&entry.tree_path, "pr", &key.original_head, &entry.base)
            .unwrap()
            .require_prepared()
            .unwrap();
        drop(guard);
        // Two independent entry points racing the same head admit one owner.
        let mut dead = entry.clone();
        dead.spawn = SpawnOutcome::ProvenDead;
        let mut guard = RepairGuard::lock(main).unwrap();
        guard.journal.entries[0] = dead;
        guard.save().unwrap();
        drop(guard);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let contenders: Vec<_> = (0..2)
            .map(|_| {
                let entry = entry.clone();
                let main = main.to_owned();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    RepairGuard::lock(&main).unwrap().reserve(entry).is_ok()
                })
            })
            .collect();
        let admitted = contenders
            .into_iter()
            .map(|t| usize::from(t.join().unwrap()))
            .sum::<usize>();
        assert_eq!(admitted, 1);
        std::fs::write(
            main.join(".aida/repair-journal.json"),
            br#"{"version":2,"entries":[]}"#,
        )
        .unwrap();
        assert!(RepairGuard::lock(main).is_err());
    }
}
