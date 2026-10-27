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
//! In C1 nothing in production fences a source, so admission never refuses
//! and mail delivery is unchanged; `retire` has no production caller until
//! C2.
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

pub(crate) mod retire;

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

/// The operator command that records every writer host on the machine as
/// admitting (QC1). A mail-bearing source refuses retirement until it has
/// run; the command ships first as a dry-run report.
pub const ACTIVATION_COMMAND: &str = "aida mailbox retention activate";

/// Directory under the common Git dir that holds the anchor.
const ANCHOR_PARENT: &str = "aida";
const ANCHOR_DIR: &str = "mail-lifecycle";
const LOCK_FILE: &str = "lifecycle.lock";
const GENERATION_FILE: &str = "generation.json";
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "read only by retirement until C2")
)]
const ACTIVATION_FILE: &str = "activation.json";
pub(crate) const SOURCES_DIR: &str = "sources";
pub(crate) const RETAINED_DIR: &str = "retained";
pub(crate) const JOURNAL_DIR: &str = "journal";

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
    let key = source_key(&anchored.common, &anchored.admin, meta.dev(), meta.ino());
    Ok(Some(Source {
        anchor: anchored.anchor,
        key,
        mailbox,
        admin: anchored.admin,
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

/// SHA-256 over the common Git dir, the worktree admin name and the mailbox
/// directory's device and inode.
fn source_key(common: &Path, admin: &str, dev: u64, ino: u64) -> String {
    let mut h = Sha256::new();
    h.update(common.as_os_str().as_encoded_bytes());
    h.update([0]);
    h.update(admin.as_bytes());
    h.update([0]);
    h.update(format!("{dev}:{ino}").as_bytes());
    hex(&h.finalize())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn ensure_anchor(common: &Path) -> Result<PathBuf> {
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
    let Some(file) = open_checked(path, RECORD_CAP)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(RECORD_CAP + 1).read_to_end(&mut bytes)?;
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
    debug_assert!(
        !lifecycle_lock_held(),
        "lock-order violation: {class} entered while the mailbox lifecycle lock is held"
    );
    OBSERVED.with(|o| {
        if let Some(seen) = o.borrow_mut().as_mut() {
            seen.push(class);
        }
    });
}

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
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "called only by retirement until C2")
)]
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
}

/// Has an operator recorded that every writer host is admitting? Nothing in
/// C1 writes the record, so this is false in production.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "called only by retirement until C2")
)]
pub(crate) fn activated(anchor: &Path) -> Result<bool> {
    Ok(read_json::<Activation>(&anchor.join(ACTIVATION_FILE))?
        .is_some_and(|a| a.all_writer_hosts_admitting))
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
    let admitted = admit(project_root);
    if admitted.is_err() {
        forget_anchor(project_root);
    }
    admitted
}

fn admit(project_root: &Path) -> Result<Option<LocalPermit>> {
    // Git runs here, before any lock.
    let Some(source) = resolve_source(project_root)? else {
        return Ok(None);
    };
    let probe = ProbeSource::system();
    let holder = this_process();
    let held = lock(&source.anchor, LIFECYCLE_LOCK_TIMEOUT)?;
    let mut record = load_or_create(&source, &held)?;
    match &record.state {
        SourceState::Open => {}
        SourceState::Fenced { op_id, .. } | SourceState::Retired { op_id, .. } => {
            return Err(LifecycleError::SourceRetiring {
                op_id: op_id.clone(),
            })
        }
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
        kind: PermitKind::Local,
        holder,
        generation: record.generation,
    });
    store_permits(&source.anchor, &record, &held)?;
    drop(held);
    Ok(Some(LocalPermit {
        anchor: source.anchor,
        key: source.key,
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
