//! Mail-source lifecycle: admitted-writer permits and the retention barrier
//! (STORY-1843 slice C1).
//!
//! Every local mailbox (`<checkout>/.aida/mailbox/`) is a *mail source*. Its
//! lifecycle state lives outside the checkout, under the repository's verified
//! common Git directory, so removing a worktree can never remove the record of
//! what was in it:
//!
//! ```text
//! <common-git-dir>/aida/mail-lifecycle/     mode 0700, owned by this uid
//!   lifecycle.lock                           the Lifecycle flock (bounded try)
//!   generation.json                          the anchor-wide generation counter
//!   sources/<key>.json                       one record per source: state + permits
//!   retained/<key>/<generation>/             preserved bytes + manifest.json
//!   journal/<op_id>.json                     the retirement journal
//!   claims/<key>.lock                        one retirement driver per source (flock)
//!   receipts.lock                            the Receipt flock (only under Lifecycle)
//!   receipts/<sha256(key)>.json              explicit per-ID ACK ledgers (B2c, [`ack`])
//! ```
//!
//! A writer is *admitted* before it writes: under a short Lifecycle hold it
//! refuses a fenced or retired source, otherwise it appends a permit row
//! stamped with the source's generation and releases the lock. It writes with
//! no lock held and removes its row when done. The retirement barrier
//! (`retire`) fences the source, waits for every permit to clear, preserves
//! the bytes, and commits only if a final fenced check proves nothing wrote
//! during the window.
//!
//! Every file here is opened `O_NOFOLLOW` and checked with `fstat`: it must be
//! a regular file owned by this uid with one link and under a size cap.
//! Symlinks, hardlinks and FIFOs are refused.
//!
//! Every destructive or reuse entry (worktree removal, pool reset, return
//! and destroy, session end, seat-clone teardown) retires the source first
//! through [`guard`], and keeps its tree when retirement refuses (C2).
//!
//! Hardening applies to what AIDA creates and owns: the anchor directories
//! (0700) and their files (0600), whatever the umask. The common Git
//! directory is the repository's, not AIDA's: it must be a real directory
//! owned by this uid, but it may be group-writable (umask 002, or Git's
//! `core.sharedRepository` layout).
//!
//! Degraded path: a writer that cannot establish the anchor (no Git, an
//! unusable or damaged anchor, a busy lock) writes exactly as it did before
//! C1, with no permit, and its [`AdmissionCoverage`] is `Uncovered`.
//! Only a source recorded as fenced or retired refuses a write. Retirement
//! is the side that fails closed: it refuses whatever it cannot prove, and
//! its final check catches a write that took no permit.
//!
//! trace:STORY-1843 | ai:claude

pub mod ack;
pub mod activation;
pub mod guard;
pub mod leftovers;
pub(crate) mod retire;

#[cfg(test)]
mod canonical_tests;
#[cfg(test)]
mod guard_tests;
#[cfg(test)]
mod leftovers_tests;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::process_probe::{Probe, ProbeSource, ProcessIdentity};

/// How long any Lifecycle acquisition waits before refusing (QC2). The lock is
/// never held across Git, network, hooks or another lock, so a longer wait
/// means something is wrong; the caller retries rather than blocks.
pub const LIFECYCLE_LOCK_TIMEOUT: Duration = Duration::from_secs(2);

/// The operator command that reports whether every writer host on the
/// machine is admitting (QC1). A mail-bearing source refuses retirement until
/// an activation is recorded; the same command without `--dry-run` records
/// it, for a human operator at a terminal only.
pub const ACTIVATION_COMMAND: &str = "aida mailbox retention activate --dry-run";

/// Directory under the common Git dir that holds the anchor.
const ANCHOR_PARENT: &str = "aida";
const ANCHOR_DIR: &str = "mail-lifecycle";
const LOCK_FILE: &str = "lifecycle.lock";
const GENERATION_FILE: &str = "generation.json";
pub(crate) const ACTIVATION_FILE: &str = "activation.json";
pub(crate) const SOURCES_DIR: &str = "sources";
pub(crate) const RETAINED_DIR: &str = "retained";
pub(crate) const JOURNAL_DIR: &str = "journal";
/// The creation nonce inside a mailbox directory. It names this creation of
/// the directory, is never mail, and leaves only with the directory.
// trace:BUG-b6b1278890774ef993d2ebc327862470-011 | ai:claude
pub const SOURCE_ID_FILE: &str = ".source-id";
/// `<anchor>/nonce-era/<sha256(common, admin)>`: present once a checkout's
/// mailbox has carried a creation nonce, so a later mailbox there without
/// one is refused instead of resolving to the bare legacy key.
// trace:BUG-b6b1278890774ef993d2ebc327862470-011 | ai:claude
pub(crate) const NONCE_ERA_DIR: &str = "nonce-era";

/// Size cap for any anchor record file.
pub(crate) const RECORD_CAP: u64 = 1 << 20;

/// Record schema version.
pub(crate) const SCHEMA: u32 = 1;

/// Why a lifecycle operation could not run.
#[derive(Debug)]
pub enum LifecycleError {
    /// The source is fenced or retired: a writer must not write to it.
    SourceRetiring {
        op_id: String,
    },
    /// The Lifecycle lock stayed contended past [`LIFECYCLE_LOCK_TIMEOUT`].
    LockTimeout {
        waited: Duration,
    },
    /// The acquisition would break the lock order (re-entry, or a Store lock
    /// held on this thread).
    LockOrder(&'static str),
    /// A path is not what the anchor allows (symlink, hardlink, FIFO, wrong
    /// owner or mode, over the size cap).
    Unsafe {
        path: PathBuf,
        why: String,
    },
    /// The retired checkout's mailbox still exists; remove it first.
    NotRemoved {
        path: PathBuf,
    },
    /// A record could not be parsed or contradicts its source.
    Corrupt {
        path: PathBuf,
        why: String,
    },
    Io(std::io::Error),
    /// A fixture crash point fired (tests only).
    #[cfg(test)]
    InjectedCrash(&'static str),
}

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LifecycleError::SourceRetiring { op_id } => write!(
                f,
                "this mailbox is being retired (operation {op_id}); the message was not written"
            ),
            LifecycleError::LockTimeout { waited } => write!(
                f,
                "the mailbox lifecycle lock stayed busy for {}s; retry",
                waited.as_secs_f32()
            ),
            LifecycleError::LockOrder(why) => write!(f, "mailbox lifecycle lock order: {why}"),
            LifecycleError::Unsafe { path, why } => {
                write!(
                    f,
                    "refusing mailbox lifecycle path {}: {why}",
                    path.display()
                )
            }
            LifecycleError::Corrupt { path, why } => {
                write!(
                    f,
                    "mailbox lifecycle record {} is damaged: {why}",
                    path.display()
                )
            }
            LifecycleError::NotRemoved { path } => write!(
                f,
                "{} still exists; remove the retired checkout before completing",
                path.display()
            ),
            LifecycleError::Io(e) => write!(f, "mailbox lifecycle: {e}"),
            #[cfg(test)]
            LifecycleError::InjectedCrash(at) => write!(f, "injected crash at {at}"),
        }
    }
}

impl std::error::Error for LifecycleError {}

impl From<std::io::Error> for LifecycleError {
    fn from(e: std::io::Error) -> Self {
        LifecycleError::Io(e)
    }
}

impl From<LifecycleError> for std::io::Error {
    fn from(e: LifecycleError) -> Self {
        match e {
            LifecycleError::Io(e) => e,
            other => std::io::Error::other(other),
        }
    }
}

pub(crate) type Result<T> = std::result::Result<T, LifecycleError>;

// ── Records ─────────────────────────────────────────────────────────────────

/// Where a source is in its lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceState {
    Open,
    Fenced {
        op_id: String,
        reason: String,
    },
    Retired {
        op_id: String,
        retained_dir: PathBuf,
    },
}

/// Which layer an admitted writer writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermitKind {
    Local,
    Canonical,
}

/// One admitted writer. `holder` is `None` when this platform cannot name
/// the process; retirement then treats the row's liveness as unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permit {
    pub permit_id: String,
    pub kind: PermitKind,
    pub holder: Option<ProcessIdentity>,
    pub generation: u64,
}

/// `sources/<key>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRecord {
    pub schema: u32,
    pub key: String,
    pub mailbox: PathBuf,
    pub admin: String,
    pub dev: u64,
    pub ino: u64,
    /// Minted under Lifecycle from the anchor-wide counter; never derived
    /// from a path or a timestamp.
    pub generation: u64,
    pub state: SourceState,
    pub permits: Vec<Permit>,
}

/// A resolved mail source: its anchor, key and on-disk identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Source {
    pub anchor: PathBuf,
    pub key: String,
    pub mailbox: PathBuf,
    pub admin: String,
    pub dev: u64,
    pub ino: u64,
}

// ── Source resolution ───────────────────────────────────────────────────────

/// Where a checkout's anchor is, once resolved: the common Git dir, the
/// worktree admin name and the anchor directory.
#[derive(Debug, Clone)]
struct Anchored {
    common: PathBuf,
    admin: String,
    anchor: PathBuf,
}

/// Identity of the Git directory a checkout's `.git` names, read without
/// running Git: its canonical path plus device and inode. A worktree removed
/// and re-added under another admin name, or a path re-cloned as another
/// repository, resolves to a different identity.
type GitDirIdentity = (PathBuf, u64, u64);

/// Per-process cache of [`Anchored`] by [`GitDirIdentity`], so a process that
/// writes many messages runs `git rev-parse` once per Git directory. The
/// identity and the mailbox are re-read on every call; an entry is dropped on
/// any lifecycle error.
fn anchored_cache() -> &'static Mutex<HashMap<GitDirIdentity, Anchored>> {
    static CACHE: OnceLock<Mutex<HashMap<GitDirIdentity, Anchored>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Follow `<checkout>/.git` (a directory, or a `gitdir:` file for a linked
/// worktree) to the Git directory and stat it. `None` when there is no
/// `.git` to read; the caller then asks Git and caches nothing.
fn git_dir_identity(project_root: &Path) -> Option<GitDirIdentity> {
    let dot_git = project_root.join(".git");
    let meta = std::fs::symlink_metadata(&dot_git).ok()?;
    let git_dir = if meta.is_dir() {
        dot_git
    } else if meta.is_file() && meta.len() <= 4096 {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let named = PathBuf::from(text.strip_prefix("gitdir:")?.trim());
        if named.is_absolute() {
            named
        } else {
            project_root.join(named)
        }
    } else {
        return None;
    };
    let git_dir = std::fs::canonicalize(git_dir).ok()?;
    let meta = std::fs::metadata(&git_dir).ok()?;
    meta.is_dir().then(|| (git_dir, meta.dev(), meta.ino()))
}

fn forget_anchor(project_root: &Path) {
    let Some(identity) = git_dir_identity(project_root) else {
        return;
    };
    if let Ok(mut cache) = anchored_cache().lock() {
        cache.remove(&identity);
    }
}

/// Resolve the mail source for the checkout at `project_root`, creating the
/// anchor if needed. `Ok(None)` when the checkout is not in a Git repository
/// (or host Git refuses to run there, as in a seat clone): such a source has
/// no anchor, takes no permit and cannot be retired. The mailbox directory
/// must already exist; its device and inode are part of the key.
pub(crate) fn resolve_source(project_root: &Path) -> Result<Option<Source>> {
    let identity = git_dir_identity(project_root);
    let cached = identity.as_ref().and_then(|id| {
        anchored_cache()
            .lock()
            .ok()
            .and_then(|cache| cache.get(id).cloned())
    });
    let anchored = match cached {
        // The anchor is re-checked (one lstat) so a swapped directory is
        // refused, not trusted from the cache.
        Some(anchored) => {
            check_owned_dir(&anchored.anchor, true)?;
            anchored
        }
        None => {
            let Some(anchored) = locate_anchor(project_root)? else {
                return Ok(None);
            };
            if let (Some(id), Ok(mut cache)) = (identity, anchored_cache().lock()) {
                cache.insert(id, anchored.clone());
            }
            anchored
        }
    };
    let mailbox = crate::mailbox::local_mailbox_dir(project_root);
    let meta = std::fs::metadata(&mailbox)?;
    if !meta.is_dir() {
        return Err(LifecycleError::Unsafe {
            path: mailbox,
            why: "the mailbox is not a directory".into(),
        });
    }
    let nonce = source_nonce(
        &anchored.anchor,
        &anchored.common,
        &anchored.admin,
        &mailbox,
    )?;
    let key = source_key(
        &anchored.common,
        &anchored.admin,
        meta.dev(),
        meta.ino(),
        nonce.as_deref(),
    );
    Ok(Some(Source {
        anchor: anchored.anchor,
        key,
        mailbox,
        admin: anchored.admin,
        dev: meta.dev(),
        ino: meta.ino(),
    }))
}

/// Resolve a seat clone's mail source under `main`'s anchor. The clone's
/// own Git is never asked (host Git refuses to run there); the caller has
/// verified the host-recorded binding, and the key is derived from it: main's
/// common Git dir, the clone root, and the clone mailbox's device and inode.
pub(crate) fn resolve_clone_source(main: &Path, clone_root: &Path) -> Result<Option<Source>> {
    let Some(anchored) = locate_anchor(main)? else {
        return Ok(None);
    };
    let clone_root = std::fs::canonicalize(clone_root)?;
    let admin = format!("seat-clone:{}", clone_root.display());
    let mailbox = crate::mailbox::local_mailbox_dir(&clone_root);
    let meta = std::fs::symlink_metadata(&mailbox)?;
    if !meta.file_type().is_dir() {
        return Err(LifecycleError::Unsafe {
            path: mailbox,
            why: "the mailbox is not a real directory".into(),
        });
    }
    let nonce = source_nonce(&anchored.anchor, &anchored.common, &admin, &mailbox)?;
    let key = source_key(
        &anchored.common,
        &admin,
        meta.dev(),
        meta.ino(),
        nonce.as_deref(),
    );
    Ok(Some(Source {
        anchor: anchored.anchor,
        key,
        mailbox,
        admin,
        dev: meta.dev(),
        ino: meta.ino(),
    }))
}

/// Ask Git for the common and per-worktree Git dirs, verify them and create
/// the anchor. Runs Git, so it is never called under the Lifecycle lock.
fn locate_anchor(project_root: &Path) -> Result<Option<Anchored>> {
    let out = crate::git_ops::host_git_command(project_root)
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
            "--git-dir",
        ])
        .stdin(std::process::Stdio::null())
        .output()?;
    if !out.status.success() {
        return Ok(None);
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines = stdout.lines();
    let (Some(common), Some(git_dir)) = (lines.next(), lines.next()) else {
        return Ok(None);
    };
    let common = std::fs::canonicalize(common)?;
    let git_dir = std::fs::canonicalize(git_dir)?;
    check_repository_dir(&common)?;
    let admin = if git_dir == common {
        String::new()
    } else if git_dir.parent() == Some(common.join("worktrees").as_path()) {
        git_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string()
    } else {
        return Err(LifecycleError::Unsafe {
            path: git_dir,
            why: format!(
                "the Git directory is neither the common directory {} nor one of its worktrees",
                common.display()
            ),
        });
    };
    let anchor = ensure_anchor(&common)?;
    Ok(Some(Anchored {
        common,
        admin,
        anchor,
    }))
}

/// SHA-256 over the common Git dir, the worktree admin name, the mailbox
/// directory's device and inode and its creation nonce. A freed inode is
/// often handed straight back, so device and inode alone let a recreated
/// mailbox inherit the retired source's key; the nonce differs per creation.
/// A legacy directory with no nonce keeps the key it always had.
// trace:BUG-b6b1278890774ef993d2ebc327862470-011 | ai:claude
fn source_key(common: &Path, admin: &str, dev: u64, ino: u64, nonce: Option<&str>) -> String {
    let mut h = Sha256::new();
    h.update(common.as_os_str().as_encoded_bytes());
    h.update([0]);
    h.update(admin.as_bytes());
    h.update([0]);
    h.update(format!("{dev}:{ino}").as_bytes());
    if let Some(nonce) = nonce {
        h.update([0]);
        h.update(nonce.as_bytes());
    }
    hex(&h.finalize())
}

/// Hex digits in a creation nonce (128 bits).
const SOURCE_ID_HEX_LEN: usize = 32;

/// The creation nonce of the mailbox directory `mailbox`. `Ok(None)` for a
/// legacy directory that has none. One that exists but cannot be read, is
/// not a regular file of ours, or is malformed refuses as
/// [`LifecycleError::Unsafe`]: guessing a key would alias another source.
// trace:BUG-b6b1278890774ef993d2ebc327862470-011 | ai:claude
pub(crate) fn read_source_id(mailbox: &Path) -> Result<Option<String>> {
    let path = mailbox.join(SOURCE_ID_FILE);
    let unsafe_id = |why: String| LifecycleError::Unsafe {
        path: path.clone(),
        why,
    };
    let bytes = match read_checked_capped(&path, SOURCE_ID_HEX_LEN as u64 + 1) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Ok(None),
        Err(LifecycleError::Io(e)) => return Err(unsafe_id(format!("is unreadable: {e}"))),
        Err(e) => return Err(e),
    };
    let text = std::str::from_utf8(&bytes).unwrap_or_default();
    let nonce = text.strip_suffix('\n').unwrap_or(text);
    if nonce.len() != SOURCE_ID_HEX_LEN
        || !nonce
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(unsafe_id("is not a creation nonce".into()));
    }
    Ok(Some(nonce.to_string()))
}

/// The creation nonce of a checkout's mailbox, checked against the
/// checkout's nonce era. A nonce marks the era; a mailbox without one in a
/// checkout whose era has begun lost its nonce (deleted, or an older build
/// moved it as mail) and refuses as [`LifecycleError::Unsafe`], naming the
/// operator's recovery. Without the marker a nonce-less mailbox is legacy
/// and keeps its old key. A marker whose worktree admin entry was pruned or
/// recreated since is dropped first: it described another checkout.
// trace:BUG-b6b1278890774ef993d2ebc327862470-011 | ai:claude
// trace:TASK-b6b1278890774ef993d2ebc327862470-052 | ai:claude
fn source_nonce(
    anchor: &Path,
    common: &Path,
    admin: &str,
    mailbox: &Path,
) -> Result<Option<String>> {
    let marker = era_marker(anchor, common, admin);
    let dir = admin_dir(common, admin);
    if era_marker_stale(&marker, &dir) {
        #[cfg(test)]
        if let Some(hook) = BEFORE_ERA_DROP.with(|h| h.borrow_mut().take()) {
            hook(&marker);
        }
        drop_stale_era_marker(anchor, &marker, &dir);
    }
    match read_source_id(mailbox)? {
        Some(nonce) => {
            mark_nonce_era(&marker, common, admin)?;
            Ok(Some(nonce))
        }
        None if std::fs::symlink_metadata(&marker).is_ok() => Err(LifecycleError::Unsafe {
            path: mailbox.join(SOURCE_ID_FILE),
            why: format!(
                "is missing from a mailbox whose checkout has carried one (nonce era marker \
                 {}), so its source cannot be told apart from an earlier one. An older aida or \
                 aida-server may have created it: restart those on this build. Then move the \
                 mailbox aside (`mv {} {}.orphaned`) so the next write creates a new one with \
                 its own nonce, and review any messages in the moved directory by hand",
                marker.display(),
                mailbox.display(),
                mailbox.display()
            ),
        }),
        None => Ok(None),
    }
}

/// What a nonce-era marker records: the worktree admin entry it is keyed on
/// and that entry's identity when the era began, so a pruned or recreated
/// entry is told from the one that carried the nonce. A marker from before
/// this record (the bare admin name) has no identity and is dropped only
/// once its admin entry is gone.
// trace:TASK-b6b1278890774ef993d2ebc327862470-052 | ai:claude
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct EraMarker {
    pub admin: String,
    pub admin_dir: PathBuf,
    pub dev: u64,
    pub ino: u64,
}

/// The directory a worktree admin name stands for: the common Git dir for
/// the main checkout, `<common>/worktrees/<admin>` for a linked worktree,
/// the clone root for a seat clone.
// trace:TASK-b6b1278890774ef993d2ebc327862470-052 | ai:claude
fn admin_dir(common: &Path, admin: &str) -> PathBuf {
    if admin.is_empty() {
        common.to_path_buf()
    } else if let Some(root) = admin.strip_prefix("seat-clone:") {
        PathBuf::from(root)
    } else {
        common.join("worktrees").join(admin)
    }
}

/// Is the nonce-era marker at `marker` stale: its admin entry (`admin_dir`
/// for a pre-record marker) gone, or recreated with another identity?
/// Absent, unreadable or undecidable is not stale (fail closed). Identity is
/// the admin entry's (dev, ino) only: an entry recreated on a reused inode
/// looks current, so its marker is kept and a nonce-less mailbox there keeps
/// refusing until the operator recovers it (fail closed).
// trace:TASK-b6b1278890774ef993d2ebc327862470-052 | ai:claude
pub(crate) fn era_marker_stale(marker: &Path, admin_dir: &Path) -> bool {
    let Ok(Some(bytes)) = read_checked(marker) else {
        return false;
    };
    let (dir, recorded) = match serde_json::from_slice::<EraMarker>(&bytes) {
        Ok(m) => (m.admin_dir, Some((m.dev, m.ino))),
        Err(_) => (admin_dir.to_path_buf(), None),
    };
    match std::fs::symlink_metadata(&dir) {
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        Ok(meta) => recorded.is_some_and(|id| id != (meta.dev(), meta.ino())),
    }
}

/// Remove the nonce-era marker at `marker` if it is stale against
/// `admin_dir`, judged again under the Lifecycle lock. The marker path is
/// never absent while a fresh marker exists: every drop runs under the lock,
/// and a marker is only ever created where none is, so the file judged here
/// is the file unlinked. Returns whether it was removed. When the lock
/// cannot be taken (busy, or a Store lock is held) the marker is kept, which
/// fails closed: a nonce-less mailbox there keeps refusing.
// trace:TASK-b6b1278890774ef993d2ebc327862470-067 | ai:claude
pub(crate) fn drop_stale_era_marker(anchor: &Path, marker: &Path, admin_dir: &Path) -> bool {
    let Ok(_held) = lock(anchor, LIFECYCLE_LOCK_TIMEOUT) else {
        return false;
    };
    if !era_marker_stale(marker, admin_dir) {
        return false;
    }
    match std::fs::remove_file(marker) {
        Ok(()) => true,
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

/// `<anchor>/nonce-era/<sha256(common, admin)>`.
fn era_marker(anchor: &Path, common: &Path, admin: &str) -> PathBuf {
    let mut h = Sha256::new();
    h.update(common.as_os_str().as_encoded_bytes());
    h.update([0]);
    h.update(admin.as_bytes());
    anchor.join(NONCE_ERA_DIR).join(hex(&h.finalize()))
}

/// The nonce-era marker, common Git dir and admin name of the source whose
/// mailbox lives at `<project_root>/.aida/mailbox`, resolved as
/// [`writer_source`] does. `Ok(None)` when it has no anchor. Runs Git; never
/// under the lock.
fn era_marker_for(project_root: &Path) -> Result<Option<(PathBuf, PathBuf, String)>> {
    let (anchored, admin) = match crate::seat_clone_detect::seat_clone_root(project_root) {
        Some(clone_root) => {
            let Some(main) = crate::seat_clone_detect::clone_writer_main(&clone_root) else {
                return Ok(None);
            };
            let Some(anchored) = locate_anchor(&main)? else {
                return Ok(None);
            };
            let clone_root = std::fs::canonicalize(clone_root)?;
            (anchored, format!("seat-clone:{}", clone_root.display()))
        }
        None => {
            let Some(anchored) = locate_anchor(project_root)? else {
                return Ok(None);
            };
            let admin = anchored.admin.clone();
            (anchored, admin)
        }
    };
    let marker = era_marker(&anchored.anchor, &anchored.common, &admin);
    Ok(Some((marker, anchored.common, admin)))
}

/// Record, create-new, that the checkout's mailbox has carried a nonce,
/// with the identity of its admin entry (see [`EraMarker`]).
// trace:TASK-b6b1278890774ef993d2ebc327862470-052 | ai:claude
fn mark_nonce_era(marker: &Path, common: &Path, admin: &str) -> Result<()> {
    if std::fs::symlink_metadata(marker).is_ok() {
        return Ok(());
    }
    if let Some(dir) = marker.parent() {
        ensure_dir(dir, true)?;
    }
    // An admin entry that cannot be identified leaves a marker with none,
    // which is only ever dropped once the entry is gone.
    let admin_dir = admin_dir(common, admin);
    let record = match std::fs::symlink_metadata(&admin_dir) {
        Ok(meta) => serde_json::to_vec(&EraMarker {
            admin: admin.to_string(),
            admin_dir,
            dev: meta.dev(),
            ino: meta.ino(),
        })
        .map_err(|e| LifecycleError::Corrupt {
            path: marker.to_path_buf(),
            why: e.to_string(),
        })?,
        Err(_) => admin.as_bytes().to_vec(),
    };
    match write_source_id(marker, &record) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        other => Ok(other?),
    }
}

/// Rename `from` to `to` unless `to` exists, atomically: `EEXIST` when it
/// does. A plain rename silently replaces an empty directory.
// trace:BUG-b6b1278890774ef993d2ebc327862470-011 | ai:claude
pub(crate) fn rename_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = |p: &Path| {
        CString::new(p.as_os_str().as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
    };
    let (from, to) = (c(from)?, c(to)?);
    #[cfg(test)]
    if NOREPLACE_UNSUPPORTED.with(std::cell::Cell::get) {
        return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
    }
    #[cfg(target_os = "linux")]
    // SAFETY: both paths are NUL-terminated and outlive the call.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    // SAFETY: both paths are NUL-terminated and outlive the call.
    let rc = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let rc = {
        // No atomic no-replace rename here: check, then rename.
        if std::fs::symlink_metadata(std::ffi::OsStr::from_bytes(to.as_bytes())).is_ok() {
            return Err(std::io::Error::from_raw_os_error(libc::EEXIST));
        }
        // SAFETY: both paths are NUL-terminated and outlive the call.
        unsafe { libc::rename(from.as_ptr(), to.as_ptr()) }
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Does this rename error mean the filesystem cannot rename without
/// replacing, rather than that the rename itself failed?
pub(crate) fn noreplace_unsupported(e: &std::io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::EINVAL | libc::ENOSYS | libc::ENOTSUP)
    )
}

#[cfg(test)]
thread_local! {
    /// Makes [`rename_noreplace`] fail as on a filesystem without it.
    pub(crate) static NOREPLACE_UNSUPPORTED: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
    /// Runs between the in-place `mkdir` and its nonce.
    pub(crate) static IN_PLACE_WINDOW: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    /// Runs between judging a nonce-era marker stale and dropping it.
    pub(crate) static BEFORE_ERA_DROP: std::cell::RefCell<Option<Box<dyn FnOnce(&Path)>>> =
        const { std::cell::RefCell::new(None) };
    /// Runs between staging a new mailbox and renaming it into place.
    pub(crate) static BEFORE_PLACE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Create the mailbox directory `mailbox` if it is absent, born holding a
/// fresh creation nonce: the directory and its `.source-id` are staged beside
/// it and renamed into place, so no reader ever sees the new directory
/// without its nonce. A directory that already exists (or that a concurrent
/// creator renamed in first) is kept as it is. Where the filesystem cannot
/// rename without replacing, see [`create_mailbox_dir_in_place`].
// trace:BUG-b6b1278890774ef993d2ebc327862470-011 | ai:claude
pub fn create_mailbox_dir(mailbox: &Path) -> std::io::Result<()> {
    match std::fs::metadata(mailbox) {
        Ok(meta) if meta.is_dir() => return Ok(()),
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{} is not a directory", mailbox.display()),
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let (Some(parent), Some(name)) = (mailbox.parent(), mailbox.file_name()) else {
        return Err(std::io::Error::other(format!(
            "{} has no parent directory",
            mailbox.display()
        )));
    };
    std::fs::create_dir_all(parent)?;
    // A crashed creator's staging is reaped once it is old enough that no
    // live creator can still be placing it.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-052 | ai:claude
    leftovers::reap_stale_staging(
        parent,
        &name.to_string_lossy(),
        std::time::SystemTime::now(),
    );
    let staging = parent.join(format!(
        ".{}.creating.{}",
        name.to_string_lossy(),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir(&staging)?;
    let staged = mint_source_id(&staging).and_then(|()| {
        #[cfg(test)]
        if let Some(hook) = BEFORE_PLACE.with(|h| h.borrow_mut().take()) {
            hook();
        }
        rename_noreplace(&staging, mailbox)
    });
    match staged {
        Ok(()) => File::open(parent)?.sync_all(),
        Err(e) => {
            let _ = std::fs::remove_file(staging.join(SOURCE_ID_FILE));
            let _ = std::fs::remove_dir(&staging);
            if noreplace_unsupported(&e) {
                return create_mailbox_dir_in_place(mailbox);
            }
            // EEXIST: another creator placed the mailbox first; keep it.
            if e.raw_os_error() == Some(libc::EEXIST)
                && std::fs::metadata(mailbox).is_ok_and(|m| m.is_dir())
            {
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}

/// [`create_mailbox_dir`] on a filesystem without a no-replace rename,
/// ordered to fail closed: the checkout's nonce era is recorded first, then
/// the directory is made in place and its nonce written. A resolver in the
/// window between the two finds the era begun and no nonce, and refuses
/// (retriably) instead of keying the directory as legacy.
// trace:BUG-b6b1278890774ef993d2ebc327862470-011 | ai:claude
fn create_mailbox_dir_in_place(mailbox: &Path) -> std::io::Result<()> {
    let root = mailbox.parent().and_then(Path::parent).unwrap_or(mailbox);
    if let Some((marker, common, admin)) = era_marker_for(root)? {
        mark_nonce_era(&marker, &common, &admin)?;
    }
    match std::fs::create_dir(mailbox) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(e) => return Err(e),
    }
    #[cfg(test)]
    if let Some(hook) = IN_PLACE_WINDOW.with(|h| h.borrow_mut().take()) {
        hook();
    }
    mint_source_id(mailbox)?;
    match mailbox.parent() {
        Some(parent) => File::open(parent)?.sync_all(),
        None => Ok(()),
    }
}

/// Write 128 random bits as `dir/.source-id`: create-new, mode 0600, synced.
fn mint_source_id(dir: &Path) -> std::io::Result<()> {
    let mut nonce = [0u8; SOURCE_ID_HEX_LEN / 2];
    File::open("/dev/urandom")?.read_exact(&mut nonce)?;
    write_source_id(
        &dir.join(SOURCE_ID_FILE),
        format!("{}\n", hex(&nonce)).as_bytes(),
    )
}

/// Write a nonce (or era marker) file create-new, mode 0600, synced with
/// its directory.
fn write_source_id(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    match path.parent() {
        Some(dir) => File::open(dir)?.sync_all(),
        None => Ok(()),
    }
}

/// Is this mailbox-root entry the creation nonce rather than mail?
pub(crate) fn is_source_id(name: &str, meta: &std::fs::Metadata) -> bool {
    name == SOURCE_ID_FILE && meta.file_type().is_file()
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn ensure_anchor(common: &Path) -> Result<PathBuf> {
    let parent = common.join(ANCHOR_PARENT);
    ensure_dir(&parent, false)?;
    let anchor = parent.join(ANCHOR_DIR);
    ensure_dir(&anchor, true)?;
    for sub in [SOURCES_DIR, RETAINED_DIR, JOURNAL_DIR] {
        ensure_dir(&anchor.join(sub), true)?;
    }
    Ok(anchor)
}

/// Create `dir` (mode 0700) if absent, then require a real directory owned by
/// this uid; `private` also requires that no group or other bit is set,
/// otherwise only the group/other write bits are refused.
pub(crate) fn ensure_dir(dir: &Path, private: bool) -> Result<()> {
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    check_owned_dir(dir, private)
}

/// The common Git dir belongs to the repository, not to AIDA: require a real
/// directory owned by this uid, and leave its mode alone (umask 002 and
/// `core.sharedRepository` make it group-writable).
fn check_repository_dir(dir: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(dir)?;
    let unsafe_dir = |why: String| LifecycleError::Unsafe {
        path: dir.to_path_buf(),
        why,
    };
    if !meta.file_type().is_dir() {
        return Err(unsafe_dir("not a directory (or a symlink)".into()));
    }
    if meta.uid() != euid() {
        return Err(unsafe_dir(format!("owned by uid {}", meta.uid())));
    }
    Ok(())
}

fn check_owned_dir(dir: &Path, private: bool) -> Result<()> {
    let meta = std::fs::symlink_metadata(dir)?;
    let unsafe_dir = |why: String| LifecycleError::Unsafe {
        path: dir.to_path_buf(),
        why,
    };
    if !meta.file_type().is_dir() {
        return Err(unsafe_dir("not a directory (or a symlink)".into()));
    }
    if meta.uid() != euid() {
        return Err(unsafe_dir(format!("owned by uid {}", meta.uid())));
    }
    let forbidden = if private { 0o077 } else { 0o022 };
    if meta.mode() & forbidden != 0 {
        return Err(unsafe_dir(format!(
            "mode {:o} is too open",
            meta.mode() & 0o777
        )));
    }
    Ok(())
}

pub(crate) fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

// ── Safe IO ─────────────────────────────────────────────────────────────────

/// Refuse anything but a regular file owned by this uid with one link and at
/// most `cap` bytes.
pub(crate) fn check_regular(meta: &std::fs::Metadata, path: &Path, cap: u64) -> Result<()> {
    let refuse = |why: String| {
        Err(LifecycleError::Unsafe {
            path: path.to_path_buf(),
            why,
        })
    };
    if !meta.file_type().is_file() {
        return refuse("not a regular file".into());
    }
    if meta.nlink() != 1 {
        return refuse(format!("has {} hard links", meta.nlink()));
    }
    if meta.uid() != euid() {
        return refuse(format!("owned by uid {}", meta.uid()));
    }
    if meta.len() > cap {
        return refuse(format!("{} bytes is over the {cap}-byte cap", meta.len()));
    }
    Ok(())
}

/// Open `path` without following a symlink and without blocking on a FIFO,
/// then `fstat` it. `Ok(None)` when it does not exist.
pub(crate) fn open_checked(path: &Path, cap: u64) -> Result<Option<File>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(LifecycleError::Unsafe {
                path: path.to_path_buf(),
                why: "is a symlink".into(),
            })
        }
        Err(e) => return Err(e.into()),
    };
    check_regular(&file.metadata()?, path, cap)?;
    Ok(Some(file))
}

/// Read a capped anchor file; `Ok(None)` when absent.
pub(crate) fn read_checked(path: &Path) -> Result<Option<Vec<u8>>> {
    read_checked_capped(path, RECORD_CAP)
}

/// [`read_checked`] with an explicit byte cap.
pub(crate) fn read_checked_capped(path: &Path, cap: u64) -> Result<Option<Vec<u8>>> {
    let Some(file) = open_checked(path, cap)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(cap + 1).read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

static STAGING_SEQ: AtomicU64 = AtomicU64::new(0);

/// Write `bytes` to `path` durably: a fresh `O_EXCL | O_NOFOLLOW` temp file
/// (mode 0600) is written and fsynced, renamed over the target, and the
/// directory is fsynced. An existing target must pass [`check_regular`].
pub(crate) fn write_durable(path: &Path, bytes: &[u8]) -> Result<()> {
    write_replace(path, bytes, true)
}

/// [`write_durable`] without the fsyncs when `sync` is false: the rename is
/// still atomic for every reader on this machine, but a crash of the machine
/// may lose the write.
fn write_replace(path: &Path, bytes: &[u8], sync: bool) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => check_regular(&meta, path, u64::MAX)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("record");
    let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.tmp.{}.{seq}", std::process::id()));
    let staged = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)?;
        file.write_all(bytes)?;
        if sync {
            file.sync_all()?;
        }
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = staged {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    if sync {
        fsync_dir(dir)?;
    }
    Ok(())
}

pub(crate) fn fsync_dir(dir: &Path) -> Result<()> {
    File::open(dir)?.sync_all()?;
    Ok(())
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let Some(bytes) = read_checked(path)? else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| LifecycleError::Corrupt {
            path: path.to_path_buf(),
            why: e.to_string(),
        })
}

pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| LifecycleError::Corrupt {
        path: path.to_path_buf(),
        why: e.to_string(),
    })?;
    write_durable(path, &bytes)
}

// ── The Lifecycle lock ──────────────────────────────────────────────────────

thread_local! {
    static HELD: Cell<bool> = const { Cell::new(false) };
    static OBSERVED: RefCell<Option<Vec<&'static str>>> = const { RefCell::new(None) };
}

/// True while this thread holds the Lifecycle lock. The Store write lock
/// refuses to start inside a Lifecycle hold.
pub fn lifecycle_lock_held() -> bool {
    HELD.with(Cell::get)
}

/// Record that this thread is entering `class` (a Git command, the worktree
/// pool lock, a seat permit). None of them may start inside a Lifecycle
/// hold: the Lifecycle lock is a leaf.
pub fn enter_lock_class(class: &'static str) {
    let violation = lifecycle_lock_held();
    debug_assert!(
        !violation,
        "lock-order violation: {class} entered while the mailbox lifecycle lock is held"
    );
    OBSERVED.with(|o| {
        if let Some(seen) = o.borrow_mut().as_mut() {
            // Recorded in every build, so an optimized test run still sees
            // the violation the debug assertion would have raised.
            if violation {
                seen.push(UNDER_LIFECYCLE);
            }
            seen.push(class);
        }
    });
}

/// Record that this thread entered `class` from inside a Lifecycle hold
/// where the lock order allows it (Lifecycle then Receipt). Observation only.
pub(crate) fn note_lock_class(class: &'static str) {
    OBSERVED.with(|o| {
        if let Some(seen) = o.borrow_mut().as_mut() {
            seen.push(class);
        }
    });
}

/// Pushed by the observer right before a class entered while the Lifecycle
/// lock was held: a lock-order violation.
#[doc(hidden)]
pub const UNDER_LIFECYCLE: &str = "under-lifecycle";

/// Run `f` and return the lock classes this thread entered meanwhile, in
/// order (test seam, same role as `store_write_lock_contended_for`).
#[doc(hidden)]
pub fn observe_lock_classes<T>(f: impl FnOnce() -> T) -> (T, Vec<&'static str>) {
    let previous = OBSERVED.with(|o| o.borrow_mut().replace(Vec::new()));
    let out = f();
    let seen = OBSERVED.with(|o| std::mem::replace(&mut *o.borrow_mut(), previous));
    (out, seen.unwrap_or_default())
}

/// A held Lifecycle lock. It is a leaf: while it is held this thread takes no
/// other lock and runs no Git, network or hook command.
#[must_use = "the Lifecycle lock is released when the guard drops"]
pub(crate) struct LifecycleGuard {
    file: File,
}

impl Drop for LifecycleGuard {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
        HELD.with(|h| h.set(false));
    }
}

/// Take the anchor's Lifecycle lock with a bounded try.
pub(crate) fn lock(anchor: &Path, timeout: Duration) -> Result<LifecycleGuard> {
    if lifecycle_lock_held() {
        return Err(LifecycleError::LockOrder(
            "the Lifecycle lock is not re-entrant",
        ));
    }
    if crate::db::any_store_write_lock_held() {
        return Err(LifecycleError::LockOrder(
            "the Lifecycle lock is never taken while a Store write lock is held",
        ));
    }
    let path = anchor.join(LOCK_FILE);
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
            })
        }
        Err(e) => return Err(e.into()),
    };
    check_regular(&file.metadata()?, &path, 0)?;
    let start = Instant::now();
    loop {
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => break,
            Err(e) if crate::file_lock::is_lock_contended(&e) => {
                if start.elapsed() >= timeout {
                    return Err(LifecycleError::LockTimeout {
                        waited: start.elapsed(),
                    });
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(e.into()),
        }
    }
    enter_lock_class("lifecycle");
    HELD.with(|h| h.set(true));
    Ok(LifecycleGuard { file })
}

pub(crate) fn record_path(anchor: &Path, key: &str) -> PathBuf {
    anchor.join(SOURCES_DIR).join(format!("{key}.json"))
}

/// Mint the next generation from the anchor-wide counter. Lifecycle held.
pub(crate) fn mint_generation(anchor: &Path, _held: &LifecycleGuard) -> Result<u64> {
    #[derive(Serialize, Deserialize)]
    struct Counter {
        next: u64,
    }
    let path = anchor.join(GENERATION_FILE);
    let next = read_json::<Counter>(&path)?.map_or(1, |c| c.next);
    write_json(&path, &Counter { next: next + 1 })?;
    Ok(next)
}

/// Load the record for `key`. Lifecycle held.
pub(crate) fn load_record(
    anchor: &Path,
    key: &str,
    _held: &LifecycleGuard,
) -> Result<Option<SourceRecord>> {
    let path = record_path(anchor, key);
    let record = read_json::<SourceRecord>(&path)?;
    if let Some(r) = &record {
        if r.key != key {
            return Err(LifecycleError::Corrupt {
                path,
                why: format!("the record names key {}", r.key),
            });
        }
    }
    Ok(record)
}

/// Load the record for `source`, creating an `Open` one with a fresh
/// generation if there is none. Lifecycle held.
pub(crate) fn load_or_create(source: &Source, held: &LifecycleGuard) -> Result<SourceRecord> {
    if let Some(record) = load_record(&source.anchor, &source.key, held)? {
        return Ok(record);
    }
    let record = SourceRecord {
        schema: SCHEMA,
        key: source.key.clone(),
        mailbox: source.mailbox.clone(),
        admin: source.admin.clone(),
        dev: source.dev,
        ino: source.ino,
        generation: mint_generation(&source.anchor, held)?,
        state: SourceState::Open,
        permits: Vec::new(),
    };
    store_record(&source.anchor, &record, held)?;
    Ok(record)
}

/// Persist `record`. Lifecycle held.
pub(crate) fn store_record(
    anchor: &Path,
    record: &SourceRecord,
    _held: &LifecycleGuard,
) -> Result<()> {
    write_json(&record_path(anchor, &record.key), record)
}

/// Persist a permit-row change. Lifecycle held. While the source is `Open`
/// the write skips the fsyncs: a row only has to be visible to processes on
/// this machine, and a machine crash ends every writer it could name. A
/// fenced or retired record is always written durably.
fn store_permits(anchor: &Path, record: &SourceRecord, _held: &LifecycleGuard) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(record).map_err(|e| LifecycleError::Corrupt {
        path: record_path(anchor, &record.key),
        why: e.to_string(),
    })?;
    let sync = record.state != SourceState::Open;
    write_replace(&record_path(anchor, &record.key), &bytes, sync)
}

/// Remove the record for `key`. Lifecycle held.
pub(crate) fn delete_record(anchor: &Path, key: &str, _held: &LifecycleGuard) -> Result<()> {
    let path = record_path(anchor, key);
    match std::fs::remove_file(&path) {
        Ok(()) => fsync_dir(&anchor.join(SOURCES_DIR)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// `activation.json`: the operator's record that every writer host on this
/// machine admits before writing (QC1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Activation {
    pub schema: u32,
    pub all_writer_hosts_admitting: bool,
    pub recorded_at: String,
    /// The source build that recorded it ([`source_build`]): the version and
    /// git SHA that `aida` and `aida-server` built from one checkout share.
    /// `None` in a record written before records named their source build
    /// (those named an executable's inode), which no build honours.
    #[serde(default)]
    pub source_build: Option<String>,
}

static SOURCE_BUILD: OnceLock<String> = OnceLock::new();

/// The identity of a build of `version` at `git_sha`, or `None` when the SHA
/// is unknown or the tree had uncommitted changes (`dirty`): a build that
/// cannot name its exact source never honours or records an activation.
/// R4-F5: two dirty builds of one SHA can differ in their local edits (one
/// may drop admission), so a shared `-dirty` identity would let one honour
/// the other's record, the R2-F6 rollback hazard.
// trace:STORY-1843 | ai:claude
pub fn source_build_id(version: &str, git_sha: &str, dirty: bool) -> Option<String> {
    let sha = git_sha.trim();
    if dirty || sha.is_empty() || sha == "unknown" {
        return None;
    }
    Some(format!("{version}+{sha}"))
}

/// Name this process's source build, once, at startup. Each binary that can
/// retire mail calls it with its own build stamps; a process that never
/// does has no source build and honours no activation (fail closed).
// trace:STORY-1843 | ai:claude
pub fn set_source_build(version: &str, git_sha: &str, dirty: bool) {
    if let Some(id) = source_build_id(version, git_sha, dirty) {
        let _ = SOURCE_BUILD.set(id);
    }
}

/// This process's source build, as [`set_source_build`] named it.
pub fn source_build() -> Option<&'static str> {
    SOURCE_BUILD.get().map(String::as_str)
}

/// Whether an activation is recorded under an anchor, for this build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationState {
    /// Nothing is recorded.
    Absent,
    /// Recorded by this source build.
    Current,
    /// Recorded by another source build (`None`: one that named none), so
    /// this build must record its own. `this` is this build's identity.
    OtherBuild {
        recorded: Option<String>,
        this: Option<String>,
    },
}

/// The activation recorded under `anchor`, judged against this process's
/// source build. A record made from another source (an older binary after
/// `aida dev rollback`, or a newer one) does not count: that build must
/// record its own. `aida-server` and a re-promoted build of the same source
/// share the identity, so they honour it.
// trace:STORY-1843 | ai:claude
pub fn activation_state(anchor: &Path) -> Result<ActivationState> {
    let Some(record) = read_json::<Activation>(&anchor.join(ACTIVATION_FILE))? else {
        return Ok(ActivationState::Absent);
    };
    if !record.all_writer_hosts_admitting {
        return Ok(ActivationState::Absent);
    }
    let this = source_build();
    Ok(match (&record.source_build, this) {
        (Some(recorded), Some(this)) if recorded == this => ActivationState::Current,
        _ => ActivationState::OtherBuild {
            recorded: record.source_build,
            this: this.map(str::to_string),
        },
    })
}

/// Has an operator recorded, with this source build, that every writer host
/// is admitting ([`activation::record`])?
pub(crate) fn activated(anchor: &Path) -> Result<bool> {
    Ok(activation_state(anchor)? == ActivationState::Current)
}

// ── Local writer admission ──────────────────────────────────────────────────

/// An admitted local write. Call [`LocalPermit::complete`] after the write;
/// dropping it without that removes the row on a best-effort basis.
#[must_use = "a permit row stays until the permit completes or drops"]
#[derive(Debug)]
pub struct LocalPermit {
    anchor: PathBuf,
    key: String,
    permit_id: String,
    done: bool,
}

impl LocalPermit {
    /// Remove the row. On failure the permit drops undone, so `Drop` tries
    /// once more; a row that still survives is reaped by the next admission
    /// once this process is proven dead.
    pub fn complete(mut self) -> Result<()> {
        release_permit(&self.anchor, &self.key, &self.permit_id)?;
        self.done = true;
        Ok(())
    }
}

impl Drop for LocalPermit {
    fn drop(&mut self) {
        if !self.done {
            let _ = release_permit(&self.anchor, &self.key, &self.permit_id);
        }
    }
}

/// Admit one local write to the mailbox of the checkout at `project_root`.
/// `Ok(None)` when the checkout has no anchor (not a Git repository). Refuses
/// with [`LifecycleError::SourceRetiring`] when the source is fenced or
/// retired. The mailbox directory must already exist.
pub fn admit_local_write(project_root: &Path) -> Result<Option<LocalPermit>> {
    admit_as(project_root, PermitKind::Local)
}

fn admit_as(project_root: &Path, kind: PermitKind) -> Result<Option<LocalPermit>> {
    let admitted = admit(project_root, kind);
    if admitted.is_err() {
        forget_anchor(project_root);
    }
    admitted
}

fn admit(project_root: &Path, kind: PermitKind) -> Result<Option<LocalPermit>> {
    // Git runs here, before any lock.
    let Some(source) = writer_source(project_root)? else {
        return Ok(None);
    };
    match admit_resolved(&source, kind)? {
        Ok(permit) => Ok(Some(permit)),
        // A retirement whose driver is gone (it died, or its destructive step
        // never ran) is reopened once, then admission retries.
        Err(op_id) if retire::reopen_abandoned(&source, &op_id)? => admit_resolved(&source, kind)?
            .map(Some)
            .map_err(|op_id| LifecycleError::SourceRetiring { op_id }),
        Err(op_id) => Err(LifecycleError::SourceRetiring { op_id }),
    }
}

/// The source a writer at `project_root` takes its permit under. Inside a
/// seat clone (whose own Git is never asked) that is the clone's source under
/// its main checkout's anchor, through the clone binding, so the fence the
/// clone's teardown takes there is honoured. A clone with no verifiable
/// binding has no anchor.
// trace:STORY-1843 | ai:claude
fn writer_source(project_root: &Path) -> Result<Option<Source>> {
    match crate::seat_clone_detect::seat_clone_root(project_root) {
        Some(clone_root) => match crate::seat_clone_detect::clone_writer_main(&clone_root) {
            Some(main) => resolve_clone_source(&main, &clone_root),
            None => Ok(None),
        },
        None => resolve_source(project_root),
    }
}

/// Admit under a short Lifecycle hold. A fenced source refuses with
/// `SourceRetiring`; a retired one returns `Ok(Err(op_id))` so the caller can
/// try to reopen it if its retirement was abandoned.
fn admit_resolved(
    source: &Source,
    kind: PermitKind,
) -> Result<std::result::Result<LocalPermit, String>> {
    let probe = ProbeSource::system();
    let holder = this_process();
    let held = lock(&source.anchor, LIFECYCLE_LOCK_TIMEOUT)?;
    let mut record = load_or_create(source, &held)?;
    match &record.state {
        SourceState::Open => {}
        SourceState::Fenced { op_id, .. } => {
            return Err(LifecycleError::SourceRetiring {
                op_id: op_id.clone(),
            })
        }
        SourceState::Retired { op_id, .. } => return Ok(Err(op_id.clone())),
    }
    // Reap rows whose writer is proven dead (killed mid-write, or a release
    // that failed twice), so an open source's record never grows to the cap.
    // Rows of unknown liveness stay for retirement to refuse on.
    record.permits.retain(|p| {
        !p.holder
            .as_ref()
            .is_some_and(|h| matches!(probe.probe(h), Probe::Dead(_)))
    });
    let permit_id = uuid::Uuid::new_v4().to_string();
    record.permits.push(Permit {
        permit_id: permit_id.clone(),
        kind,
        holder,
        generation: record.generation,
    });
    store_permits(&source.anchor, &record, &held)?;
    drop(held);
    Ok(Ok(LocalPermit {
        anchor: source.anchor.clone(),
        key: source.key.clone(),
        permit_id,
        done: false,
    }))
}

/// This process's identity, captured once.
fn this_process() -> Option<ProcessIdentity> {
    static ME: OnceLock<(u32, Option<ProcessIdentity>)> = OnceLock::new();
    let pid = std::process::id();
    match ME.get_or_init(|| (pid, ProbeSource::system().capture(pid).ok())) {
        (cached, id) if *cached == pid => id.clone(),
        // A forked child has its own identity.
        _ => ProbeSource::system().capture(pid).ok(),
    }
}

/// Whether a local write is covered by the retention barrier. This is the
/// write side's admission result, not the logical inbox's
/// `mailbox_logical::RetentionCoverage`.
#[derive(Debug)]
#[must_use = "a permit row stays until the permit completes or drops"]
pub enum AdmissionCoverage {
    /// Admitted: retirement waits for this write.
    Admitted(LocalPermit),
    /// No anchor could be established; the write proceeds as it did before
    /// C1, with no permit. `None` when the checkout has no Git repository.
    Uncovered(Option<String>),
}

/// The delivery-side admission policy: refuse only a source recorded as
/// fenced or retired; on any other lifecycle failure warn once and write
/// uncovered, exactly as before C1.
pub fn cover_local_write(project_root: &Path) -> Result<AdmissionCoverage> {
    match admit_local_write(project_root) {
        Ok(Some(permit)) => Ok(AdmissionCoverage::Admitted(permit)),
        Ok(None) => Ok(AdmissionCoverage::Uncovered(None)),
        Err(e @ LifecycleError::SourceRetiring { .. }) => Err(e),
        Err(e) => {
            static WARNED: AtomicBool = AtomicBool::new(false);
            if !WARNED.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "warning: mail retention is not established for {}; delivering without it: {e}",
                    project_root.display()
                );
            }
            Ok(AdmissionCoverage::Uncovered(Some(e.to_string())))
        }
    }
}

fn release_permit(anchor: &Path, key: &str, permit_id: &str) -> Result<()> {
    let held = lock(anchor, LIFECYCLE_LOCK_TIMEOUT)?;
    let Some(mut record) = load_record(anchor, key, &held)? else {
        return Ok(());
    };
    let before = record.permits.len();
    record.permits.retain(|p| p.permit_id != permit_id);
    if record.permits.len() != before {
        store_permits(anchor, &record, &held)?;
    }
    Ok(())
}

// ── Canonical publication admission ─────────────────────────────────────────

thread_local! {
    /// Canonical permit rows dropped while this thread held a Store write
    /// lock. Releasing one takes Lifecycle, which never starts inside a
    /// Store-held region, so they wait for the outermost Store guard.
    static DEFERRED: RefCell<Vec<LocalPermit>> = const { RefCell::new(Vec::new()) };
}

/// An admitted canonical publication: the local mail a sync or digest is
/// about to copy into the orphan store. Taken **before** the Store write lock
/// and moved into its closure; every canonical mailbox write takes
/// `&CanonicalPermit`, so a canonical write without admission does not
/// compile.
///
/// The permit is a `Canonical` row in the permits of the local source it
/// reads from, stamped with that source's generation. It is not a held lock:
/// nothing is held across the store transaction's Git or hook work, and
/// retiring that source waits for the row to clear (or refuses on its
/// bounded timeout). A permit dropped inside a Store-held region is released
/// when the outermost Store guard drops.
///
/// `row` is `None` on the degraded path (no anchor, no local mailbox), where
/// the publication runs exactly as it did before admission existed.
#[must_use = "a permit row stays until the permit drops"]
#[derive(Debug)]
pub struct CanonicalPermit {
    row: Option<LocalPermit>,
}

impl CanonicalPermit {
    /// Whether retirement of the local source waits for this publication.
    pub fn is_admitted(&self) -> bool {
        self.row.is_some()
    }
}

impl Drop for CanonicalPermit {
    fn drop(&mut self) {
        let Some(row) = self.row.take() else {
            return;
        };
        if crate::db::any_store_write_lock_held() {
            DEFERRED.with(|d| d.borrow_mut().push(row));
        }
        // Otherwise `row` drops here and releases its permit row.
    }
}

/// Release the canonical permit rows this thread deferred while it held a
/// Store write lock. Called by the outermost Store guard after it unlocks.
pub fn release_deferred_permits() {
    if crate::db::any_store_write_lock_held() || lifecycle_lock_held() {
        return;
    }
    let rows = DEFERRED.with(|d| std::mem::take(&mut *d.borrow_mut()));
    drop(rows);
}

/// Admit a canonical publication of the local mail of the checkout at
/// `project_root` (the store's parent checkout). Call it before taking the
/// Store write lock: under one it refuses as a lock-order violation.
///
/// Refuses with [`LifecycleError::SourceRetiring`] when that local source is
/// fenced or retired, so a retiring checkout's mail is preserved rather than
/// published from under the barrier. Any other lifecycle failure warns once
/// and admits uncovered, the same policy as [`cover_local_write`].
pub fn admit_canonical_publication(project_root: &Path) -> std::io::Result<CanonicalPermit> {
    if crate::db::any_store_write_lock_held() || lifecycle_lock_held() {
        return Err(LifecycleError::LockOrder(
            "a canonical publication is admitted before the Store write lock, never under it",
        )
        .into());
    }
    // No local mailbox: there is nothing to publish from and no source to
    // retire, so there is nothing for a permit to cover.
    if !crate::mailbox::local_mailbox_dir(project_root).is_dir() {
        return Ok(CanonicalPermit { row: None });
    }
    match admit_as(project_root, PermitKind::Canonical) {
        Ok(row) => Ok(CanonicalPermit { row }),
        Err(e @ (LifecycleError::SourceRetiring { .. } | LifecycleError::LockOrder(_))) => {
            Err(e.into())
        }
        Err(e) => {
            static WARNED: AtomicBool = AtomicBool::new(false);
            if !WARNED.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "warning: mail retention is not established for {}; publishing without it: {e}",
                    project_root.display()
                );
            }
            Ok(CanonicalPermit { row: None })
        }
    }
}

/// How many canonical publications admitted under the anchor of the checkout
/// at `project_root` have not finished. A row whose writer is proven dead
/// does not count; a row of unknown liveness does (unknown means refuse).
/// `0` when the checkout has no anchor.
///
/// Reads the source records without the Lifecycle lock (each is replaced by
/// an atomic rename), so it may run inside a Store-held region. An unreadable
/// or corrupt record is an error, never zero.
pub fn live_canonical_publications(project_root: &Path) -> std::io::Result<usize> {
    let Some(anchored) = locate_anchor(project_root)? else {
        return Ok(0);
    };
    let dir = anchored.anchor.join(SOURCES_DIR);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let probe = ProbeSource::system();
    let mut live = 0usize;
    for entry in entries {
        let path = entry?.path();
        let is_record = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".json") && !n.starts_with('.'));
        if !is_record {
            continue;
        }
        let Some(record) = read_json::<SourceRecord>(&path)? else {
            continue;
        };
        live += record
            .permits
            .iter()
            .filter(|p| p.kind == PermitKind::Canonical)
            .filter(|p| {
                !p.holder
                    .as_ref()
                    .is_some_and(|h| matches!(probe.probe(h), Probe::Dead(_)))
            })
            .count();
    }
    Ok(live)
}
