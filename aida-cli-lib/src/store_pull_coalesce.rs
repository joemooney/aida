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
//! [`coalesce`] serialises pulls of one kind in one checkout on a lock file
//! and lets a caller that waited for an in-flight pull reuse its successful
//! result instead of re-running it. [`reconcile_is_current`] /
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
/// and can veto reuse (for example, when the store gained pending changes the
/// finished pull did not commit).
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
    let lock = lock_path
        .parent()
        .map(std::fs::create_dir_all)
        .transpose()
        .ok()
        .and_then(|_| {
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&lock_path)
                .ok()
        });
    // No lock file (read-only store, odd filesystem): behave as before.
    let Some(lock) = lock else {
        return run().map(|()| Turn::Ran);
    };
    let seen = read_generation(&state_path);
    let mut waited = false;
    if lock.try_lock_exclusive().is_err() {
        waited = true;
        eprintln!(
            "  waiting for a store pull already running in this checkout (its result is reused)…"
        );
        let budget = duration_setting(environment, "AIDA_PULL_COALESCE_WAIT_SECS", WAIT_BUDGET);
        let start = Instant::now();
        loop {
            std::thread::sleep(POLL);
            if lock.try_lock_exclusive().is_ok() {
                break;
            }
            if start.elapsed() >= budget {
                eprintln!(
                    "  the other store pull is still running after {}s — pulling without it",
                    budget.as_secs()
                );
                return run().map(|()| Turn::Ran);
            }
        }
    }
    // Held from here; released when `lock` drops.
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
    let result = run();
    write_state(&state_path, read_counter(&state_path) + 1, result.is_ok());
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
