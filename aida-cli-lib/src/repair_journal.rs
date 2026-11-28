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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Validation {
    #[serde(default)]
    pub advisory: bool,
    pub tree: String,
    pub command: Vec<String>,
    pub success: bool,
    pub recorded_at: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RefusalNotice {
    pub reason: String,
    pub accepted_delivery: Option<String>,
    // Set when the mailbox could not be read to prove whether the notice was
    // already accepted; nothing is sent until an operator resolves it.
    // trace:TASK-3804 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_unknown: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    #[serde(default)]
    pub snapshot_deferred: Option<String>,
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
            (self.regeneration_success == Some(true) || self.snapshot_deferred.is_some())
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    releases: Vec<ReservationRelease>,
}

#[derive(Serialize, Deserialize)]
struct ReservationRelease {
    #[serde(default)]
    merge: Option<ReleasedMerge>,
    id: String,
    uid: u32,
    at: String,
    reason: String,
}
#[derive(Serialize, Deserialize)]
struct ReleasedMerge {
    branch: String,
    head: String,
    base: String,
    pr: u64,
}

impl From<&RepairEntry> for ReleasedMerge {
    fn from(entry: &RepairEntry) -> Self {
        Self {
            branch: entry.branch.clone(),
            head: entry.key.original_head.clone(),
            base: entry.base.clone(),
            pr: entry.key.pr,
        }
    }
}
/// What one dispatch attempt reserved: the entry exactly as written, and the
/// ProvenDead entry it replaced, if any. Only that attempt holds it.
// trace:BUG-2761 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct AttemptReservation {
    entry: RepairEntry,
    replaced: Option<RepairEntry>,
}

use crate::forge::ChangeState;

/// The outside evidence an unattested release needs.
// trace:TASK-3891 | ai:claude
pub(crate) trait DeadReservationProbe {
    /// The forge's settled PR state; `None` when unknown or unreachable.
    fn pr_state(&mut self, main: &Path, pr: u64) -> Option<ChangeState>;
    /// Save the tree's uncommitted work and unique commits; the saved path.
    fn salvage(&mut self, main: &Path, tree: &Path) -> Result<PathBuf>;
}

/// The forge and the complete clone salvage (patch plus a bundle of every
/// commit origin lacks).
// trace:TASK-3891 | ai:claude
pub(crate) struct ForgeProbe;
impl DeadReservationProbe for ForgeProbe {
    fn pr_state(&mut self, main: &Path, pr: u64) -> Option<ChangeState> {
        let change = crate::forge::ChangeRef {
            id: pr,
            url: String::new(),
            branch: String::new(),
            base: String::new(),
            title: None,
        };
        match crate::forge::forge_for(main).observe_change_status(&change) {
            crate::forge_observed::Observed::Known(Ok(status)) => Some(status.state),
            _ => None,
        }
    }
    fn salvage(&mut self, main: &Path, tree: &Path) -> Result<PathBuf> {
        crate::seat_clone::salvage_complete(main, tree)
    }
}

pub(crate) struct RepairGuard {
    _lock: File,
    path: PathBuf,
    journal: Journal,
}
// trace:BUG-2722 | ai:codex
fn reservation_id(entry: &RepairEntry) -> String {
    format!(
        "pr-{}-{}-{}",
        entry.key.pr, entry.key.original_head, entry.lane_id
    )
}

// Check ancestors from root to leaf. A dangling symlink anywhere in the
// chain is not evidence that a worktree has gone away.
fn path_absent(path: &Path) -> Result<bool> {
    use std::path::Component;
    anyhow::ensure!(
        path.is_absolute()
            && path.file_name().is_some()
            && path
                .components()
                .all(|c| matches!(c, Component::RootDir | Component::Normal(_))),
        "ambiguous path {}",
        path.display()
    );
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    };
    let mut parent = File::open("/")?;
    for part in path.components().skip(1) {
        let name = std::ffi::CString::new(part.as_os_str().as_bytes())?;
        // Pin each directory before inspecting its child. Never follow a
        // replaced ancestor while proving a missing suffix.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_PATH | libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(true);
            }
            return Err(error).with_context(|| format!("inspect directory {}", path.display()));
        }
        parent = unsafe { File::from_raw_fd(fd) };
    }
    Ok(false)
}

fn reservation_lease(entry: &RepairEntry) -> Result<&'static str> {
    let id = &entry.lane_id;
    anyhow::ensure!(
        !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "invalid lease ID"
    );
    let root = &entry.key.repository;
    if path_absent(&crate::leases::leases_dir(root))? {
        return Ok("absent");
    }
    let path = crate::leases::lease_path(root, id);
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok("absent"),
        result => {
            result?;
        }
    }
    let bytes = crate::observation_envelope::read_bounded(&path, 256 * 1024)?;
    let lease: crate::SessionLease = toml::from_str(std::str::from_utf8(&bytes)?)?;
    anyhow::ensure!(
        lease.id == *id && lease.worktree_path == entry.tree_path && lease.branch == entry.branch,
        "lease identity mismatch"
    );
    anyhow::ensure!(
        crate::leases::lease_recorded_owner(&lease)
            == aida_core::liveness::RecordedLeaseOwner::Dead,
        "lease {id} has a live or unverified owner"
    );
    Ok("present without a live recorded owner")
}

impl Journal {
    fn reservation(&self, id: &str) -> Result<&RepairEntry> {
        let mut matches = self.entries.iter().filter(|e| reservation_id(e) == id);
        let entry = matches
            .next()
            .context("unknown reservation; inspect aida doctor check repair-reservations")?;
        anyhow::ensure!(matches.next().is_none(), "ambiguous reservation ID");
        Ok(entry)
    }
}

impl RepairGuard {
    pub fn lock(main: &Path) -> Result<Self> {
        let dir = main.join(".aida");
        std::fs::create_dir_all(&dir)?;
        anyhow::ensure!(
            !std::fs::symlink_metadata(&dir)?.file_type().is_symlink(),
            "repair journal directory is a symlink"
        );
        let path = dir.join("repair-journal.json");
        let lock = lock_file(&dir.join("repair-journal.lock"))?;
        let journal = Self::read(main, &path)?;
        Ok(Self {
            _lock: lock,
            path,
            journal,
        })
    }
    fn read(main: &Path, path: &Path) -> Result<Journal> {
        use std::os::unix::fs::OpenOptionsExt;
        anyhow::ensure!(
            std::fs::symlink_metadata(main.join(".aida"))?.is_dir(),
            "invalid repair journal directory"
        );
        let journal = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
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
                releases: vec![],
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
        Ok(journal)
    }
    // trace:BUG-2722 | ai:codex
    pub(crate) fn repair_findings(&self) -> Vec<crate::DoctorFinding> {
        self.journal
            .entries
            .iter()
            .filter(|e| e.spawn != SpawnOutcome::ProvenDead)
            .map(|e| {
                let id = reservation_id(e);
                let summary = format!(
                    "reserved lane {}, tree {}",
                    e.lane_id,
                    e.tree_path.display()
                );
                let action =
                    format!("aida doctor heal repair-reservations --id {id} --attest '<reason>'");
                crate::DoctorFinding {
                    category: "repair-reservations".into(),
                    action,
                    id,
                    summary,
                    safe_heal: false,
                }
            })
            .collect()
    }

    pub(crate) fn attest_reservation(
        main: &Path,
        id: &str,
        reason: &str,
        input: impl std::io::Read + std::io::IsTerminal,
    ) -> Result<()> {
        anyhow::ensure!(
            input.is_terminal() && !reason.trim().is_empty(),
            "release requires a terminal and attestation"
        );
        let snapshot = Self::read(main, &main.join(".aida/repair-journal.json"))?;
        let entry = snapshot.reservation(id)?;
        anyhow::ensure!(
            entry.spawn != SpawnOutcome::ProvenDead,
            "reservation already released"
        );
        let lease = reservation_lease(entry)?;
        let absent = path_absent(&entry.tree_path)?;
        let count = if absent {
            None
        } else {
            crate::seat_clone::repair_unpushed_count(&entry.key.repository, &entry.tree_path)
        };
        let tree_state = if absent { "absent" } else { "present" };
        let count = count.map_or_else(|| "unavailable".into(), |v| v.to_string());
        eprintln!("Entry {id}\nLease {}: {lease}\nTree {:?}: {tree_state}\nUnpushed HEAD commits: {count}\nReason: {reason:?}\nType {id} to release:", entry.lane_id, entry.tree_path);
        let mut answer = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(input), &mut answer)?;
        anyhow::ensure!(
            answer.trim_end_matches(['\r', '\n']) == id,
            "confirmation did not match"
        );
        let mut guard = Self::lock(main)?;
        anyhow::ensure!(
            guard.journal.reservation(id)? == entry,
            "reservation changed during confirmation"
        );
        anyhow::ensure!(
            reservation_lease(entry)? == lease && path_absent(&entry.tree_path)? == absent,
            "lease or tree changed during confirmation"
        );
        let merge = ReleasedMerge::from(entry);
        let entry = guard.entry_mut(&entry.key)?;
        entry.validation = None;
        entry.spawn = SpawnOutcome::ProvenDead;
        guard.journal.releases.push(ReservationRelease {
            merge: Some(merge),
            id: id.into(),
            uid: unsafe { libc::geteuid() },
            at: chrono::Utc::now().to_rfc3339(),
            reason: reason.into(),
        });
        guard.save()
    }

    /// Release, without a per-ID attestation, every reservation whose PR the
    /// forge reports CLOSED or MERGED and whose lane the lease registry shows
    /// dead, after salvaging a tree that is still there. Anything else (OPEN,
    /// unknown or unreachable forge state, a live or unverified lane, a failed
    /// salvage) is kept for the attested path. One line per reservation.
    // trace:TASK-3891 | ai:claude
    pub(crate) fn release_dead_reservations(
        main: &Path,
        probe: &mut dyn DeadReservationProbe,
    ) -> Result<Vec<String>> {
        let snapshot = Self::read(main, &main.join(".aida/repair-journal.json"))?;
        let mut lines = Vec::new();
        for entry in snapshot
            .entries
            .iter()
            .filter(|e| e.spawn != SpawnOutcome::ProvenDead)
        {
            let id = reservation_id(entry);
            let pr = entry.key.pr;
            let keep = |why: String| {
                format!("kept {id}: {why}; release it with --id {id} --attest '<reason>'")
            };
            let state = match probe.pr_state(main, pr) {
                Some(state @ (ChangeState::Closed | ChangeState::Merged)) => state,
                Some(ChangeState::Open) => {
                    lines.push(keep(format!("PR #{pr} is OPEN")));
                    continue;
                }
                None => {
                    lines.push(keep(format!("PR #{pr} state is unknown")));
                    continue;
                }
            };
            let state = if state == ChangeState::Merged {
                "MERGED"
            } else {
                "CLOSED"
            };
            let lease = match reservation_lease(entry) {
                Ok(lease) => lease,
                Err(error) => {
                    lines.push(keep(format!("lane not proven dead: {error:#}")));
                    continue;
                }
            };
            let absent = match path_absent(&entry.tree_path) {
                Ok(absent) => absent,
                Err(error) => {
                    lines.push(keep(format!("tree unreadable: {error:#}")));
                    continue;
                }
            };
            let salvage = if absent {
                "nothing to salvage".to_owned()
            } else {
                match probe.salvage(main, &entry.tree_path) {
                    Ok(path) => format!("salvage {}", path.display()),
                    Err(error) => {
                        lines.push(keep(format!("salvage failed: {error:#}")));
                        continue;
                    }
                }
            };
            // Forge reads and salvage ran unlocked; re-prove under the lock.
            let mut guard = Self::lock(main)?;
            let unchanged = guard.journal.reservation(&id).ok() == Some(entry)
                && reservation_lease(entry).ok() == Some(lease)
                && path_absent(&entry.tree_path).ok() == Some(absent);
            if !unchanged {
                lines.push(keep("reservation, lease or tree changed".into()));
                continue;
            }
            let merge = ReleasedMerge::from(entry);
            let held = guard.entry_mut(&entry.key)?;
            held.validation = None;
            held.spawn = SpawnOutcome::ProvenDead;
            let audit = format!("PR #{pr} {state}, lane dead (lease {lease}), {salvage}");
            guard.journal.releases.push(ReservationRelease {
                merge: Some(merge),
                id: id.clone(),
                uid: unsafe { libc::geteuid() },
                at: chrono::Utc::now().to_rfc3339(),
                reason: audit.clone(),
            });
            guard.save()?;
            lines.push(format!("released {id}: {audit}"));
        }
        Ok(lines)
    }

    fn entry_mut(&mut self, key: &RepairKey) -> Result<&mut RepairEntry> {
        self.journal
            .entries
            .iter_mut()
            .find(|entry| &entry.key == key)
            .context("missing repair reservation")
    }

    /// Retire exactly the reservation this attempt wrote, after its pre-agent
    /// rollback removed the tree. Another attempt's entry, a recorded spawn or
    /// attestation, or a surviving tree keeps it (`false`). A ProvenDead entry
    /// the attempt replaced is restored unchanged. No schema change.
    // trace:BUG-2761 | ai:claude
    pub fn roll_back_attempt(&mut self, attempt: &AttemptReservation) -> Result<bool> {
        let ours = &attempt.entry;
        let Some(i) = self.journal.entries.iter().position(|e| e.key == ours.key) else {
            return Ok(false);
        };
        let current = &self.journal.entries[i];
        let identity = |e: &RepairEntry| {
            (
                reservation_id(e),
                e.tree_path.clone(),
                e.reserved_at.clone(),
            )
        };
        if current.spawn != SpawnOutcome::Unknown
            || identity(current) != identity(ours)
            || !path_absent(&current.tree_path)?
        {
            return Ok(false);
        }
        match attempt.replaced.clone() {
            Some(previous) => self.journal.entries[i] = previous,
            None => {
                self.journal.entries.remove(i);
            }
        }
        self.save()?;
        Ok(true)
    }

    pub fn reserve(&mut self, mut entry: RepairEntry) -> Result<AttemptReservation> {
        let replaced = self
            .journal
            .entries
            .iter()
            .find(|e| e.key == entry.key)
            .cloned();
        if let Some(previous) = replaced.as_ref() {
            anyhow::ensure!(previous.spawn == SpawnOutcome::ProvenDead, "repair head already reserved; unknown spawn outcomes require proof that the prior lane is dead");
            // Upgrade releases written before merge inputs were included in the audit.
            for release in self
                .journal
                .releases
                .iter_mut()
                .filter(|r| r.merge.is_none() && r.id == reservation_id(previous))
            {
                release.merge = Some(ReleasedMerge::from(previous));
            }
            // A successor starts from its own observed paths, head and base.
            entry.preparation_success = None;
            entry.regeneration_success = None;
            entry.regenerated_tree = None;
            entry.validation = None;
            entry.refusal_notice = previous.refusal_notice.clone();
        }
        self.journal.entries.retain(|e| e.key != entry.key);
        self.journal.entries.push(entry.clone());
        self.save()?;
        Ok(AttemptReservation { entry, replaced })
    }

    // `command` is the trusted project policy that ran, or empty when none was
    // configured (a refusal). trace:TASK-3804 | ai:claude
    pub fn record_validation(
        &mut self,
        key: &RepairKey,
        tree: &str,
        command: &[String],
        success: bool,
        advisory: bool,
    ) -> Result<()> {
        let entry = self.entry_mut(key)?;
        entry.require_regenerated_tree(tree)?;
        entry.validation = Some(Validation {
            advisory,
            tree: tree.into(),
            command: command.to_vec(),
            success,
            recorded_at: chrono::Utc::now().to_rfc3339(),
        });
        self.save()
    }
    /// Every merge `[tree, first parent, second parent]` that matches a
    /// journaled repair on `branch` needs validation of that tree, subject to its recorded snapshot deferral.
    // trace:TASK-3804 | ai:claude
    pub fn require_validated_merges(&self, branch: &str, merges: &[[String; 3]]) -> Result<()> {
        for [tree, first, second] in merges {
            let mut current_validation = false;
            for entry in self.journal.entries.iter().filter(|entry| {
                entry.branch == branch && &entry.key.original_head == first && &entry.base == second
            }) {
                let validated = entry.spawn != SpawnOutcome::ProvenDead
                    && entry.validation.as_ref().is_some_and(|validation| {
                        &validation.tree == tree
                            && (validation.success
                                || (validation.advisory
                                    && entry.snapshot_deferred.is_some()
                                    && entry.require_regenerated_tree(tree).is_ok()))
                    });
                if !validated {
                    return Err(UnvalidatedRepairPublication {
                        pr: entry.key.pr,
                        route: "seat-committed conflict merge".into(),
                    }
                    .into());
                }
                current_validation = true;
            }
            if !current_validation {
                if let Some(merge) = self
                    .journal
                    .releases
                    .iter()
                    .filter_map(|r| r.merge.as_ref())
                    .find(|m| m.branch == branch && &m.head == first && &m.base == second)
                {
                    return Err(UnvalidatedRepairPublication {
                        pr: merge.pr,
                        route: "released conflict merge".into(),
                    }
                    .into());
                }
            }
        }
        Ok(())
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
            entry.snapshot_deferred = None;
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
        entry.snapshot_deferred = None;
        entry.regenerated_tree = None;
        entry.validation = None;
        self.save()
    }
    pub fn record_regenerated_tree(
        &mut self,
        key: &RepairKey,
        tree: &str,
        warning: Option<String>,
    ) -> Result<()> {
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
        entry.regeneration_success = warning.is_none().then_some(true);
        entry.snapshot_deferred = warning;
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
        let bytes = serde_json::to_vec_pretty(&self.journal)?;
        anyhow::ensure!(bytes.len() <= 4 * 1024 * 1024, "repair journal too large");
        aida_core::fs_atomic::write_atomic(&self.path, bytes)?;
        Ok(())
    }
}

/// A conflict-repair publication route that bypasses the host compile check on
/// the exact merged tree. Typed so callers and tests can tell it from other
/// refusals.
// trace:TASK-3804 | ai:claude
#[derive(Debug)]
pub(crate) struct UnvalidatedRepairPublication {
    pub pr: u64,
    pub route: String,
}

impl std::fmt::Display for UnvalidatedRepairPublication {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "conflict repair for PR #{} refused on route `{}`: the merged tree has not passed \
             the host compile check, so nothing was published. Run the repair as a contained \
             Codex lane, where the host checks, commits and publishes the merged tree",
            self.pr, self.route
        )
    }
}

impl std::error::Error for UnvalidatedRepairPublication {}

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

/// The stable delivery identity of the refusal notice for one PR head: the
/// mailbox thread the notice is sent on. A retry finds an accepted message by
/// this thread even when the journal never saved its acceptance receipt.
// trace:TASK-3804 | ai:claude
pub(crate) fn refusal_delivery_id(key: &RepairKey) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!(
        "{}\0{}\0{}",
        key.repository.display(),
        key.pr,
        key.original_head
    ));
    format!("repair-refusal-{:x}", digest)[..47].to_owned()
}

// Has the mailbox (local layer or canonical store) already accepted a message
// on this delivery thread? A read failure is an error, never "not delivered":
// the strict reader refuses an unreadable or malformed message file instead
// of skipping it, since a skipped copy of the accepted notice reads as absent.
// trace:TASK-3804 | ai:claude
fn mailbox_has_delivery(main: &Path, delivery: &str) -> Result<bool> {
    let messages = crate::mailbox_store::read_all_messages_strict(main)
        .context("could not read the mailbox to deduplicate the repair refusal")?;
    Ok(messages.iter().any(|message| message.thread_id == delivery))
}

// Keep the journal lock out of the mail subprocess, but serialize retries
// for this head. The mailbox thread is the delivery identity, so a lost
// acceptance receipt is reconciled from the mailbox rather than resent.
// trace:TASK-3804 | ai:codex
pub(crate) fn notify_refusal(main: &Path, key: &RepairKey, reason: &str) -> Result<()> {
    notify_refusal_with(
        main,
        key,
        reason,
        |delivery| mailbox_has_delivery(main, delivery),
        |subject, body, delivery| {
            crate::ship_from_seat::advisor_request_on_thread(main, subject, body, Some(delivery))
        },
    )
}

fn notify_refusal_with(
    main: &Path,
    key: &RepairKey,
    reason: &str,
    delivered: impl Fn(&str) -> Result<bool>,
    send: impl FnOnce(&str, &str, &str) -> Result<()>,
) -> Result<()> {
    let delivery = refusal_delivery_id(key);
    let guard = RepairGuard::lock(main)?;
    let notice_lock = guard.path.with_file_name(format!("{delivery}.lock"));
    drop(guard);
    let _delivery = lock_file(&notice_lock)?;
    let accept = |when: String| -> Result<()> {
        let mut guard = RepairGuard::lock(main)?;
        let entry = guard.entry_mut(key)?;
        let notice = entry.refusal_notice.get_or_insert_with(|| RefusalNotice {
            reason: String::new(),
            accepted_delivery: None,
            delivery_unknown: None,
        });
        notice.accepted_delivery = Some(when);
        notice.delivery_unknown = None;
        guard.save()
    };
    // Fail closed: a mailbox that cannot be read leaves the delivery state
    // unknown. Record that in the journal and refuse to send.
    // trace:TASK-3804 | ai:claude
    let lookup = |delivery: &str| -> Result<bool> {
        delivered(delivery).map_err(|error| {
            let state = format!("delivery state unknown: {error:#}");
            let recorded = RepairGuard::lock(main).and_then(|mut guard| {
                let entry = guard.entry_mut(key)?;
                let notice = entry.refusal_notice.get_or_insert_with(|| RefusalNotice {
                    reason: String::new(),
                    accepted_delivery: None,
                    delivery_unknown: None,
                });
                notice.delivery_unknown = Some(state);
                guard.save()
            });
            let error = error.context(format!(
                "repair refusal for PR #{} head {} was not sent: its delivery state is \
                 unknown. Restore the unreadable mailbox message from an intact copy, then retry",
                key.pr, key.original_head
            ));
            match recorded {
                Ok(()) => error,
                Err(journal) => error.context(format!(
                    "the unknown delivery state could not be journaled: {journal:#}"
                )),
            }
        })
    };
    {
        let guard = RepairGuard::lock(main)?;
        let entry = guard
            .journal
            .entries
            .iter()
            .find(|entry| &entry.key == key)
            .context("missing repair reservation")?;
        if entry
            .refusal_notice
            .as_ref()
            .is_some_and(|notice| notice.accepted_delivery.is_some())
        {
            return Ok(());
        }
    }
    // The mailbox accepted this head's notice earlier but its receipt was
    // never saved (host exit, journal write failure, or a timed-out sender).
    if lookup(&delivery)? {
        return accept(format!("adopted from mailbox thread {delivery}"));
    }
    let mut guard = RepairGuard::lock(main)?;
    let entry = guard.entry_mut(key)?;
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
        delivery_unknown: None,
    });
    guard.save()?;
    drop(guard);
    if let Err(error) = send(&subject, &body, &delivery) {
        // A sender timeout can follow mailbox acceptance: reconcile, never resend.
        match lookup(&delivery) {
            Ok(true) => return accept(format!("adopted from mailbox thread {delivery}")),
            Ok(false) => return Err(error),
            Err(unknown) => return Err(error.context(format!("{unknown:#}"))),
        }
    }
    accept(chrono::Utc::now().to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;
    // trace:BUG-2756 | ai:codex
    #[test]
    fn seat_committed_advisory_validation_requires_matching_deferred_tree() {
        let temp = tempfile::tempdir().unwrap();
        let mut journal = RepairGuard::lock(temp.path()).unwrap();
        let key = RepairKey {
            repository: temp.path().into(),
            pr: 4533,
            original_head: "a".repeat(40),
        };
        let tree = "c".repeat(40);
        let merge = [tree.clone(), key.original_head.clone(), "b".repeat(40)];
        journal
            .reserve(RepairEntry {
                key: key.clone(),
                dispatched_by: "direct-rework".into(),
                reserved_at: "now".into(),
                lane_id: "lease-1".into(),
                tree_path: temp.path().join("seat"),
                branch: "pr".into(),
                base: merge[2].clone(),
                spawn: SpawnOutcome::Started,
                original_conflict_paths: vec!["src/lib.rs".into()],
                regeneration_success: None,
                regenerated_tree: Some(tree.clone()),
                snapshot_deferred: Some("snapshot failed".into()),
                preparation_success: Some(true),
                validation: None,
                refusal_notice: None,
            })
            .unwrap();
        let check = |guard: &RepairGuard| {
            guard.require_validated_merges("pr", std::slice::from_ref(&merge))
        };
        let command = vec!["cargo".into(), "check".into()];
        journal
            .record_validation(&key, &tree, &command, false, false)
            .unwrap();
        assert!(check(&journal).is_err());
        journal
            .record_validation(&key, &tree, &command, false, true)
            .unwrap();
        check(&journal).unwrap();
        journal.entry_mut(&key).unwrap().regenerated_tree = Some("d".repeat(40));
        assert!(check(&journal).is_err());
        journal.entry_mut(&key).unwrap().regenerated_tree = Some(tree);
        journal.entry_mut(&key).unwrap().spawn = SpawnOutcome::ProvenDead;
        assert!(check(&journal).is_err());
    }

    // T2: a failure between reserve and worker start retires exactly that
    // attempt's entry; a prior ProvenDead entry it replaced is restored, and
    // another attempt's entry or a surviving tree is never touched.
    // trace:BUG-2761 | ai:claude
    #[test]
    fn bug_2761_attempt_rollback_retires_only_its_own_reservation() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path();
        let entry = |pr: u64, lane: &str, tree: &str, spawn: SpawnOutcome| RepairEntry {
            key: RepairKey {
                repository: main.into(),
                pr,
                original_head: "b".repeat(40),
            },
            dispatched_by: "direct-rework".into(),
            reserved_at: format!("{lane}-at"),
            lane_id: lane.into(),
            tree_path: main.join(tree),
            branch: "pr".into(),
            base: "c".repeat(40),
            spawn,
            original_conflict_paths: vec!["src/lib.rs".into()],
            regeneration_success: None,
            regenerated_tree: None,
            snapshot_deferred: None,
            preparation_success: None,
            validation: None,
            refusal_notice: None,
        };
        let entries = || RepairGuard::lock(main).unwrap().journal.entries;
        let reserve = |e| RepairGuard::lock(main).unwrap().reserve(e).unwrap();
        let roll_back = |a| {
            RepairGuard::lock(main)
                .unwrap()
                .roll_back_attempt(a)
                .unwrap()
        };
        let other = entry(1, "other-lane", "other-seat", SpawnOutcome::Unknown);
        let dead = entry(2, "old-lane", "old-seat", SpawnOutcome::ProvenDead);
        reserve(other.clone());
        reserve(dead.clone());
        let ours = reserve(entry(2, "new-lane", "gone-seat", SpawnOutcome::Unknown));
        let fresh = reserve(entry(3, "new-lane", "gone-seat-3", SpawnOutcome::Unknown));
        // Preparation evidence recorded after reserve does not change identity.
        let mut guard = RepairGuard::lock(main).unwrap();
        guard.record_preparation(&ours.entry.key, true).unwrap();
        drop(guard);
        assert!(roll_back(&ours) && roll_back(&fresh));
        assert_eq!(entries(), vec![other, dead]);
        // Already retired, or replaced by another attempt: nothing changes.
        assert!(!roll_back(&fresh));
        let replaced = reserve(entry(2, "third-lane", "gone-seat", SpawnOutcome::Unknown));
        assert!(!roll_back(&ours));
        // A tree the rollback did not remove keeps its reservation.
        std::fs::create_dir(main.join("gone-seat")).unwrap();
        assert!(!roll_back(&replaced));
        assert_eq!(entries()[1], replaced.entry);
    }

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
                    snapshot_deferred: None,
                    regenerated_tree: None,
                    preparation_success: None,
                    validation: None,
                    refusal_notice: None,
                };
                RepairGuard::lock(main).unwrap().reserve(entry).unwrap();
                notify_refusal_with(
                    main,
                    &key,
                    "compiler refusal",
                    |_| Ok(false),
                    |subject, body, delivery| {
                        assert!(subject.contains("3804"));
                        assert!(body.contains("src/lib.rs"));
                        assert!(body.contains(&key.original_head));
                        assert_eq!(delivery, refusal_delivery_id(&key));
                        sent.push(key.original_head.clone());
                        Ok(())
                    },
                )
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

    // trace:BUG-2722 | ai:codex
    #[test]
    fn bug_2722_ended_lane_moved_tree_releases_only_selected_head() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path();
        let key = reserved(main, "660c0e87e696e0560b23715a1cbda490ad330edc");
        let other = reserved(main, "other");
        let mut journal = RepairGuard::lock(main).unwrap();
        let mut entry = journal.entry_mut(&key).unwrap().clone();
        entry.key.pr = 4376;
        entry.lane_id = "01a2053df3e6".into();
        entry.branch = "bug-b6b1278890774ef993d2ebc327862470-433".into();
        entry.tree_path = main
            .join("worktrees")
            .join(format!("aida-seat-aida-{}", entry.branch));
        entry.spawn = SpawnOutcome::Unknown;
        entry.preparation_success = Some(true);
        entry.original_conflict_paths = [
            "queue_cmd/work.rs",
            "review_draft.rs",
            "tests/bug_2149_2150_standalone_reviewer_tests.rs",
        ]
        .map(|p| format!("aida-cli-lib/src/{p}"))
        .into();
        std::fs::create_dir(entry.tree_path.parent().unwrap()).unwrap();
        entry.reserved_at = "2026-01-01T00:00:00Z".into();
        let id = reservation_id(&entry);
        journal.reserve(entry.clone()).unwrap();
        use std::io::Write;
        use std::os::fd::FromRawFd;
        fn terminal(answer: &str) -> (File, File) {
            let (mut master, mut slave) = (-1, -1);
            assert_eq!(
                unsafe {
                    libc::openpty(
                        &mut master,
                        &mut slave,
                        std::ptr::null_mut(),
                        std::ptr::null(),
                        std::ptr::null(),
                    )
                },
                0
            );
            let mut master = unsafe { File::from_raw_fd(master) };
            writeln!(master, "{answer}").unwrap();
            (master, unsafe { File::from_raw_fd(slave) })
        }
        drop(journal);
        let reason = "verified moved checkout";
        std::fs::create_dir(&entry.tree_path).unwrap();
        std::fs::rename(&entry.tree_path, main.join("moved")).unwrap();
        assert!(RepairGuard::attest_reservation(
            main,
            &id,
            reason,
            File::open("/dev/null").unwrap()
        )
        .is_err());
        let (_master, input) = terminal("wrong");
        assert!(RepairGuard::attest_reservation(main, &id, reason, input).is_err());
        let dir = crate::leases::leases_dir(main);
        std::fs::create_dir(&dir).unwrap();
        let lease = crate::leases::lease_path(main, &entry.lane_id);
        let live = serde_json::json!({"id":entry.lane_id,"scope":"BUG-2669","slug":"bug-2669","owner":"implementer","worktree_path":entry.tree_path,"branch":entry.branch,"started_at":entry.reserved_at,"hostname":"host","pid_namespace":aida_core::pid_observer::PidObserver::current().namespace,"active_pid":std::process::id()});
        std::fs::write(&lease, toml::to_string(&live).unwrap()).unwrap();
        let (_master, input) = terminal(&id);
        assert!(RepairGuard::attest_reservation(main, &id, reason, input)
            .unwrap_err()
            .to_string()
            .contains("live or unverified"));
        std::fs::remove_file(lease).unwrap();
        let (_master, input) = terminal(&id);
        RepairGuard::attest_reservation(main, &id, reason, input).unwrap();
        let mut journal = RepairGuard::lock(main).unwrap();
        assert_eq!(journal.journal.releases[0].reason, reason);
        assert_eq!(journal.journal.releases[0].id, id);
        assert_eq!(journal.journal.releases[0].uid, unsafe { libc::geteuid() });
        chrono::DateTime::parse_from_rfc3339(&journal.journal.releases[0].at).unwrap();
        entry.lane_id = "successor".into();
        let old_merge = [
            String::from("old-tree"),
            entry.key.original_head.clone(),
            entry.base.clone(),
        ];
        entry.base = "new-base".into();
        entry.spawn = SpawnOutcome::Unknown;
        journal.reserve(entry.clone()).unwrap();
        assert!(journal
            .for_tree(
                &entry.tree_path,
                &entry.branch,
                &entry.key.original_head,
                &old_merge[2]
            )
            .is_err());
        assert!(journal
            .require_validated_merges(&entry.branch, &[old_merge])
            .is_err());
        let successor = journal
            .for_tree(
                &entry.tree_path,
                &entry.branch,
                &entry.key.original_head,
                "new-base",
            )
            .unwrap();
        assert!(successor.require_prepared().is_err());
        assert!(successor.validation.is_none());
        assert_eq!(
            journal.entry_mut(&other).unwrap().spawn,
            SpawnOutcome::Started
        );
    }

    // trace:TASK-3891 | ai:claude
    struct FakeProbe {
        states: std::collections::HashMap<u64, Option<ChangeState>>,
        failing_tree: Option<PathBuf>,
        salvaged: Vec<PathBuf>,
    }
    impl DeadReservationProbe for FakeProbe {
        fn pr_state(&mut self, _: &Path, pr: u64) -> Option<ChangeState> {
            self.states.get(&pr).copied().flatten()
        }
        fn salvage(&mut self, main: &Path, tree: &Path) -> Result<PathBuf> {
            self.salvaged.push(tree.to_owned());
            anyhow::ensure!(self.failing_tree.as_deref() != Some(tree), "disk full");
            Ok(main.join("salvage").join(tree.file_name().unwrap()))
        }
    }

    fn reserve_pr(main: &Path, pr: u64) -> RepairEntry {
        let mut guard = RepairGuard::lock(main).unwrap();
        let entry = RepairEntry {
            key: RepairKey {
                repository: main.into(),
                pr,
                original_head: "a".repeat(40),
            },
            dispatched_by: "direct-rework".into(),
            reserved_at: "now".into(),
            lane_id: format!("lane-{pr}"),
            tree_path: main.join(format!("seat-{pr}")),
            branch: format!("pr-{pr}"),
            base: "c".repeat(40),
            spawn: SpawnOutcome::Started,
            original_conflict_paths: vec!["src/lib.rs".into()],
            regeneration_success: None,
            snapshot_deferred: None,
            regenerated_tree: None,
            preparation_success: None,
            validation: None,
            refusal_notice: None,
        };
        guard.reserve(entry.clone()).unwrap();
        entry
    }

    fn spawn_of(main: &Path, entry: &RepairEntry) -> SpawnOutcome {
        RepairGuard::lock(main)
            .unwrap()
            .entry_mut(&entry.key)
            .unwrap()
            .spawn
            .clone()
    }

    // AC1 + AC2: CLOSED and MERGED PRs with dead lanes release without an
    // attestation, a present tree only after its salvage, each with an audit
    // record. A failed salvage keeps the reservation.
    // trace:TASK-3891 | ai:claude
    #[test]
    fn task_3891_releases_closed_and_merged_dead_lanes_after_salvage() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path();
        let gone = reserve_pr(main, 1);
        let present = reserve_pr(main, 2);
        let unsalvageable = reserve_pr(main, 3);
        std::fs::create_dir(&present.tree_path).unwrap();
        std::fs::create_dir(&unsalvageable.tree_path).unwrap();
        let mut probe = FakeProbe {
            states: [
                (1, Some(ChangeState::Closed)),
                (2, Some(ChangeState::Merged)),
                (3, Some(ChangeState::Merged)),
            ]
            .into(),
            failing_tree: Some(unsalvageable.tree_path.clone()),
            salvaged: vec![],
        };
        let lines = RepairGuard::release_dead_reservations(main, &mut probe).unwrap();
        let (gone_id, present_id) = (reservation_id(&gone), reservation_id(&present));
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(
            lines[0],
            format!(
                "released {gone_id}: PR #1 CLOSED, lane dead (lease absent), nothing to salvage"
            )
        );
        let salvage = main.join("salvage/seat-2");
        assert_eq!(
            lines[1],
            format!(
                "released {present_id}: PR #2 MERGED, lane dead (lease absent), salvage {}",
                salvage.display()
            )
        );
        assert!(lines[2].starts_with(&format!(
            "kept {}: salvage failed: disk full",
            reservation_id(&unsalvageable)
        )));
        assert!(lines[2].contains("--attest"), "{}", lines[2]);
        assert_eq!(
            probe.salvaged,
            [present.tree_path.clone(), unsalvageable.tree_path.clone()]
        );
        assert_eq!(spawn_of(main, &gone), SpawnOutcome::ProvenDead);
        assert_eq!(spawn_of(main, &present), SpawnOutcome::ProvenDead);
        assert_eq!(spawn_of(main, &unsalvageable), SpawnOutcome::Started);
        let journal = RepairGuard::lock(main).unwrap().journal;
        let releases: Vec<_> = journal
            .releases
            .iter()
            .map(|r| (&r.id, &r.reason))
            .collect();
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].0, &gone_id);
        assert!(
            releases[0].1.contains("PR #1 CLOSED") && releases[0].1.contains("nothing to salvage")
        );
        assert_eq!(releases[1].0, &present_id);
        assert!(releases[1].1.contains(&salvage.display().to_string()));
        // A second pass leaves released entries alone.
        let again = RepairGuard::release_dead_reservations(main, &mut probe).unwrap();
        assert_eq!(again.len(), 1, "{again:?}");
        assert!(again[0].starts_with("kept "));
    }

    // AC3: an OPEN PR, an unknown or unreachable forge state and a live lane
    // are never released here; each names the attested path. Nothing is
    // salvaged for them.
    // trace:TASK-3891 | ai:claude
    #[test]
    fn task_3891_open_unknown_or_live_keeps_attested_path() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path();
        let open = reserve_pr(main, 4);
        let unknown = reserve_pr(main, 5);
        let live = reserve_pr(main, 6);
        for entry in [&open, &unknown, &live] {
            std::fs::create_dir(&entry.tree_path).unwrap();
        }
        std::fs::create_dir_all(crate::leases::leases_dir(main)).unwrap();
        let lease = serde_json::json!({"id":live.lane_id,"scope":"BUG-2669","slug":"bug-2669","owner":"implementer","worktree_path":live.tree_path,"branch":live.branch,"started_at":"2026-01-01T00:00:00Z","hostname":"host","pid_namespace":aida_core::pid_observer::PidObserver::current().namespace,"active_pid":std::process::id()});
        std::fs::write(
            crate::leases::lease_path(main, &live.lane_id),
            toml::to_string(&lease).unwrap(),
        )
        .unwrap();
        let mut probe = FakeProbe {
            states: [
                (4, Some(ChangeState::Open)),
                (5, None),
                (6, Some(ChangeState::Closed)),
            ]
            .into(),
            failing_tree: None,
            salvaged: vec![],
        };
        let lines = RepairGuard::release_dead_reservations(main, &mut probe).unwrap();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].contains("PR #4 is OPEN"), "{}", lines[0]);
        assert!(lines[1].contains("PR #5 state is unknown"), "{}", lines[1]);
        assert!(lines[2].contains("lane not proven dead"), "{}", lines[2]);
        assert!(lines[2].contains("live or unverified"), "{}", lines[2]);
        for (line, entry) in lines.iter().zip([&open, &unknown, &live]) {
            let id = reservation_id(entry);
            assert!(line.starts_with(&format!("kept {id}: ")), "{line}");
            assert!(
                line.ends_with(&format!("--id {id} --attest '<reason>'")),
                "{line}"
            );
            assert_eq!(spawn_of(main, entry), SpawnOutcome::Started);
        }
        assert!(probe.salvaged.is_empty());
        assert!(RepairGuard::lock(main).unwrap().journal.releases.is_empty());
    }

    fn reserved(main: &Path, head: &str) -> RepairKey {
        let key = RepairKey {
            repository: main.into(),
            pr: 3804,
            original_head: head.into(),
        };
        RepairGuard::lock(main)
            .unwrap()
            .reserve(RepairEntry {
                key: key.clone(),
                dispatched_by: "direct-rework".into(),
                reserved_at: "now".into(),
                lane_id: "lane".into(),
                tree_path: main.join("seat"),
                branch: "pr".into(),
                base: "c".repeat(40),
                spawn: SpawnOutcome::Started,
                original_conflict_paths: vec!["src/lib.rs".into()],
                regeneration_success: None,
                snapshot_deferred: None,
                regenerated_tree: None,
                preparation_success: None,
                validation: None,
                refusal_notice: None,
            })
            .unwrap();
        key
    }

    // The mailbox accepts the notice, then the acceptance receipt is lost:
    // the host exits before the journal save, or the sender times out after
    // acceptance. Retries adopt the mailbox thread; exactly one mail exists.
    // trace:TASK-3804 | ai:claude
    #[test]
    fn task_3804_lost_acceptance_receipt_never_resends_refusal() {
        use std::cell::RefCell;
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path();
        let mailbox = RefCell::new(Vec::<String>::new());
        let delivered =
            |delivery: &str| -> Result<bool> { Ok(mailbox.borrow().iter().any(|t| t == delivery)) };
        let accept = |delivery: &str| mailbox.borrow_mut().push(delivery.to_owned());

        // 1. Accepted, then the journal receipt is lost before it is saved.
        let lost = reserved(main, &"a".repeat(40));
        notify_refusal_with(main, &lost, "refused", delivered, |_, _, d| {
            accept(d);
            Ok(())
        })
        .unwrap();
        {
            let mut guard = RepairGuard::lock(main).unwrap();
            guard.entry_mut(&lost).unwrap().refusal_notice = None;
            guard.save().unwrap();
        }
        for _ in 0..3 {
            notify_refusal_with(main, &lost, "refused", delivered, |_, _, _| {
                panic!("an accepted notice must not be resent")
            })
            .unwrap();
        }
        let guard = RepairGuard::lock(main).unwrap();
        let notice = guard.journal.entries[0].refusal_notice.as_ref().unwrap();
        assert!(notice
            .accepted_delivery
            .as_deref()
            .unwrap()
            .contains(&refusal_delivery_id(&lost)));
        drop(guard);

        // 2. Accepted, then the sender times out: adopted in the same attempt.
        let timed_out = reserved(main, &"b".repeat(40));
        notify_refusal_with(main, &timed_out, "refused", delivered, |_, _, d| {
            accept(d);
            anyhow::bail!("advisor request did not finish")
        })
        .unwrap();
        notify_refusal_with(main, &timed_out, "refused", delivered, |_, _, _| {
            panic!("an adopted notice must not be resent")
        })
        .unwrap();

        // 3. Refused before acceptance: the error stands and a retry sends once.
        let failed = reserved(main, &"d".repeat(40));
        assert!(
            notify_refusal_with(main, &failed, "refused", delivered, |_, _, _| {
                anyhow::bail!("mailbox refused")
            })
            .is_err()
        );
        notify_refusal_with(main, &failed, "refused", delivered, |_, _, d| {
            accept(d);
            Ok(())
        })
        .unwrap();

        // 4. An unreadable mailbox cannot be mistaken for "not delivered".
        let unreadable = reserved(main, &"e".repeat(40));
        assert!(notify_refusal_with(
            main,
            &unreadable,
            "refused",
            |_| anyhow::bail!("mailbox unreadable"),
            |_, _, _| panic!("an unproven mailbox state must not send")
        )
        .is_err());

        let mail = mailbox.borrow();
        assert_eq!(mail.len(), 3, "{mail:?}");
        for key in [&lost, &timed_out, &failed] {
            let id = refusal_delivery_id(key);
            assert_eq!(mail.iter().filter(|t| **t == id).count(), 1);
        }
        assert_ne!(refusal_delivery_id(&lost), refusal_delivery_id(&timed_out));
    }

    // trace:TASK-3804 | ai:codex
    #[test]
    fn task_3804_accepted_notice_read_and_parse_failures_block_resend() {
        for canonical in [false, true] {
            for unreadable in [false, true] {
                check_accepted_notice_reconciliation(canonical, unreadable);
            }
        }
    }

    fn check_accepted_notice_reconciliation(canonical: bool, unreadable: bool) {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path();
        let key = reserved(main, &"f".repeat(40));
        let delivery = refusal_delivery_id(&key);
        let accepted = aida_core::mailbox::Message {
            subject: Some(format!("Conflict repair refused for PR #{}", key.pr)),
            id: "accepted-refusal".into(),
            thread_id: delivery.clone(),
            from: "host".into(),
            to: aida_core::mailbox::Recipient::Agent("advisor".into()),
            timestamp: 1,
            in_reply_to: None,
            body: "refused".into(),
            urgent: false,
            intent: aida_core::mailbox::Intent::Request,
            retracted: false,
            deleted: false,
            archived: false,
            from_source: aida_core::mailbox::SenderSource::Explicit,
            from_role: None,
            from_seat: None,
            relayed_from: None,
        };
        crate::mailbox_store::write_message(main, &accepted).unwrap();
        let mut file = crate::mailbox_store::mailbox_dir(main).join("accepted-refusal.json");
        if canonical {
            let dir = crate::mailbox_store::canonical_dir(&main.join(".aida-store"));
            std::fs::create_dir_all(&dir).unwrap();
            let canonical_file = dir.join(file.file_name().unwrap());
            std::fs::rename(&file, &canonical_file).unwrap();
            file = canonical_file;
        }
        notify_refusal(main, &key, "refused").unwrap();
        {
            let mut guard = RepairGuard::lock(main).unwrap();
            guard.entry_mut(&key).unwrap().refusal_notice = None;
            guard.save().unwrap();
        }
        let intact = std::fs::read(&file).unwrap();
        if unreadable {
            // A directory in place of the message fails even under a privileged test user.
            std::fs::remove_file(&file).unwrap();
            std::fs::create_dir(&file).unwrap();
        } else {
            std::fs::write(&file, b"{\"id\": \"accepted-refusal\", \"thread_id\": ").unwrap();
        }
        for _ in 0..2 {
            let error = notify_refusal_with(
                main,
                &key,
                "refused",
                |d| mailbox_has_delivery(main, d),
                |_, _, _| panic!("an unproven delivery state must not send"),
            )
            .unwrap_err();
            let detail = format!("{error:#}");
            assert!(detail.contains("delivery state is unknown"), "{detail}");
            assert!(detail.contains(&file.display().to_string()), "{detail}");
            if unreadable {
                assert!(detail.contains("reading message"), "{detail}");
            } else {
                assert!(detail.contains("is not valid"), "{detail}");
            }
        }
        let guard = RepairGuard::lock(main).unwrap();
        let notice = guard.journal.entries[0].refusal_notice.as_ref().unwrap();
        assert!(notice.accepted_delivery.is_none());
        assert!(notice.delivery_unknown.is_some());
        drop(guard);

        if unreadable {
            std::fs::remove_dir(&file).unwrap();
        }
        std::fs::write(&file, intact).unwrap();
        notify_refusal(main, &key, "refused").unwrap();
        let guard = RepairGuard::lock(main).unwrap();
        let notice = guard.journal.entries[0].refusal_notice.as_ref().unwrap();
        assert_eq!(
            notice.accepted_delivery.as_deref(),
            Some(format!("adopted from mailbox thread {delivery}").as_str())
        );
        assert!(notice.delivery_unknown.is_none());
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
            snapshot_deferred: None,
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
            .record_regenerated_tree(&key, &"c".repeat(40), None)
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
