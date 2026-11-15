//! Store-pull coalescing and the no-op reconcile watermark.
//!
//! Two costs made a store pull take minutes when nothing had changed:
//!
//! * every concurrent caller (six reviewer launches each running
//!   `aida db sync --pull`, a train's post-merge `aida pull`) ran the whole
//!   pull itself, one after another on the store write lock;
//! * every pull re-ran the code-derived reconcile (auto-bump, waiting-review
//!   and stranded-review forge lookups) over the last 50 commits, even when
//!   neither the code repo nor the store had moved since the last one.
//!
//! [`coalesce`] lets one pull of a kind run per checkout and lets a caller
//! that waited for it reuse its successful result instead of re-running it.
//! The pull in flight is named by a marker file, not by a held lock: the
//! lock file is locked only while a caller decides (is a pull running, can
//! its result be reused, am I the one to run) and is closed before the pull
//! starts. No lock descriptor of this module is open while the store-write
//! window, its hooks or `git gc` run, and a pull started from inside a
//! running pull (a hook) is never made to wait for its own ancestor.
//! [`reconcile_is_current`] /
//! [`record_reconcile`] skip the wide reconcile when the code HEAD and store
//! input fingerprint match the last completed one (bounded by a TTL, so
//! forge-side changes that move neither input are still picked up).
// trace:BUG-2565 | ai:claude

use aida_core::exec_context::Environment;
use fs2::FileExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long a caller waits for an in-flight pull before running its own.
const WAIT_BUDGET: Duration = Duration::from_secs(20 * 60);
/// Poll interval while another pull holds the lock.
const POLL: Duration = Duration::from_millis(200);
/// Default lifetime of a reconcile watermark.
const WATERMARK_TTL: Duration = Duration::from_secs(15 * 60);
/// Per-worktree git-path file holding the last reconcile watermark.
pub(crate) const RECONCILE_WATERMARK_STATE: &str = "aida-reconcile-watermark";

/// What [`coalesce`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Turn {
    /// This caller ran the pull.
    Ran,
    /// A concurrent pull finished successfully while this caller waited;
    /// nothing was re-run.
    Reused,
}

/// FNV-1a, so the lock name is stable across processes without a new crate.
fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Lock and state files for one `(kind, checkout)` pair. Both end in `.lock`
/// so the store's `.aida/*.lock` ignore rule keeps them out of commits.
pub(crate) fn coalesce_paths(
    store_path: &Path,
    project_root: &Path,
    kind: &str,
) -> (PathBuf, PathBuf) {
    let root = std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let tag = format!("{kind}-{:016x}", fnv1a(&root.to_string_lossy()));
    let dir = store_path.join(".aida");
    (
        dir.join(format!("pull-{tag}.lock")),
        dir.join(format!("pull-{tag}-done.lock")),
    )
}

/// The marker naming the pull in flight, beside the lock file of
/// [`coalesce_paths`]. It ends in `.lock` for the same ignore rule; it is
/// never locked or kept open.
pub(crate) fn runner_path(lock_path: &Path) -> PathBuf {
    let name = lock_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("pull.lock");
    lock_path.with_file_name(format!(
        "{}-running.lock",
        name.strip_suffix(".lock").unwrap_or(name)
    ))
}

/// The pull in flight: the process running it, and one token per call so
/// two pulls in one process are told apart.
// trace:BUG-2565 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Runner {
    pub pid: u32,
    /// The process's start identity (`linux:<boot>:<ticks>`); empty when it
    /// could not be read.
    pub start: String,
    /// The pid namespace `pid` is numbered in; 0 when unknown.
    pub ns: u64,
    pub token: String,
}

impl Runner {
    fn current() -> Self {
        let pid = std::process::id();
        let id = aida_core::proc_identity::ProcIdentity::capture(pid).ok();
        Self {
            pid,
            start: id.as_ref().map(|id| id.legacy().start).unwrap_or_default(),
            ns: id.map(|id| id.pidns_inode).unwrap_or(0),
            token: uuid::Uuid::new_v4().to_string(),
        }
    }

    /// Whether the recorded process is still the one running. A marker left
    /// by a pull that crashed names a process that is gone.
    fn is_live(&self) -> bool {
        use aida_core::proc_identity::{observe, ProcIdentity, Verdict};
        // A marker naming this PID is this process's only while one of its
        // calls holds that token. One left by an earlier process that had
        // this PID (reuse, reboot, another namespace) names no active call.
        if self.pid == std::process::id() {
            return active_calls().contains(&self.token);
        }
        if self.start.is_empty() || self.ns == 0 {
            // No identity to compare and not this process: not vouched for.
            return false;
        }
        ProcIdentity::from_registry(self.pid, Some(&self.start), Some(self.ns))
            .is_some_and(|id| observe(&id, true) == Verdict::Alive)
    }

    /// Whether this process was started, however indirectly, by the
    /// recorded one: a hook of the pull in flight that runs a pull itself.
    fn is_ancestor_of_this_process(&self) -> bool {
        let me = std::process::id();
        self.pid != me
            && aida_core::liveness::walk_ancestor_pids(me)
                .iter()
                .skip(1)
                .any(|pid| *pid == self.pid)
    }
}

pub(crate) fn read_runner(path: &Path) -> Option<Runner> {
    let body = std::fs::read_to_string(path).ok()?;
    let mut lines = body.lines();
    Some(Runner {
        pid: lines.next()?.trim().parse().ok()?,
        start: lines.next()?.trim().to_string(),
        ns: lines.next()?.trim().parse().ok()?,
        token: lines.next()?.trim().to_string(),
    })
}

pub(crate) fn write_runner(path: &Path, runner: &Runner) -> bool {
    let tmp = path.with_file_name(format!(
        "{}.{}.tmp.lock",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("pull"),
        runner.token
    ));
    let body = format!(
        "{}\n{}\n{}\n{}\n",
        runner.pid, runner.start, runner.ns, runner.token
    );
    let written = std::fs::write(&tmp, body)
        .and_then(|_| std::fs::rename(&tmp, path))
        .is_ok();
    if !written {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// The tokens of the pulls this process is running now.
// trace:BUG-2565 | ai:claude
fn active_calls() -> std::sync::MutexGuard<'static, std::collections::HashSet<String>> {
    static ACTIVE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    ACTIVE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Marks this call active from before its marker is written, and removes
/// the marker and the mark when the pull ends, however it ends.
struct RunnerGuard<'a> {
    path: &'a Path,
    runner: &'a Runner,
}

impl<'a> RunnerGuard<'a> {
    fn new(path: &'a Path, runner: &'a Runner) -> Self {
        active_calls().insert(runner.token.clone());
        Self { path, runner }
    }
}

impl Drop for RunnerGuard<'_> {
    fn drop(&mut self) {
        if read_runner(self.path).as_ref() == Some(self.runner) {
            let _ = std::fs::remove_file(self.path);
        }
        active_calls().remove(&self.runner.token);
    }
}

/// The outcome of asking for the decision lock.
enum Decision {
    /// Locked. Held only while a caller reads and writes the marker and the
    /// state; dropping it unlocks and closes the descriptor.
    Held(std::fs::File),
    /// Another caller held it until `deadline`; no descriptor is kept.
    Busy,
    /// No lock file (read-only store, odd filesystem).
    Unavailable,
}

/// Take the lock file without blocking: one try, then retries every
/// [`POLL`] until `deadline`. Nothing slow runs under it, so contention is
/// short; a holder that stalls costs a caller at most its own wait budget.
// trace:BUG-2565 | ai:claude
fn decision_lock(lock_path: &Path, deadline: Instant) -> Decision {
    if let Some(parent) = lock_path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return Decision::Unavailable;
        }
    }
    let Ok(lock) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)
    else {
        return Decision::Unavailable;
    };
    loop {
        if lock.try_lock_exclusive().is_ok() {
            return Decision::Held(lock);
        }
        let now = Instant::now();
        if now >= deadline {
            return Decision::Busy;
        }
        std::thread::sleep(POLL.min(deadline - now));
    }
}

/// Time allowed for the lock that orders the state write before the marker
/// removal. Missing it costs only that ordering: both writes are renames.
const STATE_LOCK_WAIT: Duration = Duration::from_secs(5);

/// Generation of the last successful pull, or `None` when unknown.
fn read_generation(state: &Path) -> Option<u64> {
    let body = std::fs::read_to_string(state).ok()?;
    let mut parts = body.split_whitespace();
    let generation = parts.next()?.parse::<u64>().ok()?;
    match parts.next() {
        Some("ok") => Some(generation),
        _ => None,
    }
}

/// Raw generation counter (successful or not) — the next write increments it.
fn read_counter(state: &Path) -> u64 {
    std::fs::read_to_string(state)
        .ok()
        .and_then(|b| b.split_whitespace().next()?.parse::<u64>().ok())
        .unwrap_or(0)
}

fn write_state(state: &Path, generation: u64, ok: bool) {
    let tmp = state.with_file_name(format!(
        "{}.{}.tmp.lock",
        state.file_name().and_then(|n| n.to_str()).unwrap_or("pull"),
        std::process::id()
    ));
    let body = format!("{generation} {}\n", if ok { "ok" } else { "failed" });
    if std::fs::write(&tmp, body)
        .and_then(|_| std::fs::rename(&tmp, state))
        .is_err()
    {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Run `run` as the one pull of `kind` for this checkout. When another pull of
/// the same kind is in flight, wait for it; if it finished successfully while
/// this caller waited, return [`Turn::Reused`] without running `run`. A pull
/// that failed, crashed, or outlived the wait budget is never reused — the
/// caller runs its own. `reusable` is asked once the in-flight pull finished
/// and must veto reuse when a selected remote moved after the finished pull's
/// fetch, its freshness cannot be checked, or pending writes remain. Success
/// alone is insufficient: an older pull may have fetched before this caller
/// was invoked.
///
/// `run` is called with no lock of this module held and no descriptor of it
/// open. A caller whose own ancestor is the pull in flight runs at once.
// trace:BUG-2565 | ai:codex
pub(crate) fn coalesce(
    store_path: &Path,
    project_root: &Path,
    kind: &str,
    reusable: impl FnOnce() -> bool,
    run: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<Turn> {
    coalesce_with_environment(
        store_path,
        project_root,
        kind,
        &Environment::capture(),
        reusable,
        run,
    )
}

fn coalesce_with_environment(
    store_path: &Path,
    project_root: &Path,
    kind: &str,
    environment: &Environment,
    reusable: impl FnOnce() -> bool,
    run: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<Turn> {
    // Only a real store gets lock files; never create `.aida/` elsewhere.
    if environment.var("AIDA_PULL_COALESCE") == Ok("0") || !store_path.join(".git").exists() {
        return run().map(|()| Turn::Ran);
    }
    let (lock_path, state_path) = coalesce_paths(store_path, project_root, kind);
    let run_path = runner_path(&lock_path);
    let me = Runner::current();
    let seen = read_generation(&state_path);
    let budget = duration_setting(environment, "AIDA_PULL_COALESCE_WAIT_SECS", WAIT_BUDGET);
    let start = Instant::now();
    let deadline = start + budget;
    let mut waited = false;
    // `reusable` probes the remote: it runs once, with no lock held.
    let mut reusable = Some(reusable);
    let gave_up = |budget: Duration| {
        eprintln!(
            "  the other store pull is still running after {}s — pulling without it",
            budget.as_secs()
        );
    };
    // Decide under the lock; leave the loop as the pull to run.
    let guard = loop {
        // trace:BUG-2565 | ai:claude
        let lock = match decision_lock(&lock_path, deadline) {
            Decision::Held(lock) => lock,
            // No lock file: behave as before.
            Decision::Unavailable => return run().map(|()| Turn::Ran),
            // The descriptor is already closed.
            Decision::Busy => {
                gave_up(budget);
                return run().map(|()| Turn::Ran);
            }
        };
        match read_runner(&run_path).filter(Runner::is_live) {
            // A hook of the pull in flight ran this pull: waiting for it
            // would wait for ourselves.
            Some(holder) if holder.is_ancestor_of_this_process() => {
                drop(lock);
                return run().map(|()| Turn::Ran);
            }
            Some(_) => {
                drop(lock);
                if !waited {
                    waited = true;
                    eprintln!(
                        "  waiting for a store pull already running in this checkout (will check freshness before reuse)…"
                    );
                }
                if start.elapsed() >= budget {
                    gave_up(budget);
                    return run().map(|()| Turn::Ran);
                }
                std::thread::sleep(POLL);
                continue;
            }
            None => {}
        }
        let now = read_generation(&state_path);
        let finished_meanwhile = match (seen, now) {
            (Some(a), Some(b)) => b > a,
            (None, Some(_)) => true,
            _ => false,
        };
        if waited && finished_meanwhile {
            if let Some(reusable) = reusable.take() {
                // The freshness probe talks to the remote: never under the lock.
                drop(lock);
                if !reusable() {
                    continue;
                }
                // Revalidate: reuse only if no pull started and none ended
                // since the probe's view. Otherwise decide again, without reuse.
                match decision_lock(&lock_path, deadline) {
                    Decision::Held(lock) => {
                        let unchanged = read_runner(&run_path).filter(Runner::is_live).is_none()
                            && read_generation(&state_path) == now;
                        drop(lock);
                        if unchanged {
                            return Ok(Turn::Reused);
                        }
                        continue;
                    }
                    Decision::Unavailable => return run().map(|()| Turn::Ran),
                    Decision::Busy => {
                        gave_up(budget);
                        return run().map(|()| Turn::Ran);
                    }
                }
            }
        }
        // Active before the marker is visible, so no waiter sees a marker of
        // this process that names no active call.
        let guard = RunnerGuard::new(&run_path, &me);
        // A marker that cannot be written costs only the coalescing.
        let _ = write_runner(&run_path, &me);
        drop(lock);
        break guard;
    };
    // From here to the end of `run`, nothing of this module is locked or open.
    let result = run();
    // The state is written before the marker goes, so a waiter that sees
    // no pull in flight also sees how that pull ended.
    let lock = decision_lock(&lock_path, Instant::now() + STATE_LOCK_WAIT);
    write_state(&state_path, read_counter(&state_path) + 1, result.is_ok());
    drop(guard);
    drop(lock);
    result.map(|()| Turn::Ran)
}

/// True when origin's `branch` head is already contained in the local store
/// HEAD (equal, or behind unpushed local commits), so a pull would bring in
/// nothing. One `ls-remote`, no object transfer. An unreachable remote, an
/// unknown sha or any git error answers false and the caller pulls as before.
pub(crate) fn store_already_has_remote_head(store_path: &Path, branch: &str) -> bool {
    use aida_core::git_ops;
    let Some(remote) = git_ops::remote_branch_head_sha(store_path, "origin", branch) else {
        return false;
    };
    let Ok(local) = git_ops::head_sha(store_path) else {
        return false;
    };
    remote == local || git_ops::is_ancestor(store_path, &remote, &local).unwrap_or(false)
}

fn duration_setting(environment: &Environment, name: &str, default: Duration) -> Duration {
    environment
        .var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(default)
}

/// Refuse reuse when pending writes or a newly advertised remote head exist.
// trace:BUG-2565 | ai:codex
pub(crate) fn store_pull_reusable(store: &Path) -> bool {
    !aida_core::git_ops::has_changes_checked(store).unwrap_or(true)
        && aida_core::git_ops::current_branch(store)
            .ok()
            .is_some_and(|branch| store_already_has_remote_head(store, &branch))
}

/// The code leg pulls `origin/<current branch>`, so freshness is asked of
/// that branch; an unreadable branch vetoes reuse.
// trace:BUG-2565 | ai:claude
pub(crate) fn pull_reusable(store: &Path, root: &Path, code_only: bool, store_only: bool) -> bool {
    (code_only || store_pull_reusable(store))
        && (store_only
            || aida_core::git_ops::current_branch(root)
                .ok()
                .is_some_and(|branch| store_already_has_remote_head(root, &branch)))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Fingerprint the effective store files, independent of whether pending writes
/// have been committed. Pull bookkeeping must not invalidate identical inputs.
/// Any failed observation refuses reuse; the real index and objects stay untouched.
// trace:BUG-2593 | ai:codex
fn reconcile_key(project_root: &Path, store_path: &Path) -> Option<(String, String)> {
    use sha2::{Digest, Sha256};
    let code = aida_core::git_ops::head_sha(project_root).ok()?;
    let observe = |args: &[&str]| {
        aida_core::git_ops::host_git_command(store_path)
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| out.stdout)
    };
    // A broken/unborn store cannot supply reusable reconcile evidence.
    observe(&["rev-parse", "--verify", "HEAD^{tree}"])?;
    let mut digest = Sha256::new();
    let files = observe(&[
        "ls-files",
        "--cached",
        "--others",
        "--exclude-standard",
        "-z",
    ])?;
    let mut names: Vec<_> = files
        .split(|b| *b == 0)
        .filter(|name| !name.is_empty())
        .collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::PermissionsExt;
        let path = store_path.join(std::ffi::OsStr::from_bytes(name));
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            // A tracked deletion is absent from the effective snapshot.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return None;
        }
        digest.update([u8::from(metadata.permissions().mode() & 0o111 != 0)]);
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name);
        let content = std::fs::read(path).ok()?;
        digest.update((content.len() as u64).to_le_bytes());
        digest.update(content);
    }
    Some((code, format!("v3-{:x}", digest.finalize())))
}

/// Parse `<code> <store> <unix-secs>`.
pub(crate) fn parse_watermark(body: &str) -> Option<(String, String, u64)> {
    let mut parts = body.split_whitespace();
    let code = parts.next()?.to_string();
    let store = parts.next()?.to_string();
    let at = parts.next()?.parse::<u64>().ok()?;
    Some((code, store, at))
}

/// True when a wide (no scan base) reconcile already ran against exactly this
/// code HEAD and store input fingerprint within the TTL, so re-running it
/// can only repeat the same forge lookups.
pub(crate) fn reconcile_is_current(project_root: &Path, store_path: &Path) -> bool {
    reconcile_is_current_with_environment(project_root, store_path, &Environment::capture())
}

fn reconcile_is_current_with_environment(
    project_root: &Path,
    store_path: &Path,
    environment: &Environment,
) -> bool {
    let Some((code, store)) = reconcile_key(project_root, store_path) else {
        return false;
    };
    let lines =
        crate::store_push_pull::read_reconcile_state_lines(project_root, RECONCILE_WATERMARK_STATE);
    let Some((w_code, w_store, at)) = lines.first().and_then(|l| parse_watermark(l)) else {
        return false;
    };
    w_code == code
        && w_store == store
        && now_secs().saturating_sub(at)
            < duration_setting(
                environment,
                "AIDA_RECONCILE_WATERMARK_TTL_SECS",
                WATERMARK_TTL,
            )
            .as_secs()
}

/// Record that a reconcile completed against the current code and store
/// inputs (read after it ran, so its own store writes are included).
pub(crate) fn record_reconcile(project_root: &Path, store_path: &Path) {
    match reconcile_key(project_root, store_path) {
        Some((code, store)) => crate::store_push_pull::write_reconcile_state(
            project_root,
            RECONCILE_WATERMARK_STATE,
            &format!("{code} {store} {}", now_secs()),
        ),
        None => {
            crate::store_push_pull::clear_reconcile_state(project_root, RECONCILE_WATERMARK_STATE)
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Code-derived reconciles run on this thread (test observation only).
    static RECONCILE_RUNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn note_reconcile_run() {
    RECONCILE_RUNS.with(|c| c.set(c.get() + 1));
}

#[cfg(test)]
pub(crate) fn reconcile_runs() -> usize {
    RECONCILE_RUNS.with(std::cell::Cell::get)
}

#[cfg(test)]
#[path = "tests/bug_2565_store_pull_coalesce_tests.rs"]
mod bug_2565_store_pull_coalesce_tests;
