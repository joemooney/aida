//! Closed-change retention (TASK-3098, the contract approved on BUG-2531,
//! 2026-11-12 22:55 MST). A change's ledger is kept for good: once the forge
//! is observed to have merged or closed the change, its ledger moves from
//! `gate-ledger/<CHANGE>.jsonl` to its one archive path
//! `gate-ledger/archive/<CHANGE>.jsonl`, so closed history stops counting
//! against the active-ledger limits. Bindings and by-spec index entries
//! name the change, not the path, and survive the move.
//!
//! Every reader resolves the active path, then the archive path
//! ([`locate`]); both present is a conflict and refuses, never a choice
//! between two histories. A write to an archived change first restores it to
//! the active path ([`revive`]), so its `seq` stays monotonic and every old
//! refusal and hold stays in it.
//!
//! Reader visibility: rotation and revival rename under the ledger-wide lock
//! held EXCLUSIVELY; readers resolve and read under it held SHARED
//! ([`read_guard`]), so no reader sees a change missing from both paths
//! mid-move. A frame that already holds the lock reuses it.
// trace:TASK-3098 | ai:claude
// mandate-gate

use super::{ledger_dir, ChangeKey};
use std::path::{Path, PathBuf};

/// The archive directory's name under the ledger directory.
pub(crate) const ARCHIVE_DIR: &str = "archive";

/// `<root>/.aida/gate-ledger/archive`.
pub(crate) fn archive_dir(root: &Path) -> PathBuf {
    ledger_dir(root).join(ARCHIVE_DIR)
}

/// The one archive path of `change`'s ledger.
pub(crate) fn archive_file(root: &Path, change: ChangeKey) -> PathBuf {
    archive_dir(root).join(format!("{}.jsonl", change.as_key()))
}

/// Where a change's ledger is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Located {
    Active(PathBuf),
    Archived(PathBuf),
    /// The change has no ledger.
    Absent,
}

impl Located {
    pub(crate) fn path(&self) -> Option<&Path> {
        match self {
            Self::Active(p) | Self::Archived(p) => Some(p),
            Self::Absent => None,
        }
    }
}

/// Whether `path` names an entry (any kind: a non-file is the reader's to
/// refuse); an unreadable probe is an error, never "absent".
fn present(path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// The error for a change whose ledger is at both paths.
pub(crate) fn conflict(root: &Path, change: ChangeKey) -> std::io::Error {
    std::io::Error::other(format!(
        "refusing the gate ledger of {}: it is both active ({}) and archived ({}); one history must be removed by hand after checking which holds the newer events",
        change.as_key(),
        super::store::ledger_file(root, change).display(),
        archive_file(root, change).display()
    ))
}

/// Where `change`'s ledger is: active first, then archive. Both present
/// refuses. Call it under [`read_guard`] (or a writer's lock) so a move
/// between the two probes cannot read as absence.
pub(crate) fn locate(root: &Path, change: ChangeKey) -> std::io::Result<Located> {
    let active = super::store::ledger_file(root, change);
    let archived = archive_file(root, change);
    match (present(&active)?, present(&archived)?) {
        (true, true) => Err(conflict(root, change)),
        (true, false) => Ok(Located::Active(active)),
        (false, true) => Ok(Located::Archived(archived)),
        (false, false) => Ok(Located::Absent),
    }
}

/// The ledger-wide lock held SHARED for a read (`None` when this thread is
/// already inside a ledger lock frame, or the lock cannot be opened: a
/// reader without the lock file still resolves, it just cannot exclude a
/// concurrent move, which [`locate`]'s caller then reads as it finds it).
pub(crate) fn read_guard(root: &Path) -> Option<super::store::LedgerLock> {
    super::store::shared_read_lock(root)
}

/// How a rotation request ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rotated {
    /// Moved to the archive now.
    Archived,
    /// Already archived.
    AlreadyArchived,
    /// The change has no ledger; nothing to keep.
    NoLedger,
}

/// The forge's observed state of a change, the only evidence that may
/// rotate its ledger. A verdict field (`closed_by_merge`) is never evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForgeState {
    Open,
    Merged,
    Closed,
    /// Not read, or read as something else: keep the ledger active.
    Unknown,
}

/// Move `change`'s ledger to its archive path, under the exclusive
/// ledger-wide lock, when `observed` shows the forge merged or closed it.
/// Any other evidence leaves it active (`Ok(None)`). The change's per-change
/// lock file goes with it: no writer can hold it under the exclusive lock.
pub(crate) fn rotate(
    root: &Path,
    change: ChangeKey,
    observed: ForgeState,
) -> std::io::Result<Option<Rotated>> {
    if !matches!(observed, ForgeState::Merged | ForgeState::Closed) {
        return Ok(None);
    }
    super::store::with_exclusive(root, || {
        match locate(root, change)? {
            Located::Absent => return Ok(Some(Rotated::NoLedger)),
            Located::Archived(_) => return Ok(Some(Rotated::AlreadyArchived)),
            Located::Active(_) => {}
        }
        let dir = archive_dir(root);
        std::fs::create_dir_all(&dir)?;
        refuse_linked_dir(&dir)?;
        let from = super::store::ledger_file(root, change);
        std::fs::rename(&from, archive_file(root, change))?;
        sync_dir(&dir)?;
        sync_dir(&ledger_dir(root))?;
        let _ = std::fs::remove_file(super::store::change_lock_file(root, change));
        Ok(Some(Rotated::Archived))
    })
}

/// Restore an archived `change` ledger to its active path, under the
/// exclusive ledger-wide lock, before anything is appended to it. `false`
/// when it was not archived. A thread already inside a shared ledger frame
/// cannot take the exclusive lock (it would wait on itself), so it refuses
/// instead; inside an exclusive frame the move happens directly.
pub(crate) fn revive(root: &Path, change: ChangeKey) -> std::io::Result<bool> {
    // Cheap first look: a change that was never archived costs one probe.
    if !present(&archive_file(root, change))? {
        return Ok(false);
    }
    let move_back = || -> std::io::Result<bool> {
        match locate(root, change)? {
            Located::Archived(path) => {
                std::fs::rename(&path, super::store::ledger_file(root, change))?;
                sync_dir(&ledger_dir(root))?;
                sync_dir(&archive_dir(root))?;
                Ok(true)
            }
            _ => Ok(false),
        }
    };
    if super::store::holds_exclusive() {
        return move_back();
    }
    if super::store::in_ledger_frame() {
        return Err(std::io::Error::other(format!(
            "the gate ledger of {} is archived and cannot be restored from inside a shared ledger lock; retry the write",
            change.as_key()
        )));
    }
    super::store::with_exclusive(root, move_back)
}

/// Refuse an archive directory that is a link (a move must never leave the
/// ledger tree).
fn refuse_linked_dir(dir: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.file_type().is_dir() {
        return Err(std::io::Error::other(format!(
            "refusing the gate-ledger archive {}: it is not a plain directory",
            dir.display()
        )));
    }
    Ok(())
}

fn sync_dir(dir: &Path) -> std::io::Result<()> {
    match std::fs::File::open(dir) {
        Ok(d) => d.sync_all(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// The changes whose ledgers are archived, by name (`PR-<N>.jsonl`).
pub(crate) fn archived_changes(root: &Path) -> std::io::Result<Vec<ChangeKey>> {
    let dir = archive_dir(root);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out: Vec<ChangeKey> = entries
        .flatten()
        .filter_map(|e| super::bindings::change_ledger_name(&e.file_name().to_string_lossy()))
        .filter_map(|key| ChangeKey::parse(&key))
        .collect();
    out.sort_by_key(|c| (c.as_key().len(), c.as_key()));
    Ok(out)
}

/// What one retention sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Sweep {
    /// Active change ledgers looked at.
    pub active: usize,
    /// Moved to the archive (the forge reported them merged or closed).
    pub archived: Vec<String>,
    /// Kept active: open, or the forge's state could not be read.
    pub kept: usize,
    /// Changes whose rotation failed, with why.
    pub failed: Vec<(String, String)>,
}

/// The changes whose ledgers are active, by name.
pub(crate) fn active_changes(root: &Path) -> std::io::Result<Vec<ChangeKey>> {
    let entries = match std::fs::read_dir(ledger_dir(root)) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out: Vec<ChangeKey> = entries
        .flatten()
        .filter_map(|e| super::bindings::change_ledger_name(&e.file_name().to_string_lossy()))
        .filter_map(|key| ChangeKey::parse(&key))
        .collect();
    out.sort_by_key(|c| (c.as_key().len(), c.as_key()));
    Ok(out)
}

/// Archive every active change ledger whose change `observe` reports
/// merged or closed (the maintenance sweep for changes closed outside an
/// AIDA merge). Each rotation is its own exclusive-lock step; a change the
/// forge cannot answer for stays active.
// trace:TASK-3098 | ai:claude
pub(crate) fn rotate_closed(
    root: &Path,
    observe: &mut dyn FnMut(ChangeKey) -> ForgeState,
) -> std::io::Result<Sweep> {
    let mut sweep = Sweep::default();
    for change in active_changes(root)? {
        sweep.active += 1;
        let state = observe(change);
        match rotate(root, change, state) {
            Ok(Some(Rotated::Archived)) => sweep.archived.push(change.as_key()),
            Ok(_) => sweep.kept += 1,
            Err(e) => sweep.failed.push((change.as_key(), e.to_string())),
        }
    }
    Ok(sweep)
}
