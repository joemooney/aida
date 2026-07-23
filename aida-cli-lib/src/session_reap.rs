//! Session reap — the substrate-state-driven cleanup pass for sessions that
//! have genuinely finished.
//!
//! A scoped implementer session cannot tear down its own worktree (it is the
//! live cwd — `aida session end` correctly refuses), so a headless agent that
//! exits when its work lands leaves an orphaned worktree + lease + branch
//! behind and every spec boundary needs a human. This pass closes that gap for
//! the unambiguous case.
//!
//! # The reapable predicate
//!
//! A session is reapable when ALL of the following hold:
//!   1. its spec is **Done or Completed** — the agent already reported "finished"
//!      through AIDA (`aida queue done`, a merge auto-bump, …);
//!   2. its branch is **merged** — either an ancestor of the default branch or a
//!      forge-reported merged PR (the squash case), with zero unique unmerged
//!      commits;
//!   3. its process has **EXITED** — proven from the lease's recorded pids plus
//!      the live-process probe, not guessed.
//!
//! # The hard boundaries
//!
//! * **No terminal scraping.** Completion is read from substrate state only —
//!   spec status, branch-merged-ness, process liveness. Inferring "done" from
//!   what a terminal printed is the same fragility class as acting on unreliable
//!   session-state signals, one layer up.
//! * **No force-kill.** A session whose process is still ALIVE is left entirely
//!   untouched — its worktree is its cwd, and closing a live interactive agent is
//!   the operator's job, never AIDA's. Detect-and-leave, never terminate.
//! * **The worktree-GC safety checks are reused verbatim.** The final gate is the
//!   very same `classify_agent_worktree` predicate `aida worktree gc` runs, so a
//!   dirty worktree or one carrying unique unmerged commits is never removed.
//!
//! Anything that is not provably reapable is SKIPPED with a reason — the pass
//! defaults to preserve.
//
// trace:TASK-1177 | ai:claude

use anyhow::Result;
use colored::Colorize;

use crate::doctor_cmd::{classify_agent_worktree, AgentWorktreeFacts, AgentWorktreeVerdict};
use crate::*;

/// The reap verdict for one session lease. `Reap` carries the reason the pass
/// judged it finished; `Skip` carries the strongest objection.
// trace:TASK-1177 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReapVerdict {
    /// Spec finished + branch merged + process exited + worktree safe → reap.
    Reap(String),
    /// At least one gate failed → leave everything exactly as it is.
    Skip(String),
}

impl ReapVerdict {
    fn reason(&self) -> &str {
        match self {
            ReapVerdict::Reap(r) | ReapVerdict::Skip(r) => r,
        }
    }
}

/// Pure inputs to the reapable predicate — every probe result gathered once per
/// lease by the scanner. Keeping the predicate pure (no git / store / process
/// probes) makes the whole matrix unit-testable without a repo, a store, or a
/// real process.
// trace:TASK-1177 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct ReapFacts {
    /// True when the lease's scope resolves to a spec whose status is Done or
    /// Completed. A scope that resolves to no spec (a generic
    /// `harness-worktree` lease, a PR-review scope, …) is NOT finished — the
    /// pass has no completion signal for it and leaves it alone.
    pub spec_finished: bool,
    /// True when the session's process is VERIFIABLY gone: every pid the lease
    /// recorded is absent from the process table AND no live agent process sits
    /// inside the worktree. Unknown liveness is false (absence of evidence is
    /// never evidence of death).
    pub process_exited: bool,
    /// True when `git worktree list --porcelain` reports a `locked` line for the
    /// worktree — operator-protected, never removed.
    pub locked: bool,
    /// The worktree-GC safety facts, fed to the very same classifier
    /// `aida worktree gc` uses for its dirty / merged / unique-commit gates.
    pub worktree: AgentWorktreeFacts,
}

/// The reapable predicate. Removal requires ALL of: clean worktree, unlocked,
/// process exited, spec finished, and the worktree-GC classifier's own
/// `Removable` verdict (positive merge signal + zero unique unmerged commits).
///
/// The order is deliberate — the reason names the strongest objection:
/// uncommitted work first (costliest to lose), then operator protection, then
/// the liveness boundary (never touch a running session), then the completion
/// signal, then the shared merge/unique-commit gate.
// trace:TASK-1177 | ai:claude
pub(crate) fn classify_session_reap(facts: &ReapFacts) -> ReapVerdict {
    if facts.worktree.dirty {
        return ReapVerdict::Skip("uncommitted changes present — never auto-removed".to_string());
    }
    if facts.locked {
        return ReapVerdict::Skip("worktree is locked — operator-protected".to_string());
    }
    // HARD BOUNDARY: a live session owns its worktree as its cwd. Leave it
    // running and leave its tree in place — AIDA never force-closes an agent.
    if !facts.process_exited {
        return ReapVerdict::Skip(
            "its process is still running — left untouched (never force-closed)".to_string(),
        );
    }
    if !facts.spec_finished {
        return ReapVerdict::Skip("its spec is not finished (needs Done or Completed)".to_string());
    }
    match classify_agent_worktree(&facts.worktree) {
        AgentWorktreeVerdict::Removable(reason) => {
            ReapVerdict::Reap(format!("spec finished, process exited, {reason}"))
        }
        AgentWorktreeVerdict::Keep(reason) => ReapVerdict::Skip(reason),
    }
}

/// Has the session that minted this lease genuinely exited?
///
/// Three independent substrate signals must agree, and every one of them is a
/// state read — none of them looks at what a terminal printed:
///   * `owner_gone` — the [`aida_core::liveness::lease_owner_process_gone`]
///     tri-state over the lease's recorded pids. `None` (no pid was ever
///     recorded) means liveness is *undeterminable*, so we refuse.
///   * `lease_state` — the same `● live / ⚠ STALE` verdict `aida ps` renders.
///   * `worktree_has_live_process` — a live agent process whose cwd sits in the
///     worktree, even if it holds no lease of its own.
///
/// Pure so the matrix is testable without spawning or killing a process.
// trace:TASK-1177 | ai:claude
pub(crate) fn session_process_exited(
    lease_state: LeaseState,
    owner_gone: Option<bool>,
    worktree_has_live_process: bool,
) -> bool {
    owner_gone == Some(true)
        && !matches!(lease_state, LeaseState::Live)
        && !worktree_has_live_process
}

/// Branches the pass will never delete, whatever else holds. A session lease
/// pointing at one of these is a bookkeeping oddity, not a disposable branch.
const PROTECTED_BRANCHES: &[&str] = &["main", "master", "aida-store", "HEAD"];

/// Is `branch` off-limits for deletion? Matches the protected set by name and
/// treats any ref carrying the store branch name (e.g. a mirror-tracking
/// `gitlab/aida-store`) as protected too.
// trace:TASK-1177 | ai:claude
fn branch_is_protected(branch: &str, default_ref: Option<&str>) -> bool {
    let branch = branch.trim();
    if branch.is_empty() {
        return false;
    }
    if PROTECTED_BRANCHES
        .iter()
        .any(|p| p.eq_ignore_ascii_case(branch))
    {
        return true;
    }
    if branch
        .rsplit('/')
        .next()
        .map(|tail| tail.eq_ignore_ascii_case("aida-store"))
        .unwrap_or(false)
    {
        return true;
    }
    default_ref
        .and_then(|r| r.rsplit('/').next())
        .map(|tail| tail.eq_ignore_ascii_case(branch))
        .unwrap_or(false)
}

/// One row of the reap report — a session the pass considered, with the verdict
/// it reached and (after execution) what actually happened to it.
// trace:TASK-1177 | ai:claude
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct ReapRow {
    /// Lease id (the session id `aida ps` / `aida session leases` print).
    pub session: String,
    /// The lease's scope — normally the spec id.
    pub scope: String,
    /// Worktree path, or empty for an advisory (worktree-less) lease.
    pub worktree: String,
    /// Branch the worktree is on, or empty.
    pub branch: String,
    /// `reap` or `skip`.
    pub verdict: &'static str,
    /// Why.
    pub reason: String,
    /// Whether the lease's scope resolved to a finished spec. Drives the human
    /// report's noise filter: a skipped session whose spec IS finished is a
    /// near-miss worth naming; one whose scope isn't even a finished spec is
    /// just an ordinary in-flight session and is summarized as a count.
    pub spec_finished: bool,
    /// What the execution leg did. `None` on a scan-only / dry run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

/// The machine-readable pass result.
// trace:TASK-1177 | ai:claude
#[derive(Debug, Clone, Default, serde::Serialize)]
pub(crate) struct ReapReport {
    pub reapable: Vec<ReapRow>,
    pub skipped: Vec<ReapRow>,
    /// CHAIN slice (TASK-1179): the next queued spec's display id, set only
    /// after the pass actually reaped ≥1 session AND a next spec is queued.
    /// Suggest-only — the operator runs the launch; the reap pass never spawns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_up: Option<String>,
}

/// Resolve every lease's scope to a spec status in ONE store open, so the pass
/// stays cheap even with a dozen leases. Returns an empty map when the project
/// has no distributed store (the legacy centralized layout is out of scope) —
/// with no completion signal available, nothing is finished and nothing reaps.
// trace:TASK-1177 | ai:claude
fn finished_scopes(project_root: &std::path::Path, scopes: &[String]) -> HashSet<String> {
    let mut out = HashSet::new();
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return out;
    };
    let Ok(dispenser) = load_dispenser(&store_path) else {
        return out;
    };
    let Ok(inner) = aida_core::GitBackend::new(&store_path).map(|b| b.with_dispenser(dispenser))
    else {
        return out;
    };
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let Ok(backend) = aida_core::CachedGitBackend::with_inner(inner, &cache_path) else {
        return out;
    };
    for scope in scopes {
        if scope.trim().is_empty() {
            continue;
        }
        if let Ok(Some(req)) = backend.get_requirement_by_spec_id(scope) {
            if matches!(
                req.status,
                RequirementStatus::Done | RequirementStatus::Completed
            ) {
                out.insert(scope.to_ascii_uppercase());
            }
        }
    }
    out
}

/// Gather the facts for every session lease and classify each. Read-only: git
/// ancestry probes, a forge merged-PR lookup for the squash case, a store read,
/// and the process-liveness probe. Nothing is mutated here.
// trace:TASK-1177 | ai:claude
pub(crate) fn scan_reapable(project_root: &std::path::Path) -> ReapReport {
    let leases = list_leases(project_root);
    let mut report = ReapReport::default();
    if leases.is_empty() {
        return report;
    }

    let default_ref = resolve_default_branch_ref(project_root);
    let live = process_probe::probe_live_claude_sessions();
    let active = active_worktree_paths(project_root);
    let now = chrono::Utc::now();
    let scopes: Vec<String> = leases.iter().map(|l| l.scope.clone()).collect();
    let finished = finished_scopes(project_root, &scopes);
    let project_canon = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());

    let git_count = |args: &[&str]| -> Option<u32> {
        std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .parse::<u32>()
                    .ok()
            })
    };

    for lease in &leases {
        let has_worktree = !lease.worktree_path.as_os_str().is_empty();
        // HARD FLOOR: the main checkout and the store worktree are never reap
        // targets, however the rest of the facts read. A lease whose worktree
        // IS the project root (an in-place harness lease) would otherwise put
        // the whole checkout in the blast radius.
        let is_project_or_store_tree = has_worktree
            && (lease
                .worktree_path
                .canonicalize()
                .unwrap_or_else(|_| lease.worktree_path.clone())
                == project_canon
                || lease.branch.trim().eq_ignore_ascii_case("aida-store"));
        // Same floor for the branch: a lease pointing at a protected ref is
        // bookkeeping, not a disposable session branch.
        let protected_branch = branch_is_protected(&lease.branch, default_ref.as_deref());
        if is_project_or_store_tree || protected_branch {
            report.skipped.push(ReapRow {
                session: lease.id.clone(),
                scope: lease.scope.clone(),
                worktree: lease.worktree_path.display().to_string(),
                branch: lease.branch.clone(),
                verdict: "skip",
                reason: "protected checkout/branch — never reaped".to_string(),
                spec_finished: finished.contains(&lease.scope.to_ascii_uppercase()),
                outcome: None,
            });
            continue;
        }

        let spec_finished = finished.contains(&lease.scope.to_ascii_uppercase());

        // Liveness — the shipped detection, not a second notion. An advisory
        // lease's worktree path is empty, so the in-worktree probe is skipped
        // for it (an empty path would otherwise resolve against the cwd).
        let worktree_has_live_process =
            has_worktree && worktree_is_active(&lease.worktree_path, &active);
        let owner_gone = aida_core::liveness::lease_owner_process_gone(
            lease.active_pid,
            lease.creator_pid,
            process_probe::pid_is_alive,
        );
        let process_exited = session_process_exited(
            lease_state_for(lease, &live, now),
            owner_gone,
            worktree_has_live_process,
        );

        // Merge facts. A lease with no worktree and no branch has nothing on
        // disk that could carry unmerged work, so it counts as merged.
        let dirty = has_worktree && !worktree_dirty_entries(&lease.worktree_path).is_empty();
        let branch = lease.branch.trim();
        let (ancestor_of_main, unique_unmerged_commits) = match (branch.is_empty(), &default_ref) {
            (true, _) => (true, 0),
            (false, Some(default_ref)) => {
                let n = git_count(&["rev-list", "--count", &format!("{default_ref}..{branch}")]);
                (n == Some(0), n.unwrap_or(0))
            }
            // Without a resolvable default ref merged-ness cannot be proven.
            (false, None) => (false, 0),
        };
        // Only pay for the forge lookup when the cheap ancestry probe was
        // inconclusive (the squash-merge case) AND everything else already
        // points at a reap — a skip does not need the network call.
        let pr_merged = if !ancestor_of_main && !dirty && spec_finished && process_exited {
            matches!(
                detect_merged_pr_for_branch_via_forge(project_root, branch),
                PrLookup::Found(_)
            )
        } else {
            false
        };

        let facts = ReapFacts {
            spec_finished,
            process_exited,
            locked: has_worktree && worktree_is_locked(project_root, &lease.worktree_path),
            worktree: AgentWorktreeFacts {
                dirty,
                ancestor_of_main,
                pr_merged,
                unique_unmerged_commits,
            },
        };

        let verdict = classify_session_reap(&facts);
        let row = ReapRow {
            session: lease.id.clone(),
            scope: lease.scope.clone(),
            worktree: lease.worktree_path.display().to_string(),
            branch: lease.branch.clone(),
            verdict: match verdict {
                ReapVerdict::Reap(_) => "reap",
                ReapVerdict::Skip(_) => "skip",
            },
            reason: verdict.reason().to_string(),
            spec_finished,
            outcome: None,
        };
        match verdict {
            ReapVerdict::Reap(_) => report.reapable.push(row),
            ReapVerdict::Skip(_) => report.skipped.push(row),
        }
    }

    report.reapable.sort_by(|a, b| a.scope.cmp(&b.scope));
    report.skipped.sort_by(|a, b| a.scope.cmp(&b.scope));
    report
}

/// Execute the reap for ONE session: remove the worktree, delete the lease (and
/// its activity log / manifest companions), then delete the now-unused local
/// branch. Returns the human-readable outcome.
///
/// Re-checks dirtiness immediately before removal — anything that appeared since
/// the scan is salvaged to a patch and the worktree is left in place. The
/// removal itself goes through the shared teardown (pre-destroy cargo-clean hook
/// + worktree-pool deregistration) that the worktree-GC heal uses.
// trace:TASK-1177 | ai:claude
fn reap_one(project_root: &std::path::Path, lease: &SessionLease) -> String {
    let has_worktree = !lease.worktree_path.as_os_str().is_empty();

    if has_worktree && lease.worktree_path.exists() {
        // Never destroy work that appeared between scan and reap.
        if !worktree_dirty_entries(&lease.worktree_path).is_empty() {
            let salvage =
                salvage_worktree_patch(project_root, &lease.scope, None, &lease.worktree_path)
                    .ok()
                    .flatten();
            return format!(
                "skipped — worktree became dirty since the scan{}",
                salvage
                    .map(|p| format!(" (salvage patch: {})", p.display()))
                    .unwrap_or_default()
            );
        }
        if aida_core::worktree_pool_destroy::teardown_worktree_path(
            project_root,
            &lease.worktree_path,
            &worktree_pool_global_hooks("pre_destroy"),
        )
        .is_err()
        {
            return format!(
                "failed — could not remove worktree {}",
                lease.worktree_path.display()
            );
        }
    }

    // Lease + companions. Reuses the same aggregation-then-delete ordering
    // `aida session end` uses so the session's activity is folded into the
    // project's role activity before its log goes away.
    aggregate_session_activity_into_roles(project_root, &lease.id);
    let _ = std::fs::remove_file(lease_path(project_root, &lease.id));
    let activity = session_activity_path(project_root, &lease.id);
    if activity.exists() {
        let _ = std::fs::remove_file(&activity);
    }
    let manifest = session_manifest::manifest_path(project_root, &lease.id);
    if manifest.exists() {
        let _ = std::fs::remove_file(&manifest);
    }

    // Branch cleanup. `-D` because a squash-merged branch is not recognized as
    // merged by `-d`, and the scan already verified the work shipped. The
    // protected-ref floor is re-asserted here so the destructive call can never
    // be reached by a future caller that skipped the scan's guard.
    let branch = lease.branch.trim();
    if branch.is_empty()
        || branch_is_protected(branch, resolve_default_branch_ref(project_root).as_deref())
    {
        return "reaped — lease released".to_string();
    }
    let deleted = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["branch", "-D", branch])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if deleted {
        format!("reaped — worktree removed, lease released, branch `{branch}` deleted")
    } else {
        format!("reaped — worktree removed, lease released (branch `{branch}` already gone)")
    }
}

/// Options for one reap pass.
// trace:TASK-1177 | ai:claude
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ReapOptions {
    /// Report only — nothing is touched.
    pub dry_run: bool,
    /// Skip the confirmation prompt.
    pub yes: bool,
    /// Emit the machine-readable report instead of the human one.
    pub json: bool,
    /// Print nothing when there is nothing to reap (for the post-merge hook,
    /// which should stay quiet on the common no-op).
    pub quiet_when_empty: bool,
}

// CHAIN slice (FR-284 child, per ADR-24): after a reap actually removes a
// finished session, SUGGEST the next-spec handoff — the exact command(s) the
// operator would run to launch the next queued spec in a fresh worktree. The
// first increment is detect-and-SUGGEST only: the reap pass never auto-spawns a
// process. Auto-launch behind a `--chain` flag and a config-gated
// off/suggest/launch policy are later increments once the suggest form is
// proven. This mirrors the parent FR-284's "detect-and-notify, never terminate"
// philosophy on the launch side: detect-and-suggest, never auto-launch.
// trace:TASK-1179 | ai:claude

/// Format the "Next up" handoff block naming the exact launch commands for
/// `next_spec` in a fresh worktree. Pure so the suggest-block emission is
/// unit-testable without a queue or a process. The spec id IS the operand the
/// operator must type, so it is intentionally part of this command hint (like
/// `aida queue next` / `aida ps`), not opaque noise.
// trace:TASK-1179 | ai:claude
pub(crate) fn format_next_up_suggestion(next_spec: &str) -> String {
    format!(
        "\nNext up: {spec} is queued. Launch it in a fresh worktree:\n    \
         aida worktree enter {spec}          # take the lease + cd into a ready worktree, or\n    \
         aida agent new claude --spec {spec}   # launch an agent on it\n  \
         (suggestion only — no session was started.)",
        spec = next_spec
    )
}

/// Decide the handoff suggestion after a reap pass. Suggest-only (ADR-24):
/// returns the "Next up" block ONLY when the pass actually reaped ≥1 session
/// AND a next spec is queued — otherwise `None`, so nothing extra is printed.
/// Pure so all three cases (emit / nothing-reaped / empty-queue) are testable
/// without a store or a live process.
// trace:TASK-1179 | ai:claude
pub(crate) fn next_up_suggestion(reaped_count: usize, next_spec: Option<&str>) -> Option<String> {
    match (reaped_count, next_spec) {
        // Nothing was reaped, or the queue holds no next spec → no handoff.
        (0, _) | (_, None) => None,
        (_, Some(spec)) => Some(format_next_up_suggestion(spec)),
    }
}

/// Resolve the next queued spec the operator would launch after a reap — the
/// drivable head of the (active role's) queue in the same pickup order
/// `aida queue next` uses. Reuses the existing head resolver rather than
/// reinventing it; returns `None` on any read failure or an empty/undrivable
/// queue (a suggestion is best-effort and must never fail the reap pass).
// trace:TASK-1179 | ai:claude
fn resolve_next_queued_spec(project_root: &std::path::Path) -> Option<String> {
    let storage = Storage::new(project_root.join(".aida-store"));
    let user_id = current_user_id(None);
    let candidates = crate::queue_cmd::auto_complete_head_candidates(&storage, &user_id).ok()?;
    crate::queue_cmd::pick_auto_complete_head(&candidates)
        .ok()
        .map(|(spec, _skipped)| spec)
}

/// `aida session reap` — the supervisor pass. Scans every session lease,
/// reports the verdict for each, and reaps the ones the predicate proved
/// finished. Never prompts when there is nothing to confirm; without `--yes`
/// outside a TTY it reports and stops rather than blocking on a prompt nobody
/// can answer.
// trace:TASK-1177 | ai:claude
pub(crate) fn run_session_reap(opts: ReapOptions) -> Result<()> {
    let project_root = main_worktree_root_from(&find_project_root()?);
    let mut report = scan_reapable(&project_root);

    if report.reapable.is_empty() && report.skipped.is_empty() {
        if opts.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else if !opts.quiet_when_empty {
            println!("No session leases found — nothing to reap.");
        }
        return Ok(());
    }

    // The post-merge hook asks for silence on the common no-op, so a pass that
    // found nothing to reap prints nothing at all for it.
    let stay_silent = opts.quiet_when_empty && report.reapable.is_empty();
    if !opts.json && !stay_silent {
        // Only the NEAR-MISSES are named: a session whose spec is finished but
        // that something else held back is worth a line. An ordinary in-flight
        // session is not news, so it is summarized as a count.
        let (near_miss, routine): (Vec<&ReapRow>, Vec<&ReapRow>) =
            report.skipped.iter().partition(|r| r.spec_finished);
        if !near_miss.is_empty() {
            println!("Left in place ({}):", near_miss.len());
            for row in &near_miss {
                println!(
                    "  {} {} — {}",
                    "keep".yellow(),
                    row.scope.cyan(),
                    row.reason
                );
            }
        }
        if !routine.is_empty() {
            println!(
                "  {}",
                format!(
                    "({} session(s) whose spec is not finished — not shown)",
                    routine.len()
                )
                .dimmed()
            );
        }
        if report.reapable.is_empty() {
            println!("No session is reapable (finished + merged + process exited).");
        } else {
            println!(
                "{} finished session(s) are reapable (spec finished + branch merged + process exited):",
                report.reapable.len()
            );
            for row in &report.reapable {
                println!("  {} {} — {}", "reap".cyan(), row.scope.cyan(), row.reason);
            }
        }
    }

    if report.reapable.is_empty() {
        if opts.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        return Ok(());
    }

    if opts.dry_run {
        if opts.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            println!("\n--dry-run: nothing was reaped.");
        }
        return Ok(());
    }

    if !opts.yes {
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            if opts.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("\nRe-run with --yes to reap them.");
            }
            return Ok(());
        }
        use std::io::Write;
        eprint!(
            "\nReap the {} finished session(s)? [y/N] ",
            report.reapable.len()
        );
        std::io::stderr().flush()?;
        let mut ans = String::new();
        std::io::stdin().read_line(&mut ans)?;
        if !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted — nothing reaped.");
            return Ok(());
        }
    }

    // Re-read the leases so we act on the on-disk record, not the scan copy.
    let leases = list_leases(&project_root);
    for row in &mut report.reapable {
        let Some(lease) = leases.iter().find(|l| l.id == row.session) else {
            row.outcome = Some("skipped — lease already gone".to_string());
            continue;
        };
        let outcome = reap_one(&project_root, lease);
        if !opts.json {
            let marker = if outcome.starts_with("reaped") {
                crate::glyph(crate::glyphs::Glyph::Check)
                    .green()
                    .to_string()
            } else {
                "skip".yellow().to_string()
            };
            println!("  {marker} {} — {outcome}", row.scope.cyan());
        }
        row.outcome = Some(outcome);
    }

    // CHAIN slice (TASK-1179): a reap actually finished a session, so the next
    // spec boundary is open. If there is a next queued spec, SUGGEST the launch
    // — never spawn it. Counts only rows that genuinely reaped (a lease that
    // vanished mid-pass, or a worktree that turned dirty, did not).
    let reaped_count = report
        .reapable
        .iter()
        .filter(|r| {
            r.outcome
                .as_deref()
                .map(|o| o.starts_with("reaped"))
                .unwrap_or(false)
        })
        .count();
    let next_spec = if reaped_count >= 1 {
        resolve_next_queued_spec(&project_root)
    } else {
        None
    };
    if let Some(block) = next_up_suggestion(reaped_count, next_spec.as_deref()) {
        report.next_up = next_spec;
        if !opts.json {
            println!("{block}");
        }
    }

    if opts.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}

// The reapable-predicate matrix + the process-exited derivation.
// trace:TASK-1177 | ai:claude
#[cfg(test)]
#[path = "tests/task_1177_session_reap_tests.rs"]
mod task_1177_session_reap_tests;

// The CHAIN-slice suggest-block emission + no-op cases.
// trace:TASK-1179 | ai:claude
#[cfg(test)]
#[path = "tests/task_1179_chain_suggest_tests.rs"]
mod task_1179_chain_suggest_tests;
