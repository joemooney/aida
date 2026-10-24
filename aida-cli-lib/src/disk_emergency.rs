//! STORY-1916 — the disk guard's emergency tier, its stuck escalation, and
//! the status block that shows the guard is running.
//!
//! The guard (`doctor check disk-headroom`, every 30m) used to route a due
//! item and nothing else. On 2026-10-23 `/mnt/build` sat at 88% while 77G of
//! idle duplicate and merged-PR caches were removed by hand. This module adds:
//!
//! - **Emergency tier.** When the build-cache filesystem is at or above
//!   `[doctor.disk_headroom] emergency_used_pct` (default 95), the scheduled
//!   run deletes idle per-worktree caches, without a human, until usage drops
//!   below the ceiling minus `emergency_margin_pct`.
//! - **Stuck escalation.** When the floor is breached and a reclaim dry run
//!   frees less than the deficit, the finding says so and the operator is
//!   notified, deduped, instead of a silent re-fire every 30m.
//! - **Status.** Free space, floor, ceiling, last guard run, last automatic
//!   reclaim.
//!
//! ## What the emergency tier may delete
//!
//! Only a direct child of `canonical(<main>/target)/worktrees`, a real
//! directory, idle for at least `emergency_idle_hours` (newest mtime anywhere
//! under it). [`Allowlist::check`] re-checks every path at the delete site,
//! independently of the planner, and bails on anything else: worktree
//! sources, `.aida-store`, the main checkout's `target/agent`, and the live
//! binary under `~/.local/share/aida/live`.
//!
//! Immediately before each delete, [`busy_reason`] tries a non-blocking flock
//! on every `<cache>/<profile>/.cargo-lock` (and a cross-target build's
//! `<cache>/<triple>/<profile>/.cargo-lock`) and re-walks the cache. A held
//! lock, or any write in the idle window, skips the cache as busy (advisor
//! condition C1).
//!
//! Tiers run in order: 0 (worktree gone, PR merged or closed), 1 (idle, no
//! live lease), 2 (idle, held by a live lease). Tier 2 runs only while usage
//! is still at or above the ceiling once tiers 0-1 are spent, and each tier-2
//! delete mails the lease holder (condition C2).
// trace:STORY-1916 | ai:claude

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::worktree_reclaim::{human_bytes, WorktreeEntry};

const GIB: u64 = 1024 * 1024 * 1024;
pub(crate) const DEFAULT_EMERGENCY_USED_PCT: u8 = 95;
pub(crate) const DEFAULT_EMERGENCY_MARGIN_PCT: u8 = 5;
pub(crate) const DEFAULT_EMERGENCY_IDLE_HOURS: u64 = 2;
/// A write this recent is a build in flight, reported as `busy`.
const BUSY_WINDOW: Duration = Duration::from_secs(60);
/// The stuck escalation re-notifies at most this often...
const STUCK_REESCALATE_HOURS: i64 = 6;
/// ...unless the deficit grew by at least this much.
const STUCK_GROWTH_BYTES: u64 = 10 * GIB;
/// The scheduler job whose last run the status block reports.
const GUARD_JOB: &str = "disk-headroom-guard";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// `[doctor.disk_headroom]` emergency keys.
// trace:STORY-1916 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct EmergencyConfig {
    /// 0 disables the emergency tier.
    pub used_pct: u8,
    pub margin_pct: u8,
    pub idle_hours: u64,
}

impl EmergencyConfig {
    pub(crate) fn enabled(&self) -> bool {
        self.used_pct > 0
    }

    pub(crate) fn idle(&self) -> Duration {
        Duration::from_secs(self.idle_hours.saturating_mul(3600))
    }

    /// A run stops once usage is strictly below this.
    pub(crate) fn stop_below(&self) -> u8 {
        self.used_pct.saturating_sub(self.margin_pct)
    }
}

/// Read the emergency keys, falling back to the defaults for an absent or
/// out-of-range value. `emergency_used_pct = 0` disables the tier.
// trace:STORY-1916 | ai:claude
pub(crate) fn emergency_config(cfg: Option<&toml::Value>) -> EmergencyConfig {
    let table = cfg
        .and_then(|c| c.get("doctor"))
        .and_then(|d| d.get("disk_headroom"));
    let int = |key: &str| table.and_then(|t| t.get(key)).and_then(|v| v.as_integer());
    EmergencyConfig {
        used_pct: int("emergency_used_pct")
            .filter(|v| (0..=100).contains(v))
            .map_or(DEFAULT_EMERGENCY_USED_PCT, |v| v as u8),
        margin_pct: int("emergency_margin_pct")
            .filter(|v| (0..=50).contains(v))
            .map_or(DEFAULT_EMERGENCY_MARGIN_PCT, |v| v as u8),
        idle_hours: int("emergency_idle_hours")
            .filter(|v| *v >= 1)
            .map_or(DEFAULT_EMERGENCY_IDLE_HOURS, |v| v as u64),
    }
}

// ---------------------------------------------------------------------------
// Filesystem probes
// ---------------------------------------------------------------------------

/// One filesystem's capacity, from `statvfs`, plus the device id that tells
/// two paths' filesystems apart.
// trace:STORY-1916 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FsStats {
    pub total: u64,
    pub avail: u64,
    pub dev: u64,
}

impl FsStats {
    /// Used percentage, rounded, as `aida worktree reclaim --target-pct`
    /// computes it.
    pub(crate) fn used_pct(&self) -> u8 {
        if self.total == 0 {
            return 0;
        }
        let used = self.total.saturating_sub(self.avail);
        ((used as f64 / self.total as f64) * 100.0).round() as u8
    }
}

/// `statvfs` on `path`. Unlike a mount-table lookup by path prefix, this
/// resolves a bind mount (`<main>/target` on `/mnt/build`) to its own
/// filesystem.
// trace:STORY-1916 | ai:claude
pub(crate) fn fs_stats(path: &Path) -> Option<FsStats> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    let dev = std::fs::metadata(path).ok()?.dev();
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut buf: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c_path` is a valid NUL-terminated string and `buf` is a
    // properly sized, writable statvfs.
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut buf) } != 0 {
        return None;
    }
    let frsize = buf.f_frsize as u64;
    Some(FsStats {
        total: (buf.f_blocks as u64).saturating_mul(frsize),
        avail: (buf.f_bavail as u64).saturating_mul(frsize),
        dev,
    })
}

/// Size and newest write of a cache tree.
// trace:STORY-1916 | ai:claude
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CacheWalk {
    pub bytes: u64,
    /// Newest mtime of the root or any entry under it; `None` when the root
    /// itself could not be read.
    pub newest: Option<SystemTime>,
}

/// Walk `dir` once, following no symlinks, for its size and newest mtime.
/// Unreadable subtrees are skipped: the size is then a lower bound, and the
/// newest mtime covers what could be read.
// trace:STORY-1916 | ai:claude
pub(crate) fn walk_cache(dir: &Path) -> CacheWalk {
    let Ok(root) = std::fs::symlink_metadata(dir) else {
        return CacheWalk::default();
    };
    let mut walk = CacheWalk {
        bytes: 0,
        newest: root.modified().ok(),
    };
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(md) = entry.metadata() else { continue };
            if let Ok(m) = md.modified() {
                walk.newest = Some(walk.newest.map_or(m, |n| n.max(m)));
            }
            if md.is_dir() {
                stack.push(entry.path());
            } else if md.is_file() {
                walk.bytes = walk.bytes.saturating_add(md.len());
            }
        }
    }
    walk
}

/// How long the cache has gone unwritten. `None` (treated as fresh, never
/// deleted) when the root was unreadable or the newest mtime is in the
/// future.
// trace:STORY-1916 | ai:claude
pub(crate) fn idle_for(walk: &CacheWalk, now: SystemTime) -> Option<Duration> {
    now.duration_since(walk.newest?).ok()
}

// ---------------------------------------------------------------------------
// The allowlist
// ---------------------------------------------------------------------------

/// What the emergency tier may delete, re-checked at the delete site.
// trace:STORY-1916 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct Allowlist {
    /// `canonical(<main>/target)/worktrees`.
    pub cache_root: PathBuf,
    pub main_root: PathBuf,
    /// Paths that may be neither deleted, nor contain, nor sit inside a
    /// deleted path: `target/agent`, `target/debug`, `target/release`, the
    /// live binary directory and its link targets, every worktree source.
    pub never: Vec<PathBuf>,
}

impl Allowlist {
    /// The allowlist for a main checkout: its canonical cache root and the
    /// never-delete set, with `home` the operator's home directory (for the
    /// live binary) and `worktrees` every source checkout.
    pub(crate) fn for_main(
        main_root: &Path,
        cache_root: PathBuf,
        home: Option<&Path>,
        worktrees: &[WorktreeEntry],
    ) -> Self {
        let mut never: Vec<PathBuf> = Vec::new();
        if let Some(target) = cache_root.parent() {
            for profile in ["agent", "debug", "release"] {
                never.push(target.join(profile));
            }
        }
        if let Some(home) = home {
            let live = home.join(".local/share/aida/live");
            never.push(live.canonicalize().unwrap_or(live));
            for bin in ["aida", "aida-server"] {
                if let Ok(real) = home.join(".local/bin").join(bin).canonicalize() {
                    never.push(real);
                }
            }
        }
        for wt in worktrees {
            if wt.path != main_root {
                never.push(wt.path.clone());
            }
        }
        never.push(main_root.join(".aida-store"));
        Self {
            cache_root,
            main_root: main_root.to_path_buf(),
            never,
        }
    }

    /// Bail unless `path` is a deletable cache. Every arm is a hard refusal,
    /// not a skip: reaching one means the plan was built wrong.
    pub(crate) fn check(&self, path: &Path) -> Result<()> {
        let refuse = |why: &str| -> Result<()> {
            bail!(
                "refusing to delete {}: {why}. The emergency tier deletes only direct children \
                 of {}; reaching this means the plan was built wrong.",
                path.display(),
                self.cache_root.display()
            )
        };
        if path.parent() != Some(self.cache_root.as_path()) {
            return refuse("it is not a direct child of the per-worktree cache root");
        }
        if path
            .file_name()
            .is_none_or(|n| n.is_empty() || n == "." || n == "..")
        {
            return refuse("it has no directory name");
        }
        match std::fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_symlink() => return refuse("it is a symlink"),
            Ok(m) if !m.is_dir() => return refuse("it is not a directory"),
            Err(_) => return refuse("it cannot be read"),
            Ok(_) => {}
        }
        if self.main_root.starts_with(path) {
            return refuse("it contains the main checkout");
        }
        for n in &self.never {
            if n == path || n.starts_with(path) || path.starts_with(n) {
                return refuse(&format!("it overlaps the protected path {}", n.display()));
            }
        }
        Ok(())
    }
}

/// Advisor condition C1: is a build about to use this cache? Tries a
/// non-blocking flock on every `<cache>/<profile>/.cargo-lock` — and one
/// level deeper, a cross-target build's `<cache>/<triple>/<profile>/
/// .cargo-lock` (cargo holds it for the whole build) — and re-walks the
/// tree. Any held lock, write in the
/// last 60 s, write inside the idle window, or unreadable tree is a reason
/// to skip.
// trace:STORY-1916 | ai:claude
pub(crate) fn busy_reason(cache: &Path, now: SystemTime, idle: Duration) -> Option<String> {
    use fs2::FileExt;
    // `<cache>/<profile>/.cargo-lock`, plus a cross-target build's
    // `<cache>/<triple>/<profile>/.cargo-lock` one level deeper.
    // trace:TASK-2272 | ai:claude (TASK-1-483)
    let level1: Vec<PathBuf> = std::fs::read_dir(cache)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    let mut profiles = level1.clone();
    for dir in &level1 {
        profiles.extend(
            std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path()),
        );
    }
    for profile in profiles {
        let lock = profile.join(".cargo-lock");
        if !lock.is_file() {
            continue;
        }
        let held = match std::fs::File::open(&lock) {
            Ok(f) => match f.try_lock_exclusive() {
                Ok(()) => {
                    let _ = FileExt::unlock(&f);
                    false
                }
                Err(_) => true,
            },
            Err(_) => true,
        };
        if held {
            return Some(format!(
                "busy: cargo holds {}",
                lock.strip_prefix(cache).unwrap_or(&lock).display()
            ));
        }
    }
    match idle_for(&walk_cache(cache), now) {
        None => Some("busy: unreadable, or written in the future (clock skew)".into()),
        Some(age) if age < BUSY_WINDOW => Some("busy: written in the last 60 s".into()),
        Some(age) if age < idle => Some(format!(
            "busy: written {} min ago, inside the idle window",
            age.as_secs() / 60
        )),
        Some(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------------

/// Why a cache is an emergency candidate. The order is the tier order.
// trace:STORY-1916 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Reason {
    WorktreeGone,
    Merged,
    Closed,
    Idle,
    IdleLiveLease,
    /// TASK-2325: a terminal job's tmp dir or old session scratch, reclaimed
    /// only at the hard ceiling after the cache tiers.
    AgentScratch,
}

impl Reason {
    pub(crate) fn tier(self) -> u8 {
        match self {
            Reason::WorktreeGone | Reason::Merged | Reason::Closed => 0,
            Reason::Idle => 1,
            Reason::IdleLiveLease => 2,
            // trace:TASK-2325 | ai:claude
            Reason::AgentScratch => 3,
        }
    }

    pub(crate) fn code(self) -> &'static str {
        match self {
            Reason::WorktreeGone => "worktree-gone",
            Reason::Merged => "merged",
            Reason::Closed => "closed",
            Reason::Idle => "idle",
            Reason::IdleLiveLease => "idle-live-lease",
            // trace:TASK-2325 | ai:claude
            Reason::AgentScratch => "agent-scratch",
        }
    }
}

/// A live session lease on a worktree, for tier 2 and its mail.
// trace:STORY-1916 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Holder {
    pub worktree: PathBuf,
    pub scope: String,
    pub pid: Option<u32>,
}

/// One cache directory under the cache root.
// trace:STORY-1916 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct CacheEntry {
    pub path: PathBuf,
    pub walk: CacheWalk,
}

/// Everything the planner needs, injected.
// trace:STORY-1916 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct EmergencySurvey {
    pub caches: Vec<CacheEntry>,
    pub worktrees: Vec<WorktreeEntry>,
    /// Live leases only.
    pub holders: Vec<Holder>,
    /// Branch -> Merged/Closed, for branches with no open PR.
    pub finished_branches: BTreeMap<String, Reason>,
    /// PR number -> Merged/Closed, for detached `take-<N>` worktrees.
    pub finished_prs: BTreeMap<u64, Reason>,
    pub now: SystemTime,
    pub idle: Duration,
}

/// A cache the emergency tier may delete, in tier order.
// trace:STORY-1916 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub path: PathBuf,
    pub bytes: u64,
    pub idle: Duration,
    pub reason: Reason,
    pub holder: Option<Holder>,
}

fn dir_name(path: &Path) -> Option<&str> {
    path.file_name().and_then(|n| n.to_str())
}

/// Classify every cache. Idle for at least the window is the necessary
/// condition for every candidate; the reason only sets the tier. Within a
/// tier, oldest-idle first. Returns the candidates and the caches held back
/// with why.
// trace:STORY-1916 | ai:claude
pub(crate) fn plan(survey: &EmergencySurvey) -> (Vec<Candidate>, Vec<(PathBuf, String)>) {
    let mut candidates = Vec::new();
    let mut held = Vec::new();
    for cache in &survey.caches {
        let Some(name) = dir_name(&cache.path) else {
            continue;
        };
        let idle = match idle_for(&cache.walk, survey.now) {
            Some(idle) if idle >= survey.idle => idle,
            Some(idle) => {
                held.push((
                    cache.path.clone(),
                    format!(
                        "written {} min ago, inside the idle window",
                        idle.as_secs() / 60
                    ),
                ));
                continue;
            }
            None => {
                held.push((
                    cache.path.clone(),
                    "unreadable, or written in the future (clock skew)".to_string(),
                ));
                continue;
            }
        };
        let worktree = survey
            .worktrees
            .iter()
            .find(|w| dir_name(&w.path) == Some(name));
        let holder = survey
            .holders
            .iter()
            .find(|h| dir_name(&h.worktree) == Some(name))
            .cloned();
        let finished = worktree.and_then(|w| {
            w.branch
                .as_ref()
                .and_then(|b| survey.finished_branches.get(b))
                .or_else(|| {
                    crate::worktree_reclaim::pr_number_from_dir(name)
                        .and_then(|n| survey.finished_prs.get(&n))
                })
                .copied()
        });
        let reason = match (worktree, &holder, finished) {
            (_, Some(_), _) => Reason::IdleLiveLease,
            (None, None, _) => Reason::WorktreeGone,
            (Some(_), None, Some(done)) => done,
            (Some(_), None, None) => Reason::Idle,
        };
        candidates.push(Candidate {
            path: cache.path.clone(),
            bytes: cache.walk.bytes,
            idle,
            reason,
            holder,
        });
    }
    candidates.sort_by(|a, b| {
        a.reason
            .tier()
            .cmp(&b.reason.tier())
            .then(b.idle.cmp(&a.idle))
            .then_with(|| a.path.cmp(&b.path))
    });
    (candidates, held)
}

// ---------------------------------------------------------------------------
// Applying
// ---------------------------------------------------------------------------

/// One ledger line. Append-only JSONL at [`ledger_path`].
// trace:STORY-1916 | ai:claude
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum LedgerRow {
    Delete {
        ts: DateTime<Utc>,
        path: String,
        bytes: u64,
        reason: Reason,
        idle_secs: u64,
        used_pct_before: Option<u8>,
    },
    Skip {
        ts: DateTime<Utc>,
        path: String,
        reason: Reason,
        why: String,
    },
    Fail {
        ts: DateTime<Utc>,
        path: String,
        error: String,
    },
    Run {
        ts: DateTime<Utc>,
        freed_bytes: u64,
        deleted: usize,
        used_pct_before: Option<u8>,
        used_pct_after: Option<u8>,
        stopped: String,
    },
}

/// Why a run stopped.
// trace:STORY-1916 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    /// Usage fell below the ceiling minus the margin.
    BelowMargin,
    /// Tiers 0-1 are spent and usage is below the ceiling, so tier 2 does
    /// not run (C2).
    Tier2BelowCeiling,
    /// The usage probe failed mid-run; stop rather than delete blind.
    ProbeLost,
    /// Every candidate was tried.
    Exhausted,
    /// The allowlist refused a candidate: the plan was built wrong.
    Refused,
}

impl Stop {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Stop::BelowMargin => "below-margin",
            Stop::Tier2BelowCeiling => "below-ceiling",
            Stop::ProbeLost => "probe-lost",
            Stop::Exhausted => "exhausted",
            Stop::Refused => "refused",
        }
    }
}

/// What one run did.
// trace:STORY-1916 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct RunOutcome {
    pub rows: Vec<LedgerRow>,
    pub deleted: Vec<Candidate>,
    pub freed_bytes: u64,
    pub stopped: Stop,
    pub used_pct_before: Option<u8>,
    pub used_pct_after: Option<u8>,
    /// The allowlist refusal that stopped the run, if one did.
    pub refusal: Option<String>,
}

/// Delete candidates in order until usage is below the margin. `used_pct`
/// is re-probed before each delete, `remove` is the delete, and
/// `on_tier2` is told about each tier-2 delete (for the holder's mail).
/// Every path passes [`Allowlist::check`] and [`busy_reason`] first.
/// `on_row` sees each ledger row as it is made, so a caller on a kill
/// budget can persist them incrementally (TASK-1-481).
// trace:STORY-1916 | ai:claude
// trace:TASK-2272 | ai:claude (TASK-1-481)
pub(crate) fn apply(
    allow: &Allowlist,
    candidates: &[Candidate],
    cfg: &EmergencyConfig,
    now: SystemTime,
    used_pct: impl Fn() -> Option<u8>,
    remove: impl Fn(&Path) -> std::io::Result<()>,
    mut on_tier2: impl FnMut(&Candidate),
    mut on_row: impl FnMut(&LedgerRow),
) -> RunOutcome {
    let ts = DateTime::<Utc>::from(now);
    let before = used_pct();
    let mut rows = Vec::new();
    let mut push_row = |rows: &mut Vec<LedgerRow>, row: LedgerRow| {
        on_row(&row);
        rows.push(row);
    };
    let mut deleted = Vec::new();
    let mut freed = 0u64;
    let mut stopped = Stop::Exhausted;
    let mut refusal = None;
    for c in candidates {
        let Some(pct) = used_pct() else {
            stopped = Stop::ProbeLost;
            break;
        };
        if pct < cfg.stop_below() {
            stopped = Stop::BelowMargin;
            break;
        }
        if c.reason.tier() == 2 && pct < cfg.used_pct {
            stopped = Stop::Tier2BelowCeiling;
            break;
        }
        let path = c.path.display().to_string();
        // A refusal stops the run, but the deletes before it keep their
        // ledger rows: they are returned with it, not dropped.
        // trace:STORY-1916.ac40b516 | ai:claude
        if let Err(e) = allow.check(&c.path) {
            push_row(
                &mut rows,
                LedgerRow::Fail {
                    ts,
                    path,
                    error: e.to_string(),
                },
            );
            refusal = Some(e.to_string());
            stopped = Stop::Refused;
            break;
        }
        if let Some(why) = busy_reason(&c.path, now, cfg.idle()) {
            push_row(
                &mut rows,
                LedgerRow::Skip {
                    ts,
                    path,
                    reason: c.reason,
                    why,
                },
            );
            continue;
        }
        match remove(&c.path) {
            Ok(()) => {
                freed = freed.saturating_add(c.bytes);
                push_row(
                    &mut rows,
                    LedgerRow::Delete {
                        ts,
                        path,
                        bytes: c.bytes,
                        reason: c.reason,
                        idle_secs: c.idle.as_secs(),
                        used_pct_before: Some(pct),
                    },
                );
                if c.reason.tier() == 2 {
                    on_tier2(c);
                }
                deleted.push(c.clone());
            }
            Err(e) => push_row(
                &mut rows,
                LedgerRow::Fail {
                    ts,
                    path,
                    error: e.to_string(),
                },
            ),
        }
    }
    let after = used_pct();
    push_row(
        &mut rows,
        LedgerRow::Run {
            ts,
            freed_bytes: freed,
            deleted: deleted.len(),
            used_pct_before: before,
            used_pct_after: after,
            stopped: stopped.code().to_string(),
        },
    );
    RunOutcome {
        rows,
        deleted,
        freed_bytes: freed,
        stopped,
        used_pct_before: before,
        used_pct_after: after,
        refusal,
    }
}

/// The operator notice for a run, or `None` when the run deleted nothing
/// and got below the margin (nothing to say).
// trace:STORY-1916 | ai:claude
pub(crate) fn emergency_notice(
    outcome: &RunOutcome,
    cfg: &EmergencyConfig,
) -> Option<(String, String)> {
    let failed_to_recover = outcome
        .used_pct_after
        .is_none_or(|pct| pct >= cfg.stop_below());
    if outcome.deleted.is_empty() && !failed_to_recover && outcome.refusal.is_none() {
        return None;
    }
    let pct = |p: Option<u8>| p.map_or("?".to_string(), |p| format!("{p}%"));
    let title = if outcome.refusal.is_some() {
        "Disk emergency: auto-reclaim refused a candidate and stopped".to_string()
    } else if failed_to_recover {
        format!(
            "Disk emergency: build cache still at {} after auto-reclaim",
            pct(outcome.used_pct_after)
        )
    } else {
        format!(
            "Disk emergency: auto-reclaimed {}",
            human_bytes(outcome.freed_bytes)
        )
    };
    let mut body = format!(
        "The build-cache filesystem was at {} (ceiling {}%). The disk guard deleted {} idle \
         cache(s), freeing {}; it is now at {} (stopped: {}).",
        pct(outcome.used_pct_before),
        cfg.used_pct,
        outcome.deleted.len(),
        human_bytes(outcome.freed_bytes),
        pct(outcome.used_pct_after),
        outcome.stopped.code(),
    );
    for c in &outcome.deleted {
        body.push_str(&format!(
            "\n  {}  {}  ({})",
            human_bytes(c.bytes),
            c.path.display(),
            c.reason.code()
        ));
    }
    if let Some(refusal) = &outcome.refusal {
        body.push_str(&format!("\nStopped by the allowlist: {refusal}"));
    }
    if failed_to_recover {
        body.push_str(
            "\nUsage is still above the margin: run `aida worktree reclaim` to see what is \
             held back, or `aida session reap` for finished worktrees.",
        );
    }
    Some((title, body))
}

// ---------------------------------------------------------------------------
// Ledger and state files
// ---------------------------------------------------------------------------

/// `<main>/.aida/disk-reclaim-ledger.jsonl`.
// trace:STORY-1916 | ai:claude
pub(crate) fn ledger_path(main_root: &Path) -> PathBuf {
    main_root.join(".aida").join("disk-reclaim-ledger.jsonl")
}

/// Append rows to the ledger.
// trace:STORY-1916 | ai:claude
pub(crate) fn append_ledger(main_root: &Path, rows: &[LedgerRow]) -> Result<()> {
    use std::io::Write;
    let path = ledger_path(main_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    for row in rows {
        writeln!(file, "{}", serde_json::to_string(row)?)?;
    }
    Ok(())
}

/// The last automatic reclaim run, for the status block.
// trace:STORY-1916 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct LastReclaim {
    pub at: DateTime<Utc>,
    pub freed_bytes: u64,
    pub deleted: usize,
    pub used_pct_after: Option<u8>,
    pub stopped: String,
}

/// The newest `run` row in the ledger.
// trace:STORY-1916 | ai:claude
pub(crate) fn last_reclaim(main_root: &Path) -> Option<LastReclaim> {
    let text = std::fs::read_to_string(ledger_path(main_root)).ok()?;
    text.lines()
        .rev()
        .find_map(|line| match serde_json::from_str::<LedgerRow>(line).ok()? {
            LedgerRow::Run {
                ts,
                freed_bytes,
                deleted,
                used_pct_after,
                stopped,
                ..
            } => Some(LastReclaim {
                at: ts,
                freed_bytes,
                deleted,
                used_pct_after,
                stopped,
            }),
            _ => None,
        })
}

/// The stuck escalation's dedupe state, `<main>/.aida/disk-guard-state.json`.
// trace:STORY-1916 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GuardState {
    #[serde(default)]
    pub last_stuck_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_stuck_deficit: Option<u64>,
    /// When the last "still over the ceiling, nothing deleted" emergency
    /// notice went out.
    #[serde(default)]
    pub last_emergency_stuck_at: Option<DateTime<Utc>>,
    /// The refusal the last emergency notice carried, so a persistent plan
    /// bug re-notifies on the stuck cadence instead of every 30 m.
    // trace:TASK-2272 | ai:claude (TASK-1-484)
    #[serde(default)]
    pub last_emergency_refusal: Option<String>,
}

fn guard_state_path(main_root: &Path) -> PathBuf {
    main_root.join(".aida").join("disk-guard-state.json")
}

pub(crate) fn load_guard_state(main_root: &Path) -> GuardState {
    std::fs::read_to_string(guard_state_path(main_root))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub(crate) fn save_guard_state(main_root: &Path, state: &GuardState) -> Result<()> {
    let path = guard_state_path(main_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(state)?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Stuck escalation
// ---------------------------------------------------------------------------

/// Re-escalate a stuck floor at most every 6h, or sooner when the deficit
/// grew by at least 10 GiB since the last escalation. Pure.
// trace:STORY-1916 | ai:claude
pub(crate) fn should_escalate(state: &GuardState, now: DateTime<Utc>, deficit: u64) -> bool {
    let Some(last) = state.last_stuck_at else {
        return true;
    };
    let stale = now - last >= chrono::Duration::hours(STUCK_REESCALATE_HOURS);
    let grew = deficit
        >= state
            .last_stuck_deficit
            .unwrap_or(0)
            .saturating_add(STUCK_GROWTH_BYTES);
    stale || grew
}

/// Should this emergency run notify? A run that deleted something always
/// does, and so does a NEW refusal: each is news. A run that deleted nothing
/// and is still over the margin repeats every tick, and so does a refusal
/// from a persistent plan bug, so both are deduped like the stuck
/// escalation: at most every 6h. Pure.
// trace:STORY-1916.ac40b516 | ai:claude
// trace:TASK-2272 | ai:claude (TASK-1-484)
pub(crate) fn should_notify_emergency(
    state: &GuardState,
    now: DateTime<Utc>,
    outcome: &RunOutcome,
) -> bool {
    if !outcome.deleted.is_empty() {
        return true;
    }
    if outcome.refusal.is_some() && outcome.refusal != state.last_emergency_refusal {
        return true;
    }
    state
        .last_emergency_stuck_at
        .is_none_or(|last| now - last >= chrono::Duration::hours(STUCK_REESCALATE_HOURS))
}

/// Update the notice dedupe state after an emergency run notified. A
/// nothing-deleted run stamps the stuck cadence; ANY refusal is recorded,
/// including a partial run's (deletes, then the refusal), or the next stuck
/// tick would re-notify the same refusal as new. Returns whether the state
/// changed and needs saving. Pure.
// trace:TASK-2272 | ai:claude (TASK-1-484)
// trace:TASK-2322 | ai:claude
pub(crate) fn note_emergency_notified(
    state: &mut GuardState,
    now: DateTime<Utc>,
    outcome: &RunOutcome,
) -> bool {
    let stuck = outcome.deleted.is_empty();
    if !stuck && outcome.refusal.is_none() {
        return false;
    }
    if stuck {
        state.last_emergency_stuck_at = Some(now);
    }
    state.last_emergency_refusal = outcome.refusal.clone();
    true
}

/// The stuck sentence the finding and the notice carry. Pure.
// trace:STORY-1916 | ai:claude
pub(crate) fn stuck_summary(reclaimable: u64, deficit: u64, top: &[(PathBuf, u64)]) -> String {
    let mut text = format!(
        "STUCK: `aida worktree reclaim` can free only {} of the {} needed to restore the floor, \
         so the guard cannot clear this on its own",
        human_bytes(reclaimable),
        human_bytes(deficit)
    );
    if !top.is_empty() {
        let list: Vec<String> = top
            .iter()
            .map(|(p, b)| format!("{} {}", human_bytes(*b), p.display()))
            .collect();
        text.push_str(&format!("; top consumers: {}", list.join(", ")));
    }
    text
}

/// Is `<main>/target` on the filesystem `dev`? Then its caches are what a
/// breach of that filesystem should list, bind mount or not.
// trace:STORY-1916.ac641c6e | ai:claude
pub(crate) fn target_on_device(main_root: &Path, dev: u64) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(main_root.join("target")).is_ok_and(|m| m.dev() == dev)
}

/// The five largest directories under `<main>/target`, counting each
/// per-worktree cache separately rather than `worktrees/` as one.
// trace:STORY-1916 | ai:claude
pub(crate) fn top_consumers(main_root: &Path, limit: usize) -> Vec<(PathBuf, u64)> {
    let Ok(target) = main_root.join("target").canonicalize() else {
        return Vec::new();
    };
    let children = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect()
    };
    let worktrees = target.join("worktrees");
    let mut dirs: Vec<PathBuf> = children(&target)
        .into_iter()
        .filter(|p| *p != worktrees)
        .collect();
    dirs.extend(children(&worktrees));
    let mut sized: Vec<(PathBuf, u64)> = dirs
        .into_iter()
        .map(|p| {
            let bytes = walk_cache(&p).bytes;
            (p, bytes)
        })
        .collect();
    sized.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    sized.truncate(limit);
    sized
}

// ---------------------------------------------------------------------------
// Status (AC4)
// ---------------------------------------------------------------------------

/// One probed filesystem.
// trace:STORY-1916 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FsStatus {
    /// `project` or `build-cache`.
    pub role: &'static str,
    pub path: String,
    pub free_bytes: u64,
    pub total_bytes: u64,
    pub used_pct: u8,
}

/// The guard at a glance.
// trace:STORY-1916 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DiskStatus {
    pub filesystems: Vec<FsStatus>,
    pub floor_gib: u64,
    pub emergency: EmergencyConfig,
    pub last_guard_run: Option<DateTime<Utc>>,
    pub last_guard_result: Option<String>,
    pub last_reclaim: Option<LastReclaim>,
}

/// The probe paths: the project root, plus `<main>/target` when it is a
/// different filesystem (a bind mount, as on the dev host).
// trace:STORY-1916 | ai:claude
pub(crate) fn probe_paths(
    main_root: &Path,
    stats: impl Fn(&Path) -> Option<FsStats>,
) -> Vec<(&'static str, PathBuf, FsStats)> {
    let mut out = Vec::new();
    let project = stats(main_root);
    if let Some(s) = project {
        out.push(("project", main_root.to_path_buf(), s));
    }
    if let Ok(target) = main_root.join("target").canonicalize() {
        if let Some(s) = stats(&target) {
            if project.is_none_or(|p| p.dev != s.dev) {
                out.push(("build-cache", target, s));
            }
        }
    }
    out
}

/// Gather the status block.
// trace:STORY-1916 | ai:claude
pub(crate) fn status(main_root: &Path, floor_gib: u64, cfg: EmergencyConfig) -> DiskStatus {
    let guard = crate::schedule_ledger::load(&main_root.join(".aida-store"), GUARD_JOB);
    DiskStatus {
        filesystems: probe_paths(main_root, fs_stats)
            .into_iter()
            .map(|(role, path, s)| FsStatus {
                role,
                path: path.display().to_string(),
                free_bytes: s.avail,
                total_bytes: s.total,
                used_pct: s.used_pct(),
            })
            .collect(),
        floor_gib,
        emergency: cfg,
        last_guard_run: guard.as_ref().and_then(|g| g.last_run),
        last_guard_result: guard.and_then(|g| g.result),
        last_reclaim: last_reclaim(main_root),
    }
}

/// Human rendering of the status block.
// trace:STORY-1916 | ai:claude
pub(crate) fn render_status(s: &DiskStatus) -> String {
    let mut out = String::from("\n  disk guard\n");
    for fs in &s.filesystems {
        out.push_str(&format!(
            "    {:<12} {} free of {} ({}% used)  {}\n",
            fs.role,
            human_bytes(fs.free_bytes),
            human_bytes(fs.total_bytes),
            fs.used_pct,
            fs.path
        ));
    }
    out.push_str(&format!("    floor        {} GiB free\n", s.floor_gib));
    if s.emergency.enabled() {
        out.push_str(&format!(
            "    ceiling      {}% used: auto-reclaim of caches idle >= {}h, down to {}%\n",
            s.emergency.used_pct,
            s.emergency.idle_hours,
            s.emergency.stop_below()
        ));
    } else {
        out.push_str("    ceiling      off (emergency_used_pct = 0)\n");
    }
    out.push_str(&format!(
        "    last run     {}\n",
        match (&s.last_guard_run, &s.last_guard_result) {
            (Some(at), Some(r)) => format!("{} ({r})", at.format("%Y-%m-%d %H:%M UTC")),
            (Some(at), None) => at.format("%Y-%m-%d %H:%M UTC").to_string(),
            _ => "never recorded".to_string(),
        }
    ));
    out.push_str(&format!(
        "    last reclaim {}\n",
        match &s.last_reclaim {
            Some(r) => format!(
                "{}: freed {} across {} cache(s), {} after ({})",
                r.at.format("%Y-%m-%d %H:%M UTC"),
                human_bytes(r.freed_bytes),
                r.deleted,
                r.used_pct_after
                    .map_or("?".to_string(), |p| format!("{p}%")),
                r.stopped
            ),
            None => "none".to_string(),
        }
    ));
    out
}

// ---------------------------------------------------------------------------
// Production wiring
// ---------------------------------------------------------------------------

/// `git worktree list` entries of the repository at `main_root`.
fn worktree_entries(main_root: &Path) -> Vec<WorktreeEntry> {
    aida_core::git_ops::host_git_command(main_root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            crate::worktree_reclaim::parse_worktree_entries(&String::from_utf8_lossy(&o.stdout))
        })
        .unwrap_or_default()
}

/// Live session leases, through the shared staleness predicate.
fn live_holders(main_root: &Path) -> Vec<Holder> {
    aida_core::liveness::read_session_leases(main_root)
        .into_iter()
        .filter(|l| !l.worktree_path.as_os_str().is_empty())
        .filter(|l| {
            let gone = aida_core::liveness::lease_owner_process_gone(
                l.active_pid,
                l.active_pid_start_time.as_deref(),
                l.creator_pid,
                l.creator_pid_start_time.as_deref(),
                aida_core::liveness::process_identity_is_alive,
            );
            // A lease with no PID at all reads as live, as in reclaim.
            !gone.unwrap_or(false)
        })
        .map(|l| Holder {
            pid: l.active_pid.or(l.creator_pid),
            scope: l.scope,
            worktree: l.worktree_path,
        })
        .collect()
}

/// Finished PRs from `gh`: branch -> reason and number -> reason. A branch
/// with any open PR is not finished. `None` when `gh` is unreachable.
fn finished_from_gh(main_root: &Path) -> Option<(BTreeMap<String, Reason>, BTreeMap<u64, Reason>)> {
    let out = std::process::Command::new("gh")
        .current_dir(main_root)
        .args([
            "pr",
            "list",
            "--state",
            "all",
            "--limit",
            "300",
            "--json",
            "number,headRefName,state",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).ok()?;
    Some(finished_from_rows(&rows))
}

/// Pure half of [`finished_from_gh`].
// trace:STORY-1916 | ai:claude
pub(crate) fn finished_from_rows(
    rows: &[serde_json::Value],
) -> (BTreeMap<String, Reason>, BTreeMap<u64, Reason>) {
    let mut open: BTreeSet<String> = BTreeSet::new();
    let mut branches = BTreeMap::new();
    let mut numbers = BTreeMap::new();
    for row in rows {
        let branch = row["headRefName"].as_str().unwrap_or_default().to_string();
        let reason = match row["state"].as_str() {
            Some("MERGED") => Reason::Merged,
            Some("CLOSED") => Reason::Closed,
            _ => {
                open.insert(branch);
                continue;
            }
        };
        if let Some(n) = row["number"].as_u64() {
            numbers.insert(n, reason);
        }
        // Merged wins over closed for a branch with several finished PRs.
        let slot = branches.entry(branch).or_insert(reason);
        if reason == Reason::Merged {
            *slot = Reason::Merged;
        }
    }
    branches.retain(|b, _| !open.contains(b));
    (branches, numbers)
}

/// The cache dirs under the cache root, real directories only.
fn cache_entries(cache_root: &Path) -> Vec<CacheEntry> {
    let mut out: Vec<CacheEntry> = std::fs::read_dir(cache_root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink()))
        .map(|e| {
            let path = e.path();
            CacheEntry {
                walk: walk_cache(&path),
                path,
            }
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Mail the holder of a tier-2 cache that its next build starts cold (C2).
/// Best-effort; addressed like the FR-284 session notice so it lands in the
/// live session's headline.
fn mail_holder(main_root: &Path, c: &Candidate) {
    let Some(holder) = &c.holder else { return };
    let id = uuid::Uuid::new_v4().to_string();
    let msg = aida_core::mailbox::Message {
        subject: Some(format!(
            "disk emergency: build cache for {} deleted",
            holder.worktree.display()
        )),
        id: id.clone(),
        thread_id: id,
        from: "aida-disk-guard".into(),
        to: aida_core::mailbox::Recipient::Agent(crate::current_user_id(None)),
        timestamp: Utc::now().timestamp_millis(),
        in_reply_to: None,
        body: format!(
            "The build-cache filesystem was over the emergency ceiling, so the disk guard \
             deleted the idle cache {} ({}) of your worktree {} (scope {}). Nothing was lost but \
             build time: your next cargo build there starts cold.",
            c.path.display(),
            human_bytes(c.bytes),
            holder.worktree.display(),
            holder.scope
        ),
        urgent: false,
        intent: aida_core::mailbox::Intent::Fyi,
        retracted: false,
        deleted: false,
        archived: false,
        from_source: aida_core::mailbox::SenderSource::Explicit,
        from_role: None,
        relayed_from: None,
    };
    let _ = crate::mailbox_store::write_message(main_root, &msg);
}

/// The scheduled emergency run. `None` when the tier is off, the cache
/// filesystem is under the ceiling, or another run holds the lock.
// trace:STORY-1916 | ai:claude
pub(crate) fn run_emergency(main_root: &Path, cfg: &EmergencyConfig) -> Option<RunOutcome> {
    let cache_root = crate::worktree_reclaim::cache_root_of(main_root)?;
    let probe_root = cache_root.clone();
    run_emergency_probed(main_root, &cache_root, cfg, move || {
        fs_stats(&probe_root).map(|s| s.used_pct())
    })
}

/// [`run_emergency`] with the cache root and usage probe injected, so the
/// production glue (lock, gh / offline fallback, incremental ledger appends,
/// notice dedupe state) is testable on a real tempdir whatever its
/// filesystem's actual usage.
// trace:TASK-2272 | ai:claude (TASK-1-482)
pub(crate) fn run_emergency_probed(
    main_root: &Path,
    cache_root: &Path,
    cfg: &EmergencyConfig,
    used_pct: impl Fn() -> Option<u8>,
) -> Option<RunOutcome> {
    use fs2::FileExt;
    if !cfg.enabled() {
        return None;
    }
    if used_pct()? < cfg.used_pct {
        return None;
    }
    let lock_path = main_root.join(".aida").join("disk-emergency.lock");
    std::fs::create_dir_all(lock_path.parent()?).ok()?;
    let lock = std::fs::File::create(&lock_path).ok()?;
    lock.try_lock_exclusive().ok()?;

    let worktrees = worktree_entries(main_root);
    let home = crate::home_dir();
    let allow = Allowlist::for_main(
        main_root,
        cache_root.to_path_buf(),
        home.as_deref(),
        &worktrees,
    );
    let (finished_branches, finished_prs) = finished_from_gh(main_root).unwrap_or_else(|| {
        // gh unreachable: git can still prove merges (STORY-1916 AC2), under
        // reclaim's overall scan budget.
        let branches = crate::worktree_reclaim::merged_offline(main_root, &worktrees)
            .into_iter()
            .map(|b| (b, Reason::Merged))
            .collect();
        (branches, BTreeMap::new())
    });
    let now = SystemTime::now();
    let survey = EmergencySurvey {
        caches: cache_entries(&cache_root),
        worktrees,
        holders: live_holders(main_root),
        finished_branches,
        finished_prs,
        now,
        idle: cfg.idle(),
    };
    let (candidates, _held) = plan(&survey);
    // The scheduled child is killed at its 120 s budget: each ledger row is
    // appended as it happens, so a kill mid-apply loses no Delete row.
    // trace:TASK-2272 | ai:claude (TASK-1-481)
    let outcome = apply(
        &allow,
        &candidates,
        cfg,
        now,
        &used_pct,
        |p| std::fs::remove_dir_all(p),
        |c| mail_holder(main_root, c),
        |row| {
            let _ = append_ledger(main_root, std::slice::from_ref(row));
        },
    );
    // TASK-2325: at the hard ceiling the repo's caches alone may not be
    // enough — terminal jobs' tmp dirs and old session scratch (never build
    // caches, never a live job or session) are then reclaimed too, until
    // usage drops below the ceiling. trace:TASK-2325 | ai:claude
    let outcome = scratch_at_hard_ceiling(main_root, cfg, now, &used_pct, outcome);
    let mut state = load_guard_state(main_root);
    let now_utc = Utc::now();
    if let Some((title, body)) = emergency_notice(&outcome, cfg) {
        if should_notify_emergency(&state, now_utc, &outcome) {
            let _ = crate::notify::send_direct(main_root, "disk-emergency", &title, &body);
            // trace:TASK-2272 | ai:claude (TASK-1-484)
            if note_emergency_notified(&mut state, now_utc, &outcome) {
                let _ = save_guard_state(main_root, &state);
            }
        }
    }
    Some(outcome)
}

/// TASK-2325: the emergency tier's last resort. Runs only while usage is
/// still AT OR ABOVE the hard ceiling after the cache candidates: terminal
/// jobs' tmp dirs and old session scratch — never build caches, never a live
/// job or session — each re-checked at the delete site and ledgered like
/// every other emergency delete. The age window is the emergency idle window
/// or the scratch default, whichever is longer, so this never deletes
/// younger scratch than the ordinary scan would list.
// trace:TASK-2325 | ai:claude
fn scratch_at_hard_ceiling(
    main_root: &Path,
    cfg: &EmergencyConfig,
    now: SystemTime,
    used_pct: &impl Fn() -> Option<u8>,
    mut outcome: RunOutcome,
) -> RunOutcome {
    if !matches!(used_pct(), Some(pct) if pct >= cfg.used_pct) {
        return outcome;
    }
    let mut survey =
        crate::scratch_reclaim::survey_from_env(crate::scratch_reclaim::DEFAULT_MIN_AGE_DAYS, now);
    survey.min_age = survey.min_age.max(cfg.idle());
    survey.build_caches.clear();
    let (rows, _held) = crate::scratch_reclaim::plan(&survey);
    let ts = DateTime::<Utc>::from(now);
    let mut push = |outcome: &mut RunOutcome, row: LedgerRow| {
        let _ = append_ledger(main_root, std::slice::from_ref(&row));
        outcome.rows.push(row);
    };
    for row in rows {
        let Some(pct) = used_pct() else {
            outcome.stopped = Stop::ProbeLost;
            break;
        };
        if pct < cfg.used_pct {
            break;
        }
        let path = row.path.display().to_string();
        if let Err(e) = crate::scratch_reclaim::check(&survey, &row) {
            push(
                &mut outcome,
                LedgerRow::Fail {
                    ts,
                    path,
                    error: e.to_string(),
                },
            );
            outcome.refusal = Some(e.to_string());
            outcome.stopped = Stop::Refused;
            break;
        }
        let removed = if row.path.is_dir() {
            std::fs::remove_dir_all(&row.path)
        } else {
            std::fs::remove_file(&row.path)
        };
        match removed {
            Ok(()) => {
                outcome.freed_bytes = outcome.freed_bytes.saturating_add(row.bytes);
                push(
                    &mut outcome,
                    LedgerRow::Delete {
                        ts,
                        path,
                        bytes: row.bytes,
                        reason: Reason::AgentScratch,
                        idle_secs: row.idle.as_secs(),
                        used_pct_before: Some(pct),
                    },
                );
            }
            Err(e) => push(
                &mut outcome,
                LedgerRow::Fail {
                    ts,
                    path,
                    error: e.to_string(),
                },
            ),
        }
    }
    outcome.used_pct_after = used_pct();
    outcome
}

/// Bytes a `worktree reclaim` dry run could free on the filesystem `dev`.
// trace:STORY-1916 | ai:claude
pub(crate) fn reclaimable_on(main_root: &Path, dev: u64) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let opts = crate::worktree_reclaim::ReclaimOptions {
        apply: false,
        min_age_mins: 60,
        target_pct: None,
        include_live: false,
        include_open_prs: false,
        json: false,
        // This measure is per-device over the repo's caches; scratch has its
        // own survey. trace:TASK-2325 | ai:claude
        scratch_age_days: crate::scratch_reclaim::DEFAULT_MIN_AGE_DAYS,
        no_scratch: true,
    };
    let Ok(survey) = crate::worktree_reclaim::survey_from_env(main_root, &opts) else {
        return 0;
    };
    crate::worktree_reclaim::plan(&survey)
        .reclaim
        .iter()
        .filter(|r| std::fs::metadata(&r.target).is_ok_and(|m| m.dev() == dev))
        .map(|r| r.bytes)
        .sum()
}

#[cfg(test)]
#[path = "tests/story_1916_disk_emergency_tests.rs"]
mod story_1916_disk_emergency_tests;
