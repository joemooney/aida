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
    // Successful publication regeneration is bound to the exact staged tree.
    // Older boolean-only records cannot supply this evidence.
    #[serde(default)]
    pub regenerated_tree: Option<String>,
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
    pub fn require_regenerated_tree(&self, tree: &str) -> Result<()> {
        self.require_prepared()?;
        anyhow::ensure!(
            self.regeneration_success == Some(true)
                && self.regenerated_tree.as_deref() == Some(tree),
            "staged repair tree changed or lacks successful regeneration; retry publication"
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
        let lock = lock_file(&dir.join("repair-journal.lock"))?;
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
    fn entry_mut(&mut self, key: &RepairKey) -> Result<&mut RepairEntry> {
        self.journal
            .entries
            .iter_mut()
            .find(|entry| &entry.key == key)
            .context("missing repair reservation")
    }

    pub fn reserve(&mut self, mut entry: RepairEntry) -> Result<()> {
        if let Some(previous) = self.journal.entries.iter().find(|e| e.key == entry.key) {
            anyhow::ensure!(previous.spawn == SpawnOutcome::ProvenDead, "repair head already reserved; unknown spawn outcomes require proof that the prior lane is dead");
            entry.refusal_notice = previous.refusal_notice.clone();
        }
        self.journal.entries.retain(|e| e.key != entry.key);
        self.journal.entries.push(entry);
        self.save()
    }

    pub fn record_validation(&mut self, key: &RepairKey, tree: &str, success: bool) -> Result<()> {
        let entry = self.entry_mut(key)?;
        entry.require_regenerated_tree(tree)?;
        entry.validation = Some(Validation {
            tree: tree.into(),
            command: crate::lane_job::REPAIR_CHECK
                .iter()
                .map(|arg| (*arg).into())
                .collect(),
            success,
            recorded_at: chrono::Utc::now().to_rfc3339(),
        });
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
        let matching: Vec<_> = self
            .journal
            .entries
            .iter()
            .filter(|entry| {
                entry.tree_path == tree
                    && entry.branch == branch
                    && entry.spawn != SpawnOutcome::ProvenDead
            })
            .collect();
        anyhow::ensure!(
            matching
                .iter()
                .all(|entry| entry.lane_id == previous || entry.lane_id == successor),
            "repair belongs to a different lane; adoption refused"
        );
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
        if !success {
            entry.regenerated_tree = None;
            entry.validation = None;
        }
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
        entry.regenerated_tree = None;
        entry.validation = None;
        self.save()
    }
    pub fn record_regenerated_tree(&mut self, key: &RepairKey, tree: &str) -> Result<()> {
        anyhow::ensure!(
            tree.len() >= 40 && tree.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid regenerated tree id"
        );
        let entry = self
            .journal
            .entries
            .iter_mut()
            .find(|e| &e.key == key)
            .context("missing repair reservation")?;
        anyhow::ensure!(
            entry.preparation_success == Some(true),
            "repair preparation did not succeed"
        );
        entry.regeneration_success = Some(true);
        entry.regenerated_tree = Some(tree.to_owned());
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

fn lock_file(path: &Path) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    anyhow::ensure!(
        lock.metadata()?.is_file(),
        "repair lock is not a regular file"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match lock.try_lock_exclusive() {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(e) => return Err(e).context("repair journal lock unavailable"),
        }
    }
    Ok(lock)
}

// Keep the journal lock out of the mail subprocess, but serialize retries
// for this head until the mailbox acceptance receipt has been persisted.
// trace:TASK-3804 | ai:codex
pub(crate) fn notify_refusal(main: &Path, key: &RepairKey, reason: &str) -> Result<()> {
    notify_refusal_with(main, key, reason, |subject, body| {
        crate::ship_from_seat::advisor_request(main, subject, body)
    })
}

fn notify_refusal_with(
    main: &Path,
    key: &RepairKey,
    reason: &str,
    send: impl FnOnce(&str, &str) -> Result<()>,
) -> Result<()> {
    use sha2::{Digest, Sha256};
    let guard = RepairGuard::lock(main)?;
    let notice_id = Sha256::digest(format!("{}\0{}", key.pr, key.original_head));
    let notice_lock = guard
        .path
        .with_file_name(format!("repair-notice-{notice_id:x}.lock"));
    drop(guard);
    let _delivery = lock_file(&notice_lock)?;
    let mut guard = RepairGuard::lock(main)?;
    let entry = guard.entry_mut(key)?;
    if entry
        .refusal_notice
        .as_ref()
        .is_some_and(|notice| notice.accepted_delivery.is_some())
    {
        return Ok(());
    }
    // Compiler or generator diagnostics can be much larger than a notice.
    let reason: String = reason.chars().take(4096).collect();
    let subject = format!("Conflict repair refused for PR #{}", key.pr);
    let body = format!(
        "Head: {}\nLane: {}\nConflict paths: {:?}\nValidation: {:?}\nReason: {reason}",
        key.original_head, entry.lane_id, entry.original_conflict_paths, entry.validation
    );
    entry.refusal_notice = Some(RefusalNotice {
        reason,
        accepted_delivery: None,
    });
    guard.save()?;
    drop(guard);
    send(&subject, &body)?;
    let mut guard = RepairGuard::lock(main)?;
    let notice = guard
        .entry_mut(key)?
        .refusal_notice
        .as_mut()
        .context("missing repair refusal")?;
    notice.accepted_delivery = Some(chrono::Utc::now().to_rfc3339());
    guard.save()
}

#[cfg(test)]
mod tests {
    use super::*;
    // trace:TASK-3804 | ai:codex
    #[test]
    fn task_3804_repair_refusal_sends_once_per_pr_head() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path();
        let mut sent = vec![];
        for head in ["a".repeat(40), "b".repeat(40)] {
            let key = RepairKey {
                repository: main.into(),
                pr: 3804,
                original_head: head,
            };
            for attempt in 0..3 {
                let entry = RepairEntry {
                    key: key.clone(),
                    dispatched_by: "direct-rework".into(),
                    reserved_at: "now".into(),
                    lane_id: format!("lane-{attempt}"),
                    tree_path: main.join("seat"),
                    branch: "pr".into(),
                    base: "c".repeat(40),
                    spawn: SpawnOutcome::ProvenDead,
                    original_conflict_paths: vec!["src/lib.rs".into()],
                    regeneration_success: None,
                    regenerated_tree: None,
                    preparation_success: None,
                    validation: None,
                    refusal_notice: None,
                };
                RepairGuard::lock(main).unwrap().reserve(entry).unwrap();
                notify_refusal_with(main, &key, "compiler refusal", |subject, body| {
                    assert!(subject.contains("3804"));
                    assert!(body.contains("src/lib.rs"));
                    assert!(body.contains(&key.original_head));
                    sent.push(key.original_head.clone());
                    Ok(())
                })
                .unwrap();
            }
            assert_eq!(
                sent.iter()
                    .filter(|head| *head == &key.original_head)
                    .count(),
                1
            );
        }
        assert_eq!(sent.len(), 2);
    }

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
            regenerated_tree: None,
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
        // A verified successor cannot steal another live lane's reservation.
        let mut guard = RepairGuard::lock(main).unwrap();
        assert!(guard
            .transfer_adopted_lane(&entry.tree_path, "pr", "other", "new")
            .is_err());
        guard
            .transfer_adopted_lane(&entry.tree_path, "pr", "lease-1", "new")
            .unwrap();
        guard
            .transfer_adopted_lane(&entry.tree_path, "pr", "lease-1", "new")
            .unwrap();
        guard
            .record_regenerated_tree(&key, &"c".repeat(40))
            .unwrap();
        drop(guard);
        let mut guard = RepairGuard::lock(main).unwrap();
        assert_eq!(
            guard
                .for_tree(&entry.tree_path, "pr", &key.original_head, &entry.base)
                .unwrap()
                .regenerated_tree
                .as_deref(),
            Some("c".repeat(40).as_str())
        );
        let loaded = guard
            .for_tree(&entry.tree_path, "pr", &key.original_head, &entry.base)
            .unwrap();
        loaded.require_regenerated_tree(&"c".repeat(40)).unwrap();
        assert!(loaded.require_regenerated_tree(&"d".repeat(40)).is_err());
        // Failure/recovery invalidates any earlier exact-tree success.
        guard.record_regeneration(&key, false).unwrap();
        assert!(guard
            .for_tree(&entry.tree_path, "pr", &key.original_head, &entry.base)
            .unwrap()
            .regenerated_tree
            .is_none());
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
