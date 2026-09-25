//! Store write lock for the git-canonical requirements store.
//!
//! Every write to a git store's `objects/` tree and `metadata.yaml` runs under
//! one advisory file lock at `<store>/.aida/store-write.lock`. The file is
//! always empty (it is never rewritten, so it never shows up as a change),
//! and `.aida/*.lock` is excluded from git in every store the lock is taken
//! in: via the store `.gitignore` where present, else the store worktree's
//! `info/exclude`, so `git add -A` can never commit it. Holding it for the whole
//! read-modify-write window turns a per-spec update into a compare-and-swap:
//! the writer re-reads the object under the lock, so no other lock-respecting
//! writer can land between that read and the write.
//!
//! The lock is re-entrant per thread and store: a write path that already
//! holds it (for example `CachedGitBackend`, which keeps it across the cache
//! upsert) can call an inner write path that takes it again. Other threads and
//! processes open their own descriptor and block, because `flock(2)` /
//! `LockFileEx` locks conflict across open file descriptions.
//!
//! trace:BUG-1612 | ai:claude

use anyhow::{Context, Result};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Default wait for the lock before giving up. A whole-store save of a large
/// store (write + commit) is the longest holder. Overridable with
/// `AIDA_STORE_LOCK_TIMEOUT_SECS`.
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Lock file name inside `<store>/.aida/`.
pub(crate) const STORE_WRITE_LOCK_FILE: &str = "store-write.lock";

thread_local! {
    /// Per-thread hold count by canonical store root (re-entrancy).
    static HELD: RefCell<HashMap<PathBuf, usize>> = RefCell::new(HashMap::new());
}

/// RAII guard for the store write lock. Dropping it releases one level of the
/// per-thread hold; the file lock is released when the outermost guard drops.
#[must_use = "the store write lock is released when the guard is dropped"]
pub(crate) struct StoreWriteGuard {
    key: PathBuf,
    file: Option<File>,
}

impl Drop for StoreWriteGuard {
    fn drop(&mut self) {
        HELD.with(|held| {
            let mut held = held.borrow_mut();
            if let Some(n) = held.get_mut(&self.key) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    held.remove(&self.key);
                }
            }
        });
        if let Some(file) = self.file.take() {
            use fs2::FileExt;
            let _ = FileExt::unlock(&file);
        }
    }
}

fn lock_key(root: &Path) -> PathBuf {
    std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

fn timeout() -> Duration {
    let secs = std::env::var("AIDA_STORE_LOCK_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

/// Store roots whose lock exclusion is already confirmed in this process.
static EXCLUDED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Make sure `.aida/*.lock` can never be committed from the store at `root`.
/// Checked once per process per store: a `.gitignore` line is enough (no git
/// call); otherwise the pattern is added to the worktree's `info/exclude`. A
/// directory that is not a git worktree root yet is re-checked next time.
// trace:BUG-1612 | ai:claude
fn ensure_lock_excluded(root: &Path) {
    let key = lock_key(root);
    let mut done = EXCLUDED.lock().unwrap_or_else(|p| p.into_inner());
    if done.contains(&key) {
        return;
    }
    let ignored = std::fs::read_to_string(root.join(".gitignore"))
        .map(|g| g.lines().any(|l| l.trim() == ".aida/*.lock"))
        .unwrap_or(false);
    if ignored {
        done.push(key);
        return;
    }
    if !root.join(".git").exists() {
        return;
    }
    if crate::git_ops::ensure_store_lock_excluded(root).is_ok() {
        done.push(key);
    }
}

/// Acquire the store write lock for the git store rooted at `root`.
// trace:BUG-1612 | ai:claude
pub(crate) fn acquire(root: &Path) -> Result<StoreWriteGuard> {
    let key = lock_key(root);
    let reentrant = HELD.with(|held| {
        let mut held = held.borrow_mut();
        match held.get_mut(&key) {
            Some(n) if *n > 0 => {
                *n += 1;
                true
            }
            _ => false,
        }
    });
    if reentrant {
        return Ok(StoreWriteGuard { key, file: None });
    }

    let dir = root.join(".aida");
    std::fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    let path = dir.join(STORE_WRITE_LOCK_FILE);
    ensure_lock_excluded(root);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("Failed to open store write lock {}", path.display()))?;

    let start = Instant::now();
    let limit = timeout();
    loop {
        use fs2::FileExt;
        match file.try_lock_exclusive() {
            Ok(()) => break,
            Err(e) if crate::file_lock::is_lock_contended(&e) => {
                if start.elapsed() >= limit {
                    anyhow::bail!(
                        "Timed out after {}s waiting for the store write lock {}. \
                         Another AIDA command is writing the store; retry when it finishes.",
                        limit.as_secs(),
                        path.display()
                    );
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                return Err(e).with_context(|| {
                    format!("Failed to acquire store write lock {}", path.display())
                })
            }
        }
    }

    HELD.with(|held| held.borrow_mut().insert(key.clone(), 1));
    Ok(StoreWriteGuard {
        key,
        file: Some(file),
    })
}

/// Whether this thread currently holds the store write lock for `root`.
#[cfg(test)]
pub(crate) fn held_by_this_thread(root: &Path) -> bool {
    let key = lock_key(root);
    HELD.with(|held| held.borrow().get(&key).is_some_and(|n| *n > 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn lock_is_reentrant_on_one_thread() {
        let dir = tempfile::tempdir().unwrap();
        let outer = acquire(dir.path()).unwrap();
        {
            let _inner = acquire(dir.path()).unwrap();
            assert!(held_by_this_thread(dir.path()));
        }
        assert!(held_by_this_thread(dir.path()));
        drop(outer);
        assert!(!held_by_this_thread(dir.path()));
    }

    #[test]
    fn lock_excludes_another_thread_until_released() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let guard = acquire(&root).unwrap();
        let (tx, rx) = mpsc::channel();
        let r2 = root.clone();
        let handle = std::thread::spawn(move || {
            let _g = acquire(&r2).unwrap();
            tx.send(Instant::now()).unwrap();
        });
        std::thread::sleep(Duration::from_millis(150));
        assert!(
            rx.try_recv().is_err(),
            "second thread must block while the lock is held"
        );
        let released = Instant::now();
        drop(guard);
        let acquired = rx.recv().unwrap();
        handle.join().unwrap();
        assert!(acquired >= released);
    }
}
