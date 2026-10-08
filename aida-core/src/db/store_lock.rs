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

// trace:TASK-1526 | ai:codex
pub(super) fn holding_write() -> bool {
    HELD.with(|held| !held.borrow().is_empty())
}

impl StoreWriteGuard {
    /// True for the guard that owns the file lock (the outermost acquisition
    /// on this thread), false for a re-entrant level.
    // trace:TASK-1717 | ai:claude
    pub(crate) fn is_outermost(&self) -> bool {
        self.file.is_some()
    }
}

/// Run `f` as one store-writer transaction: take the store write lock for the
/// git store at `root`, check that the store is safe to write while holding
/// it, then run `f` and release on return. Everything a caller does inside
/// `f` (reading the branch, index, HEAD or canonical files, staging,
/// committing, rebasing and its own cleanup) is serialized with every other
/// writer of the same store, including `GitBackend`'s spec and queue writes.
///
/// The lock is the one `GitBackend` uses (same file, same canonical root,
/// re-entrant on this thread), so `f` may call backend writers or nest this
/// function. It is not handed to child processes: the lock file is opened
/// close-on-exec, so a hook or `git` child neither holds nor inherits it.
/// Do not run network pushes, forge calls or cache refreshes inside `f`.
///
/// Before `f` changes anything, the outermost holder also refuses a hook
/// profile git would run inside the transaction that can call back into an
/// `aida` writer (see `git_ops::ensure_store_hooks_supported`): such a hook
/// would wait on this lock forever.
// trace:TASK-1717 | ai:claude
pub fn with_store_write_lock<T>(root: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let guard = acquire(root)?;
    crate::git_ops::ensure_store_write_safe(root)?;
    if guard.is_outermost() {
        crate::git_ops::ensure_store_hooks_supported(root)?;
    }
    f()
}

/// Whether this thread holds the store write lock for `root`.
// trace:TASK-1717 | ai:claude
pub fn store_write_lock_held(root: &Path) -> bool {
    let key = lock_key(root);
    HELD.with(|held| held.borrow().get(&key).is_some_and(|n| *n > 0))
}

/// Whether this thread holds the store write lock for any store.
// trace:TASK-1717 | ai:claude
pub fn any_store_write_lock_held() -> bool {
    holding_write()
}

/// Path of the store write lock file for the store rooted at `root`.
// trace:TASK-1717 | ai:claude
pub fn store_write_lock_path(root: &Path) -> PathBuf {
    root.join(".aida").join(STORE_WRITE_LOCK_FILE)
}

/// Whether this thread currently holds the store write lock for `root`.
#[cfg(test)]
pub(crate) fn held_by_this_thread(root: &Path) -> bool {
    store_write_lock_held(root)
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

    // trace:BUG-1677 | ai:claude — the store is a linked worktree of the
    // project repo, so its `info/exclude` is the project's shared one. Taking
    // the store lock may add the anchored `.aida/*.lock` there and nothing
    // else: a legitimate `*.tmp.*` file in the project must stay visible.
    #[test]
    fn bug_1677_acquire_never_hides_project_files_through_the_shared_exclude() {
        use std::process::Command;
        fn git(cwd: &Path, args: &[&str]) -> std::process::Output {
            Command::new("git")
                .current_dir(cwd)
                .args(args)
                .output()
                .unwrap_or_else(|e| panic!("git {args:?} in {}: {e}", cwd.display()))
        }
        fn git_ok(cwd: &Path, args: &[&str]) -> String {
            let out = git(cwd, args);
            assert!(
                out.status.success(),
                "git {args:?} failed:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        git_ok(&project, &["init", "-q", "-b", "main"]);
        git_ok(&project, &["config", "user.email", "aida@example.invalid"]);
        git_ok(&project, &["config", "user.name", "AIDA Test"]);
        std::fs::write(project.join("README.md"), "seed\n").unwrap();
        git_ok(&project, &["add", "-A"]);
        git_ok(&project, &["commit", "-qm", "seed"]);
        // The store as `aida init` attaches it: a linked worktree on an
        // orphan branch, whose .gitignore carries the bare `*.lock` line
        // (not `.aida/*.lock`), so the exclude fallback really runs.
        let store = project.join(".aida-store");
        git_ok(
            &project,
            &["worktree", "add", "-q", "--detach", store.to_str().unwrap()],
        );
        git_ok(&store, &["checkout", "-q", "--orphan", "aida-store"]);
        std::fs::write(
            store.join(".gitignore"),
            "# Node-local state\n.aida/\n*.lock\n",
        )
        .unwrap();

        let guard = acquire(&store).unwrap();
        drop(guard);

        // The project's own files are untouched by the store lock.
        std::fs::write(project.join("notes.tmp.txt"), "keep me\n").unwrap();
        let check = git(&project, &["check-ignore", "-q", "notes.tmp.txt"]);
        assert!(
            !check.status.success(),
            "notes.tmp.txt in the project worktree must not be ignored"
        );
        // The lock exclusion itself still lands, anchored to `.aida/`.
        let exclude = git_ok(&store, &["rev-parse", "--git-path", "info/exclude"]);
        let exclude = if Path::new(&exclude).is_absolute() {
            PathBuf::from(exclude)
        } else {
            store.join(exclude)
        };
        let exclude = std::fs::read_to_string(&exclude).unwrap_or_default();
        assert!(
            exclude.lines().any(|l| l.trim() == ".aida/*.lock"),
            "{exclude}"
        );
        assert!(
            !exclude.lines().any(|l| l.trim().contains("tmp")),
            "no staging-file pattern may reach the shared exclude: {exclude}"
        );
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

    // trace:TASK-1717 | ai:claude
    #[test]
    fn task_1717_closure_nests_and_excludes_other_threads_until_it_returns() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let (tx, rx) = mpsc::channel();
        let r2 = root.clone();
        let entered = std::sync::Arc::new(std::sync::Barrier::new(2));
        let entered2 = entered.clone();
        let other = std::thread::spawn(move || {
            entered2.wait();
            with_store_write_lock(&r2, || {
                tx.send(Instant::now()).unwrap();
                Ok(())
            })
            .unwrap();
        });
        let released = with_store_write_lock(&root, || {
            // Same-thread nesting works and keeps the outer hold.
            with_store_write_lock(&root, || {
                assert!(store_write_lock_held(&root));
                Ok(())
            })?;
            assert!(store_write_lock_held(&root));
            entered.wait();
            std::thread::sleep(Duration::from_millis(200));
            assert!(rx.try_recv().is_err(), "another thread waits");
            Ok(Instant::now())
        })
        .unwrap();
        assert!(!store_write_lock_held(&root));
        let acquired = rx.recv_timeout(Duration::from_secs(30)).unwrap();
        other.join().unwrap();
        assert!(acquired >= released);
    }

    // trace:TASK-1717 | ai:claude — a symlinked path to the store names the
    // same lock (same file, same inode), and a different store does not wait.
    #[cfg(unix)]
    #[test]
    fn task_1717_symlinked_root_shares_the_lock_and_other_stores_do_not_wait() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("store");
        std::fs::create_dir_all(&root).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        let other = dir.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        let guard = acquire(&root).unwrap();
        {
            use std::os::unix::fs::MetadataExt;
            let a = std::fs::metadata(store_write_lock_path(&root)).unwrap();
            let b = std::fs::metadata(store_write_lock_path(&link)).unwrap();
            assert_eq!(a.ino(), b.ino());
        }
        let (tx, rx) = mpsc::channel();
        let l2 = link.clone();
        let o2 = other.clone();
        let handle = std::thread::spawn(move || {
            with_store_write_lock(&o2, || Ok(())).unwrap();
            tx.send("other").unwrap();
            with_store_write_lock(&l2, || Ok(())).unwrap();
            tx.send("link").unwrap();
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(30)).unwrap(), "other");
        std::thread::sleep(Duration::from_millis(200));
        assert!(rx.try_recv().is_err(), "the symlinked path waits");
        drop(guard);
        assert_eq!(rx.recv_timeout(Duration::from_secs(30)).unwrap(), "link");
        handle.join().unwrap();
    }

    // trace:TASK-1717 | ai:claude — the safety check runs after the lock is
    // taken, and the closure never runs on an unsafe store.
    #[test]
    fn task_1717_closure_refuses_a_store_mid_rebase_without_running() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(Command::new("git")
            .current_dir(root)
            .args(["init", "-q", "-b", "main"])
            .status()
            .unwrap()
            .success());
        std::fs::create_dir_all(root.join(".git").join("rebase-merge")).unwrap();
        let mut ran = false;
        let err = with_store_write_lock(root, || {
            ran = true;
            Ok(())
        })
        .unwrap_err();
        assert!(!ran);
        assert!(err.to_string().contains("rebase in progress"), "{err:#}");
        assert!(!store_write_lock_held(root), "released on refusal");
    }
}
