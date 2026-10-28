//! Leftovers of mailbox creation and reuse, surfaced and reaped.
//!
//! Three kinds of entry outlive the operation that made them:
//!
//! - `<checkout>/.aida/.mailbox-tomb-<nonce>/`: a retired mailbox renamed
//!   aside by a reuse. An empty one (only directories and its creation nonce)
//!   is removed; one holding an entry written after the retirement is mail
//!   and is only ever reported, for an operator to move out by hand.
//! - `<checkout>/.aida/.mailbox.creating.<uuid>/`: a crashed creator's
//!   staging directory.
//! - `<anchor>/nonce-era/<hash>`: a nonce-era marker whose worktree admin
//!   entry was pruned or recreated, and `.<hash>.drop.<id>` asides an
//!   earlier build left while dropping one (a fresh aside whose marker is
//!   gone is linked back).
//!
//! Tombs, staging and asides are touched only once older than
//! [`STALE_AFTER`], so a reuse, creation or drop still in flight never loses
//! its entry. Markers are dropped under the Lifecycle lock. The retention
//! dry run reports; `aida agent gc` reaps.
//!
//! trace:TASK-b6b1278890774ef993d2ebc327862470-052 | ai:claude

use serde::Serialize;
use std::collections::BTreeSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Age below which a tomb or staging directory is never reaped.
pub const STALE_AFTER: Duration = Duration::from_secs(10 * 60);

/// Prefix of a retired mailbox renamed aside by a reuse.
pub(crate) const TOMB_PREFIX: &str = ".mailbox-tomb-";

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Leftovers {
    /// Tombs holding an entry written after a retirement: never removed.
    pub stranded_tombs: Vec<PathBuf>,
    /// Empty tombs, stale staging directories and stale nonce-era markers:
    /// removed by a reaping scan, listed by a reporting one.
    pub reapable: Vec<PathBuf>,
}

/// Scan every checkout the anchor of `project_root` has a source for (and
/// `project_root` itself). With `reap`, remove what is reapable; `reapable`
/// then lists what was removed.
pub fn scan(project_root: &Path, reap: bool) -> Leftovers {
    scan_at(project_root, reap, SystemTime::now())
}

pub(crate) fn scan_at(project_root: &Path, reap: bool, now: SystemTime) -> Leftovers {
    let mut out = Leftovers::default();
    let mut aida_dirs = BTreeSet::from([project_root.join(".aida")]);
    let anchor = super::locate_anchor(project_root).ok().flatten();
    if let Some(anchored) = &anchor {
        for mailbox in source_mailboxes(&anchored.anchor) {
            if let Some(dir) = mailbox.parent() {
                aida_dirs.insert(dir.to_path_buf());
            }
        }
    }
    for dir in &aida_dirs {
        scan_aida_dir(dir, reap, now, &mut out);
    }
    if let Some(anchored) = &anchor {
        scan_era_markers(&anchored.anchor, reap, now, &mut out);
    }
    out
}

/// Remove `<parent>/.<name>.creating.*` staging directories older than
/// [`STALE_AFTER`]. Best effort: a failure leaves the directory for later.
pub(crate) fn reap_stale_staging(parent: &Path, name: &str, now: SystemTime) {
    let prefix = format!(".{name}.creating.");
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with(&prefix) && is_stale_dir(&path, now) {
            let _ = remove_staging(&path);
        }
    }
}

/// The mailbox paths of the anchor's source records.
fn source_mailboxes(anchor: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(anchor.join(super::SOURCES_DIR)) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|e| {
            super::read_json::<super::SourceRecord>(&e.path())
                .ok()
                .flatten()
        })
        .map(|r| r.mailbox)
        .collect()
}

fn scan_aida_dir(dir: &Path, reap: bool, now: SystemTime, out: &mut Leftovers) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with(TOMB_PREFIX) {
            if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_dir()) {
                continue;
            }
            // Anything but directories and the nonce is mail; an unreadable
            // tomb counts as mail too.
            if super::guard::check_empty_tree(&path, true).is_err() {
                out.stranded_tombs.push(path);
            } else if is_stale_dir(&path, now) && (!reap || remove_empty_tomb(&path).is_ok()) {
                out.reapable.push(path);
            }
        } else if name.starts_with(".mailbox.creating.")
            && is_stale_dir(&path, now)
            && (!reap || remove_staging(&path).is_ok())
        {
            out.reapable.push(path);
        }
    }
}

fn scan_era_markers(anchor: &Path, reap: bool, now: SystemTime, out: &mut Leftovers) {
    let Ok(entries) = std::fs::read_dir(anchor.join(super::NONCE_ERA_DIR)) else {
        return;
    };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        // Only a marker that names its admin entry can be judged here; a
        // pre-record one is judged when its checkout next resolves.
        let Ok(Some(bytes)) = super::read_checked(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_slice::<super::EraMarker>(&bytes) else {
            continue;
        };
        if let Some(marker) = aside_marker(&path) {
            if is_old(&path, now) && (!reap || settle_aside(&path, &marker, &record.admin_dir)) {
                out.reapable.push(path);
            }
        } else if super::era_marker_stale(&path, &record.admin_dir)
            && (!reap || super::drop_stale_era_marker(anchor, &path, &record.admin_dir))
        {
            out.reapable.push(path);
        }
    }
}

/// The marker a drop aside (`.<marker>.drop.<id>`, left by a build that
/// renamed markers aside to drop them) belongs to.
// trace:TASK-b6b1278890774ef993d2ebc327862470-067 | ai:claude
fn aside_marker(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let (marker, _) = name.strip_prefix('.')?.split_once(".drop.")?;
    Some(path.with_file_name(marker))
}

/// Settle an old drop aside: a fresh one whose marker is absent (its
/// dropper crashed before linking it back) is linked back first, never over
/// a marker that stands; then the aside is removed. Gone counts as settled.
// trace:TASK-b6b1278890774ef993d2ebc327862470-067 | ai:claude
fn settle_aside(aside: &Path, marker: &Path, admin_dir: &Path) -> bool {
    if !super::era_marker_stale(aside, admin_dir) {
        match std::fs::hard_link(aside, marker) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
            Err(_) => return false,
        }
    }
    match std::fs::remove_file(aside) {
        Ok(()) => true,
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

/// Is `path` a real directory last changed at least [`STALE_AFTER`] before
/// `now`? Its change time counts, so a rename into place restarts the clock.
fn is_stale_dir(path: &Path, now: SystemTime) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir()) && is_old(path, now)
}

/// Was `path` last changed (or renamed) at least [`STALE_AFTER`] before
/// `now`?
fn is_old(path: &Path, now: SystemTime) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    let changed = u64::try_from(meta.ctime())
        .ok()
        .map(|s| UNIX_EPOCH + Duration::new(s, u32::try_from(meta.ctime_nsec()).unwrap_or(0)));
    let latest = match (meta.modified().ok(), changed) {
        (Some(m), Some(c)) => m.max(c),
        (m, c) => match m.or(c) {
            Some(t) => t,
            None => return false,
        },
    };
    now.duration_since(latest)
        .is_ok_and(|age| age >= STALE_AFTER)
}

/// Remove an empty tomb: its subdirectories, its nonce, then itself. A file
/// that appears meanwhile makes a step fail and the tomb stays.
fn remove_empty_tomb(tomb: &Path) -> std::io::Result<()> {
    super::guard::remove_empty_tree_in(tomb, true)?;
    remove_nonce(tomb)?;
    std::fs::remove_dir(tomb)
}

/// Remove a staging directory: its nonce, then itself. Anything else in it
/// makes `remove_dir` fail and it stays.
fn remove_staging(dir: &Path) -> std::io::Result<()> {
    remove_nonce(dir)?;
    std::fs::remove_dir(dir)
}

fn remove_nonce(dir: &Path) -> std::io::Result<()> {
    let nonce = dir.join(super::SOURCE_ID_FILE);
    match std::fs::symlink_metadata(&nonce) {
        Ok(meta) if super::is_source_id(super::SOURCE_ID_FILE, &meta) => {
            std::fs::remove_file(&nonce)
        }
        Ok(_) => Err(std::io::Error::other(format!(
            "{} is not a creation nonce",
            nonce.display()
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
