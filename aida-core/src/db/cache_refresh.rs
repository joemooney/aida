//! Single-flight cache refresh and invocation-scoped read diagnostics.
// trace:TASK-1526 | ai:codex

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::thread::ThreadId;
use std::time::Duration;

// The registry owns the descriptor, not a backend instance. Nested opens on
// the owning thread share it; independent threads remain ordinary contenders.
// trace:TASK-1526 | ai:codex
struct HeldLock {
    _file: File,
    owner: ThreadId,
    depth: usize,
}
static HELD: OnceLock<Mutex<HashMap<PathBuf, HeldLock>>> = OnceLock::new();

// trace:TASK-1526 | ai:codex
pub struct RefreshLock {
    path: PathBuf,
    nested: bool,
    // Guard must drop on the owning thread.
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl RefreshLock {
    // trace:TASK-1526 | ai:codex
    pub fn try_acquire(cache_path: &Path) -> std::io::Result<Option<Self>> {
        #[cfg(test)]
        if let Some(hook) = BEFORE_ACQUIRE.with(|c| c.borrow_mut().take()) {
            hook();
        }
        #[cfg(test)]
        if UNSUPPORTED.with(|c| c.get()) {
            return Err(std::io::Error::from(std::io::ErrorKind::Unsupported));
        }
        let path = super::cache_lock::cache_sidecar_path(cache_path, "refresh.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        // The sidecar address is still derived solely by cache_sidecar_path.
        // Normalize its registry key after creation so aliases through a
        // symlinked directory cannot make this thread contend with itself.
        let path = std::fs::canonicalize(&path)?;
        let mut held = HELD.get_or_init(Default::default).lock().unwrap();
        let owner = std::thread::current().id();
        if let Some(existing) = held.get_mut(&path) {
            if existing.owner != owner {
                return Ok(None);
            }
            existing.depth += 1;
            return Ok(Some(Self {
                path,
                nested: true,
                _thread: Default::default(),
            }));
        }
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => {
                held.insert(
                    path.clone(),
                    HeldLock {
                        _file: file,
                        owner,
                        depth: 1,
                    },
                );
                Ok(Some(Self {
                    path,
                    nested: false,
                    _thread: Default::default(),
                }))
            }
            Err(e) if crate::file_lock::is_lock_contended(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    // trace:TASK-1526 | ai:codex
    pub(super) fn held_by_current_thread(cache_path: &Path) -> bool {
        let path = super::cache_lock::cache_sidecar_path(cache_path, "refresh.lock");
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        HELD.get_or_init(Default::default)
            .lock()
            .unwrap()
            .get(&path)
            .is_some_and(|held| held.owner == std::thread::current().id())
    }

    /// `try_acquire` returns `Ok(None)` from exactly two places: an existing
    /// registry entry owned by another thread, or a contended `flock`. A bare
    /// `assert!(..is_some())` discards which one, so a rare CI-only failure
    /// arrives with no way to tell a descriptor-lifecycle defect from an
    /// unrelated holder. Report both, and the errno, at the point of failure.
    // trace:TASK-1526 | ai:claude
    #[cfg(test)]
    pub(super) fn diagnose_unavailable(cache_path: &Path) -> String {
        let path = super::cache_lock::cache_sidecar_path(cache_path, "refresh.lock");
        let key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let registry = HELD
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .get(&key)
            .map(|held| format!("owner={:?} depth={}", held.owner, held.depth));
        // A successful probe releases on close at the end of this statement.
        let probe = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .and_then(|file| fs2::FileExt::try_lock_exclusive(&file));
        format!(
            "sidecar={path:?} current_thread={:?} registry_entry={registry:?} \
             direct_flock={:?} {}",
            std::thread::current().id(),
            probe.as_ref().err().map(|e| (e.kind(), e.raw_os_error())),
            lock_holders(&path),
        )
    }

    pub fn is_nested(&self) -> bool {
        self.nested
    }
}

/// `registry_entry=None` beside a contended `direct_flock` looks
/// self-contradictory and invites the wrong fix. Two very different states
/// print that way: a descriptor of ours that genuinely outlived its guard, and
/// a descriptor of ours that a concurrent `fork` duplicated into a child which
/// has not reached `exec` yet (see `acquire_past_fork_window`). Only the first
/// is a defect here. Naming the holding pids from `/proc/locks`, and this
/// process's own descriptors on the same file, NARROWS that from the CI log
/// alone instead of leaving it to inference.
///
/// It is evidence, not an identity check, and a reader should not treat it as
/// more: `/proc/locks` is read as a whole while it can change underneath, and
/// a row's pid is the pid that CREATED the lock, which is not necessarily the
/// process whose descriptor is keeping it alive.
// trace:BUG-1729 | ai:claude
#[cfg(all(test, target_os = "linux"))]
fn lock_holders(sidecar: &Path) -> String {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = std::fs::metadata(sidecar) else {
        return "holders=<sidecar stat failed>".to_string();
    };
    // Matching the inode alone can name a lock on an identically numbered
    // inode on another device, so match `/proc/locks`'s own MAJ:MIN:INO key.
    let dev = meta.dev();
    let key = format!(
        "{:02x}:{:02x}:{}",
        libc::major(dev),
        libc::minor(dev),
        meta.ino()
    );
    let ino = meta.ino();
    let me = std::process::id().to_string();
    let mut holders: Vec<String> = Vec::new();
    if let Ok(locks) = std::fs::read_to_string("/proc/locks") {
        for line in locks.lines() {
            // "1: FLOCK  ADVISORY  WRITE 4242 08:02:123456 0 EOF", with an
            // extra "->" token on a blocked waiter's row.
            let mut f: Vec<&str> = line.split_whitespace().collect();
            if f.get(1) == Some(&"->") {
                f.remove(1);
            }
            let (Some(pid), Some(dev_ino)) = (f.get(4), f.get(5)) else {
                continue;
            };
            if **dev_ino != *key {
                continue;
            }
            let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"))
                .map(|c| c.trim().to_string())
                .unwrap_or_else(|_| "<gone>".to_string());
            let whose = if *pid == me { "SELF" } else { "other" };
            holders.push(format!(
                "{whose}/pid={pid}/comm={comm}/{}/{}",
                f.get(1).unwrap_or(&"?"),
                f.get(3).unwrap_or(&"?")
            ));
        }
    }
    // A descriptor of ours on the same inode is what a leak looks like from the
    // inside. Reading the directory opens one itself, on a different inode.
    let mut own_fds: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc/self/fd") {
        for entry in entries.flatten() {
            let same =
                std::fs::metadata(entry.path()).is_ok_and(|m| m.ino() == ino && m.dev() == dev);
            if same {
                own_fds.push(entry.file_name().to_string_lossy().to_string());
            }
        }
    }
    format!("dev_ino={key} holders={holders:?} own_fds={own_fds:?}")
}

// trace:BUG-1729 | ai:claude
#[cfg(all(test, not(target_os = "linux")))]
fn lock_holders(_sidecar: &Path) -> String {
    "holders=<needs /proc/locks>".to_string()
}

/// A `flock` belongs to the open file description, and `fork` duplicates every
/// descriptor into the child. `O_CLOEXEC` — which Rust sets on every `File` —
/// closes the copy at `exec`, NOT at `fork`. So for the width of the fork→exec
/// window, any thread in this process that spawns a subprocess hands the child
/// a second reference to this module's locked description, and a guard dropped
/// inside that window closes the parent's copy without releasing the lock.
/// `try_acquire` then reports contention against an empty registry until the
/// child reaches `exec`.
///
/// That is the kernel's contract, not a defect here: nothing in `RefreshLock`
/// can stop a sibling thread from forking, `O_CLOEXEC` is already the strongest
/// available mitigation, and in production the caller simply loses the
/// single-flight race and serves stale — the designed degradation. It is only a
/// problem for a test that demands the lock be free at the instant the last
/// guard drops, which is not something the kernel promises. Such a test waits
/// for the window to close, bounded, and reports the diagnostic for a
/// descriptor that never comes back.
///
/// The bound is a coarse backstop, not a measurement: the window closes as soon
/// as the racing child reaches `exec`, which is microseconds on an idle host,
/// so 5s is already three orders of magnitude of headroom while keeping a real
/// failure prompt. The response to a spurious crossing is to raise it.
///
/// It retries every `Ok(None)`, including a registry entry owned by another
/// thread, because `try_acquire` does not report which fired. In the tests that
/// use it no other thread owns the entry, so the only reachable `None` is the
/// contended `flock`; a caller where that is not true would be masking
/// cross-thread contention and should assert on `held_by_current_thread`
/// instead.
// trace:BUG-1729 | ai:claude
#[cfg(test)]
pub(super) fn acquire_past_fork_window(cache_path: &Path) -> std::io::Result<Option<RefreshLock>> {
    let deadline = std::time::Instant::now() + FORK_WINDOW_BOUND;
    loop {
        if let Some(lock) = RefreshLock::try_acquire(cache_path)? {
            return Ok(Some(lock));
        }
        let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) else {
            return Ok(None);
        };
        std::thread::sleep(left.min(Duration::from_millis(20)));
    }
}

// trace:BUG-1729 | ai:claude
#[cfg(test)]
const FORK_WINDOW_BOUND: Duration = Duration::from_secs(5);

impl Drop for RefreshLock {
    fn drop(&mut self) {
        let mut held = HELD.get().unwrap().lock().unwrap();
        let lock = held.get_mut(&self.path).unwrap();
        lock.depth -= 1;
        if lock.depth == 0 {
            held.remove(&self.path);
        }
    }
}

// A refresh waiter must never invert store -> SQLite lock ordering.
// trace:TASK-1526 | ai:codex
pub(super) fn may_wait() -> bool {
    !super::cache::holding_write() && !super::store_lock::holding_write()
}

// trace:TASK-1526 | ai:codex
pub(super) fn wait_poll(delay: Duration) {
    debug_assert!(
        may_wait(),
        "refresh wait while holding a store or SQLite write lock"
    );
    std::thread::sleep(delay);
}

/// A reader that finds a pending schema migration waits far longer than an
/// ordinary bounded read: only a full rebuild can apply the new schema, so the
/// alternative is failing the invocation outright. Tests drive the window
/// through `migration_wait_limit` rather than spending it in wall-clock, so the
/// assertions measure the state machine instead of the runner's scheduler.
// trace:TASK-1526 | ai:claude
pub(super) const MIGRATION_WAIT: Duration = Duration::from_secs(15);

#[cfg(test)]
thread_local! {
    static MIGRATION_WAIT_OVERRIDE: std::cell::Cell<Option<Duration>> =
        const { std::cell::Cell::new(None) };
}

/// Per-thread, so libtest's thread-per-test isolation already scopes it.
// trace:TASK-1526 | ai:claude
#[cfg(test)]
pub(super) fn set_migration_wait_limit(limit: Option<Duration>) -> Option<Duration> {
    MIGRATION_WAIT_OVERRIDE.with(|c| c.replace(limit))
}

// trace:TASK-1526 | ai:claude
pub(super) fn migration_wait_limit() -> Duration {
    #[cfg(test)]
    if let Some(limit) = MIGRATION_WAIT_OVERRIDE.with(|c| c.get()) {
        return limit;
    }
    MIGRATION_WAIT
}

/// An advisory read must exit silently instead of serving incompatible rows.
// trace:TASK-1526 | ai:codex
#[derive(Debug, thiserror::Error)]
#[error("the cache is being upgraded; re-run shortly")]
pub struct AdvisoryCacheUnavailable;

// trace:TASK-1526 | ai:codex
#[derive(Clone, Copy, Debug)]
pub struct ReadBudget(pub Duration);
impl Default for ReadBudget {
    fn default() -> Self {
        Self(Duration::from_millis(
            std::env::var("AIDA_CACHE_READ_WAIT_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1500),
        ))
    }
}

// trace:TASK-1526 | ai:codex
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshState {
    WorkerRunning,
    WriterBusy,
    Deferred,
}

// trace:TASK-1526 | ai:codex
#[derive(Clone, Debug, serde::Serialize)]
pub struct StaleServe {
    pub stale: bool,
    pub cache_head: Option<String>,
    pub store_head: String,
    pub built_at: Option<String>,
    pub refreshing: RefreshState,
}
impl StaleServe {
    pub fn note(&self) -> String {
        let time = self
            .built_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M:%S %Z")
                    .to_string()
            })
            .unwrap_or_else(|| {
                self.built_at
                    .clone()
                    .unwrap_or_else(|| "an unknown time".into())
            });
        match self.refreshing {
            RefreshState::Deferred => format!("note: showing results cached at {time}; refresh is deferred. Run `aida cache rebuild` for current data."),
            RefreshState::WorkerRunning => format!("note: showing results cached at {time}; the cache is refreshing. Re-run in a few seconds for current data."),
            RefreshState::WriterBusy => format!("note: showing results cached at {time}; the cache write lock was busy and refresh was deferred. Re-run to retry, or run `aida cache rebuild` for current data."),
        }
    }
}

// trace:TASK-1526 | ai:codex
#[derive(Default)]
struct Collected {
    touched: bool,
    wait_deadline: Option<std::time::Instant>,
    interactive: bool,
    budget: Option<ReadBudget>,
    stale: Option<StaleServe>,
}
thread_local! {
    static COLLECTED: RefCell<Collected> = RefCell::new(Collected::default());
}

/// One CLI invocation or MCP tools/call. Nested scopes restore their caller.
// trace:TASK-1526 | ai:codex
pub struct CacheReadScope(Option<Collected>, std::marker::PhantomData<std::rc::Rc<()>>);
impl CacheReadScope {
    pub fn new() -> Self {
        Self(
            Some(COLLECTED.with(|c| c.replace(Collected::default()))),
            Default::default(),
        )
    }
    pub fn configure(&self, interactive: bool, budget: Option<ReadBudget>) {
        COLLECTED.with(|c| {
            let mut c = c.borrow_mut();
            c.interactive = interactive;
            c.budget = budget;
        });
    }
    pub fn touched(&self) -> bool {
        COLLECTED.with(|c| c.borrow().touched)
    }
    pub fn stale(&self) -> Option<StaleServe> {
        COLLECTED.with(|c| c.borrow().stale.clone())
    }
    pub fn metadata(&self) -> serde_json::Value {
        cache_read_metadata()
    }
}
impl Default for CacheReadScope {
    fn default() -> Self {
        Self::new()
    }
}
impl Drop for CacheReadScope {
    fn drop(&mut self) {
        COLLECTED.with(|c| c.replace(self.0.take().unwrap()));
    }
}

// trace:TASK-1526 | ai:codex
pub fn configure_read_policy(interactive: bool, budget: Option<ReadBudget>) {
    COLLECTED.with(|c| {
        let mut c = c.borrow_mut();
        c.interactive = interactive;
        c.budget = budget;
    });
}

/// Start a new UI refresh after invalidating every displayed cached view.
// trace:TASK-1526 | ai:codex
pub fn reset_cache_read_observations() {
    COLLECTED.with(|c| {
        let mut c = c.borrow_mut();
        c.touched = false;
        c.stale = None;
        c.wait_deadline = None;
    });
}

// trace:TASK-1526 | ai:codex
pub fn cache_read_note() -> Option<String> {
    COLLECTED.with(|c| c.borrow().stale.as_ref().map(StaleServe::note))
}

// trace:TASK-1526 | ai:codex
pub fn cache_read_touched() -> bool {
    COLLECTED.with(|c| c.borrow().touched)
}

// trace:TASK-1526 | ai:codex
pub fn cache_read_metadata() -> serde_json::Value {
    COLLECTED.with(|c| {
        c.borrow()
            .stale
            .as_ref()
            .map(|s| serde_json::to_value(s).unwrap())
            .unwrap_or_else(|| serde_json::json!({"stale": false}))
    })
}

// trace:TASK-1526 | ai:codex
pub(super) fn read_policy() -> (bool, ReadBudget) {
    COLLECTED.with(|c| {
        let c = c.borrow();
        (
            c.interactive,
            c.budget.unwrap_or_else(|| {
                if super::cache::fast_fail_cache_enabled() {
                    ReadBudget(Duration::ZERO)
                } else {
                    ReadBudget::default()
                }
            }),
        )
    })
}

// One deadline across backends in an invocation. No-holder branches never sleep.
// trace:TASK-1526 | ai:codex
pub(super) fn wait_remaining(budget: Duration) -> Duration {
    COLLECTED.with(|c| {
        let mut c = c.borrow_mut();
        let now = std::time::Instant::now();
        let proposed = now + budget;
        let deadline = c.wait_deadline.get_or_insert(proposed);
        *deadline = (*deadline).min(proposed);
        deadline.saturating_duration_since(now)
    })
}

// trace:TASK-1526 | ai:codex
pub fn record_read(stale: Option<StaleServe>) {
    COLLECTED.with(|c| {
        let mut c = c.borrow_mut();
        c.touched = true;
        // Once a stale result has been consumed, a subsequent fresh backend
        // cannot retroactively make that earlier output fresh.
        if c.stale.is_none() {
            c.stale = stale;
        }
    });
}

#[cfg(test)]
thread_local! {
    static COUNTS: RefCell<HashMap<&'static str, usize>> = RefCell::new(HashMap::new());
    pub(super) static BEFORE_ACQUIRE: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
    pub(super) static INCREMENTAL_ERROR: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    pub(super) static UNSUPPORTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
#[cfg(test)]
pub(super) fn test_count(name: &'static str) {
    COUNTS.with(|c| *c.borrow_mut().entry(name).or_default() += 1);
}
#[cfg(test)]
pub(super) fn test_counts() -> HashMap<&'static str, usize> {
    COUNTS.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // One invocation gets one deadline, so a later read in the SAME scope can
    // be left with no wait budget at all. A caller modelling a re-run must
    // open a new scope; a test that re-uses a spent one has zero tolerance for
    // transient contention and fails on a loaded runner.
    // trace:TASK-1526 | ai:claude
    #[test]
    fn wait_remaining_is_zero_once_the_invocation_deadline_is_spent() {
        let scope = CacheReadScope::new();
        assert!(!wait_remaining(Duration::from_millis(40)).is_zero());
        std::thread::sleep(Duration::from_millis(80));
        assert!(wait_remaining(MIGRATION_WAIT).is_zero());
        drop(scope);
        let _fresh = CacheReadScope::new();
        assert!(!wait_remaining(MIGRATION_WAIT).is_zero());
    }

    // The shipped migration window stays 15s; only tests shorten it.
    // trace:TASK-1526 | ai:claude
    #[test]
    fn migration_wait_limit_defaults_to_the_shipped_window() {
        assert_eq!(migration_wait_limit(), MIGRATION_WAIT);
        let prev = set_migration_wait_limit(Some(Duration::from_millis(7)));
        assert_eq!(prev, None);
        assert_eq!(migration_wait_limit(), Duration::from_millis(7));
        set_migration_wait_limit(None);
        assert_eq!(migration_wait_limit(), MIGRATION_WAIT);
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    fn nested_freshen_in_lock_holder_does_not_wait() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("cache.db");
        let outer = RefreshLock::try_acquire(&path).unwrap().unwrap();
        let start = std::time::Instant::now();
        let inner = RefreshLock::try_acquire(&path).unwrap().unwrap();
        assert!(inner.is_nested());
        // This budget was briefly removed on the theory that `is_nested()`
        // already proves "does not wait". It does not: the nested branch is
        // reached only after an `open`, a `canonicalize`, and a blocking
        // `HELD.lock()`, so the state says which path was taken and not that
        // the path was prompt. Restored, and left to AC1's sweep of this
        // family's wall-clock assertions rather than weakened here.
        // trace:BUG-1729 | ai:claude
        assert!(start.elapsed() < Duration::from_millis(100));
        let p = path.clone();
        assert!(
            std::thread::spawn(move || RefreshLock::try_acquire(&p).unwrap().is_none())
                .join()
                .unwrap()
        );
        drop(outer);
        let p = path.clone();
        assert!(
            std::thread::spawn(move || RefreshLock::try_acquire(&p).unwrap().is_none())
                .join()
                .unwrap()
        );
        drop(inner);
        // Not a bare `try_acquire`: a concurrent `fork` anywhere in this test
        // binary can keep the lock alive past this guard's drop until the child
        // reaches `exec`. See `acquire_past_fork_window`.
        // trace:BUG-1729 | ai:claude
        assert!(
            acquire_past_fork_window(&path).unwrap().is_some(),
            "re-acquire after the last guard dropped returned None for the whole \
             fork window; {}",
            RefreshLock::diagnose_unavailable(&path)
        );
    }

    /// The race that made this module's CI failure unreproducible on a quiet
    /// host, made deterministic. `pre_exec` runs in the child after `fork` and
    /// before `exec`, with the parent's descriptors — including the guard's —
    /// still open, so lingering there reproduces a descheduled pre-exec child
    /// exactly. Dropping the guard in that window must leave the lock held,
    /// with an empty registry, and the diagnostic must say SELF rather than
    /// implicate an unrelated holder.
    // trace:BUG-1729 | ai:claude
    #[cfg(target_os = "linux")]
    #[test]
    fn a_forked_child_holds_the_lock_past_its_guard_until_exec() {
        use std::os::unix::process::CommandExt;
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("cache.db");
        let guard = RefreshLock::try_acquire(&path).unwrap().unwrap();

        // `Command::spawn` does not return until the child reaches `exec` — it
        // reads the child's error pipe to closure — so the window cannot be
        // observed from the spawning thread. The child announces itself down a
        // plain pipe from `pre_exec` and then lingers, and this thread observes
        // the window while another thread sits inside `spawn`.
        // `O_CLOEXEC` on both ends is what makes this safe to wait on: it does
        // NOT stop the child inheriting them at `fork`, so the child can still
        // write, but it closes them at `exec`. So either the byte arrives, or
        // the child reached `exec` and the read sees EOF — never an
        // indefinite block on a write end this process is still holding.
        let mut fds = [0 as libc::c_int; 2];
        assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
        let (read_fd, write_fd) = (fds[0], fds[1]);
        let spawner = std::thread::spawn(move || {
            let mut cmd = std::process::Command::new("true");
            // SAFETY: after `fork` the child calls only `write` and
            // `nanosleep`, both async-signal-safe, which is the constraint on a
            // post-fork closure in a multithreaded parent. `std::thread::sleep`
            // would NOT satisfy it. The window is 2s against the microseconds
            // of work below.
            unsafe {
                cmd.pre_exec(move || {
                    let _ = libc::write(write_fd, b"x".as_ptr().cast(), 1);
                    let ts = libc::timespec {
                        tv_sec: 2,
                        tv_nsec: 0,
                    };
                    libc::nanosleep(&ts, std::ptr::null_mut());
                    Ok(())
                });
            }
            cmd.spawn().unwrap().wait().unwrap();
        });
        // Bounded: a child that dies before writing must fail this test, never
        // hang it.
        let mut pfd = libc::pollfd {
            fd: read_fd,
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(
            unsafe { libc::poll(&mut pfd, 1, 10_000) },
            1,
            "the child neither reported reaching pre_exec nor closed the pipe"
        );
        let mut byte = [0u8; 1];
        assert_eq!(
            unsafe { libc::read(read_fd, byte.as_mut_ptr().cast(), 1) },
            1,
            "the child reached exec without reporting from pre_exec"
        );

        drop(guard);
        let report = RefreshLock::diagnose_unavailable(&path);
        assert!(
            RefreshLock::try_acquire(&path).unwrap().is_none(),
            "the inherited descriptor did not hold the lock past the guard; {report}"
        );
        assert!(report.contains("registry_entry=None"), "{report}");
        assert!(report.contains("WouldBlock"), "{report}");
        // The lock record keeps the pid that CREATED it, so the holder reads as
        // this process even though the descriptor keeping it alive belongs to a
        // child. That pair is what a CI log has to go on: a holder of SELF with
        // an empty `own_fds` says an inherited descriptor, not a leak here,
        // though it cannot say whose fork produced it; a holder of SELF with a
        // descriptor still listed is a leak, which
        // `the_diagnostic_names_a_descriptor_no_guard_owns` pins.
        assert!(report.contains("SELF/pid="), "{report}");
        assert!(report.contains("own_fds=[]"), "{report}");

        spawner.join().unwrap();
        // Once the child execs, its copy closes and the lock is free again.
        assert!(
            acquire_past_fork_window(&path).unwrap().is_some(),
            "the lock stayed held after the forked child exited; {}",
            RefreshLock::diagnose_unavailable(&path)
        );
        unsafe { libc::close(read_fd) };
    }

    /// A diagnostic is worth only what it says at the one moment it is read.
    /// Inject the defect it exists to name — a descriptor holding the sidecar
    /// that no guard owns — and assert it names this process and the
    /// descriptor, not just "contended".
    // trace:BUG-1729 | ai:claude
    #[cfg(target_os = "linux")]
    #[test]
    fn the_diagnostic_names_a_descriptor_no_guard_owns() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("cache.db");
        let sidecar = super::super::cache_lock::cache_sidecar_path(&path, "refresh.lock");
        let leaked = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&sidecar)
            .unwrap();
        fs2::FileExt::try_lock_exclusive(&leaked).unwrap();
        let report = RefreshLock::diagnose_unavailable(&path);
        assert!(report.contains("registry_entry=None"), "{report}");
        assert!(report.contains("WouldBlock"), "{report}");
        assert!(report.contains("SELF/pid="), "{report}");
        assert!(report.contains("/FLOCK/WRITE"), "{report}");
        // Positively, not `!contains("own_fds=[]")`: that negation also passes
        // when the field is missing altogether. A populated list opens with a
        // quoted descriptor name.
        assert!(report.contains("own_fds=[\""), "{report}");
        drop(leaked);
        // Observed failing here as a bare `try_acquire` while this file was
        // being written: the sibling fork test above ran concurrently and its
        // pre-exec child had inherited `leaked`, so dropping it released
        // nothing. The race is not hypothetical, and it reaches any test that
        // holds a descriptor across another test's spawn.
        // trace:BUG-1729 | ai:claude
        assert!(
            acquire_past_fork_window(&path).unwrap().is_some(),
            "the leaked descriptor's lock outlived it for the whole fork \
             window; {}",
            RefreshLock::diagnose_unavailable(&path)
        );
    }

    // trace:TASK-1526 | ai:codex
    #[cfg(unix)]
    #[test]
    fn nested_refresh_through_directory_alias_uses_same_registry_entry() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let outer = RefreshLock::try_acquire(&real.join("cache.db"))
            .unwrap()
            .unwrap();
        let inner = RefreshLock::try_acquire(&alias.join("cache.db"))
            .unwrap()
            .unwrap();
        assert!(inner.is_nested());
        assert!(RefreshLock::held_by_current_thread(&alias.join("cache.db")));
        drop(inner);
        drop(outer);
    }

    // trace:TASK-1526 | ai:codex
    #[test]
    fn stale_note_wording_matches_refresh_state() {
        let mut s = StaleServe {
            stale: true,
            cache_head: Some("old".into()),
            store_head: "new".into(),
            built_at: None,
            refreshing: RefreshState::WriterBusy,
        };
        assert!(s.note().contains("cache write lock was busy"));
        assert!(!s.note().contains("cache is refreshing"));
        s.refreshing = RefreshState::Deferred;
        assert!(s.note().contains("refresh is deferred"));
        assert!(!s.note().contains("refreshing"));
        assert!(!s.note().contains("requested"));
        assert!(!s.note().contains("few seconds"));
        s.refreshing = RefreshState::WorkerRunning;
        assert!(s.note().contains("cache is refreshing"));
        let scope = CacheReadScope::new();
        record_read(Some(s));
        {
            let nested = CacheReadScope::new();
            assert!(!nested.touched());
            record_read(None);
            assert_eq!(nested.metadata()["stale"], false);
        }
        assert_eq!(scope.metadata()["stale"], true);
    }
}
