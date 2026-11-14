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
//! HEAD match the last completed one (bounded by a TTL, so forge-side changes
//! that move neither HEAD are still picked up).
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
    /// The process's start identity; empty when it could not be read.
    pub start: String,
    pub token: String,
}

impl Runner {
    fn current() -> Self {
        let pid = std::process::id();
        Self {
            pid,
            start: aida_core::liveness::process_start_identity(pid).unwrap_or_default(),
            token: uuid::Uuid::new_v4().to_string(),
        }
    }

    /// Whether the recorded process is still the one running. A marker left
    /// by a pull that crashed names a process that is gone.
    fn is_live(&self) -> bool {
        if self.start.is_empty() {
            // No identity to compare: only this very process can vouch.
            return self.pid == std::process::id();
        }
        aida_core::liveness::process_identity_matches_strict(self.pid, &self.start)
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
        token: lines.next()?.trim().to_string(),
    })
}

pub(crate) fn write_runner(path: &Path, runner: &Runner) -> bool {
    let tmp = path.with_file_name(format!(
        "{}.{}.tmp.lock",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("pull"),
        runner.token
    ));
    let body = format!("{}\n{}\n{}\n", runner.pid, runner.start, runner.token);
    let written = std::fs::write(&tmp, body)
        .and_then(|_| std::fs::rename(&tmp, path))
        .is_ok();
    if !written {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Removes this call's marker when the pull ends, however it ends.
struct RunnerGuard<'a> {
    path: &'a Path,
    runner: &'a Runner,
}

impl Drop for RunnerGuard<'_> {
    fn drop(&mut self) {
        if read_runner(self.path).as_ref() == Some(self.runner) {
            let _ = std::fs::remove_file(self.path);
        }
    }
}

/// The lock file, locked. Held only while a caller reads and writes the
/// marker and the state; dropping it unlocks and closes the descriptor.
fn decision_lock(lock_path: &Path) -> Option<std::fs::File> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)
        .ok()?;
    lock.lock_exclusive().ok()?;
    Some(lock)
}

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
    let mut waited = false;
    // Decide under the lock; leave the loop as the pull to run.
    loop {
        // No lock file (read-only store, odd filesystem): behave as before.
        // trace:BUG-2565 | ai:claude
        let Some(lock) = decision_lock(&lock_path) else {
            return run().map(|()| Turn::Ran);
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
                    eprintln!(
                        "  the other store pull is still running after {}s — pulling without it",
                        budget.as_secs()
                    );
                    return run().map(|()| Turn::Ran);
                }
                std::thread::sleep(POLL);
                continue;
            }
            None => {}
        }
        if waited {
            let now = read_generation(&state_path);
            let finished_meanwhile = match (seen, now) {
                (Some(a), Some(b)) => b > a,
                (None, Some(_)) => true,
                _ => false,
            };
            if finished_meanwhile && reusable() {
                return Ok(Turn::Reused);
            }
        }
        // A marker that cannot be written costs only the coalescing.
        let _ = write_runner(&run_path, &me);
        drop(lock);
        break;
    }
    // From here to the end of `run`, nothing of this module is locked or open.
    let guard = RunnerGuard {
        path: &run_path,
        runner: &me,
    };
    let result = run();
    // The state is written before the marker goes, so a waiter that sees
    // no pull in flight also sees how that pull ended.
    let lock = decision_lock(&lock_path);
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

pub(crate) fn pull_reusable(store: &Path, root: &Path, code_only: bool, store_only: bool) -> bool {
    (code_only || store_pull_reusable(store))
        && (store_only
            || store_already_has_remote_head(root, &aida_core::git_ops::default_branch_name(root)))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `(code HEAD, store HEAD)` when both are readable and the store has no
/// pending changes; `None` means "no watermark applies".
fn reconcile_key(project_root: &Path, store_path: &Path) -> Option<(String, String)> {
    use aida_core::git_ops;
    let code = git_ops::head_sha(project_root).ok()?;
    let store = git_ops::head_sha(store_path).ok()?;
    if git_ops::has_changes_checked(store_path).unwrap_or(true) {
        return None;
    }
    Some((code, store))
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
/// code HEAD and store HEAD within the TTL, so re-running it can only repeat
/// the same forge lookups.
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
/// HEADs (read after it ran, so its own store commits are included). A dirty
/// store records nothing: the next pull commits it and moves the store HEAD.
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
mod tests;
