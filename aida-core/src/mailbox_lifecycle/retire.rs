//! The retention barrier (C SKETCH v1 section 4).
//!
//! `retire_source` fences a source so new admissions refuse, waits for every
//! admitted writer to finish, preserves the mailbox bytes under the anchor,
//! and commits only after a final fenced check proves that nothing wrote
//! during the window. Each phase is written to `journal/<op_id>.json` (with
//! fsync) before its side effect, so a crash at any point replays forward
//! from the last durable phase and never rolls back over preserved bytes.
//!
//! A refusal reopens the source, copies any preserved bytes back into the
//! mailbox and keeps the retained copy: the caller keeps its tree, lease,
//! claim and manifest. Retirement is retryable; it never blocks.
//!
//! Retention bound: every attempt that reaches preservation keeps its copy
//! under `retained/<key>/<generation>/`, refused attempts included, and C1
//! prunes none of them. A source refused N times after preserving holds N
//! copies of its mailbox; pruning copies superseded by a later attempt or a
//! committed retirement is C2 work.
//!
//! The caller performs the destructive step after `Retired` and then calls
//! `complete_retirement`. C2 wires the destructive entries; until then this
//! module has no production caller.
//!
//! trace:STORY-1843 | ai:claude

#![cfg_attr(
    not(test),
    expect(dead_code, reason = "C2 wires the destructive entries to the barrier")
)]

use super::{
    activated, check_regular, delete_record, ensure_dir, fsync_dir, hex, load_or_create,
    load_record, lock, mint_generation, open_checked, read_json, resolve_source, store_record,
    write_json, LifecycleError, Result, SourceState, JOURNAL_DIR, LIFECYCLE_LOCK_TIMEOUT,
    RETAINED_DIR, SCHEMA,
};
use crate::process_probe::{Probe, ProbeSource};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Per-file cap for a preserved mailbox file.
const PRESERVE_CAP: u64 = 64 << 20;

/// How many stale operations one call finishes before starting its own.
const MAX_REPLAYS: usize = 4;

/// Retirement journal phases, in order. `Aborted` is the terminal phase of a
/// refused operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Fenced,
    Quiesced,
    Preserved,
    Committed,
    Removed,
    Finalized,
    Aborted,
}

/// Why a retirement refused. The caller keeps its tree either way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum RefusalReason {
    /// The checkout has no verified common Git directory to anchor to.
    Unanchored,
    UnsafeMailbox {
        detail: String,
    },
    /// The Lifecycle lock stayed busy; retry.
    LockBusy,
    /// Admitted writers were still running when the wait ran out.
    LiveWriters {
        permits: usize,
    },
    /// A permit's writer cannot be proven alive or dead.
    UnknownWriterLiveness {
        detail: String,
    },
    /// The source holds mail, receipt or watermark bytes and no operator has
    /// recorded that every writer host is admitting (QC1).
    MailBearingBeforeActivation,
    PreserveFailed {
        detail: String,
    },
    ManifestMismatch {
        path: PathBuf,
    },
    /// Something wrote to the mailbox after the bytes were preserved.
    LateWriter {
        entries: Vec<String>,
    },
    StateChanged {
        detail: String,
    },
}

impl std::fmt::Display for RefusalReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RefusalReason::Unanchored => {
                write!(
                    f,
                    "the checkout is not in a Git repository this host can verify"
                )
            }
            RefusalReason::UnsafeMailbox { detail } => write!(f, "unsafe mailbox: {detail}"),
            RefusalReason::LockBusy => write!(f, "the mailbox lifecycle lock is busy; retry"),
            RefusalReason::LiveWriters { permits } => {
                write!(f, "{permits} mail writer(s) are still writing; retry")
            }
            RefusalReason::UnknownWriterLiveness { detail } => write!(
                f,
                "a mail writer's process cannot be proven alive or dead ({detail})"
            ),
            RefusalReason::MailBearingBeforeActivation => write!(
                f,
                "the mailbox holds mail, and not every mail writer on this machine is known to \
                 take a write permit yet; run `{}` to check and record that",
                super::ACTIVATION_COMMAND
            ),
            RefusalReason::PreserveFailed { detail } => {
                write!(f, "the mailbox could not be preserved: {detail}")
            }
            RefusalReason::ManifestMismatch { path } => write!(
                f,
                "the preserved copy of {} does not match the original",
                path.display()
            ),
            RefusalReason::LateWriter { entries } => write!(
                f,
                "something wrote to the mailbox while it was being retired ({})",
                entries.join(", ")
            ),
            RefusalReason::StateChanged { detail } => {
                write!(f, "the mailbox changed during retirement: {detail}")
            }
        }
    }
}

/// What the caller holds after `Retired`: enough to finish the operation
/// once the checkout is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RetirementTicket {
    pub anchor: PathBuf,
    pub op_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RetireOutcome {
    /// The bytes are preserved and the source is retired. The caller may now
    /// perform its destructive step, then call [`complete_retirement`].
    Retired {
        ticket: RetirementTicket,
        generation: u64,
        retained_dir: PathBuf,
    },
    /// Nothing was destroyed: the caller keeps its tree, lease, claim and
    /// manifest. `retained_dir` names a preserved copy that is also kept.
    Refused {
        reason: RefusalReason,
        retained_dir: Option<PathBuf>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct RetireOptions {
    pub reason: String,
    /// Bounded wait for admitted writers to finish.
    pub quiesce_timeout: Duration,
    pub probe: ProbeSource,
    /// Copy and verify instead of renaming, as across devices.
    pub force_copy: bool,
}

impl Default for RetireOptions {
    fn default() -> Self {
        RetireOptions {
            reason: "retire".into(),
            quiesce_timeout: Duration::from_secs(5),
            probe: ProbeSource::system(),
            force_copy: false,
        }
    }
}

/// `journal/<op_id>.json`. Carries everything needed to finish the operation
/// after the checkout is gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct JournalEntry {
    pub schema: u32,
    pub op_id: String,
    pub anchor: PathBuf,
    pub key: String,
    pub mailbox: PathBuf,
    pub dev: u64,
    pub ino: u64,
    pub generation: u64,
    pub reason: String,
    pub phase: Phase,
    pub refusal: Option<RefusalReason>,
    pub updated_at: String,
}

impl JournalEntry {
    fn retained_dir(&self) -> PathBuf {
        self.anchor
            .join(RETAINED_DIR)
            .join(&self.key)
            .join(self.generation.to_string())
    }

    fn kept_retained(&self) -> Option<PathBuf> {
        let dir = self.retained_dir();
        dir.exists().then_some(dir)
    }
}

/// `retained/<key>/<generation>/manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub schema: u32,
    pub key: String,
    pub generation: u64,
    pub op_id: String,
    pub original_path: PathBuf,
    pub evidence_origin: String,
    pub count: usize,
    pub bytes: u64,
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ManifestFile {
    pub path: PathBuf,
    pub sha256: String,
    pub len: u64,
}

// ── Test seams ──────────────────────────────────────────────────────────────

/// Points where a fixture can crash the barrier or run a hook. Production
/// builds pass through them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Point {
    /// Right after the journal records this phase, before its side effect.
    Journal(Phase),
    AfterFence,
    MidPreserve,
    BeforeFinalCheck,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CopyFault {
    Corrupt,
    DiskFull,
}

#[cfg(test)]
type Hook = Box<dyn FnOnce()>;

#[cfg(test)]
thread_local! {
    pub(crate) static CRASH_AT: std::cell::Cell<Option<Point>> = const { std::cell::Cell::new(None) };
    pub(crate) static HOOK_AT: std::cell::RefCell<Option<(Point, Hook)>> =
        const { std::cell::RefCell::new(None) };
    pub(crate) static COPY_FAULT: std::cell::Cell<Option<CopyFault>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn point(at: Point) -> Result<()> {
    let hook = HOOK_AT.with(|h| {
        let mut h = h.borrow_mut();
        match h.as_ref() {
            Some((p, _)) if *p == at => h.take().map(|(_, f)| f),
            _ => None,
        }
    });
    if let Some(hook) = hook {
        hook();
    }
    if CRASH_AT.with(|c| c.get()) == Some(at) {
        CRASH_AT.with(|c| c.set(None));
        return Err(LifecycleError::InjectedCrash(match at {
            Point::Journal(_) => "journal",
            Point::AfterFence => "after-fence",
            Point::MidPreserve => "mid-preserve",
            Point::BeforeFinalCheck => "before-final-check",
        }));
    }
    Ok(())
}

#[cfg(not(test))]
#[inline(always)]
fn point(_: Point) -> Result<()> {
    Ok(())
}

// ── Entry points ────────────────────────────────────────────────────────────

/// Retire the mail source of the checkout at `project_root`.
///
/// Finishes any interrupted operation on the same source first. `Err` means
/// the barrier could not run or record its progress; the journal then holds
/// the last durable phase and the next call replays it.
pub(crate) fn retire_source(project_root: &Path, opts: &RetireOptions) -> Result<RetireOutcome> {
    let mailbox = crate::mailbox::local_mailbox_dir(project_root);
    match std::fs::symlink_metadata(&mailbox) {
        Ok(meta) if meta.file_type().is_dir() => {}
        Ok(_) => {
            return Ok(refused(
                RefusalReason::UnsafeMailbox {
                    detail: format!("{} is not a real directory", mailbox.display()),
                },
                None,
            ))
        }
        // An absent mailbox still needs an identity to fence: a writer could
        // create it during the window.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir_all(&mailbox)?,
        Err(e) => return Err(e.into()),
    }
    let source = match resolve_source(project_root) {
        Ok(Some(source)) => source,
        Ok(None) => return Ok(refused(RefusalReason::Unanchored, None)),
        Err(LifecycleError::Unsafe { path, why }) => {
            return Ok(refused(
                RefusalReason::UnsafeMailbox {
                    detail: format!("{}: {why}", path.display()),
                },
                None,
            ))
        }
        Err(e) => return Err(e),
    };
    for _ in 0..MAX_REPLAYS {
        let held = match lock(&source.anchor, LIFECYCLE_LOCK_TIMEOUT) {
            Ok(held) => held,
            Err(LifecycleError::LockTimeout { .. }) => {
                return Ok(refused(RefusalReason::LockBusy, None))
            }
            Err(e) => return Err(e),
        };
        let mut record = load_or_create(&source, &held)?;
        let pending = match &record.state {
            SourceState::Fenced { op_id, .. } | SourceState::Retired { op_id, .. } => {
                Some(require_journal(&source.anchor, op_id)?)
            }
            SourceState::Open => unfinished_for(&source.anchor, &source.key)?,
        };
        if let Some(entry) = pending {
            drop(held);
            match entry.phase {
                Phase::Aborted => finish_abort(&entry)?,
                // The old checkout is gone and a new one reused its inode.
                Phase::Removed | Phase::Finalized => finalize(entry)?,
                _ => return drive(entry, opts),
            }
            continue;
        }
        let entry = JournalEntry {
            schema: SCHEMA,
            op_id: uuid::Uuid::new_v4().to_string(),
            anchor: source.anchor.clone(),
            key: source.key.clone(),
            mailbox: source.mailbox.clone(),
            dev: source.dev,
            ino: source.ino,
            generation: record.generation,
            reason: opts.reason.clone(),
            phase: Phase::Fenced,
            refusal: None,
            updated_at: now(),
        };
        write_journal(&entry)?;
        point(Point::Journal(Phase::Fenced))?;
        record.state = SourceState::Fenced {
            op_id: entry.op_id.clone(),
            reason: entry.reason.clone(),
        };
        store_record(&source.anchor, &record, &held)?;
        drop(held);
        point(Point::AfterFence)?;
        return drive(entry, opts);
    }
    Err(LifecycleError::Corrupt {
        path: source.anchor.join(JOURNAL_DIR),
        why: format!("more than {MAX_REPLAYS} interrupted operations for one source"),
    })
}

/// Record that the caller removed the retired checkout, then forget the
/// source. Refuses while the retired mailbox directory still exists.
pub(crate) fn complete_retirement(ticket: &RetirementTicket) -> Result<()> {
    let mut entry = require_journal(&ticket.anchor, &ticket.op_id)?;
    match entry.phase {
        Phase::Committed => {
            if let Ok(meta) = std::fs::symlink_metadata(&entry.mailbox) {
                if meta.dev() == entry.dev && meta.ino() == entry.ino {
                    return Err(LifecycleError::NotRemoved {
                        path: entry.mailbox.clone(),
                    });
                }
            }
            advance(&mut entry, Phase::Removed)?;
            finalize(entry)
        }
        Phase::Removed => finalize(entry),
        Phase::Finalized => Ok(()),
        other => Err(LifecycleError::Corrupt {
            path: journal_path(&ticket.anchor, &ticket.op_id),
            why: format!("operation is at {other:?}, not committed"),
        }),
    }
}

// ── Phases ──────────────────────────────────────────────────────────────────

fn drive(mut entry: JournalEntry, opts: &RetireOptions) -> Result<RetireOutcome> {
    loop {
        match entry.phase {
            Phase::Fenced => {
                ensure_fenced(&entry)?;
                if let Err(reason) = quiesce(&entry, opts)? {
                    return abort(entry, reason);
                }
                advance(&mut entry, Phase::Quiesced)?;
            }
            Phase::Quiesced => {
                let data = entry.retained_dir().join("data");
                if !activated(&entry.anchor)?
                    && (holds_bytes(&entry.mailbox)? || holds_bytes(&data)?)
                {
                    return abort(entry, RefusalReason::MailBearingBeforeActivation);
                }
                if let Err(reason) = preserve(&entry, opts)? {
                    return abort(entry, reason);
                }
                advance(&mut entry, Phase::Preserved)?;
            }
            Phase::Preserved => {
                point(Point::BeforeFinalCheck)?;
                if let Err(reason) = final_check(&mut entry)? {
                    return abort(entry, reason);
                }
            }
            Phase::Committed => {
                ensure_retired(&entry)?;
                return Ok(RetireOutcome::Retired {
                    retained_dir: entry.retained_dir(),
                    generation: entry.generation,
                    ticket: RetirementTicket {
                        anchor: entry.anchor,
                        op_id: entry.op_id,
                    },
                });
            }
            Phase::Removed | Phase::Finalized => {
                return Ok(RetireOutcome::Retired {
                    retained_dir: entry.retained_dir(),
                    generation: entry.generation,
                    ticket: RetirementTicket {
                        anchor: entry.anchor,
                        op_id: entry.op_id,
                    },
                })
            }
            Phase::Aborted => {
                finish_abort(&entry)?;
                let reason = entry
                    .refusal
                    .clone()
                    .unwrap_or(RefusalReason::StateChanged {
                        detail: "an earlier attempt was refused".into(),
                    });
                return Ok(refused(reason, entry.kept_retained()));
            }
        }
    }
}

/// Replay: the journal says Fenced but the record may still be Open.
fn ensure_fenced(entry: &JournalEntry) -> Result<()> {
    let held = lock(&entry.anchor, LIFECYCLE_LOCK_TIMEOUT)?;
    let mut record = require_record(entry, &held)?;
    match &record.state {
        SourceState::Fenced { op_id, .. } if *op_id == entry.op_id => Ok(()),
        SourceState::Open => {
            record.state = SourceState::Fenced {
                op_id: entry.op_id.clone(),
                reason: entry.reason.clone(),
            };
            store_record(&entry.anchor, &record, &held)
        }
        other => Err(conflict(entry, other)),
    }
}

/// Replay: the journal says Committed but the record may still be Fenced.
fn ensure_retired(entry: &JournalEntry) -> Result<()> {
    let held = lock(&entry.anchor, LIFECYCLE_LOCK_TIMEOUT)?;
    let mut record = require_record(entry, &held)?;
    match &record.state {
        SourceState::Retired { op_id, .. } if *op_id == entry.op_id => Ok(()),
        SourceState::Fenced { op_id, .. } if *op_id == entry.op_id => {
            record.state = SourceState::Retired {
                op_id: entry.op_id.clone(),
                retained_dir: entry.retained_dir(),
            };
            store_record(&entry.anchor, &record, &held)
        }
        other => Err(conflict(entry, other)),
    }
}

/// Wait, with no lock held between polls, for every permit to clear. A row
/// whose process is proven dead is reaped; a row whose liveness cannot be
/// observed refuses (unknown means refuse).
fn quiesce(
    entry: &JournalEntry,
    opts: &RetireOptions,
) -> Result<std::result::Result<(), RefusalReason>> {
    let deadline = Instant::now() + opts.quiesce_timeout;
    loop {
        let held = match lock(&entry.anchor, LIFECYCLE_LOCK_TIMEOUT) {
            Ok(held) => held,
            Err(LifecycleError::LockTimeout { .. }) if Instant::now() < deadline => continue,
            Err(LifecycleError::LockTimeout { .. }) => return Ok(Err(RefusalReason::LockBusy)),
            Err(e) => return Err(e),
        };
        let mut record = require_record(entry, &held)?;
        let before = record.permits.len();
        let mut alive = 0usize;
        let mut unknown = None;
        record.permits.retain(|permit| {
            let probe = match &permit.holder {
                Some(holder) => opts.probe.probe(holder),
                None => Probe::Unknown("the writer recorded no process identity".into()),
            };
            match probe {
                Probe::Dead(_) => false,
                Probe::Alive => {
                    alive += 1;
                    true
                }
                Probe::Unknown(why) => {
                    unknown.get_or_insert(why);
                    true
                }
            }
        });
        if record.permits.len() != before {
            store_record(&entry.anchor, &record, &held)?;
        }
        drop(held);
        if let Some(detail) = unknown {
            return Ok(Err(RefusalReason::UnknownWriterLiveness { detail }));
        }
        if alive == 0 {
            return Ok(Ok(()));
        }
        if Instant::now() >= deadline {
            return Ok(Err(RefusalReason::LiveWriters { permits: alive }));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Move every mailbox entry into `retained/<key>/<generation>/data/` (rename
/// on one filesystem, copy and verify across devices), then write the
/// manifest. Idempotent, so a crash mid-way replays.
fn preserve(
    entry: &JournalEntry,
    opts: &RetireOptions,
) -> Result<std::result::Result<(), RefusalReason>> {
    let retained = entry.retained_dir();
    let data = retained.join("data");
    for dir in [retained.parent().unwrap_or(&retained), &retained, &data] {
        ensure_dir(dir, true)?;
    }
    let same_fs = !opts.force_copy && std::fs::metadata(&data)?.dev() == entry.dev;
    let mut moved = 0usize;
    if let Err(reason) = move_tree(&entry.mailbox, &data, same_fs, &mut moved)? {
        return Ok(Err(reason));
    }
    let mut files = Vec::new();
    if let Err(reason) = manifest_files(&data, &data, &mut files)? {
        return Ok(Err(reason));
    }
    let manifest = Manifest {
        schema: SCHEMA,
        key: entry.key.clone(),
        generation: entry.generation,
        op_id: entry.op_id.clone(),
        original_path: entry.mailbox.clone(),
        evidence_origin: "local-mailbox".into(),
        count: files.len(),
        bytes: files.iter().map(|f| f.len).sum(),
        files,
    };
    write_json(&retained.join("manifest.json"), &manifest)?;
    Ok(Ok(()))
}

fn move_tree(
    src: &Path,
    dst: &Path,
    same_fs: bool,
    moved: &mut usize,
) -> Result<std::result::Result<(), RefusalReason>> {
    for (name, path, meta) in sorted_entries(src)? {
        let target = dst.join(&name);
        if meta.file_type().is_dir() {
            ensure_dir(&target, true)?;
            if let Err(reason) = move_tree(&path, &target, same_fs, moved)? {
                return Ok(Err(reason));
            }
            // A writer that took no permit may have added an entry here after
            // the listing: refuse (the abort restores and reopens) rather than
            // fail with the source left fenced.
            if let Err(e) = std::fs::remove_dir(&path) {
                return Ok(Err(RefusalReason::StateChanged {
                    detail: format!("{} could not be emptied: {e}", path.display()),
                }));
            }
            continue;
        }
        if let Err(e) = check_regular(&meta, &path, PRESERVE_CAP) {
            return Ok(Err(RefusalReason::PreserveFailed {
                detail: e.to_string(),
            }));
        }
        if std::fs::symlink_metadata(&target).is_ok() {
            // Replay after a crash between copy and source removal.
            if sha256_file(&path)? != sha256_file(&target)? {
                return Ok(Err(RefusalReason::PreserveFailed {
                    detail: format!("{} is already preserved with other bytes", path.display()),
                }));
            }
            if let Err(reason) = remove_preserved(&path) {
                return Ok(Err(reason));
            }
        } else if same_fs {
            if let Err(e) = std::fs::rename(&path, &target) {
                return Ok(Err(RefusalReason::PreserveFailed {
                    detail: format!("moving {}: {e}", path.display()),
                }));
            }
        } else {
            if let Err(reason) = copy_verified(&path, &target)? {
                return Ok(Err(reason));
            }
            if let Err(reason) = remove_preserved(&path) {
                return Ok(Err(reason));
            }
        }
        *moved += 1;
        if *moved == 1 {
            point(Point::MidPreserve)?;
        }
    }
    fsync_dir(dst)?;
    fsync_dir(src)?;
    Ok(Ok(()))
}

/// Remove a source file whose bytes are already preserved.
fn remove_preserved(path: &Path) -> std::result::Result<(), RefusalReason> {
    std::fs::remove_file(path).map_err(|e| RefusalReason::PreserveFailed {
        detail: format!("removing preserved {}: {e}", path.display()),
    })
}

static COPY_SEQ: AtomicU64 = AtomicU64::new(0);

/// Copy `src` to `dst` through a fsynced temp file and compare SHA-256 of
/// both before the rename. A mismatch or a write error refuses and leaves the
/// source untouched.
fn copy_verified(src: &Path, dst: &Path) -> Result<std::result::Result<(), RefusalReason>> {
    let dir = dst.parent().unwrap_or(Path::new("."));
    let name = dst.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let seq = COPY_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.copy.{}.{seq}", std::process::id()));
    let staged = (|| -> Result<std::result::Result<(), RefusalReason>> {
        let Some(mut input) = open_checked(src, PRESERVE_CAP)? else {
            return Ok(Err(RefusalReason::StateChanged {
                detail: format!("{} vanished during preservation", src.display()),
            }));
        };
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)?;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            #[cfg(test)]
            if COPY_FAULT.with(|f| f.get()) == Some(CopyFault::DiskFull) {
                return Err(std::io::Error::from_raw_os_error(libc::ENOSPC).into());
            }
            out.write_all(&buf[..n])?;
        }
        #[cfg(test)]
        if COPY_FAULT.with(|f| f.get()) == Some(CopyFault::Corrupt) {
            out.write_all(b"!")?;
        }
        out.sync_all()?;
        drop(out);
        if hex(&hasher.finalize()) != sha256_file(&tmp)? {
            return Ok(Err(RefusalReason::ManifestMismatch {
                path: src.to_path_buf(),
            }));
        }
        std::fs::rename(&tmp, dst)?;
        Ok(Ok(()))
    })();
    match staged {
        Ok(Ok(())) => Ok(Ok(())),
        Ok(Err(reason)) => {
            let _ = std::fs::remove_file(&tmp);
            Ok(Err(reason))
        }
        Err(LifecycleError::Io(e)) => {
            let _ = std::fs::remove_file(&tmp);
            Ok(Err(RefusalReason::PreserveFailed {
                detail: format!("copying {}: {e}", src.display()),
            }))
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn manifest_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<ManifestFile>,
) -> Result<std::result::Result<(), RefusalReason>> {
    for (_, path, meta) in sorted_entries(dir)? {
        if meta.file_type().is_dir() {
            if let Err(reason) = manifest_files(root, &path, out)? {
                return Ok(Err(reason));
            }
            continue;
        }
        if let Err(e) = check_regular(&meta, &path, PRESERVE_CAP) {
            return Ok(Err(RefusalReason::PreserveFailed {
                detail: e.to_string(),
            }));
        }
        out.push(ManifestFile {
            path: path.strip_prefix(root).unwrap_or(&path).to_path_buf(),
            sha256: sha256_file(&path)?,
            len: meta.len(),
        });
    }
    Ok(Ok(()))
}

/// Under Lifecycle: the generation, the `Fenced{op_id}` state, the mailbox's
/// device and inode and an empty mailbox must all be unchanged since the
/// fence. Then journal `Committed` and mark the source `Retired`.
fn final_check(entry: &mut JournalEntry) -> Result<std::result::Result<(), RefusalReason>> {
    let held = match lock(&entry.anchor, LIFECYCLE_LOCK_TIMEOUT) {
        Ok(held) => held,
        Err(LifecycleError::LockTimeout { .. }) => return Ok(Err(RefusalReason::LockBusy)),
        Err(e) => return Err(e),
    };
    let mut record = require_record(entry, &held)?;
    let changed = |detail: String| Ok(Err(RefusalReason::StateChanged { detail }));
    if record.generation != entry.generation {
        return changed(format!(
            "generation {} became {}",
            entry.generation, record.generation
        ));
    }
    if !matches!(&record.state, SourceState::Fenced { op_id, .. } if *op_id == entry.op_id) {
        return changed("the source is no longer fenced by this operation".into());
    }
    if !record.permits.is_empty() {
        return changed("a write permit appeared after the fence".into());
    }
    match std::fs::symlink_metadata(&entry.mailbox) {
        Ok(meta)
            if meta.file_type().is_dir() && meta.dev() == entry.dev && meta.ino() == entry.ino => {}
        _ => return changed("the mailbox directory was replaced".into()),
    }
    let late: Vec<String> = sorted_entries(&entry.mailbox)?
        .into_iter()
        .map(|(name, _, _)| name)
        .collect();
    if !late.is_empty() {
        return Ok(Err(RefusalReason::LateWriter { entries: late }));
    }
    advance(entry, Phase::Committed)?;
    record.state = SourceState::Retired {
        op_id: entry.op_id.clone(),
        retained_dir: entry.retained_dir(),
    };
    store_record(&entry.anchor, &record, &held)?;
    Ok(Ok(()))
}

/// Journal the refusal, then undo the fence.
fn abort(mut entry: JournalEntry, reason: RefusalReason) -> Result<RetireOutcome> {
    entry.refusal = Some(reason.clone());
    advance(&mut entry, Phase::Aborted)?;
    finish_abort(&entry)?;
    Ok(refused(reason, entry.kept_retained()))
}

/// Copy preserved bytes back into the mailbox where absent (the retained
/// copy is kept) and reopen the source under a new generation. Idempotent.
fn finish_abort(entry: &JournalEntry) -> Result<()> {
    let data = entry.retained_dir().join("data");
    if data.is_dir() {
        std::fs::create_dir_all(&entry.mailbox)?;
        restore_tree(&data, &entry.mailbox)?;
    }
    let held = lock(&entry.anchor, LIFECYCLE_LOCK_TIMEOUT)?;
    if let Some(mut record) = load_record(&entry.anchor, &entry.key, &held)? {
        if matches!(&record.state, SourceState::Fenced { op_id, .. } if *op_id == entry.op_id) {
            // A fresh generation keeps each attempt's retained copy immutable:
            // the reopened mailbox changes, and the next attempt preserves
            // into its own directory.
            record.state = SourceState::Open;
            record.generation = mint_generation(&entry.anchor, &held)?;
            store_record(&entry.anchor, &record, &held)?;
        }
    }
    Ok(())
}

fn restore_tree(src: &Path, dst: &Path) -> Result<()> {
    for (name, path, meta) in sorted_entries(src)? {
        let target = dst.join(&name);
        if meta.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
            restore_tree(&path, &target)?;
        } else if std::fs::symlink_metadata(&target).is_err() {
            match copy_verified(&path, &target)? {
                Ok(()) => {}
                Err(reason) => {
                    return Err(LifecycleError::Io(std::io::Error::other(
                        reason.to_string(),
                    )))
                }
            }
        }
    }
    fsync_dir(dst)
}

/// Journal `Finalized` after the record is gone.
fn finalize(mut entry: JournalEntry) -> Result<()> {
    if entry.phase == Phase::Finalized {
        return Ok(());
    }
    let held = lock(&entry.anchor, LIFECYCLE_LOCK_TIMEOUT)?;
    if let Some(record) = load_record(&entry.anchor, &entry.key, &held)? {
        if matches!(&record.state, SourceState::Retired { op_id, .. } if *op_id == entry.op_id) {
            delete_record(&entry.anchor, &entry.key, &held)?;
        }
    }
    drop(held);
    advance(&mut entry, Phase::Finalized)
}

// ── Journal ─────────────────────────────────────────────────────────────────

fn journal_path(anchor: &Path, op_id: &str) -> PathBuf {
    anchor.join(JOURNAL_DIR).join(format!("{op_id}.json"))
}

fn write_journal(entry: &JournalEntry) -> Result<()> {
    write_json(&journal_path(&entry.anchor, &entry.op_id), entry)
}

/// Record the next phase durably, before its side effect.
fn advance(entry: &mut JournalEntry, phase: Phase) -> Result<()> {
    entry.phase = phase;
    entry.updated_at = now();
    write_journal(entry)?;
    point(Point::Journal(phase))
}

fn require_journal(anchor: &Path, op_id: &str) -> Result<JournalEntry> {
    let path = journal_path(anchor, op_id);
    if op_id.is_empty() || !op_id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err(LifecycleError::Corrupt {
            path,
            why: format!("operation id {op_id:?} is not a UUID"),
        });
    }
    read_json::<JournalEntry>(&path)?.ok_or(LifecycleError::Corrupt {
        path,
        why: "the operation's journal entry is missing".into(),
    })
}

/// An unfinished operation on `key` that the source record does not name: a
/// crash after the journal recorded `Fenced` but before the record did, or
/// after the record was deleted but before `Finalized`.
fn unfinished_for(anchor: &Path, key: &str) -> Result<Option<JournalEntry>> {
    for (name, path, _) in sorted_entries(&anchor.join(JOURNAL_DIR))? {
        if !name.ends_with(".json") || name.starts_with('.') {
            continue;
        }
        let Some(entry) = read_json::<JournalEntry>(&path)? else {
            continue;
        };
        if entry.key == key && !matches!(entry.phase, Phase::Finalized | Phase::Aborted) {
            return Ok(Some(entry));
        }
    }
    Ok(None)
}

fn require_record(
    entry: &JournalEntry,
    held: &super::LifecycleGuard,
) -> Result<super::SourceRecord> {
    load_record(&entry.anchor, &entry.key, held)?.ok_or_else(|| LifecycleError::Corrupt {
        path: super::record_path(&entry.anchor, &entry.key),
        why: format!("operation {} has no source record", entry.op_id),
    })
}

fn conflict(entry: &JournalEntry, state: &SourceState) -> LifecycleError {
    LifecycleError::Corrupt {
        path: super::record_path(&entry.anchor, &entry.key),
        why: format!(
            "operation {} at {:?} found the source in state {state:?}",
            entry.op_id, entry.phase
        ),
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn refused(reason: RefusalReason, retained_dir: Option<PathBuf>) -> RetireOutcome {
    RetireOutcome::Refused {
        reason,
        retained_dir,
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

type Entry = (String, PathBuf, std::fs::Metadata);

/// Entries of `dir` by name, with `lstat` metadata. Empty when absent.
fn sorted_entries(dir: &Path) -> Result<Vec<Entry>> {
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for item in read {
        let item = item?;
        let path = item.path();
        let meta = std::fs::symlink_metadata(&path)?;
        out.push((item.file_name().to_string_lossy().into_owned(), path, meta));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Does `dir` hold anything but directories? Any file counts as mail,
/// receipt or watermark bytes, even an empty one.
fn holds_bytes(dir: &Path) -> Result<bool> {
    for (_, path, meta) in sorted_entries(dir)? {
        if !meta.file_type().is_dir() || holds_bytes(&path)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn sha256_file(path: &Path) -> Result<String> {
    let Some(mut file) = open_checked(path, PRESERVE_CAP)? else {
        return Err(std::io::Error::from(std::io::ErrorKind::NotFound).into());
    };
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}
