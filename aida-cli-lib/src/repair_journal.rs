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
    // Set when the mailbox could not be read to prove whether the notice was
    // already accepted; nothing is sent until an operator resolves it.
    // trace:TASK-3804 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_unknown: Option<String>,
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
        let dir = main.join(".aida");
        std::fs::create_dir_all(&dir)?;
        anyhow::ensure!(
            !std::fs::symlink_metadata(&dir)?.file_type().is_symlink(),
            "repair journal directory is a symlink"
        );
        let path = dir.join("repair-journal.json");
        let lock = lock_file(&dir.join("repair-journal.lock"))?;
        let journal = load_journal(&path, main)?;
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
            // Name the holder and the one command that releases a dead one.
            // trace:BUG-2688 | ai:claude
            anyhow::ensure!(
                previous.spawn == SpawnOutcome::ProvenDead,
                "repair head already reserved by lane {} for seat {}; unknown spawn outcomes \
                 require proof that the prior lane is dead. If that lane has ended and its seat \
                 is gone, run `aida doctor heal {HEAL_CATEGORY} --yes` to release the \
                 reservation, then retry",
                previous.lane_id,
                previous.tree_path.display()
            );
            entry.refusal_notice = previous.refusal_notice.clone();
        }
        self.journal.entries.retain(|e| e.key != entry.key);
        self.journal.entries.push(entry);
        self.save()
    }

    // `command` is the trusted project policy that ran, or empty when none was
    // configured (a refusal). trace:TASK-3804 | ai:claude
    pub fn record_validation(
        &mut self,
        key: &RepairKey,
        tree: &str,
        command: &[String],
        success: bool,
    ) -> Result<()> {
        let entry = self.entry_mut(key)?;
        entry.require_regenerated_tree(tree)?;
        entry.validation = Some(Validation {
            tree: tree.into(),
            command: command.to_vec(),
            success,
            recorded_at: chrono::Utc::now().to_rfc3339(),
        });
        self.save()
    }
    /// Every merge `[tree, first parent, second parent]` that matches a
    /// journaled repair on `branch` needs successful validation of that tree.
    // trace:TASK-3804 | ai:claude
    pub fn require_validated_merges(&self, branch: &str, merges: &[[String; 3]]) -> Result<()> {
        for [tree, first, second] in merges {
            for entry in self.journal.entries.iter().filter(|entry| {
                entry.branch == branch && &entry.key.original_head == first && &entry.base == second
            }) {
                let validated = entry
                    .validation
                    .as_ref()
                    .is_some_and(|validation| validation.success && &validation.tree == tree);
                if !validated {
                    return Err(UnvalidatedRepairPublication {
                        pr: entry.key.pr,
                        route: "seat-committed conflict merge".into(),
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
    /// A launch that was refused or failed before any worker spawned, and
    /// whose setup was rolled back, proves its own lane dead: release the
    /// reservations that lane made, so the next attempt can reserve the head.
    /// Only an `unknown` entry of exactly this lane whose seat is gone is
    /// released; an adopted seat's entry went back to its orphan lease first.
    // trace:BUG-2688 | ai:claude
    pub fn release_unspawned(&mut self, lane_id: &str) -> Result<usize> {
        let mut released = 0;
        for entry in &mut self.journal.entries {
            if entry.lane_id == lane_id
                && entry.spawn == SpawnOutcome::Unknown
                && path_is_absent(&entry.tree_path)
            {
                entry.spawn = SpawnOutcome::ProvenDead;
                released += 1;
            }
        }
        if released > 0 {
            self.save()?;
        }
        Ok(released)
    }
    #[cfg(test)]
    pub(crate) fn entries(&self) -> &[RepairEntry] {
        &self.journal.entries
    }
    fn save(&self) -> Result<()> {
        aida_core::fs_atomic::write_atomic(&self.path, serde_json::to_vec_pretty(&self.journal)?)?;
        Ok(())
    }
}

/// `aida doctor` category for reservations whose lane is provably gone.
// trace:BUG-2688 | ai:claude
pub(crate) const HEAL_CATEGORY: &str = "repair-journal";

// Only a definite "no such path" counts; a permission or I/O error is unknown.
// trace:BUG-2688 | ai:claude
fn path_is_absent(path: &Path) -> bool {
    matches!(
        std::fs::symlink_metadata(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    )
}

/// Release the reservations of a launch that was rolled back before any
/// worker spawned. Best effort: a failure is reported with the command that
/// releases the reservation later, and never masks the launch's own error.
// trace:BUG-2688 | ai:claude
pub(crate) fn release_unspawned_lane(main: &Path, lane_id: &str) {
    if path_is_absent(&main.join(".aida/repair-journal.json")) {
        return;
    }
    match RepairGuard::lock(main).and_then(|mut journal| journal.release_unspawned(lane_id)) {
        Ok(0) => {}
        Ok(released) => eprintln!(
            "  released {released} conflict-repair reservation(s) of the rolled-back launch"
        ),
        Err(error) => eprintln!(
            "  could not release the rolled-back launch's conflict-repair reservation: \
             {error:#}; run `aida doctor heal {HEAL_CATEGORY} --yes` before retrying"
        ),
    }
}

/// The doctor finding id of one reservation: its PR and original head.
// trace:BUG-2688 | ai:claude
pub(crate) fn reservation_id(key: &RepairKey) -> String {
    format!("pr-{}-{}", key.pr, key.original_head)
}

/// Why a live reservation cannot be released, or `None` when its lane is
/// provably gone: no lease names the lane or its seat, no lane run for it is
/// alive, and the seat tree no longer exists. Any evidence that cannot be
/// read keeps the reservation.
// trace:BUG-2688 | ai:claude
fn reservation_holder(
    main: &Path,
    entry: &RepairEntry,
    leases: &[crate::leases::SessionLease],
) -> Option<String> {
    if let Some(lease) = leases
        .iter()
        .find(|lease| lease.id == entry.lane_id || lease.worktree_path == entry.tree_path)
    {
        return Some(format!(
            "lease {} still holds the lane or its seat",
            lease.id
        ));
    }
    match crate::lane_liveness::headless_lane_run_alive(main, &entry.lane_id) {
        Ok(false) => {}
        Ok(true) => return Some(format!("lane run for {} is still alive", entry.lane_id)),
        Err(error) => return Some(format!("lane run state is unknown: {error:#}")),
    }
    if !path_is_absent(&entry.tree_path) {
        return Some(format!(
            "seat {} still exists or cannot be inspected",
            entry.tree_path.display()
        ));
    }
    None
}

/// Live reservations whose lane is provably gone, for `aida doctor`. A
/// project with no journal has none. The scan takes no lock and creates
/// nothing, so it also runs where the host's `.aida` is read-only.
// trace:BUG-2688 | ai:claude
pub(crate) fn dead_lane_reservations(main: &Path) -> Result<Vec<RepairEntry>> {
    let journal_path = main.join(".aida/repair-journal.json");
    if path_is_absent(&journal_path) {
        return Ok(Vec::new());
    }
    let journal = load_journal(&journal_path, main)?;
    let live: Vec<_> = journal
        .entries
        .into_iter()
        .filter(|entry| entry.spawn != SpawnOutcome::ProvenDead)
        .collect();
    // With no live reservation there is no question for the leases to answer.
    if live.is_empty() {
        return Ok(live);
    }
    let leases = crate::leases::list_leases_strict_all(main).map_err(anyhow::Error::msg)?;
    Ok(live
        .into_iter()
        .filter(|entry| reservation_holder(main, entry, &leases).is_none())
        .collect())
}

/// Release one reservation found by [`dead_lane_reservations`]. The proof is
/// re-read under the journal lock, and the journal is copied to a backup file
/// beside it before the entry is marked `proven_dead`. `Ok(Err(why))` is a
/// reservation that is kept; `Ok(Ok(backup))` names the backup written.
// trace:BUG-2688 | ai:claude
pub(crate) fn heal_dead_lane_reservation(
    main: &Path,
    id: &str,
) -> Result<std::result::Result<PathBuf, String>> {
    let leases = crate::leases::list_leases_strict_all(main).map_err(anyhow::Error::msg)?;
    let mut guard = RepairGuard::lock(main)?;
    let Some(index) = guard
        .journal
        .entries
        .iter()
        .position(|entry| reservation_id(&entry.key) == id)
    else {
        return Ok(Err("reservation is no longer in the repair journal".into()));
    };
    let entry = &guard.journal.entries[index];
    if entry.spawn == SpawnOutcome::ProvenDead {
        return Ok(Err("reservation is already released".into()));
    }
    if let Some(holder) = reservation_holder(main, entry, &leases) {
        return Ok(Err(holder));
    }
    let backup = guard.path.with_file_name(format!(
        "repair-journal.backup-{}.json",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ")
    ));
    let current = std::fs::read(&guard.path).context("cannot read repair journal for backup")?;
    aida_core::fs_atomic::write_atomic(&backup, current)
        .context("cannot back up repair journal; reservation kept")?;
    guard.journal.entries[index].spawn = SpawnOutcome::ProvenDead;
    guard.save()?;
    Ok(Ok(backup))
}

/// `aida doctor` findings: one safe heal per reservation whose lane is
/// provably gone. A journal or lease store that cannot be read is reported,
/// never healed.
// trace:BUG-2688 | ai:claude
pub(crate) fn scan(main: &Path) -> Vec<crate::DoctorFinding> {
    match dead_lane_reservations(main) {
        Ok(entries) => entries
            .iter()
            .map(|entry| crate::DoctorFinding {
                category: HEAL_CATEGORY.to_string(),
                id: reservation_id(&entry.key),
                summary: format!(
                    "conflict-repair reservation for PR #{} is held by lane {}, which has no \
                     lease, no live lane run and no seat at {}; it blocks the next repair of \
                     this PR head",
                    entry.key.pr,
                    entry.lane_id,
                    entry.tree_path.display()
                ),
                action: format!(
                    "run `aida doctor heal {HEAL_CATEGORY} --yes` to release it (the journal \
                     is backed up first)"
                ),
                safe_heal: true,
            })
            .collect(),
        Err(error) => vec![crate::DoctorFinding {
            category: HEAL_CATEGORY.to_string(),
            id: "unreadable".to_string(),
            summary: format!("conflict-repair reservations could not be checked: {error:#}"),
            action: "inspect `.aida/repair-journal.json` and `aida session leases --all`; \
                     nothing is released while this evidence is unreadable"
                .to_string(),
            safe_heal: false,
        }],
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

// Bounded, validated read of the journal file. Writers replace the file
// atomically, so a reader that only reports (the doctor scan) may call this
// without the sidecar lock; every mutation re-reads under it.
// trace:STORY-2710 | ai:codex
// trace:BUG-2688 | ai:claude
fn load_journal(path: &Path, main: &Path) -> Result<Journal> {
    use std::os::unix::fs::OpenOptionsExt;
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

    fn unknown_entry(main: &Path, pr: u64, lane: &str, tree: &Path) -> RepairEntry {
        RepairEntry {
            key: RepairKey {
                repository: main.into(),
                pr,
                original_head: "a".repeat(40),
            },
            dispatched_by: "direct-rework".into(),
            reserved_at: "then".into(),
            lane_id: lane.into(),
            tree_path: tree.into(),
            branch: "pr".into(),
            base: "b".repeat(40),
            spawn: SpawnOutcome::Unknown,
            original_conflict_paths: vec!["src/lib.rs".into()],
            regeneration_success: None,
            regenerated_tree: None,
            preparation_success: None,
            validation: None,
            refusal_notice: None,
        }
    }

    fn spawn_of(main: &Path, pr: u64) -> SpawnOutcome {
        let guard = RepairGuard::lock(main).unwrap();
        let entry = guard.entries().iter().find(|e| e.key.pr == pr).unwrap();
        entry.spawn.clone()
    }

    // A rolled-back launch releases only its own unknown reservation, and only
    // once its seat is gone. trace:BUG-2688 | ai:claude
    #[test]
    fn bug_2688_unspawned_release_is_limited_to_the_lane_and_a_gone_seat() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path();
        let kept_seat = main.join("kept-seat");
        std::fs::create_dir(&kept_seat).unwrap();
        let mut started = unknown_entry(main, 3, "lane", &main.join("gone-3"));
        started.spawn = SpawnOutcome::Started;
        let mut guard = RepairGuard::lock(main).unwrap();
        for entry in [
            unknown_entry(main, 1, "lane", &main.join("gone-1")),
            unknown_entry(main, 2, "lane", &kept_seat),
            started,
            unknown_entry(main, 4, "other-lane", &main.join("gone-4")),
        ] {
            guard.reserve(entry).unwrap();
        }
        drop(guard);
        release_unspawned_lane(main, "lane");
        assert_eq!(spawn_of(main, 1), SpawnOutcome::ProvenDead);
        assert_eq!(
            spawn_of(main, 2),
            SpawnOutcome::Unknown,
            "seat still exists"
        );
        assert_eq!(spawn_of(main, 3), SpawnOutcome::Started, "a worker spawned");
        assert_eq!(spawn_of(main, 4), SpawnOutcome::Unknown, "another lane");
        // The released head can be reserved again; the others still refuse,
        // naming the holder and the command that releases a dead one.
        let mut guard = RepairGuard::lock(main).unwrap();
        guard
            .reserve(unknown_entry(main, 1, "next", &main.join("next-seat")))
            .unwrap();
        let refusal = guard
            .reserve(unknown_entry(main, 4, "next", &main.join("next-seat")))
            .unwrap_err()
            .to_string();
        assert!(refusal.contains("other-lane"), "{refusal}");
        assert!(
            refusal.contains("aida doctor heal repair-journal --yes"),
            "{refusal}"
        );
        // No journal: nothing is created by the release.
        let empty = tempfile::tempdir().unwrap();
        release_unspawned_lane(empty.path(), "lane");
        assert!(!empty.path().join(".aida").exists());
    }

    fn write_lease(main: &Path, id: &str, tree: &Path) {
        let path = crate::lease_path(main, id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!(
                "id = {id:?}\nscope = \"BUG-2688\"\nslug = \"bug-2688\"\nowner = \"test\"\n\
                 worktree_path = {}\nbranch = \"pr\"\nstarted_at = \"2026-11-20T00:00:00Z\"\n\
                 hostname = \"h\"\nrole = \"implementer\"\n",
                aida_core::toml_quote::toml_string(&tree.display().to_string()),
            ),
        )
        .unwrap();
    }

    fn write_lane_run(main: &Path, lane: &str, body: &str) -> PathBuf {
        let path = main.join(format!(".aida/lanes/headless-{lane}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    fn lane_run(lane: &str, pid: u32) -> String {
        serde_json::json!({
            "lease_id": lane,
            "spec": "BUG-2688",
            "vendor": "codex",
            "pid": pid,
            "log_path": "/nonexistent/log",
            "started_at": "2026-11-20T00:00:00Z",
            "exited_at": "2026-11-20T00:01:00Z",
        })
        .to_string()
    }

    // The heal releases a reservation only when the lease, the lane run and
    // the seat are all provably gone, and backs the journal up first. Every
    // remaining or unreadable piece of evidence keeps the reservation.
    // trace:BUG-2688 | ai:claude
    #[test]
    fn bug_2688_heal_releases_only_a_provably_gone_lane_and_backs_up_the_journal() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path();
        // No journal: nothing to report and nothing created.
        assert!(scan(main).is_empty());
        assert!(!main.join(".aida").exists());

        let seat = main.join("seat");
        let entry = unknown_entry(main, 4365, "lane-1", &seat);
        let id = reservation_id(&entry.key);
        RepairGuard::lock(main).unwrap().reserve(entry).unwrap();
        let kept = |why: &str| {
            assert!(scan(main).is_empty(), "{why}: must not be reported");
            let outcome = heal_dead_lane_reservation(main, &id).unwrap();
            assert!(outcome.is_err(), "{why}: must be kept");
            assert_eq!(spawn_of(main, 4365), SpawnOutcome::Unknown, "{why}");
            outcome.unwrap_err()
        };

        // 1. The lane's lease still exists.
        write_lease(main, "lane-1", &main.join("elsewhere"));
        assert!(kept("lane lease").contains("lane-1"));
        std::fs::remove_file(crate::lease_path(main, "lane-1")).unwrap();
        // 2. Another lease holds the seat path.
        write_lease(main, "successor", &seat);
        assert!(kept("seat lease").contains("successor"));
        // 3. A lease file that cannot be parsed: ownership is unknown.
        std::fs::write(crate::lease_path(main, "successor"), "id = ").unwrap();
        let unreadable = scan(main);
        assert_eq!(unreadable.len(), 1);
        assert!(!unreadable[0].safe_heal, "{unreadable:?}");
        assert!(heal_dead_lane_reservation(main, &id).is_err());
        assert_eq!(spawn_of(main, 4365), SpawnOutcome::Unknown);
        std::fs::remove_file(crate::lease_path(main, "successor")).unwrap();
        // 4. The seat tree still exists.
        std::fs::create_dir(&seat).unwrap();
        assert!(kept("seat exists").contains("still exists"));
        std::fs::remove_dir(&seat).unwrap();
        // 5. The lane run is alive despite its exit stamp, or unreadable.
        let record = write_lane_run(main, "lane-1", &lane_run("lane-1", std::process::id()));
        assert!(kept("live lane run").contains("still alive"));
        std::fs::write(&record, "{\"lease_id\": ").unwrap();
        assert!(kept("torn lane record").contains("unknown"));

        // 6. A finished lane run, no lease, no seat: reported and healed.
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let dead_pid = child.id();
        child.wait().unwrap();
        std::fs::write(&record, lane_run("lane-1", dead_pid)).unwrap();
        let findings = scan(main);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].id, id);
        assert_eq!(findings[0].category, HEAL_CATEGORY);
        assert!(findings[0].safe_heal);
        let before = std::fs::read(main.join(".aida/repair-journal.json")).unwrap();
        let backup = heal_dead_lane_reservation(main, &id).unwrap().unwrap();
        assert_eq!(std::fs::read(&backup).unwrap(), before);
        assert_eq!(backup.parent(), Some(main.join(".aida").as_path()));
        assert_eq!(spawn_of(main, 4365), SpawnOutcome::ProvenDead);
        // Healed once: nothing more to report, a repeat is a no-op, and the
        // head can be reserved by the next lane.
        assert!(scan(main).is_empty());
        assert!(heal_dead_lane_reservation(main, &id).unwrap().is_err());
        assert!(heal_dead_lane_reservation(main, "pr-1-missing")
            .unwrap()
            .is_err());
        write_lease(main, "lane-2", &main.join("seat-2"));
        RepairGuard::lock(main)
            .unwrap()
            .reserve(unknown_entry(main, 4365, "lane-2", &main.join("seat-2")))
            .unwrap();
        // A lane id that is not a lease id never reaches a file lookup.
        RepairGuard::lock(main)
            .unwrap()
            .reserve(unknown_entry(main, 9, "../escape", &main.join("gone")))
            .unwrap();
        assert!(scan(main).is_empty());
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
