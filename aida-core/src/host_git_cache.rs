//! BUG-2192: per-process memo for the read-only lookups
//! [`crate::git_ops::host_git_command`] makes before every host git call over
//! a store: the store's git common directory (a `git rev-parse` spawn) and the
//! submodule names in the index or `HEAD` `.gitmodules` (a `git config --blob`
//! spawn). `aida show` builds about eight store git commands, so these spawns
//! doubled its CPU cost.
//!
//! A memo entry holds a successful answer, keyed by stamps of every file git
//! read to produce it: the `.git` entry, the `commondir` file and, for the
//! `.gitmodules` probe, the index, `HEAD`, the branch ref and `packed-refs`.
//! A stamp is the entry's device, inode, size, mode, mtime, ctime and link
//! count. Git replaces these files by renaming a lock file over them, so any
//! change gives a new inode and ctime, and the entry is recomputed. Nothing is
//! memoised when a stamped file changed within the last two seconds (a
//! timestamp tick cannot hide a rewrite), when the stamps moved while the
//! answer was being computed, when a git environment variable redirects
//! discovery, or off Unix. A failure is never memoised: every refusal is
//! recomputed, so the memo only skips work whose answer is unchanged.
//! trace:BUG-2192 | ai:claude

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// How recent a stamped file's ctime may be before its answer is not memoised.
const SETTLE: std::time::Duration = std::time::Duration::from_secs(2);

/// Environment that changes where git looks; any of it set disables the memo.
const GIT_DISCOVERY_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_COMMON_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_NAMESPACE",
];

/// One file's identity as `symlink_metadata` reports it; `None` when absent.
pub(crate) type Stamp = Option<[i64; 9]>;

// Test seam: treat every file as settled, so a test can see a memo hit on a
// fixture it just made.

/// The submodule names `lookup` reads from the regular file `file` (a
/// repository's `.gitmodules`), memoised while the file is unchanged.
/// TASK-1942: every host git call over a linked worktree with a `.gitmodules`
/// re-read it with a `git config` spawn. A symlink, whose target the stamp
/// cannot see, is never memoised.
// trace:TASK-1952 | trace:TASK-1942 | ai:claude
pub(crate) fn gitmodules_file_names(
    file: &Path,
    lookup: impl FnOnce() -> anyhow::Result<Vec<String>>,
) -> anyhow::Result<Vec<String>> {
    static MEMO: std::sync::OnceLock<Memo<Vec<String>>> = std::sync::OnceLock::new();
    let at = std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf());
    memoised(&MEMO, &at, gitmodules_file_key, lookup)
}

/// The stamp of a regular `.gitmodules` file; `None` for anything else.
fn gitmodules_file_key(file: &Path) -> Option<Vec<Stamp>> {
    if !std::fs::symlink_metadata(file).ok()?.file_type().is_file() {
        return None;
    }
    Some(vec![stamp(file)?])
}

#[cfg(test)]
thread_local! { pub(crate) static ALWAYS_SETTLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

// Test-only count of the lookup git processes this thread spawned.
#[cfg(test)]
thread_local! { pub(crate) static LOOKUP_SPAWNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

/// Record one lookup git spawn (counted in test builds only).
pub(crate) fn note_lookup_spawn() {
    #[cfg(test)]
    LOOKUP_SPAWNS.with(|n| n.set(n.get() + 1));
}

/// `path`'s stamp, or `None` (do not memoise) when it cannot be read, changed
/// within [`SETTLE`], or the platform has no inode identity.
// trace:BUG-2192 | ai:claude
pub(crate) fn stamp(path: &Path) -> Option<Stamp> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = match std::fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(None),
            Err(_) => return None,
        };
        #[cfg(test)]
        let settled_always = ALWAYS_SETTLED.with(std::cell::Cell::get);
        #[cfg(not(test))]
        let settled_always = false;
        let ctime = std::time::UNIX_EPOCH
            + std::time::Duration::new(
                u64::try_from(meta.ctime()).ok()?,
                u32::try_from(meta.ctime_nsec()).ok()?,
            );
        let settled = std::time::SystemTime::now()
            .duration_since(ctime)
            .is_ok_and(|age| age >= SETTLE);
        if !settled && !settled_always {
            return None;
        }
        Some(Some([
            meta.dev() as i64,
            meta.ino() as i64,
            meta.size() as i64,
            i64::from(meta.mode()),
            meta.mtime(),
            meta.mtime_nsec(),
            meta.ctime(),
            meta.ctime_nsec(),
            meta.nlink() as i64,
        ]))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// The git directory and common directory of the repository whose top is
/// `top`, derived from its `.git` entry and `commondir` file the way git
/// does, plus the stamps of the files read.
struct GitDirs {
    git_dir: PathBuf,
    common_dir: PathBuf,
    stamps: Vec<Stamp>,
}

/// [`GitDirs`] for `top`; `None` when `top` has no `.git` directory or
/// `gitdir:` file, a file is unsettled, or environment redirects discovery.
fn git_dirs(top: &Path) -> Option<GitDirs> {
    if GIT_DISCOVERY_ENV
        .iter()
        .any(|name| std::env::var_os(name).is_some())
    {
        return None;
    }
    let dotgit = top.join(".git");
    let dotgit_stamp = stamp(&dotgit)??;
    let meta = std::fs::symlink_metadata(&dotgit).ok()?;
    let git_dir = if meta.is_dir() {
        dotgit
    } else if meta.is_file() {
        let body = crate::seat_clone_detect::read_regular_file(&dotgit)?;
        let target = PathBuf::from(body.strip_prefix("gitdir:")?.trim());
        if target.is_absolute() {
            target
        } else {
            top.join(target)
        }
    } else {
        return None;
    };
    let commondir = git_dir.join("commondir");
    let commondir_stamp = stamp(&commondir)?;
    let common_dir = match commondir_stamp {
        None => git_dir.clone(),
        Some(_) => {
            let body = crate::seat_clone_detect::read_regular_file(&commondir)?;
            let target = PathBuf::from(body.trim_end_matches(['\n', '\r']));
            if target.is_absolute() {
                target
            } else {
                git_dir.join(target)
            }
        }
    };
    Some(GitDirs {
        git_dir,
        common_dir,
        stamps: vec![Some(dotgit_stamp), commondir_stamp],
    })
}

/// The stamps that decide the common directory of the repository at `top`.
fn common_dir_key(top: &Path) -> Option<Vec<Stamp>> {
    git_dirs(top).map(|dirs| dirs.stamps)
}

/// The stamps that decide what `:.gitmodules` and `HEAD:.gitmodules` name in
/// the repository whose top is `top`, `top/.gitmodules` included.
fn gitmodules_blob_key(top: &Path) -> Option<Vec<Stamp>> {
    let dirs = git_dirs(top)?;
    let mut stamps = dirs.stamps;
    // A reftable repository keeps refs where these stamps cannot see them.
    if stamp(&dirs.common_dir.join("reftable"))?.is_some() {
        return None;
    }
    let head = dirs.git_dir.join("HEAD");
    stamps.push(stamp(&head)?);
    stamps.push(stamp(&dirs.git_dir.join("index"))?);
    stamps.push(stamp(&top.join(".gitmodules"))?);
    let head_body = crate::seat_clone_detect::read_regular_file(&head)?;
    if let Some(target) = head_body.strip_prefix("ref:") {
        // Branches live in the common directory; any other symref target
        // (or a malformed one) is not memoised.
        let target = target.trim();
        if !target.starts_with("refs/heads/") || target.contains("..") {
            return None;
        }
        stamps.push(stamp(&dirs.common_dir.join(target))?);
        stamps.push(stamp(&dirs.common_dir.join("packed-refs"))?);
    }
    Some(stamps)
}

type Memo<V> = Mutex<HashMap<PathBuf, (Vec<Stamp>, V)>>;

/// `compute()` for `at`, memoised under `key(at)`: a stored answer is reused
/// while the key is unchanged; a fresh `Ok` answer is stored only when the key
/// reads the same before and after computing it.
fn memoised<V: Clone, E>(
    memo: &'static std::sync::OnceLock<Memo<V>>,
    at: &Path,
    key: impl Fn(&Path) -> Option<Vec<Stamp>>,
    compute: impl FnOnce() -> Result<V, E>,
) -> Result<V, E> {
    let memo = memo.get_or_init(Default::default);
    let before = key(at);
    if let Some(before) = &before {
        let map = memo
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((stamps, value)) = map.get(at) {
            if stamps == before {
                return Ok(value.clone());
            }
        }
    }
    let value = compute()?;
    if let Some(before) = before {
        if key(at).as_ref() == Some(&before) {
            memo.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(at.to_path_buf(), (before, value.clone()));
        }
    }
    Ok(value)
}

/// The store's `git rev-parse --git-common-dir` answer from `lookup`,
/// memoised while its `.git` entry and `commondir` file are unchanged. Only a
/// successful lookup (`Some`) is memoised.
// trace:BUG-2192 | ai:claude
pub(crate) fn common_dir(
    store: &Path,
    lookup: impl FnOnce() -> anyhow::Result<Option<String>>,
) -> anyhow::Result<Option<String>> {
    static MEMO: std::sync::OnceLock<Memo<String>> = std::sync::OnceLock::new();
    let at = std::path::absolute(store).unwrap_or_else(|_| store.to_path_buf());
    let found = memoised(&MEMO, &at, common_dir_key, || match lookup() {
        Ok(Some(dir)) => Ok(dir),
        other => Err(other),
    });
    match found {
        Ok(dir) => Ok(Some(dir)),
        Err(other) => other,
    }
}

/// The submodule names `lookup` finds in the index (else `HEAD`)
/// `.gitmodules` of the repository whose top is `top`, which has no
/// `.gitmodules` file, memoised while the index, `HEAD` and its branch are
/// unchanged and the file stays absent.
// trace:BUG-2192 | ai:claude
pub(crate) fn gitmodules_blob_names(
    top: &Path,
    lookup: impl FnOnce() -> anyhow::Result<Vec<String>>,
) -> anyhow::Result<Vec<String>> {
    static MEMO: std::sync::OnceLock<Memo<Vec<String>>> = std::sync::OnceLock::new();
    memoised(&MEMO, top, gitmodules_blob_key, lookup)
}
