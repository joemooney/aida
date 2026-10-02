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
// trace:TASK-1562 | ai:claude

use std::collections::BTreeSet;
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

/// A `target/` that passed every rail.
// trace:TASK-1562 | ai:claude
#[derive(Debug, Clone)]
pub struct Reclaimable {
    pub target: PathBuf,
    pub branch: Option<String>,
    pub bytes: u64,
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
}

/// The classification result: what would be deleted, largest first, and every
/// path that was held back with its reason.
// trace:TASK-1562 | ai:claude
#[derive(Debug, Clone)]
pub struct Plan {
    pub main_root: PathBuf,
    pub reclaim: Vec<Reclaimable>,
    pub skipped: Vec<(PathBuf, Skip)>,
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

/// Has this `target/` been touched within `min_age_mins`?
///
/// Reads the directory's own mtime, which cargo bumps as it writes — the same
/// signal the reference script used via `find -mmin`.
// trace:TASK-1562 | ai:claude
fn touched_within(target: &Path, min_age: Duration, now: SystemTime) -> bool {
    let Ok(md) = std::fs::metadata(target) else {
        return false;
    };
    let Ok(mtime) = md.modified() else {
        return false;
    };
    match now.duration_since(mtime) {
        // Younger than the floor, or an mtime in the future (clock skew), both
        // read as "possibly in flight" — the conservative arm.
        Ok(age) => age < min_age,
        Err(_) => true,
    }
}

/// Classify every worktree against the six rails. Deletes nothing.
///
/// The returned `reclaim` list is ordered largest-first, so a `--target-pct`
/// run frees the most space per deletion; `skipped` preserves the walk order so
/// the printed report reads like the worktree list.
// trace:TASK-1562 | ai:claude
pub fn plan(survey: &Survey) -> Plan {
    let protected = protected_target(&survey.main_root);
    let min_age = Duration::from_secs(survey.min_age_mins.saturating_mul(60));

    // Branch -> open PR, as a set for the rail-5 test. `None` is the
    // fail-safe arm and is handled separately below.
    let open_prs: Option<BTreeSet<&str>> = survey
        .open_pr_branches
        .as_ref()
        .map(|b| b.iter().map(String::as_str).collect());

    let live: Vec<&LeaseHold> = survey.leases.iter().filter(|l| l.alive).collect();

    let mut reclaim = Vec::new();
    let mut skipped = Vec::new();

    for wt in &survey.worktrees {
        let target = wt.path.join("target");

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
        if target == protected || wt.path == survey.main_root {
            skipped.push((wt.path.clone(), Skip::MainCheckout));
            continue;
        }
        // Rail 3.
        if wt.path.file_name().is_some_and(|n| n == ".aida-store") {
            skipped.push((wt.path.clone(), Skip::Store));
            continue;
        }
        if !target.is_dir() {
            skipped.push((wt.path.clone(), Skip::NoTarget));
            continue;
        }
        // Rail 4.
        if !survey.include_live {
            if let Some(hold) = live.iter().find(|l| l.worktree_path == wt.path) {
                skipped.push((wt.path.clone(), Skip::LiveSession { pid: hold.pid }));
                continue;
            }
        }
        // Rail 5, including its fail-safe arm: when open-PR state is
        // unknown we hold EVERY branch back rather than reclaiming
        // everything. The reference shell script had this inverted.
        if !survey.include_open_prs {
            match (&open_prs, &wt.branch) {
                (None, _) => {
                    skipped.push((wt.path.clone(), Skip::PrStateUnknown));
                    continue;
                }
                (Some(set), Some(branch)) if set.contains(branch.as_str()) => {
                    skipped.push((
                        wt.path.clone(),
                        Skip::OpenPr {
                            branch: branch.clone(),
                        },
                    ));
                    continue;
                }
                _ => {}
            }
        }
        // Rail 6.
        if touched_within(&target, min_age, survey.now) {
            skipped.push((
                wt.path.clone(),
                Skip::RecentlyTouched {
                    mins: survey.min_age_mins,
                },
            ));
            continue;
        }

        reclaim.push(Reclaimable {
            bytes: dir_size_bytes(&target),
            target,
            branch: wt.branch.clone(),
        });
    }

    reclaim.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.target.cmp(&b.target)));

    Plan {
        main_root: survey.main_root.clone(),
        reclaim,
        skipped,
    }
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
    let out = std::process::Command::new("git")
        .current_dir(dir)
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

/// Branches with an open PR, via `gh`. `None` when `gh` is missing or the call
/// fails — the fail-safe arm of rail 5.
// trace:TASK-1562 | ai:claude
fn open_pr_branches(dir: &Path) -> Option<Vec<String>> {
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
            "headRefName",
            "--jq",
            ".[].headRefName",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
    )
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
    Ok(Survey {
        worktrees: parse_worktree_entries(&porcelain),
        open_pr_branches: if opts.include_open_prs {
            // Not consulted when the rail is waived; skip the network call.
            Some(Vec::new())
        } else {
            open_pr_branches(&main_root)
        },
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
    println!("{:>10}  TARGET", "SIZE");
    for r in &plan.reclaim {
        println!("{:>10}  {}", human_bytes(r.bytes), r.target.display());
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
