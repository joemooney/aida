//! `aida worktree reclaim` — reclaim disk by deleting stale `target/` build
//! caches inside this repository's git worktrees.
//!
//! A `target/` directory is a cache, never a source of truth: nothing but
//! rebuild time is lost. What makes this worth a command rather than a one-off
//! `rm -rf` is that the safe candidate set is narrow and every one of the rails
//! below was needed in practice — on 2026-09-30 the host hit 89% with 86G
//! sitting in 11 stale worktree caches, and the operator hand-wrote the delete
//! list twice because `aida session reap` does not touch the cache of a
//! worktree it keeps and `cargo clean` inside a worktree clears the SHARED
//! `CARGO_TARGET_DIR` (which points at the main checkout) rather than the stale
//! one.
//!
//! ## Why this is `worktree reclaim` and not a flag on `session reap`
//!
//! `aida session reap` reaps only sessions that have genuinely finished: spec
//! Done/Completed, branch merged, process exited. A stale cache is reclaimable
//! in precisely the worktrees that do NOT satisfy that contract — an unmerged
//! branch parked mid-review, a worktree whose spec is still In Progress but
//! whose build went cold hours ago. `gc` removes worktrees that are finished;
//! `reclaim` removes the regenerable cache inside worktrees that are KEPT.
//!
//! ## The rails, in the order they are applied
//!
//! 1. Candidates come from `git worktree list --porcelain`, so only worktrees
//!    of THIS repository are ever considered — unrelated sibling projects are
//!    invisible to the walk.
//! 2. The main checkout's `target/` is never a candidate. It holds the live
//!    `aida` binary and is every worktree's `CARGO_TARGET_DIR`.
//! 3. The `.aida-store` worktree is never a candidate.
//! 4. A worktree held by a LIVE session lease is skipped (`--include-live`).
//! 5. A worktree whose branch has an OPEN pull request is skipped
//!    (`--include-open-prs`): you may still need it to verify that PR.
//! 6. A `target/` touched within `--min-age-mins` is skipped as a possible
//!    in-flight build.
//!
//! ## Rail 2 is enforced twice, deliberately
//!
//! [`plan`] filters the main checkout out of the candidate list, and [`apply`]
//! re-checks every path against `main_root/target` immediately before deleting
//! and refuses. The second check is what makes the guarantee independent of
//! ordering: an upstream filter bug cannot reach `remove_dir_all` on the live
//! cache even if that cache is the largest candidate in the plan.
//!
//! ## Where the caches actually are (STORY-1916)
//!
//! BUG-1783 isolates each worktree's cargo cache at
//! `<main>/target/worktrees/<worktree-dir-name>`, not `<worktree>/target`. The
//! planner considers both. It also reclaims an orphaned cache (no worktree
//! and no live lease with that dir name), an idle duplicate (a second
//! worktree on the same open PR whose cache is cold), and, when `gh` cannot
//! be reached, a branch that git alone proves merged (ancestry, or a squash
//! whose patch-id is on the default branch). A branch it cannot prove stays
//! held, and only that branch.
// trace:TASK-1562 | ai:claude
// trace:STORY-1916 | ai:claude

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{bail, Result};

/// Why a worktree's `target/` was not a reclaim candidate. The reason is
/// user-facing output: under disk pressure the operator needs to know that
/// `aida-bug-1693` was held back for an open PR, not merely that something was
/// skipped.
// trace:TASK-1562 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// Rail 2 — the live `aida` binary's cache and every worktree's
    /// `CARGO_TARGET_DIR`.
    MainCheckout,
    /// Rail 3 — the `.aida-store` orphan-branch worktree.
    Store,
    /// Nothing to reclaim.
    NoTarget,
    /// Rail 4 — a live session lease owns this worktree as its cwd.
    LiveSession { pid: Option<u32> },
    /// Rail 5 — the branch has an open pull request.
    OpenPr { branch: String },
    /// Rail 5, fail-safe arm: open-PR state could not be determined, so every
    /// branch is treated as possibly having one.
    PrStateUnknown,
    /// Rail 6 — a build may be in flight.
    RecentlyTouched { mins: u64 },
}

impl Skip {
    /// One-line reason for the human and `--json` renderings.
    pub fn reason(&self) -> String {
        match self {
            Skip::MainCheckout => {
                "main checkout — holds the live aida binary and is every worktree's \
                 CARGO_TARGET_DIR"
                    .to_string()
            }
            Skip::Store => ".aida-store worktree".to_string(),
            Skip::NoTarget => "no target/ directory".to_string(),
            Skip::LiveSession { pid: Some(pid) } => format!("live session lease (pid {pid})"),
            Skip::LiveSession { pid: None } => "live session lease".to_string(),
            Skip::OpenPr { branch } => format!("open PR on branch {branch}"),
            Skip::PrStateUnknown => {
                "open-PR state unknown (gh unavailable) — pass --include-open-prs to \
                 reclaim anyway"
                    .to_string()
            }
            Skip::RecentlyTouched { mins } => {
                format!("target/ touched in the last {mins} min — a build may be in flight")
            }
        }
    }

    /// Stable machine token for `--json`.
    pub fn code(&self) -> &'static str {
        match self {
            Skip::MainCheckout => "main-checkout",
            Skip::Store => "store",
            Skip::NoTarget => "no-target",
            Skip::LiveSession { .. } => "live-session",
            Skip::OpenPr { .. } => "open-pr",
            Skip::PrStateUnknown => "pr-state-unknown",
            Skip::RecentlyTouched { .. } => "recently-touched",
        }
    }
}

/// One worktree of this repository, as `git worktree list --porcelain` reports
/// it. `branch` is `None` for a detached record.
// trace:TASK-1562 | ai:claude
#[derive(Debug, Clone)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub branch: Option<String>,
}

/// A session lease's hold on a worktree, with liveness already resolved by the
/// shared staleness predicate so the planner stays pure.
// trace:TASK-1562 | ai:claude
#[derive(Debug, Clone)]
pub struct LeaseHold {
    pub worktree_path: PathBuf,
    pub alive: bool,
    pub pid: Option<u32>,
}

/// Why a cache is reclaimable. `Stale` is the TASK-1562 case; the rest are
/// the STORY-1916 additions, each of which lifts one specific hold.
// trace:STORY-1916 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReclaimWhy {
    /// Passed every rail: no open PR, no live lease, cold.
    Stale,
    /// A `<main>/target/worktrees/<name>` cache with no worktree and no live
    /// lease of that name.
    Orphan,
    /// `gh` was unreachable, but git proved the branch merged.
    MergedOffline,
    /// Another worktree on the same open PR has the fresher cache.
    IdleDuplicate,
}

impl ReclaimWhy {
    /// Stable machine token for `--json`.
    pub fn code(&self) -> &'static str {
        match self {
            ReclaimWhy::Stale => "stale",
            ReclaimWhy::Orphan => "orphan",
            ReclaimWhy::MergedOffline => "merged",
            ReclaimWhy::IdleDuplicate => "idle-duplicate",
        }
    }
}

/// A `target/` that passed every rail.
// trace:TASK-1562 | ai:claude
#[derive(Debug, Clone)]
pub struct Reclaimable {
    pub target: PathBuf,
    pub branch: Option<String>,
    pub bytes: u64,
    // trace:STORY-1916 | ai:claude
    pub why: ReclaimWhy,
}

/// Everything the planner needs, injected so a test can build a fixture
/// instead of mocking a filesystem.
// trace:TASK-1562 | ai:claude
#[derive(Debug, Clone)]
pub struct Survey {
    /// The main checkout's root — the parent of `git rev-parse
    /// --git-common-dir`. Its `target/` is protected.
    pub main_root: PathBuf,
    /// Every worktree of this repo, including the main checkout.
    pub worktrees: Vec<WorktreeEntry>,
    /// Branches with an open PR. `None` means the state could not be
    /// determined, which fails SAFE: every branch is then held back.
    pub open_pr_branches: Option<Vec<String>>,
    /// Session leases, liveness pre-resolved.
    pub leases: Vec<LeaseHold>,
    pub min_age_mins: u64,
    pub include_live: bool,
    pub include_open_prs: bool,
    /// Injected clock, so rail 6 is testable.
    pub now: SystemTime,
    /// `<main>/target/worktrees`, the BUG-1783 per-worktree cache root, when
    /// it exists.
    // trace:STORY-1916 | ai:claude
    pub cache_root: Option<PathBuf>,
    /// Open PRs as `(number, head branch)`, used to match a detached
    /// `take-<N>` / `pr-<N>` worktree to its PR. Empty when unknown.
    // trace:STORY-1916 | ai:claude
    pub open_prs: Vec<(u64, String)>,
    /// Branches git alone proved merged. Consulted only when
    /// `open_pr_branches` is `None`.
    // trace:STORY-1916 | ai:claude
    pub merged_offline: BTreeSet<String>,
}

/// The classification result: what would be deleted, largest first, and every
/// path that was held back with its reason.
// trace:TASK-1562 | ai:claude
#[derive(Debug, Clone)]
pub struct Plan {
    pub main_root: PathBuf,
    pub reclaim: Vec<Reclaimable>,
    pub skipped: Vec<(PathBuf, Skip)>,
    /// The per-worktree cache root `apply` accepts direct children of.
    // trace:STORY-1916 | ai:claude
    pub cache_root: Option<PathBuf>,
}

impl Plan {
    /// Total reclaimable bytes — reported before anything is deleted.
    pub fn reclaimable_bytes(&self) -> u64 {
        self.reclaim.iter().map(|r| r.bytes).sum()
    }
}

/// What `apply` actually did.
// trace:TASK-1562 | ai:claude
#[derive(Debug, Clone, Default)]
pub struct Reclaimed {
    pub deleted: Vec<(PathBuf, u64)>,
    pub failed: Vec<(PathBuf, String)>,
    /// Entries left alone because `--target-pct` was already satisfied.
    pub stopped_early: Vec<PathBuf>,
    pub freed_bytes: u64,
}

/// The one path this command must never delete.
///
/// Named rather than inlined so [`plan`] and [`apply`] cannot disagree about
/// what rail 2 protects.
// trace:TASK-1562 | ai:claude
pub fn protected_target(main_root: &Path) -> PathBuf {
    main_root.join("target")
}

/// Recursive on-disk size of a directory, following no symlinks.
///
/// Unreadable entries are skipped rather than failing the walk: a partially
/// readable cache still reports a useful lower bound, and the number is only
/// ever used to order and report candidates.
// trace:TASK-1562 | ai:claude
pub fn dir_size_bytes(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(md) = entry.metadata() else { continue };
            if md.is_dir() {
                stack.push(entry.path());
            } else if md.is_file() {
                total = total.saturating_add(md.len());
            }
        }
    }
    total
}

/// Has this cache been written within `min_age`?
///
/// STORY-1916: judged on the NEWEST mtime anywhere under the cache, not the
/// directory's own. Cargo writes into `<profile>/deps` and `.fingerprint`,
/// which leaves a `<main>/target/worktrees/<name>` root's own mtime hours
/// old in the middle of a live build. An mtime in the future (clock skew)
/// reads as "possibly in flight", the conservative arm.
// trace:TASK-1562 | ai:claude
// trace:STORY-1916 | ai:claude
fn touched_within(newest: Option<SystemTime>, min_age: Duration, now: SystemTime) -> bool {
    let Some(mtime) = newest else {
        return false;
    };
    match now.duration_since(mtime) {
        Ok(age) => age < min_age,
        Err(_) => true,
    }
}

/// The PR number a detached review worktree is named for: `take-2830`,
/// `pr-2830`, `aida-pr-2830` (an optional `-<suffix>` is allowed). Pure.
// trace:STORY-1916 | ai:claude
pub fn pr_number_from_dir(name: &str) -> Option<u64> {
    ["aida-pr-", "take-", "pr-"].iter().find_map(|prefix| {
        let digits = name.strip_prefix(prefix)?.split('-').next()?;
        (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
            .then(|| digits.parse().ok())
            .flatten()
    })
}

/// A real directory, not a symlink to one: a symlinked cache would make the
/// delete land somewhere this command never classified.
// trace:STORY-1916 | ai:claude
fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

/// The open-PR group a worktree belongs to, for the idle-duplicate rule. A
/// branch worktree groups by its branch; a detached `take-<N>` worktree by
/// the head branch of open PR N, or by `pr:<N>` when PR state is unknown.
// trace:STORY-1916 | ai:claude
fn pr_group(
    wt: &WorktreeEntry,
    open_prs: Option<&BTreeSet<&str>>,
    by_number: &BTreeMap<u64, &str>,
) -> Option<String> {
    let number = wt
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(pr_number_from_dir);
    match open_prs {
        Some(set) => wt
            .branch
            .as_deref()
            .filter(|b| set.contains(b))
            .or_else(|| number.and_then(|n| by_number.get(&n).copied()))
            .map(str::to_string),
        None => wt
            .branch
            .clone()
            .or_else(|| number.map(|n| format!("pr:{n}"))),
    }
}

/// A cache held back by rail 5, kept for the idle-duplicate pass.
// trace:STORY-1916 | ai:claude
struct Held {
    worktree: PathBuf,
    target: PathBuf,
    branch: Option<String>,
    walk: crate::disk_emergency::CacheWalk,
    group: Option<String>,
    skip: Skip,
}

/// Per-group bookkeeping for the idle-duplicate rule: which worktrees are in
/// the group, which one has the freshest cache, and whether a live session
/// holds any of them (a live member is always the keeper).
// trace:STORY-1916 | ai:claude
#[derive(Default)]
struct Group {
    members: BTreeSet<PathBuf>,
    keeper: Option<(SystemTime, PathBuf)>,
    live: bool,
}

/// Classify every worktree against the six rails, plus the STORY-1916
/// layout, orphan, offline-merge and idle-duplicate rules. Deletes nothing.
///
/// The returned `reclaim` list is ordered largest-first, so a `--target-pct`
/// run frees the most space per deletion; `skipped` preserves the walk order so
/// the printed report reads like the worktree list.
// trace:TASK-1562 | ai:claude
// trace:STORY-1916 | ai:claude
pub fn plan(survey: &Survey) -> Plan {
    let protected = protected_target(&survey.main_root);
    let min_age = Duration::from_secs(survey.min_age_mins.saturating_mul(60));

    // Branch -> open PR, as a set for the rail-5 test. `None` is the
    // fail-safe arm and is handled separately below.
    let open_prs: Option<BTreeSet<&str>> = survey
        .open_pr_branches
        .as_ref()
        .map(|b| b.iter().map(String::as_str).collect());
    let by_number: BTreeMap<u64, &str> = survey
        .open_prs
        .iter()
        .map(|(n, b)| (*n, b.as_str()))
        .collect();

    let live: Vec<&LeaseHold> = survey.leases.iter().filter(|l| l.alive).collect();

    let mut reclaim = Vec::new();
    let mut skipped = Vec::new();
    let mut held: Vec<Held> = Vec::new();
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    // Dir names that own a cache under `cache_root`; anything else there is
    // an orphan.
    let mut owned: BTreeSet<std::ffi::OsString> = live
        .iter()
        .filter_map(|l| l.worktree_path.file_name().map(|n| n.to_os_string()))
        .collect();

    for wt in &survey.worktrees {
        if let Some(name) = wt.path.file_name() {
            owned.insert(name.to_os_string());
        }

        // Rail 2. Compared against the named protected PATH, not against "is
        // this the first porcelain record", so a reordered or hand-built
        // worktree list cannot slip it through.
        //
        // The second clause is not an independent rail: `target == protected`
        // already implies `wt.path == main_root` for identical strings. It is
        // there for the case where the two are the same directory spelled
        // differently — `git worktree list` reports a realpath while
        // `--git-common-dir` may hand back a symlinked parent — in which case
        // the join comparison misses and the path comparison is the one that
        // fires. Keeping both is cheap; pretending they are separate rails
        // would not be.
        if wt.path.join("target") == protected || wt.path == survey.main_root {
            skipped.push((wt.path.clone(), Skip::MainCheckout));
            continue;
        }
        // Rail 3.
        if wt.path.file_name().is_some_and(|n| n == ".aida-store") {
            skipped.push((wt.path.clone(), Skip::Store));
            continue;
        }
        // A worktree's cache is `<wt>/target`, the BUG-1783
        // `<cache_root>/<wt dir name>`, or both.
        let targets: Vec<PathBuf> = [
            Some(wt.path.join("target")),
            survey
                .cache_root
                .as_ref()
                .zip(wt.path.file_name())
                .map(|(root, name)| root.join(name)),
        ]
        .into_iter()
        .flatten()
        .filter(|t| is_real_dir(t))
        .collect();
        if targets.is_empty() {
            skipped.push((wt.path.clone(), Skip::NoTarget));
            continue;
        }
        let group = pr_group(wt, open_prs.as_ref(), &by_number);
        // Rail 4.
        if !survey.include_live {
            if let Some(hold) = live.iter().find(|l| l.worktree_path == wt.path) {
                if let Some(g) = &group {
                    let entry = groups.entry(g.clone()).or_default();
                    entry.members.insert(wt.path.clone());
                    entry.live = true;
                }
                skipped.push((wt.path.clone(), Skip::LiveSession { pid: hold.pid }));
                continue;
            }
        }

        for target in targets {
            let walk = crate::disk_emergency::walk_cache(&target);
            // Rail 5, including its fail-safe arm: when open-PR state is
            // unknown we hold every branch back that git cannot prove merged
            // (STORY-1916), rather than reclaiming everything. The reference
            // shell script had this inverted.
            let mut why = ReclaimWhy::Stale;
            if !survey.include_open_prs {
                let hold = match (&open_prs, &wt.branch) {
                    (None, Some(branch)) if survey.merged_offline.contains(branch) => {
                        why = ReclaimWhy::MergedOffline;
                        None
                    }
                    (None, _) => Some(Skip::PrStateUnknown),
                    (Some(set), Some(branch)) if set.contains(branch.as_str()) => {
                        Some(Skip::OpenPr {
                            branch: branch.clone(),
                        })
                    }
                    // A detached `take-<N>` worktree whose PR N is open is
                    // that PR's worktree too.
                    // trace:STORY-1916.ac149566 | ai:claude
                    (Some(_), None) => group.clone().map(|branch| Skip::OpenPr { branch }),
                    _ => None,
                };
                if let Some(skip) = hold {
                    // Only a member that stays held may be the group's
                    // keeper: a keeper that is itself reclaimed would take
                    // the open PR's last warm cache with it.
                    // trace:STORY-1916.ac149566 | ai:claude
                    if let Some(g) = &group {
                        let entry = groups.entry(g.clone()).or_default();
                        entry.members.insert(wt.path.clone());
                        let newest = walk.newest.unwrap_or(SystemTime::UNIX_EPOCH);
                        if entry.keeper.as_ref().is_none_or(|(t, _)| newest > *t) {
                            entry.keeper = Some((newest, wt.path.clone()));
                        }
                    }
                    held.push(Held {
                        worktree: wt.path.clone(),
                        target,
                        branch: wt.branch.clone(),
                        walk,
                        group: group.clone(),
                        skip,
                    });
                    continue;
                }
            }
            // Rail 6.
            if touched_within(walk.newest, min_age, survey.now) {
                skipped.push((
                    wt.path.clone(),
                    Skip::RecentlyTouched {
                        mins: survey.min_age_mins,
                    },
                ));
                continue;
            }
            reclaim.push(Reclaimable {
                bytes: walk.bytes,
                target,
                branch: wt.branch.clone(),
                why,
            });
        }
    }

    // Idle duplicates: a rail-5 hold is lifted for a worktree that shares its
    // open PR with another worktree holding the fresher (or a live) cache.
    // Rail 6 still applies, so a duplicate mid-build is never touched.
    for h in held {
        let duplicate = h
            .group
            .as_ref()
            .and_then(|g| groups.get(g))
            .is_some_and(|g| {
                g.members.len() >= 2
                    && (g.live || g.keeper.as_ref().is_some_and(|(_, k)| *k != h.worktree))
            });
        if !duplicate {
            skipped.push((h.worktree, h.skip));
        } else if touched_within(h.walk.newest, min_age, survey.now) {
            skipped.push((
                h.worktree,
                Skip::RecentlyTouched {
                    mins: survey.min_age_mins,
                },
            ));
        } else {
            reclaim.push(Reclaimable {
                bytes: h.walk.bytes,
                target: h.target,
                branch: h.branch,
                why: ReclaimWhy::IdleDuplicate,
            });
        }
    }

    // Orphans: a cache whose worktree and lease are both gone.
    if let Some(cache_root) = &survey.cache_root {
        let mut orphans: Vec<PathBuf> = std::fs::read_dir(cache_root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| !owned.contains(&e.file_name()))
            .map(|e| e.path())
            .filter(|p| is_real_dir(p))
            .collect();
        orphans.sort();
        for target in orphans {
            let walk = crate::disk_emergency::walk_cache(&target);
            if touched_within(walk.newest, min_age, survey.now) {
                skipped.push((
                    target,
                    Skip::RecentlyTouched {
                        mins: survey.min_age_mins,
                    },
                ));
                continue;
            }
            reclaim.push(Reclaimable {
                bytes: walk.bytes,
                target,
                branch: None,
                why: ReclaimWhy::Orphan,
            });
        }
    }

    reclaim.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.target.cmp(&b.target)));

    Plan {
        main_root: survey.main_root.clone(),
        reclaim,
        skipped,
        cache_root: survey.cache_root.clone(),
    }
}

/// STORY-1916: the delete-site allowlist beyond rail 2. A path must be a
/// worktree's own `target/` or a direct child of the plan's cache root, must
/// not be a symlink, and must not sit anywhere else inside the main
/// checkout's `target/` (`agent/` holds the live dev build).
// trace:STORY-1916 | ai:claude
fn refuse_outside_allowlist(plan: &Plan, target: &Path) -> Result<()> {
    let refuse = |why: &str| -> Result<()> {
        bail!(
            "refusing to delete {}: {why}. Reaching this means the plan was built wrong.",
            target.display()
        )
    };
    if std::fs::symlink_metadata(target).is_ok_and(|m| m.file_type().is_symlink()) {
        return refuse("it is a symlink");
    }
    let in_cache_root = plan
        .cache_root
        .as_deref()
        .is_some_and(|root| target.parent() == Some(root));
    if in_cache_root {
        return Ok(());
    }
    let main_target = protected_target(&plan.main_root);
    let canonical = main_target.canonicalize().ok();
    let inside_main_target = target.starts_with(&main_target)
        || canonical.as_deref().is_some_and(|c| target.starts_with(c));
    if inside_main_target {
        return refuse(
            "it is inside the main checkout's target/ but not a per-worktree cache under \
             target/worktrees",
        );
    }
    if target.file_name().is_none_or(|n| n != "target") {
        return refuse("it is neither a worktree's target/ nor a per-worktree cache");
    }
    Ok(())
}

/// Delete the planned caches.
///
/// `disk_pct` is injected: when `stop_at_pct` is set, it is consulted before
/// each deletion and the run stops as soon as the filesystem is at or below the
/// target, leaving the remaining caches warm.
///
/// Every path is re-checked against [`protected_target`] here, independently of
/// [`plan`]. That is rail 2's real guarantee: it does not depend on the plan
/// being correctly ordered or correctly filtered.
// trace:TASK-1562 | ai:claude
pub fn apply(
    plan: &Plan,
    stop_at_pct: Option<u8>,
    disk_pct: impl Fn(&Path) -> Option<u8>,
    remove: impl Fn(&Path) -> std::io::Result<()>,
) -> Result<Reclaimed> {
    let protected = protected_target(&plan.main_root);
    let mut out = Reclaimed::default();

    for entry in &plan.reclaim {
        if entry.target == protected {
            bail!(
                "refusing to delete {}: it is the main checkout's target/, which holds the \
                 live aida binary and is every worktree's CARGO_TARGET_DIR. This is rail 2 \
                 re-checked at the delete site; reaching it means the plan was built wrong.",
                entry.target.display()
            );
        }
        refuse_outside_allowlist(plan, &entry.target)?;

        if let Some(floor) = stop_at_pct {
            if let Some(pct) = disk_pct(&plan.main_root) {
                if pct <= floor {
                    out.stopped_early.push(entry.target.clone());
                    continue;
                }
            }
        }

        match remove(&entry.target) {
            Ok(()) => {
                out.freed_bytes = out.freed_bytes.saturating_add(entry.bytes);
                out.deleted.push((entry.target.clone(), entry.bytes));
            }
            Err(e) => out.failed.push((entry.target.clone(), e.to_string())),
        }
    }

    Ok(out)
}

/// Human-readable byte count for the report.
// trace:TASK-1562 | ai:claude
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

// ---------------------------------------------------------------------------
// Production wiring: gather the survey from the real environment, render, act.
// ---------------------------------------------------------------------------

/// `git` in `dir`, captured, `None` on any failure.
// trace:TASK-1562 | ai:claude
fn git_capture(dir: &Path, args: &[&str]) -> Option<String> {
    // TASK-2053: refuses a seat clone (run from inside one: no survey).
    // trace:TASK-2053 | ai:claude
    let out = aida_core::git_ops::host_git_command(dir)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The main checkout's root — the parent of `git rev-parse --git-common-dir`.
/// From a linked worktree this resolves to the SHARED checkout, which is both
/// where session leases live and the directory whose `target/` is protected.
// trace:TASK-1562 | ai:claude
fn main_checkout_root(cwd: &Path) -> Option<PathBuf> {
    let raw = git_capture(cwd, &["rev-parse", "--git-common-dir"])?;
    let p = PathBuf::from(&raw);
    let abs = if p.is_absolute() { p } else { cwd.join(p) };
    abs.parent().map(Path::to_path_buf)
}

/// Parse `git worktree list --porcelain` into entries. Each record is
/// `worktree <path>` optionally followed by `branch refs/heads/<name>`; a
/// detached record has no branch line. Pure.
// trace:TASK-1562 | ai:claude
pub fn parse_worktree_entries(porcelain: &str) -> Vec<WorktreeEntry> {
    let mut out: Vec<WorktreeEntry> = Vec::new();
    for line in porcelain.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            out.push(WorktreeEntry {
                path: PathBuf::from(p.trim()),
                branch: None,
            });
        } else if let Some(b) = line.strip_prefix("branch ") {
            if let Some(last) = out.last_mut() {
                let short = b.trim().strip_prefix("refs/heads/").unwrap_or(b.trim());
                last.branch = Some(short.to_string());
            }
        }
    }
    out
}

/// Open PRs as `(number, head branch)`, via `gh`. `None` when `gh` is missing
/// or the call fails — the fail-safe arm of rail 5.
// trace:TASK-1562 | ai:claude
// trace:STORY-1916 | ai:claude
fn open_prs(dir: &Path) -> Option<Vec<(u64, String)>> {
    let out = std::process::Command::new("gh")
        .current_dir(dir)
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            "200",
            "--json",
            "number,headRefName",
            "--jq",
            r#".[] | "\(.number) \(.headRefName)""#,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(parse_number_branch_lines(&String::from_utf8_lossy(
        &out.stdout,
    )))
}

/// Parse `<number> <branch>` lines. Pure.
// trace:STORY-1916 | ai:claude
pub fn parse_number_branch_lines(text: &str) -> Vec<(u64, String)> {
    text.lines()
        .filter_map(|l| {
            let (n, b) = l.trim().split_once(' ')?;
            Some((n.parse().ok()?, b.trim().to_string()))
        })
        .filter(|(_, b)| !b.is_empty())
        .collect()
}

/// How git alone proved a branch merged into the default branch.
// trace:STORY-1916 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeProof {
    /// The branch tip is an ancestor of the default branch.
    Ancestor,
    /// The branch's cumulative diff has the same patch-id as one commit on
    /// the default branch: a squash merge.
    Squash,
}

/// The squash scan's per-branch deadline (advisor condition C3).
pub const SQUASH_SCAN_DEADLINE: Duration = Duration::from_secs(20);
/// The squash scan's commit bound.
const SQUASH_SCAN_MAX_COMMITS: &str = "--max-count=2000";

/// Prove `branch` merged into `default_ref` using git only, for when `gh`
/// cannot be reached. `None` = not proven, which the caller treats as
/// unknown (held), never as merged. That includes a squash scan that runs
/// past `deadline`. Read-only: no object is written.
// trace:STORY-1916 | ai:claude
pub fn branch_merged_offline(
    repo: &Path,
    branch: &str,
    default_ref: &str,
    deadline: Duration,
) -> Option<MergeProof> {
    let head = format!("refs/heads/{branch}");
    let ancestor = aida_core::git_ops::host_git_command(repo)
        .args(["merge-base", "--is-ancestor", &head, default_ref])
        .output()
        .ok()?;
    match ancestor.status.code() {
        Some(0) => return Some(MergeProof::Ancestor),
        Some(1) => {}
        _ => return None,
    }
    let mb = aida_core::git_ops::host_git_command(repo)
        .args(["merge-base", &head, default_ref])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())?;
    let stop = std::time::Instant::now() + deadline;
    let ours = patch_ids(
        repo,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-renames",
            &mb,
            &head,
        ],
        stop,
    )?;
    // An empty branch diff has no patch-id and proves nothing.
    let ours = ours.first()?.clone();
    let theirs = patch_ids(
        repo,
        &[
            "log",
            "-p",
            "--no-color",
            "--no-ext-diff",
            "--no-renames",
            "--no-merges",
            SQUASH_SCAN_MAX_COMMITS,
            &format!("{mb}..{default_ref}"),
        ],
        stop,
    )?;
    theirs.contains(&ours).then_some(MergeProof::Squash)
}

/// Run `git <producer>` piped into `git patch-id --stable` and return the
/// patch-ids, or `None` on any failure or once `stop` passes (both children
/// are killed). The output is drained on a thread so a large patch-id list
/// cannot fill the pipe and stall the wait.
// trace:STORY-1916 | ai:claude
fn patch_ids(repo: &Path, producer: &[&str], stop: std::time::Instant) -> Option<Vec<String>> {
    use std::io::Read;
    use std::process::Stdio;
    if std::time::Instant::now() >= stop {
        return None;
    }
    let mut source = aida_core::git_ops::host_git_command(repo)
        .args(producer)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let piped = source.stdout.take()?;
    let mut ids = match aida_core::git_ops::host_git_command(repo)
        .args(["patch-id", "--stable"])
        .stdin(piped)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            let _ = source.kill();
            let _ = source.wait();
            return None;
        }
    };
    let mut out = ids.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = out.read_to_string(&mut text);
        text
    });
    let finished = loop {
        match ids.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if std::time::Instant::now() < stop => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => break false,
        }
    };
    if !finished {
        let _ = ids.kill();
        let _ = source.kill();
        let _ = ids.wait();
        let _ = source.wait();
        return None;
    }
    let source_ok = source.wait().is_ok_and(|s| s.success());
    let text = reader.join().ok()?;
    source_ok.then(|| {
        text.lines()
            .filter_map(|l| l.split_whitespace().next().map(str::to_string))
            .collect()
    })
}

/// The whole offline scan's budget, so a host with many worktrees and an
/// unreachable `gh` cannot stall a scheduled tick for minutes. Branches
/// after the budget is spent stay unproven (held).
const OFFLINE_SCAN_BUDGET: Duration = Duration::from_secs(60);

/// Branches git alone proves merged, for every worktree branch.
// trace:STORY-1916 | ai:claude
pub(crate) fn merged_offline(main_root: &Path, worktrees: &[WorktreeEntry]) -> BTreeSet<String> {
    let Some(default_ref) = crate::resolve_default_branch_ref(main_root) else {
        return BTreeSet::new();
    };
    let budget_end = std::time::Instant::now() + OFFLINE_SCAN_BUDGET;
    let mut merged = BTreeSet::new();
    for branch in worktrees.iter().filter_map(|w| w.branch.as_deref()) {
        let left = budget_end.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        if branch_merged_offline(
            main_root,
            branch,
            &default_ref,
            left.min(SQUASH_SCAN_DEADLINE),
        )
        .is_some()
        {
            merged.insert(branch.to_string());
        }
    }
    merged
}

/// `<main>/target/worktrees`, canonical, when it exists.
// trace:STORY-1916 | ai:claude
pub fn cache_root_of(main_root: &Path) -> Option<PathBuf> {
    let target = protected_target(main_root).canonicalize().ok()?;
    let root = target.join("worktrees");
    is_real_dir(&root).then_some(root)
}

/// Session leases with liveness resolved through the SHARED staleness
/// predicate, so a reclaim agrees with what `aida ps` calls live. Using the
/// recorded start identity (not a bare `kill -0`) is what keeps a recycled PID
/// — or a low-numbered kernel thread — from reading as a live agent.
// trace:TASK-1562 | ai:claude
fn lease_holds(main_root: &Path) -> Vec<LeaseHold> {
    aida_core::liveness::read_session_leases(main_root)
        .into_iter()
        .filter(|l| !l.worktree_path.as_os_str().is_empty())
        .map(|l| {
            let pid = l.active_pid.or(l.creator_pid);
            let gone = aida_core::liveness::lease_owner_process_gone(
                l.active_pid,
                l.active_pid_start_time.as_deref(),
                l.creator_pid,
                l.creator_pid_start_time.as_deref(),
                aida_core::liveness::process_identity_is_alive,
            );
            LeaseHold {
                worktree_path: l.worktree_path,
                // No PID at all (a legacy lease) reads as LIVE: holding a cache
                // back costs rebuild time, deleting one out from under a
                // running build costs the build.
                alive: !gone.unwrap_or(false),
                pid,
            }
        })
        .collect()
}

/// Used-percentage of the filesystem containing `path`, via sysinfo.
// trace:TASK-1562 | ai:claude
fn disk_used_pct(path: &Path) -> Option<u8> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let disk = disks
        .iter()
        .filter(|d| path.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())?;
    let total = disk.total_space();
    if total == 0 {
        return None;
    }
    let used = total.saturating_sub(disk.available_space());
    Some(((used as f64 / total as f64) * 100.0).round() as u8)
}

/// Options as the CLI presents them.
// trace:TASK-1562 | ai:claude
#[derive(Debug, Clone)]
pub struct ReclaimOptions {
    pub apply: bool,
    pub min_age_mins: u64,
    pub target_pct: Option<u8>,
    pub include_live: bool,
    pub include_open_prs: bool,
    pub json: bool,
}

/// Build the survey from the environment rooted at `cwd`.
// trace:TASK-1562 | ai:claude
pub fn survey_from_env(cwd: &Path, opts: &ReclaimOptions) -> Result<Survey> {
    let Some(main_root) = main_checkout_root(cwd) else {
        bail!(
            "not inside a git repository (no --git-common-dir from {})",
            cwd.display()
        );
    };
    let Some(porcelain) = git_capture(cwd, &["worktree", "list", "--porcelain"]) else {
        bail!(
            "`git worktree list --porcelain` failed in {}",
            cwd.display()
        );
    };
    let worktrees = parse_worktree_entries(&porcelain);
    // Not consulted when the rail is waived; skip the network call.
    let prs = if opts.include_open_prs {
        Some(Vec::new())
    } else {
        open_prs(&main_root)
    };
    // trace:STORY-1916 | ai:claude
    let merged_offline = if prs.is_none() {
        merged_offline(&main_root, &worktrees)
    } else {
        BTreeSet::new()
    };
    Ok(Survey {
        open_pr_branches: prs
            .as_ref()
            .map(|p| p.iter().map(|(_, b)| b.clone()).collect()),
        open_prs: prs.unwrap_or_default(),
        merged_offline,
        cache_root: cache_root_of(&main_root),
        worktrees,
        leases: lease_holds(&main_root),
        main_root,
        min_age_mins: opts.min_age_mins,
        include_live: opts.include_live,
        include_open_prs: opts.include_open_prs,
        now: SystemTime::now(),
    })
}

/// Should this run render JSON?
///
/// Resolved from the output mode, not from the local `--json` bool alone:
/// BUG-1749 was exactly a renderer picked off a local flag while a
/// `--format`/`AIDA_OUTPUT_FORMAT` pin said something else. Pure, so the
/// precedence is testable.
// trace:TASK-1562 | ai:claude
pub fn renders_json(json_flag: bool, pin: Option<crate::OutputFormat>) -> bool {
    json_flag || matches!(pin, Some(crate::OutputFormat::Json))
}

/// `aida worktree reclaim`.
// trace:TASK-1562 | ai:claude
pub fn run(opts: &ReclaimOptions) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let survey = survey_from_env(&cwd, opts)?;
    let plan = plan(&survey);

    if renders_json(opts.json, crate::output_format_override()) {
        return render_json(&plan, opts);
    }

    println!(
        "main checkout : {}   (its target/ is PROTECTED)",
        survey.main_root.display()
    );
    println!(
        "mode          : {}",
        if opts.apply {
            "APPLY — caches below will be deleted"
        } else {
            "dry run — pass --apply to delete"
        }
    );
    println!();

    for (path, skip) in &plan.skipped {
        if matches!(skip, Skip::NoTarget) {
            continue; // Nothing to say about a worktree with no cache.
        }
        println!("  skip  {}  — {}", path.display(), skip.reason());
    }

    if plan.reclaim.is_empty() {
        println!();
        println!("nothing to reclaim.");
        return Ok(());
    }

    println!();
    println!("{:>10}  {:<14}  TARGET", "SIZE", "WHY");
    for r in &plan.reclaim {
        println!(
            "{:>10}  {:<14}  {}",
            human_bytes(r.bytes),
            r.why.code(),
            r.target.display()
        );
    }
    println!();
    println!(
        "reclaimable: {} across {} target dir(s)",
        human_bytes(plan.reclaimable_bytes()),
        plan.reclaim.len()
    );

    if !opts.apply {
        println!("re-run with --apply to delete.");
        return Ok(());
    }

    let done = apply(&plan, opts.target_pct, disk_used_pct, |p| {
        std::fs::remove_dir_all(p)
    })?;
    println!();
    for (path, bytes) in &done.deleted {
        println!("deleted {:>10}  {}", human_bytes(*bytes), path.display());
    }
    for (path, err) in &done.failed {
        println!("FAILED  {}  — {err}", path.display());
    }
    if !done.stopped_early.is_empty() {
        println!(
            "stopped at --target-pct {}: {} cache(s) left warm",
            opts.target_pct.unwrap_or(0),
            done.stopped_early.len()
        );
    }
    println!(
        "freed {} across {} target dir(s)",
        human_bytes(done.freed_bytes),
        done.deleted.len()
    );
    Ok(())
}

/// `--json` projection. Reports reclaimable bytes whether or not `--apply`
/// was passed, so a caller can decide before acting.
// trace:TASK-1562 | ai:claude
fn render_json(plan: &Plan, opts: &ReclaimOptions) -> Result<()> {
    let candidates: Vec<serde_json::Value> = plan
        .reclaim
        .iter()
        .map(|r| {
            serde_json::json!({
                "target": r.target.display().to_string(),
                "branch": r.branch,
                "bytes": r.bytes,
                "why": r.why.code(),
            })
        })
        .collect();
    let skipped: Vec<serde_json::Value> = plan
        .skipped
        .iter()
        .filter(|(_, s)| !matches!(s, Skip::NoTarget))
        .map(|(p, s)| {
            serde_json::json!({
                "path": p.display().to_string(),
                "code": s.code(),
                "reason": s.reason(),
            })
        })
        .collect();

    let mut body = serde_json::json!({
        "main_root": plan.main_root.display().to_string(),
        "protected": protected_target(&plan.main_root).display().to_string(),
        "applied": opts.apply,
        "reclaimable_bytes": plan.reclaimable_bytes(),
        "candidates": candidates,
        "skipped": skipped,
    });

    if opts.apply {
        let done = apply(plan, opts.target_pct, disk_used_pct, |p| {
            std::fs::remove_dir_all(p)
        })?;
        body["freed_bytes"] = serde_json::json!(done.freed_bytes);
        body["deleted"] = serde_json::json!(done
            .deleted
            .iter()
            .map(|(p, _)| p.display().to_string())
            .collect::<Vec<_>>());
        body["failed"] = serde_json::json!(done
            .failed
            .iter()
            .map(|(p, e)| serde_json::json!({"path": p.display().to_string(), "error": e}))
            .collect::<Vec<_>>());
    }
    println!("{}", serde_json::to_string_pretty(&body)?);
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_1916_reclaim_tests.rs"]
mod story_1916_reclaim_tests;
