//! Post-flush explicit ACK and the private receipt anchor (STORY-1843 slice
//! B2c).
//!
//! An explicit read acknowledges only what it actually delivered. A reader
//! first [`AckPlan::prepare`]s a plan from the snapshot it is about to show:
//! the selected record IDs, the recipient keys each is addressed to, and the
//! exact replicas (source, file fingerprint, directory identity and
//! lifecycle generation) it was read from. Preparing writes nothing. The
//! reader then serializes and flushes its output, and only after a
//! successful flush calls [`AckPlan::commit`]. Output that fails, or a
//! process killed mid-print, never commits: a possible duplicate read beats
//! silent loss. This is the explicit successor to BUG-1482's
//! write-before-print marking (signoff 60 #3).
//!
//! Commit order is Lifecycle then Receipt (63 #6). Authority is re-resolved
//! by the caller before any lock; under the locks only read-only validation
//! runs: each holding source is still `Open` at the planned generation, each
//! replica still has the planned bytes, and every key's ledger has room.
//! A refusal there writes nothing. An arrival after the plan was prepared is
//! never added to it.
//!
//! The anchor is `<common-git-dir>/aida/mail-lifecycle/receipts/`, one
//! ledger per recipient key ([`crate::mailbox_logical::ReceiptLedger`]),
//! each replaced by its own atomic, durable rename. Publication is
//! idempotent and convergent, not one atomic step (advisor ruling on signoff
//! 60 #5): a write that fails after some keys' ledgers were renamed into
//! place leaves those keys acknowledged and the others not, and the commit
//! reports [`AckError::Incomplete`]. A message counts as acknowledged in the
//! anchor only when every recipient key it matched holds its receipt, so
//! the next read shows it again and plans exactly the keys still missing it.
//! Existing per-ID receipts in the sources stay as inputs; nothing is
//! imported, pruned or migrated.
//!
//! trace:STORY-1843 | ai:claude

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::mailbox_logical::{
    read_receipt_ledger, receipt_keys, receipt_ledger_name, replica_fingerprint, LogicalSnapshot,
    ReadEvidence, ReceiptLedger, ResolvedAudience, SnapshotRecord, SourceKind, RECEIPT_LEDGER_CAP,
    RECEIPT_LEDGER_MAX_ENTRIES, RECEIPT_LEDGER_SCHEMA,
};

use super::{
    check_regular, ensure_anchor, ensure_dir, load_record, lock, read_json, record_path,
    resolve_source, write_durable, LifecycleError, LifecycleGuard, SourceRecord, SourceState,
    LIFECYCLE_LOCK_TIMEOUT,
};

/// Directory under the lifecycle anchor that holds the ledgers.
pub(crate) const RECEIPTS_DIR: &str = "receipts";
const RECEIPT_LOCK_FILE: &str = "receipts.lock";

/// How long the Receipt lock is tried before the commit refuses.
pub const RECEIPT_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
/// Most IDs one plan may acknowledge.
pub const MAX_PLAN_IDS: usize = 4096;
/// Most recipient keys one plan may touch.
pub const MAX_PLAN_KEYS: usize = 64;

/// Why no ACK was planned or committed. The messages are safe to show: they
/// never carry message IDs, bodies, recipients or private paths.
#[derive(Debug)]
pub enum AckError {
    /// The view was partial; a partial view never advances an ACK.
    Partial,
    /// More IDs or keys than one plan may carry.
    TooLarge,
    /// A planned key is no longer one of the reader's inboxes.
    Authority,
    /// A holding source is being retired, or was retired, since the plan.
    Retiring,
    /// A replica or source changed since the plan (a tombstone, an edit, a
    /// replaced directory).
    Changed,
    /// A key's ledger has no room; nothing was written or pruned.
    LedgerFull,
    /// The lifecycle or receipt lock, or the anchor, refused.
    Lifecycle(LifecycleError),
    /// Writing a ledger failed after at least one other key's ledger was
    /// already published: those keys hold the receipts, the rest do not.
    /// The next read shows the mail again and plans only the missing keys.
    Incomplete(LifecycleError),
}

impl std::fmt::Display for AckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AckError::Partial => f.write_str("the view was partial, so nothing was acknowledged"),
            AckError::TooLarge => write!(
                f,
                "more than {MAX_PLAN_IDS} messages or {MAX_PLAN_KEYS} inboxes to acknowledge at once"
            ),
            AckError::Authority => {
                f.write_str("an inbox in the read is no longer this session's")
            }
            AckError::Retiring => f.write_str("a worktree holding the mail is being retired"),
            AckError::Changed => f.write_str("the mail changed after it was shown"),
            AckError::LedgerFull => f.write_str("the receipt ledger is full"),
            AckError::Lifecycle(LifecycleError::LockTimeout { .. }) => {
                f.write_str("the mailbox lifecycle lock stayed busy")
            }
            AckError::Lifecycle(LifecycleError::LockOrder(_)) => {
                f.write_str("the mailbox lock order refused the acknowledgement")
            }
            AckError::Lifecycle(LifecycleError::Unsafe { .. })
            | AckError::Lifecycle(LifecycleError::Corrupt { .. }) => {
                f.write_str("the mailbox lifecycle anchor is not usable")
            }
            AckError::Lifecycle(_) => f.write_str("the mailbox lifecycle anchor refused"),
            AckError::Incomplete(_) => f.write_str(
                "the read was recorded for only some of its inboxes; the next read shows that mail again and records the rest",
            ),
        }
    }
}

impl std::error::Error for AckError {}

impl From<LifecycleError> for AckError {
    fn from(e: LifecycleError) -> Self {
        AckError::Lifecycle(e)
    }
}

/// One source a planned record was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedSource {
    root: PathBuf,
    canonical: bool,
    dev: u64,
    ino: u64,
    /// The source's lifecycle key and the record seen at prepare (`None`:
    /// no record yet). Canonical and local-only sources have no lifecycle.
    lifecycle: Option<(String, Option<u64>)>,
}

impl PlannedSource {
    fn components(&self) -> &'static [&'static str] {
        if self.canonical {
            &["mailbox"]
        } else {
            &[".aida", "mailbox"]
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedAck {
    id: String,
    /// Shown as covered only by a legacy watermark: the ACK resolves it,
    /// with no read time claimed.
    legacy: bool,
    keys: Vec<String>,
    /// (index into `sources`, fingerprint) for every replica read.
    replicas: Vec<(usize, String)>,
}

/// A prepared, not yet committed acknowledgement of exactly the records a
/// reader is about to deliver. Owned by one response; never shared.
#[must_use = "a prepared ACK does nothing until it is committed after output is flushed"]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AckPlan {
    /// The verified common Git directory (`None`: a local-only project,
    /// which has no anchor; its ACK is the caller's local receipt alone).
    common: Option<PathBuf>,
    sources: Vec<PlannedSource>,
    entries: Vec<PlannedAck>,
}

/// What a committed ACK recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AckCommitted {
    /// Message IDs per recipient key, for the caller's local receipt mirror.
    pub by_key: BTreeMap<String, Vec<String>>,
    /// IDs whose ACK resolved a legacy watermark: acknowledged, with no
    /// read time (a local timed receipt must not be written for them).
    pub legacy: std::collections::BTreeSet<String>,
    /// Whether the receipts went to the common-Git anchor (`false` only for
    /// a local-only project).
    pub anchored: bool,
}

impl AckCommitted {
    /// How many messages were acknowledged.
    pub fn messages(&self) -> usize {
        let mut ids: Vec<&String> = self.by_key.values().flatten().collect();
        ids.sort();
        ids.dedup();
        ids.len()
    }
}

impl AckPlan {
    /// Plan the ACK of `selected`, records of `snapshot` that the reader is
    /// about to deliver in full. Only unread records and records covered by
    /// a legacy watermark alone are planned (an explicit ACK resolves the
    /// latter, signoff 60 #2); archived and retracted records are not.
    /// Planning is per recipient key: a key whose anchor ledger already holds
    /// a record's receipt (left by an earlier, partly published commit) is
    /// not planned again, so a retry plans exactly the keys still missing it.
    /// `Ok(None)` when nothing needs acknowledging. Writes nothing; runs Git
    /// (to name each source's lifecycle record), reads the planned keys'
    /// ledgers and takes no lock.
    // trace:STORY-1843 | ai:claude
    pub fn prepare(
        snapshot: &LogicalSnapshot,
        common: Option<&Path>,
        audience: &ResolvedAudience,
        selected: &[&SnapshotRecord],
    ) -> Result<Option<Self>, AckError> {
        let wanted: Vec<&SnapshotRecord> = selected
            .iter()
            .copied()
            .filter(|r| {
                !r.message.archived
                    && !r.message.retracted
                    && matches!(
                        r.read,
                        ReadEvidence::Unread | ReadEvidence::LegacyWatermark { .. }
                    )
            })
            .collect();
        if wanted.is_empty() {
            return Ok(None);
        }
        if !snapshot.ack_eligible() {
            return Err(AckError::Partial);
        }
        if wanted.len() > MAX_PLAN_IDS {
            return Err(AckError::TooLarge);
        }
        let mut plan = AckPlan {
            common: common.map(Path::to_path_buf),
            sources: Vec::new(),
            entries: Vec::new(),
        };
        let mut by_snapshot_index: BTreeMap<usize, usize> = BTreeMap::new();
        let mut keys_seen: std::collections::BTreeSet<String> = Default::default();
        let mut ledgers: BTreeMap<String, Option<ReceiptLedger>> = BTreeMap::new();
        for record in wanted {
            // A cohort delivery is acknowledged under the reader's own UUID
            // key only (slice D1). trace:STORY-1843 | ai:claude
            let mut keys = receipt_keys(audience, record);
            keys_seen.extend(keys.iter().cloned());
            if keys_seen.len() > MAX_PLAN_KEYS {
                return Err(AckError::TooLarge);
            }
            if let Some(common) = common {
                let mut missing = Vec::with_capacity(keys.len());
                for key in keys {
                    if !ledgers.contains_key(&key) {
                        let ledger =
                            read_receipt_ledger(common, &key).map_err(|_| AckError::Partial)?;
                        ledgers.insert(key.clone(), ledger);
                    }
                    let held = ledgers[&key]
                        .as_ref()
                        .is_some_and(|l| l.receipts.contains_key(&record.message.id));
                    if !held {
                        missing.push(key);
                    }
                }
                keys = missing;
            }
            if keys.is_empty() {
                continue;
            }
            let mut replicas = Vec::new();
            for provenance in &record.provenance {
                let index = match by_snapshot_index.get(&provenance.source) {
                    Some(index) => *index,
                    None => {
                        let source = plan.planned_source(snapshot, provenance.source)?;
                        plan.sources.push(source);
                        by_snapshot_index.insert(provenance.source, plan.sources.len() - 1);
                        plan.sources.len() - 1
                    }
                };
                replicas.push((index, provenance.fingerprint.clone()));
            }
            if replicas.is_empty() {
                return Err(AckError::Changed);
            }
            plan.entries.push(PlannedAck {
                id: record.message.id.clone(),
                legacy: matches!(record.read, ReadEvidence::LegacyWatermark { .. }),
                keys,
                replicas,
            });
        }
        Ok((!plan.entries.is_empty()).then_some(plan))
    }

    fn planned_source(
        &self,
        snapshot: &LogicalSnapshot,
        index: usize,
    ) -> Result<PlannedSource, AckError> {
        let report = snapshot.sources.get(index).ok_or(AckError::Changed)?;
        let identity = report.identity.ok_or(AckError::Partial)?;
        let (root, canonical) = match &report.kind {
            SourceKind::Local(root) => (root.clone(), false),
            SourceKind::Canonical(root) => (root.clone(), true),
        };
        let lifecycle = if canonical || self.common.is_none() {
            None
        } else {
            match resolve_source(&root)? {
                None => None,
                Some(source) => {
                    if (source.dev, source.ino) != (identity.dev, identity.ino) {
                        return Err(AckError::Changed);
                    }
                    // Read without the lock: records are replaced by rename,
                    // and the commit revalidates under Lifecycle.
                    let record =
                        read_json::<SourceRecord>(&record_path(&source.anchor, &source.key))?;
                    let generation = match record {
                        None => None,
                        Some(r) if r.state == SourceState::Open => Some(r.generation),
                        Some(_) => return Err(AckError::Retiring),
                    };
                    Some((source.key, generation))
                }
            }
        };
        Ok(PlannedSource {
            root,
            canonical,
            dev: identity.dev,
            ino: identity.ino,
            lifecycle,
        })
    }

    /// The planned message IDs per recipient key.
    pub fn by_key(&self) -> BTreeMap<String, Vec<String>> {
        let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for entry in &self.entries {
            for key in &entry.keys {
                out.entry(key.clone()).or_default().push(entry.id.clone());
            }
        }
        out
    }

    /// How many messages the plan would acknowledge.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Commit the plan, after the reader's output was flushed. `current` is
    /// the reader's audience re-resolved after the flush and before any
    /// lock; every planned key must still be in it. Lifecycle then Receipt.
    /// A refusal before publication (authority, a retiring source, a changed
    /// replica, a full ledger, a busy lock) writes nothing. Each key's ledger
    /// is then published by its own rename; a failure after the first one is
    /// [`AckError::Incomplete`], and the next read converges the rest.
    // trace:STORY-1843 | ai:claude
    pub fn commit(self, current: &ResolvedAudience) -> Result<AckCommitted, AckError> {
        let by_key = self.by_key();
        let legacy: std::collections::BTreeSet<String> = self
            .entries
            .iter()
            .filter(|e| e.legacy)
            .map(|e| e.id.clone())
            .collect();
        if by_key.keys().any(|key| !current.holds_key(key)) {
            return Err(AckError::Authority);
        }
        let Some(common) = &self.common else {
            return Ok(AckCommitted {
                by_key,
                legacy,
                anchored: false,
            });
        };
        // Facts before locks: the anchor and its receipts directory exist.
        let anchor = ensure_anchor(common)?;
        ensure_dir(&anchor.join(RECEIPTS_DIR), true)?;

        let held = lock(&anchor, LIFECYCLE_LOCK_TIMEOUT)?;
        self.validate_sources(&anchor, &held)?;
        self.validate_replicas()?;
        let receipts = receipt_lock(&anchor, &held, RECEIPT_LOCK_TIMEOUT)?;
        publish(&anchor, &by_key, &legacy, &receipts)?;
        drop(receipts);
        drop(held);
        Ok(AckCommitted {
            by_key,
            legacy,
            anchored: true,
        })
    }

    /// Every lifecycle-tracked holding source is still `Open` at the planned
    /// generation (or, if it had no record, gained only an `Open` one for
    /// the same directory). Lifecycle held; read-only.
    fn validate_sources(&self, anchor: &Path, held: &LifecycleGuard) -> Result<(), AckError> {
        for source in &self.sources {
            let Some((key, planned)) = &source.lifecycle else {
                continue;
            };
            match (planned, load_record(anchor, key, held)?) {
                (_, Some(r)) if r.state != SourceState::Open => return Err(AckError::Retiring),
                (Some(generation), Some(r)) if r.generation == *generation => {}
                (None, Some(r)) if (r.dev, r.ino) == (source.dev, source.ino) => {}
                (None, None) => {}
                _ => return Err(AckError::Changed),
            }
        }
        Ok(())
    }

    /// Every planned replica still holds the planned bytes in the planned
    /// directory. Read-only, no-follow, bounded.
    fn validate_replicas(&self) -> Result<(), AckError> {
        for entry in &self.entries {
            for (index, planned) in &entry.replicas {
                let source = &self.sources[*index];
                match replica_fingerprint(&source.root, source.components(), &entry.id) {
                    Ok(Some((fp, dev, ino)))
                        if &fp == planned && (dev, ino) == (source.dev, source.ino) => {}
                    _ => return Err(AckError::Changed),
                }
            }
        }
        Ok(())
    }
}

/// Write every key's ledger. Each key's room is checked before anything is
/// written, so a full ledger refuses with nothing published; each ledger is
/// then replaced by its own atomic durable rename, and an ID already
/// recorded keeps its first ACK time (idempotent). The renames are not one
/// atomic step: a write that fails after an earlier key's rename returns
/// [`AckError::Incomplete`] with that key's receipts in place.
fn publish(
    anchor: &Path,
    by_key: &BTreeMap<String, Vec<String>>,
    legacy: &std::collections::BTreeSet<String>,
    _held: &ReceiptGuard,
) -> Result<(), AckError> {
    let dir = anchor.join(RECEIPTS_DIR);
    let now = chrono::Utc::now().timestamp_millis();
    let mut updates = Vec::new();
    for (key, ids) in by_key {
        let path = dir.join(receipt_ledger_name(key));
        let mut ledger = match super::read_checked_capped(&path, RECEIPT_LEDGER_CAP)? {
            None => ReceiptLedger {
                schema: RECEIPT_LEDGER_SCHEMA,
                key: key.clone(),
                receipts: BTreeMap::new(),
                legacy: Default::default(),
            },
            Some(bytes) => ReceiptLedger::parse(&bytes, key).map_err(|_| {
                AckError::Lifecycle(LifecycleError::Corrupt {
                    path: path.clone(),
                    why: "the receipt ledger does not parse".into(),
                })
            })?,
        };
        let before = ledger.receipts.len();
        for id in ids {
            if !ledger.receipts.contains_key(id) {
                ledger.receipts.insert(id.clone(), now);
                if legacy.contains(id) {
                    ledger.legacy.insert(id.clone());
                }
            }
        }
        if ledger.receipts.len() == before {
            continue;
        }
        if ledger.receipts.len() > RECEIPT_LEDGER_MAX_ENTRIES {
            return Err(AckError::LedgerFull);
        }
        let bytes = serde_json::to_vec(&ledger).map_err(|e| {
            AckError::Lifecycle(LifecycleError::Corrupt {
                path: path.clone(),
                why: e.to_string(),
            })
        })?;
        if bytes.len() as u64 > RECEIPT_LEDGER_CAP {
            return Err(AckError::LedgerFull);
        }
        updates.push((path, bytes));
    }
    for (written, (path, bytes)) in updates.into_iter().enumerate() {
        write_ledger(&path, &bytes, written).map_err(|e| {
            if written == 0 {
                AckError::Lifecycle(e)
            } else {
                AckError::Incomplete(e)
            }
        })?;
    }
    Ok(())
}

/// One ledger's durable rename; the `written`-th write fails when a test
/// asked for it.
fn write_ledger(path: &Path, bytes: &[u8], written: usize) -> Result<(), LifecycleError> {
    #[cfg(test)]
    if FAIL_PUBLISH_AFTER.with(Cell::get) == Some(written) {
        return Err(LifecycleError::InjectedCrash("a receipt ledger write"));
    }
    let _ = written;
    write_durable(path, bytes)
}

#[cfg(test)]
thread_local! {
    /// Fail the ledger write after this many renames (tests only).
    static FAIL_PUBLISH_AFTER: Cell<Option<usize>> = const { Cell::new(None) };
}

// ── The Receipt lock ────────────────────────────────────────────────────────

thread_local! {
    static RECEIPT_HELD: Cell<bool> = const { Cell::new(false) };
}

/// A held Receipt lock. Taken only inside a Lifecycle hold (Lifecycle then
/// Receipt) and, like Lifecycle, a leaf.
#[must_use = "the Receipt lock is released when the guard drops"]
pub(crate) struct ReceiptGuard {
    file: File,
}

impl Drop for ReceiptGuard {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
        RECEIPT_HELD.with(|h| h.set(false));
    }
}

/// Take the anchor's Receipt lock with a bounded try. The `&LifecycleGuard`
/// is the lock-order proof: Receipt is never taken without Lifecycle.
pub(crate) fn receipt_lock(
    anchor: &Path,
    _lifecycle: &LifecycleGuard,
    timeout: Duration,
) -> Result<ReceiptGuard, AckError> {
    if RECEIPT_HELD.with(Cell::get) {
        return Err(LifecycleError::LockOrder("the Receipt lock is not re-entrant").into());
    }
    let path = anchor.join(RECEIPT_LOCK_FILE);
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(LifecycleError::Unsafe {
                path,
                why: "is a symlink".into(),
            }
            .into())
        }
        Err(e) => return Err(LifecycleError::from(e).into()),
    };
    check_regular(&file.metadata().map_err(LifecycleError::from)?, &path, 0)?;
    let start = Instant::now();
    loop {
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => break,
            Err(e) if crate::file_lock::is_lock_contended(&e) => {
                if start.elapsed() >= timeout {
                    return Err(LifecycleError::LockTimeout {
                        waited: start.elapsed(),
                    }
                    .into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(LifecycleError::from(e).into()),
        }
    }
    super::note_lock_class("receipt");
    RECEIPT_HELD.with(|h| h.set(true));
    Ok(ReceiptGuard { file })
}

#[cfg(test)]
#[path = "ack_tests.rs"]
mod tests;
