//! Worktree warm-pool registry + primitives (STORY-714).
//!
//! AIDA's historical model treats a per-spec worktree as **disposable**:
//! `aida queue work` / `aida agent new` / the orchestrator create a sibling
//! worktree, the implementer works in it, and `aida session end` deletes it
//! with `git worktree remove --force`. The expensive, warm thing is the
//! worktree's `target/` cache; the branch is the cheap, throwaway thing.
//!
//! The warm-pool (ported from treehouse) inverts that: it keeps a fixed pool
//! of worktrees per repo and, on hand-back, **resets the worktree to a clean
//! detached-HEAD base instead of deleting it**. `acquire` prefers an idle
//! pooled worktree (reset-not-create); `return_to_pool` resets-and-idles
//! instead of removing; the directory persists so the build cache stays warm.
//!
//! Two whole bug classes are *dissolved* (not patched):
//!   * **TASK-0396** (cross-worktree cargo `target/.fingerprint` poison) — a
//!     tree that is never removed never leaves a dangling absolute path; the
//!     one delete path (`destroy`) runs a `cargo clean` `pre_destroy` hook.
//!   * **BUG-553** (branch-stacking on worktree reuse) — every `acquire`
//!     unconditionally hard-resets to a detached default ref, so no caller can
//!     reuse a tree and forget to base-reset.
//!
//! State lives under `.aida/worktree-pool/` (per-clone runtime state; the
//! `.aida/*` gitignore block already covers it — BUG-73). All mutations run
//! under an advisory file lock (`pool.lock`) so parallel fan-out implementers
//! can't be handed the same idle tree.
//!
//! trace:STORY-714 trace:TASK-0396 trace:BUG-553 | ai:claude

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::git_ops;

/// Default cap on the number of pool worktrees (treehouse's default). A
/// fan-out wider than this blocks on "all worktrees in use"; the cap is
/// configurable via `[worktree_pool] max_trees` in `.aida/config.toml`.
pub const DEFAULT_MAX_TREES: usize = 16;

/// Default lease TTL (seconds): a durable lease whose `leased_at` is older than
/// this is treated as EXPIRED and reclaimable. A pooled lease is reserved by
/// `acquire` while the short-lived start process exits, so a session that dies
/// without `return_to_pool` would otherwise pin its tree forever. Six hours is
/// comfortably longer than any healthy drive yet short enough to recover a
/// leaked reservation the same working session. Configurable via
/// `[worktree_pool] lease_ttl_secs` in `.aida/config.toml`.
// trace:TASK-1008 | ai:claude
pub const DEFAULT_LEASE_TTL_SECS: i64 = 6 * 60 * 60;

/// Pure decision helper: is a lease stamped at `leased_at` expired at `now`,
/// given a TTL of `ttl` seconds? A lease with no `leased_at` stamp can't be
/// judged stale (returns false — never wrongly reclaimed). A non-positive TTL
/// disables expiry. The boundary is inclusive (age == ttl → expired).
// trace:TASK-1008 | ai:claude
pub fn lease_expired(leased_at: Option<i64>, now: i64, ttl: i64) -> bool {
    if ttl <= 0 {
        return false;
    }
    match leased_at {
        Some(ts) => now.saturating_sub(ts) >= ttl,
        None => false,
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One worktree in the pool. Serde skips empty/default fields so a pre-pool or
/// minimally-populated entry round-trips cleanly (mirrors treehouse's
/// `WorktreeEntry` json tags).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PoolEntry {
    /// Stable pool name, e.g. `aida-pool-3`.
    pub name: String,
    /// Absolute path to the worktree directory.
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    /// PID of the process that currently owns the tree. Self-heals to None
    /// when the owner dies (PID-liveness).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_pid: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_started_at: Option<i64>,
    /// Durable reservation — survives with zero live processes inside the
    /// tree. Distinct from `owner_pid` (which self-heals on owner death).
    #[serde(default, skip_serializing_if = "is_false")]
    pub leased: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_holder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leased_at: Option<i64>,
    /// Set while a `destroy` is mid-flight so a concurrent acquire skips it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub destroying: bool,
}

impl PoolEntry {
    /// True when the entry is free to hand out: not leased, no live owner, not
    /// mid-destroy. Dirtiness is checked separately against the filesystem.
    fn is_idle(&self) -> bool {
        !self.leased && self.owner_pid.is_none() && !self.destroying
    }

    /// True when this leased entry is a stale reservation safe to reclaim: it
    /// carries a durable lease, has no live owner process, and its `leased_at`
    /// is older than the TTL. This is the reservation-leak backstop — a session
    /// that reserved a tree (`acquire` with a `lease_holder`) and then died
    /// without `return_to_pool` would otherwise pin the tree forever. Reuses
    /// the same PID-liveness check `heal_state` uses, so a still-live owner is
    /// never reclaimed even past the TTL.
    // trace:TASK-1008 | ai:claude
    fn is_lease_expired(&self, now: i64, ttl: i64) -> bool {
        self.leased
            && !self.destroying
            && !owner_is_live(self)
            && lease_expired(self.leased_at, now, ttl)
    }
}

/// The on-disk registry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Pool {
    #[serde(default)]
    pub entries: Vec<PoolEntry>,
}

/// Options controlling `acquire`.
#[derive(Debug, Clone, Default)]
pub struct AcquireOptions {
    /// When set, stamp a durable reservation (`leased`) under this holder name
    /// instead of (or in addition to) the PID-liveness owner stamp. Use for a
    /// headless drain that parks `NeedsAttention` and must keep its tree.
    pub lease_holder: Option<String>,
    /// Cap on pool size; falls back to [`DEFAULT_MAX_TREES`] when None.
    pub max_trees: Option<usize>,
    /// Lease TTL in seconds; falls back to [`DEFAULT_LEASE_TTL_SECS`] when None.
    /// A stale lease older than this (with no live owner) is reclaimable.
    // trace:TASK-1008 | ai:claude
    pub lease_ttl_secs: Option<i64>,
    /// Shell commands run after a fresh `git worktree add` (machine-global
    /// config only — see `worktree_hooks`).
    pub post_create_hooks: Vec<String>,
}

/// The classified live state of a pool entry, for `aida worktree pool status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolState {
    /// Idle, clean, ready to hand out.
    Available,
    /// A live process owns it.
    InUse,
    /// Durably reserved (lease), possibly with no live process.
    Leased,
    /// A durable lease whose `leased_at` is older than the TTL and whose owner
    /// process is gone — a stale reservation the next `acquire` may reclaim.
    Expired,
    /// Idle but has uncommitted changes.
    Dirty,
    /// A destroy is mid-flight.
    Destroying,
    /// The caller is currently inside this worktree.
    Here,
}

impl PoolState {
    pub fn label(self) -> &'static str {
        match self {
            PoolState::Available => "available",
            PoolState::InUse => "in-use",
            PoolState::Leased => "leased",
            PoolState::Expired => "expired",
            PoolState::Dirty => "dirty",
            PoolState::Destroying => "destroying",
            PoolState::Here => "here",
        }
    }
}

/// A pool entry plus its classified live state and current HEAD.
#[derive(Debug, Clone)]
pub struct PoolStatus {
    pub entry: PoolEntry,
    pub state: PoolState,
    pub head: Option<String>,
}

// ── Paths ───────────────────────────────────────────────────────────────────

/// The pool's runtime directory: `<project_root>/.aida/worktree-pool/`.
pub fn pool_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("worktree-pool")
}

fn state_path(project_root: &Path) -> PathBuf {
    pool_dir(project_root).join("pool.json")
}

fn lock_path(project_root: &Path) -> PathBuf {
    pool_dir(project_root).join("pool.lock")
}

// ── PID liveness ─────────────────────────────────────────────────────────────

/// True when `pid` is a live process. Unix uses `kill(pid, 0)`; other
/// platforms conservatively return true (treat unknown as alive, so a tree is
/// never wrongly reclaimed — cross-platform parity is a tracked followup).
fn pid_is_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only probes existence/permission; it sends nothing.
        let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if rc == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// True when the entry has a live owner process. Public so the destroy
/// classifier can distinguish a self-healable crash from a genuine in-use tree.
pub fn owner_is_live(entry: &PoolEntry) -> bool {
    entry.owner_pid.map(pid_is_alive).unwrap_or(false)
}

// ── State I/O + locking ──────────────────────────────────────────────────────

/// Read the registry, returning an empty pool when none exists yet (today's
/// behavior — a missing/empty registry decodes to empty).
pub fn read_state(project_root: &Path) -> Result<Pool> {
    let path = state_path(project_root);
    match std::fs::read_to_string(&path) {
        Ok(s) if !s.trim().is_empty() => serde_json::from_str(&s)
            .with_context(|| format!("parse worktree pool registry at {}", path.display())),
        _ => Ok(Pool::default()),
    }
}

fn write_state(project_root: &Path, pool: &Pool) -> Result<()> {
    let dir = pool_dir(project_root);
    std::fs::create_dir_all(&dir).with_context(|| format!("create pool dir {}", dir.display()))?;
    let json = serde_json::to_string_pretty(pool).context("serialize worktree pool registry")?;
    crate::write_atomic(&state_path(project_root), json.as_bytes())
        .with_context(|| "write worktree pool registry")?;
    Ok(())
}

/// Run `f` against the registry while holding an advisory file lock, then
/// persist the (possibly mutated) pool. The lock serializes concurrent
/// acquires so two callers can't be handed the same idle tree. `heal_state`
/// runs before `f` so callers always see a self-healed view.
///
/// **Cross-platform guarantee.** The exclusive lock is acquired through
/// `fs2::FileExt::lock_exclusive`, which maps to `flock(2)` on Unix and to
/// `LockFileEx` on Windows — so the same mutual exclusion holds on both, and
/// the nightly cross-platform matrix exercises this code path on each OS.
/// (Treehouse, the upstream this pool was ported from, splits the same concern
/// into `lock_unix.go` / `lock_windows.go`; `fs2` gives us that parity behind a
/// single call, with no `cfg`-gated divergence.) The lock is held for the whole
/// read-modify-write window, so two callers serialize even when each opens its
/// own descriptor on the same `pool.lock` file.
pub fn with_state_lock<T>(
    project_root: &Path,
    f: impl FnOnce(&mut Pool) -> Result<T>,
) -> Result<T> {
    let dir = pool_dir(project_root);
    std::fs::create_dir_all(&dir).with_context(|| format!("create pool dir {}", dir.display()))?;

    // The lock file is a stable advisory token; its contents are irrelevant.
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_path(project_root))
        .with_context(|| "open worktree pool lock")?;

    #[cfg(feature = "native")]
    {
        use fs2::FileExt;
        lock_file
            .lock_exclusive()
            .with_context(|| "acquire worktree pool lock")?;
    }

    let result = (|| {
        let mut pool = read_state(project_root)?;
        heal_state(&mut pool);
        let out = f(&mut pool)?;
        write_state(project_root, &pool)?;
        Ok(out)
    })();

    #[cfg(feature = "native")]
    {
        use fs2::FileExt;
        let _ = FileExt::unlock(&lock_file);
    }
    drop(lock_file);
    result
}

/// Self-heal the in-memory registry: drop entries whose directory is gone, and
/// clear `owner_*` when the owner pid is dead. A durable `leased` entry with no
/// live pid stays reserved (the reservation survives process death by design).
pub fn heal_state(pool: &mut Pool) {
    pool.entries.retain(|e| e.path.exists());
    for e in &mut pool.entries {
        if let Some(pid) = e.owner_pid {
            if !pid_is_alive(pid) {
                e.owner_pid = None;
                e.owner_started_at = None;
            }
        }
    }
}

// ── Acquire / return ─────────────────────────────────────────────────────────

/// Pool worktree names are namespaced by the project's directory so two repos
/// sharing a parent dir (`../`) never collide on `aida-pool-0`.
fn pool_name_prefix(project_root: &Path) -> String {
    let slug = project_root
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("project");
    format!("aida-pool-{slug}")
}

fn next_pool_name(pool: &Pool, project_root: &Path) -> (String, usize) {
    let prefix = pool_name_prefix(project_root);
    let mut n = 0usize;
    loop {
        let name = format!("{prefix}-{n}");
        if !pool.entries.iter().any(|e| e.name == name) {
            return (name, n);
        }
        n += 1;
    }
}

fn pool_path_for(project_root: &Path, name: &str) -> PathBuf {
    // Siblings of the project root (../aida-pool-<slug>-<n>), matching AIDA's
    // existing `../aida-<slug>` worktree convention. Falls back to nesting
    // under the root if it has no parent (a filesystem root — never in
    // practice).
    match project_root.parent() {
        Some(parent) => parent.join(name),
        None => project_root.join(name),
    }
}

/// Acquire a worktree from the pool: prefer an idle clean tree (reset-not-
/// create), else create a new one up to the cap. The returned tree is always a
/// freshly base-reset detached HEAD. The caller's PID is stamped as owner (and
/// a durable lease, if `lease_holder` is set).
pub fn acquire(project_root: &Path, opts: &AcquireOptions) -> Result<PathBuf> {
    let base_ref = git_ops::furthest_ahead_default_ref(project_root)?;
    let max_trees = opts.max_trees.unwrap_or(DEFAULT_MAX_TREES);
    let lease_ttl = opts.lease_ttl_secs.unwrap_or(DEFAULT_LEASE_TTL_SECS);
    let now = now_ts();

    with_state_lock(project_root, |pool| {
        // 1. Prefer a reclaimable entry → reset + hand out. "Reclaimable" is a
        //    plain idle tree, OR a stale lease whose TTL has elapsed with no
        //    live owner (the reservation-leak backstop, TASK-1008). An idle
        //    tree must be clean; a stale-expired lease is reset regardless of
        //    dirtiness (the dead session's uncommitted changes are abandoned),
        //    so a leaked dirty reservation can't pin the tree forever.
        let idle_idx = pool.entries.iter().position(|e| {
            if e.destroying || !e.path.exists() {
                return false;
            }
            if e.is_lease_expired(now, lease_ttl) {
                true
            } else {
                e.is_idle() && !git_ops::worktree_is_dirty(&e.path)
            }
        });

        if let Some(idx) = idle_idx {
            let path = pool.entries[idx].path.clone();
            git_ops::reset_worktree_to(&path, &base_ref)
                .with_context(|| format!("reset pooled worktree {}", path.display()))?;
            stamp_acquired(&mut pool.entries[idx], opts);
            return Ok(path);
        }

        // 2. None idle — create a new one if under the cap.
        if pool.entries.len() >= max_trees {
            anyhow::bail!(
                "worktree pool is full ({} of {} in use); raise [worktree_pool] max_trees \
                 or wait for an in-flight worktree to be returned",
                pool.entries.len(),
                max_trees
            );
        }

        let (name, _) = next_pool_name(pool, project_root);
        let path = pool_path_for(project_root, &name);
        git_ops::add_detached_worktree(project_root, &path, &base_ref)
            .with_context(|| format!("create pool worktree {}", path.display()))?;

        // Best-effort post_create hooks (warm the cache, etc.); never fatal.
        crate::worktree_hooks::run_hooks(&opts.post_create_hooks, &path, "post_create");

        let mut entry = PoolEntry {
            name,
            path: path.clone(),
            created_at: Some(now_ts()),
            ..Default::default()
        };
        stamp_acquired(&mut entry, opts);
        pool.entries.push(entry);
        Ok(path)
    })
}

fn stamp_acquired(entry: &mut PoolEntry, opts: &AcquireOptions) {
    entry.owner_pid = Some(std::process::id() as i32);
    entry.owner_started_at = Some(now_ts());
    entry.destroying = false;
    // Clear any prior lease first — a reclaimed stale-expired lease (TASK-1008)
    // must not keep the dead session's holder/`leased_at`. Re-stamp only if the
    // new acquirer asks for a durable reservation. (For a plain idle reuse or a
    // fresh create the entry already carries no lease, so this is a no-op.)
    entry.leased = false;
    entry.lease_holder = None;
    entry.leased_at = None;
    if let Some(holder) = &opts.lease_holder {
        entry.leased = true;
        entry.lease_holder = Some(holder.clone());
        entry.leased_at = Some(now_ts());
    }
}

/// Return a worktree to the pool: reset it to a clean detached base and mark it
/// idle (clear owner + lease). The directory **persists** — this is the
/// structural dissolution of TASK-0396 (a tree that is never removed never
/// leaves a dangling `target/.fingerprint` behind). Returns an error if the
/// path is not a registered pool worktree.
pub fn return_to_pool(project_root: &Path, worktree_path: &Path) -> Result<()> {
    let base_ref = git_ops::furthest_ahead_default_ref(project_root)?;
    let target = canonical(worktree_path);

    with_state_lock(project_root, |pool| {
        let idx = pool
            .entries
            .iter()
            .position(|e| canonical(&e.path) == target)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{} is not a registered pool worktree",
                    worktree_path.display()
                )
            })?;

        let path = pool.entries[idx].path.clone();
        git_ops::reset_worktree_to(&path, &base_ref)
            .with_context(|| format!("reset returned worktree {}", path.display()))?;
        let e = &mut pool.entries[idx];
        e.owner_pid = None;
        e.owner_started_at = None;
        e.leased = false;
        e.lease_holder = None;
        e.leased_at = None;
        e.destroying = false;
        Ok(())
    })
}

/// True when `path` is registered in the pool (used to decide whether
/// `session end --return` has a pool tree to hand back).
pub fn is_pool_worktree(project_root: &Path, path: &Path) -> bool {
    let target = canonical(path);
    read_state(project_root)
        .map(|p| p.entries.iter().any(|e| canonical(&e.path) == target))
        .unwrap_or(false)
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

// ── Status ───────────────────────────────────────────────────────────────────

/// Classify every pool entry for a read-only status view. `cwd` (when given)
/// marks the entry the caller is currently inside as `Here`. `lease_ttl_secs`
/// is the TTL used to flag a stale reservation as `Expired` (TASK-1008).
pub fn list(
    project_root: &Path,
    cwd: Option<&Path>,
    lease_ttl_secs: i64,
) -> Result<Vec<PoolStatus>> {
    let cwd_canon = cwd.map(canonical);
    let now = now_ts();
    with_state_lock(project_root, |pool| {
        let mut out = Vec::with_capacity(pool.entries.len());
        for e in &pool.entries {
            let here = cwd_canon
                .as_ref()
                .map(|c| *c == canonical(&e.path))
                .unwrap_or(false);
            let dirty = git_ops::worktree_is_dirty(&e.path);
            let state = if here {
                PoolState::Here
            } else if e.destroying {
                PoolState::Destroying
            } else if e.owner_pid.is_some() {
                PoolState::InUse
            } else if e.is_lease_expired(now, lease_ttl_secs) {
                PoolState::Expired
            } else if e.leased {
                PoolState::Leased
            } else if dirty {
                PoolState::Dirty
            } else {
                PoolState::Available
            };
            let head = git_ops::worktree_head_sha(&e.path);
            out.push(PoolStatus {
                entry: e.clone(),
                state,
                head,
            });
        }
        Ok(out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_entry_is_idle_only_when_unleased_unowned_not_destroying() {
        let mut e = PoolEntry {
            name: "aida-pool-0".into(),
            path: PathBuf::from("/tmp/x"),
            ..Default::default()
        };
        assert!(e.is_idle());
        e.owner_pid = Some(1234);
        assert!(!e.is_idle());
        e.owner_pid = None;
        e.leased = true;
        assert!(!e.is_idle());
        e.leased = false;
        e.destroying = true;
        assert!(!e.is_idle());
    }

    #[test]
    fn lease_expired_pure_decision() {
        let ttl = 100;
        // No stamp → never judged stale.
        assert!(!lease_expired(None, 10_000, ttl));
        // Fresh lease (younger than TTL) → not expired.
        assert!(!lease_expired(Some(9_950), 10_000, ttl));
        // Exactly at the TTL boundary → expired (inclusive).
        assert!(lease_expired(Some(9_900), 10_000, ttl));
        // Older than the TTL → expired.
        assert!(lease_expired(Some(1), 10_000, ttl));
        // Non-positive TTL disables expiry entirely.
        assert!(!lease_expired(Some(1), 10_000, 0));
        assert!(!lease_expired(Some(1), 10_000, -5));
        // A clock skew (leased_at in the future) saturates to 0 → not expired.
        assert!(!lease_expired(Some(20_000), 10_000, ttl));
    }

    #[test]
    fn is_lease_expired_requires_lease_dead_owner_and_ttl() {
        let now = 10_000;
        let ttl = 100;
        let mut e = PoolEntry {
            name: "aida-pool-0".into(),
            path: PathBuf::from("/tmp/x"),
            leased: true,
            leased_at: Some(1), // very old
            ..Default::default()
        };
        // Leased, old, no owner → reclaimable.
        assert!(e.is_lease_expired(now, ttl));
        // A live owner (i32::MAX is dead here, so use the current pid) is NOT
        // reclaimable even past the TTL.
        e.owner_pid = Some(std::process::id() as i32);
        assert!(!e.is_lease_expired(now, ttl));
        e.owner_pid = None;
        // A fresh lease is not expired.
        e.leased_at = Some(now);
        assert!(!e.is_lease_expired(now, ttl));
        // An un-leased idle entry is never "lease expired" (it's plain idle).
        e.leased = false;
        e.leased_at = None;
        assert!(!e.is_lease_expired(now, ttl));
        // A mid-destroy entry is never reclaimed via the lease path.
        e.leased = true;
        e.leased_at = Some(1);
        e.destroying = true;
        assert!(!e.is_lease_expired(now, ttl));
    }

    #[test]
    fn heal_state_clears_dead_owner_keeps_lease() {
        // A clearly-dead pid (i32::MAX is never a live process here).
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().to_path_buf();
        let mut pool = Pool {
            entries: vec![
                PoolEntry {
                    name: "a".into(),
                    path: live.clone(),
                    owner_pid: Some(i32::MAX),
                    owner_started_at: Some(1),
                    ..Default::default()
                },
                PoolEntry {
                    name: "b".into(),
                    path: live.clone(),
                    leased: true,
                    lease_holder: Some("drain".into()),
                    leased_at: Some(1),
                    ..Default::default()
                },
            ],
        };
        heal_state(&mut pool);
        // dead owner cleared
        assert_eq!(pool.entries[0].owner_pid, None);
        assert_eq!(pool.entries[0].owner_started_at, None);
        // lease survives with no pid
        assert!(pool.entries[1].leased);
        assert_eq!(pool.entries[1].lease_holder.as_deref(), Some("drain"));
    }

    #[test]
    fn heal_state_drops_missing_paths() {
        let mut pool = Pool {
            entries: vec![PoolEntry {
                name: "gone".into(),
                path: PathBuf::from("/nonexistent/aida-pool-99"),
                ..Default::default()
            }],
        };
        heal_state(&mut pool);
        assert!(pool.entries.is_empty());
    }

    #[test]
    fn next_pool_name_skips_taken_and_namespaces_by_project() {
        let root = Path::new("/work/myrepo");
        let pool = Pool {
            entries: vec![
                PoolEntry {
                    name: "aida-pool-myrepo-0".into(),
                    ..Default::default()
                },
                PoolEntry {
                    name: "aida-pool-myrepo-1".into(),
                    ..Default::default()
                },
            ],
        };
        let (name, n) = next_pool_name(&pool, root);
        assert_eq!(name, "aida-pool-myrepo-2");
        assert_eq!(n, 2);
    }

    #[test]
    fn read_state_empty_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let pool = read_state(dir.path()).unwrap();
        assert!(pool.entries.is_empty());
    }

    #[test]
    fn write_then_read_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let pool = Pool {
            entries: vec![PoolEntry {
                name: "aida-pool-0".into(),
                path: PathBuf::from("/tmp/aida-pool-0"),
                created_at: Some(42),
                ..Default::default()
            }],
        };
        write_state(dir.path(), &pool).unwrap();
        let back = read_state(dir.path()).unwrap();
        assert_eq!(back.entries.len(), 1);
        assert_eq!(back.entries[0].name, "aida-pool-0");
        assert_eq!(back.entries[0].created_at, Some(42));
        // default/empty fields are skipped on serialize but decode back to default
        assert_eq!(back.entries[0].owner_pid, None);
        assert!(!back.entries[0].leased);
    }

    // The advisory `pool.lock` must serialize concurrent `with_state_lock`
    // calls so two acquirers never run their read-modify-write windows at the
    // same time — otherwise two fan-out implementers could be handed one idle
    // tree. This guarantee is cross-platform: `fs2::lock_exclusive` is
    // `flock(2)` on Unix and `LockFileEx` on Windows, so the same serialization
    // holds on both (the nightly cross-platform matrix runs this test on each).
    // Gated on `native` because the lock is a no-op without it (fs2).
    // trace:TASK-1011 | ai:claude
    #[cfg(feature = "native")]
    #[test]
    fn state_lock_serializes_concurrent_acquirers() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let root: PathBuf = dir.path().to_path_buf();

        // How many closures are inside the critical section right now, and the
        // high-water mark ever observed. A working lock keeps the latter at 1.
        let in_section = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));

        let n_threads = 8usize;
        let handles: Vec<_> = (0..n_threads)
            .map(|i| {
                let root = root.clone();
                let in_section = Arc::clone(&in_section);
                let max_seen = Arc::clone(&max_seen);
                std::thread::spawn(move || {
                    with_state_lock(&root, |pool| {
                        // If the lock serializes, `now` is always 1 here.
                        let now = in_section.fetch_add(1, Ordering::SeqCst) + 1;
                        max_seen.fetch_max(now, Ordering::SeqCst);
                        // Hold the section long enough that an unserialized peer
                        // would overlap and bump `max_seen` past 1.
                        std::thread::sleep(std::time::Duration::from_millis(15));
                        in_section.fetch_sub(1, Ordering::SeqCst);
                        // Mutate under the lock so a lost read-modify-write would
                        // drop an entry. The dir must exist or `heal_state` (run
                        // at the next acquire) would prune the missing path.
                        let p = root.join(format!("aida-pool-{i}"));
                        std::fs::create_dir_all(&p).unwrap();
                        pool.entries.push(PoolEntry {
                            name: format!("aida-pool-{i}"),
                            path: p,
                            ..Default::default()
                        });
                        Ok(())
                    })
                    .unwrap();
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // Mutual exclusion: two acquirers never held `pool.lock` at once.
        assert_eq!(
            max_seen.load(Ordering::SeqCst),
            1,
            "two acquirers held pool.lock concurrently — lock did not serialize"
        );
        // Serialized read-modify-write: every thread's push survived (none was
        // lost to a concurrent read-then-overwrite).
        let final_pool = read_state(&root).unwrap();
        assert_eq!(
            final_pool.entries.len(),
            n_threads,
            "an RMW was lost — the lock did not serialize writes"
        );
    }
}

/// Integration tests that drive real `git` over a tempdir repo — they exercise
/// the acquire/return/reset behaviors that the pure-logic tests above can't.
// trace:STORY-714 | ai:claude
#[cfg(test)]
mod git_integration_tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(ok, "git {args:?} failed in {}", dir.display());
    }

    /// A throwaway git repo with one commit on `main`, configured for commits.
    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.email", "t@t.t"]);
        git(p, &["config", "user.name", "t"]);
        std::fs::write(p.join("README.md"), "seed").unwrap();
        git(p, &["add", "-A"]);
        git(p, &["commit", "-qm", "seed"]);
        dir
    }

    fn opts() -> AcquireOptions {
        AcquireOptions {
            max_trees: Some(4),
            ..Default::default()
        }
    }

    #[test]
    fn acquire_creates_then_return_keeps_directory() {
        let repo = init_repo();
        let root = repo.path();
        let path = acquire(root, &opts()).unwrap();
        assert!(path.exists(), "acquired worktree dir should exist");
        assert_eq!(read_state(root).unwrap().entries.len(), 1);

        // Work in it: branch + commit.
        git(&path, &["checkout", "-q", "-b", "feat-a"]);
        std::fs::write(path.join("a.txt"), "x").unwrap();
        git(&path, &["add", "-A"]);
        git(&path, &["commit", "-qm", "a"]);

        return_to_pool(root, &path).unwrap();
        // return_resets_and_keeps_directory: dir persists, entry still registered.
        assert!(
            path.exists(),
            "returned worktree dir must persist (TASK-0396)"
        );
        assert_eq!(read_state(root).unwrap().entries.len(), 1);
    }

    #[test]
    fn acquire_prefers_idle_over_create() {
        let repo = init_repo();
        let root = repo.path();
        let p1 = acquire(root, &opts()).unwrap();
        return_to_pool(root, &p1).unwrap(); // now idle
        let p2 = acquire(root, &opts()).unwrap();
        assert_eq!(p1, p2, "idle tree should be reused, not a new one created");
        assert_eq!(
            read_state(root).unwrap().entries.len(),
            1,
            "no second tree should be created when an idle one exists"
        );
    }

    #[test]
    fn acquire_base_reset_dissolves_branch_stacking() {
        let repo = init_repo();
        let root = repo.path();
        let p1 = acquire(root, &opts()).unwrap();
        // Stack a branch + commit, then return.
        git(&p1, &["checkout", "-q", "-b", "feat-a"]);
        std::fs::write(p1.join("a.txt"), "x").unwrap();
        git(&p1, &["add", "-A"]);
        git(&p1, &["commit", "-qm", "stacked"]);
        return_to_pool(root, &p1).unwrap();

        // Re-acquire the same warm tree: must be detached at base, NOT stacked.
        let p2 = acquire(root, &opts()).unwrap();
        assert_eq!(p1, p2);
        let head_branch = Command::new("git")
            .current_dir(&p2)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .unwrap();
        let head = String::from_utf8_lossy(&head_branch.stdout);
        assert_eq!(head.trim(), "HEAD", "acquire must hand out a detached HEAD");

        let log = Command::new("git")
            .current_dir(&p2)
            .args(["log", "--oneline"])
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&log.stdout);
        assert!(
            !log.contains("stacked"),
            "prior branch's commit must not be stacked on the reused tree (BUG-553); log was: {log}"
        );
    }

    #[test]
    fn heal_state_self_heals_acquire_after_owner_exit() {
        // Simulate a crashed owner: stamp a dead pid directly, then heal.
        let repo = init_repo();
        let root = repo.path();
        let p1 = acquire(root, &opts()).unwrap();
        with_state_lock(root, |pool| {
            pool.entries[0].owner_pid = Some(i32::MAX); // a dead pid
            pool.entries[0].owner_started_at = Some(1);
            Ok(())
        })
        .unwrap();
        // Next acquire heals the dead owner and reuses the tree.
        let p2 = acquire(root, &opts()).unwrap();
        assert_eq!(p1, p2);
        assert_eq!(read_state(root).unwrap().entries.len(), 1);
    }

    /// TASK-1008: a leaked durable lease (the owning session died without
    /// returning) is reclaimed by the next `acquire` once its TTL elapses.
    /// Without the TTL the lease survives process death by design, so a leaked
    /// reservation would pin the tree forever; with it, the stale entry is
    /// reset + re-handed-out and the new acquirer's holder replaces the dead
    /// one. A pool with one leased tree must NOT create a second (cap-respecting
    /// reclaim, not leak-then-grow).
    // trace:TASK-1008 | ai:claude
    #[test]
    fn acquire_reclaims_expired_lease_without_growing_pool() {
        let repo = init_repo();
        let root = repo.path();

        // Acquire WITH a durable lease (a headless-drain style reservation).
        let leasing = AcquireOptions {
            max_trees: Some(1), // cap of 1 → a leak would block all future work
            lease_holder: Some("dead-drain".into()),
            ..Default::default()
        };
        let p1 = acquire(root, &leasing).unwrap();
        // Simulate the short-lived start process exiting: clear the owner pid
        // but keep the durable lease, and age `leased_at` past any sane TTL.
        with_state_lock(root, |pool| {
            pool.entries[0].owner_pid = None;
            pool.entries[0].owner_started_at = None;
            pool.entries[0].leased_at = Some(1); // ancient
            Ok(())
        })
        .unwrap();

        // A tiny TTL makes the ancient lease expired. The next acquire reclaims
        // the SAME tree rather than failing "pool is full".
        let reclaiming = AcquireOptions {
            max_trees: Some(1),
            lease_ttl_secs: Some(1),
            ..Default::default()
        };
        let p2 = acquire(root, &reclaiming).unwrap();
        assert_eq!(p1, p2, "expired lease should be reclaimed, not a new tree");
        let pool = read_state(root).unwrap();
        assert_eq!(pool.entries.len(), 1, "reclaim must not grow the pool");
        // The dead holder is gone; the reclaimer left no new lease (it asked
        // for none), so the entry is a plain owned tree.
        assert!(!pool.entries[0].leased);
        assert_eq!(pool.entries[0].lease_holder, None);
        assert_eq!(pool.entries[0].leased_at, None);
        assert!(pool.entries[0].owner_pid.is_some());
    }

    /// A NON-expired lease (TTL not yet elapsed) is left reserved — a healthy
    /// in-flight reservation is never stolen out from under its holder.
    // trace:TASK-1008 | ai:claude
    #[test]
    fn acquire_does_not_reclaim_fresh_lease() {
        let repo = init_repo();
        let root = repo.path();
        let leasing = AcquireOptions {
            max_trees: Some(1),
            lease_holder: Some("live-drain".into()),
            ..Default::default()
        };
        let _p1 = acquire(root, &leasing).unwrap();
        with_state_lock(root, |pool| {
            pool.entries[0].owner_pid = None; // start process exited
            pool.entries[0].leased_at = Some(now_ts()); // but lease is fresh
            Ok(())
        })
        .unwrap();
        // A large TTL means the fresh lease is NOT expired; with a cap of 1 and
        // the one tree reserved, acquire must refuse (pool full), not steal it.
        let reclaiming = AcquireOptions {
            max_trees: Some(1),
            lease_ttl_secs: Some(1_000_000),
            ..Default::default()
        };
        let err = acquire(root, &reclaiming).unwrap_err();
        assert!(
            err.to_string().contains("pool is full"),
            "fresh lease must stay reserved; got: {err}"
        );
    }

    /// The full session lifecycle the warm-pool relies on: acquire a detached
    /// tree, create the session branch ON it (what `session_start` does after
    /// acquire), then return it. The return must reset the tree off the branch
    /// back to a clean detached base — the acquire→checkout→return path the
    /// advisor asked to have automated before the default flips on.
    // trace:STORY-714 trace:TASK-982 | ai:claude
    #[test]
    fn acquire_checkout_branch_return_resets_off_branch() {
        let repo = init_repo();
        let root = repo.path();

        // 1. acquire — pool hands out a detached tree at the base.
        let path = acquire(root, &opts()).unwrap();
        let head_branch = |p: &Path| -> String {
            let o = Command::new("git")
                .current_dir(p)
                .args(["rev-parse", "--abbrev-ref", "HEAD"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        assert_eq!(
            head_branch(&path),
            "HEAD",
            "acquire hands out a detached HEAD"
        );

        // 2. checkout -b — the worker creates its session branch on the tree.
        git(&path, &["checkout", "-b", "task-x-work"]);
        assert_eq!(head_branch(&path), "task-x-work");
        std::fs::write(path.join("impl.txt"), "work").unwrap();
        git(&path, &["add", "-A"]);
        git(&path, &["commit", "-qm", "implement task-x"]);

        // 3. return — resets the tree off the branch back to a detached base.
        return_to_pool(root, &path).unwrap();
        assert!(path.exists(), "return keeps the directory");
        assert_eq!(
            head_branch(&path),
            "HEAD",
            "return must leave the tree detached at the base, not on the session branch"
        );
        let log = Command::new("git")
            .current_dir(&path)
            .args(["log", "--oneline"])
            .output()
            .unwrap();
        assert!(
            !String::from_utf8_lossy(&log.stdout).contains("implement task-x"),
            "the session branch's commit must not be on the returned tree's HEAD"
        );
        // And the entry is idle (returnable to the next acquire).
        let pool = read_state(root).unwrap();
        assert_eq!(pool.entries.len(), 1);
        assert!(pool.entries[0].owner_pid.is_none() && !pool.entries[0].leased);
    }
}
