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

use crate::doctor_cmd::{
    branch_content_fully_landed, branch_paths_match_default, classify_agent_worktree,
    AgentWorktreeFacts, AgentWorktreeVerdict,
};
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
    /// BUG-2206: true for an attested-reviewer lease (scope `PR-N`) whose PR
    /// the forge reports MERGED or CLOSED, and whose review branch holds no
    /// commit past the recorded PR head (TASK-2095). A PR scope has no spec, so this is
    /// its completion signal; the review branch is a local copy of the PR
    /// head, so the merge gate does not apply to it.
    // trace:TASK-1955 | ai:claude (BUG-2206)
    pub review_pr_terminal: bool,
    /// True when the session's process is gone: every recorded pid is absent
    /// from the process table, or a normal worktree lease is Dormant with no
    /// recorded pid and no live agent process sits inside the worktree.
    pub process_exited: bool,
    /// True when `git worktree list --porcelain` reports a `locked` line for the
    /// worktree — operator-protected, never removed.
    pub locked: bool,
    /// True when the session has no worktree, or its worktree's HEAD is
    /// checked out on the lease's branch. A detached HEAD or a different
    /// branch means the merge facts (probed on the lease branch) say nothing
    /// about the commits the worktree actually holds — keep it.
    // trace:BUG-1657 | ai:claude
    pub head_on_branch: bool,
    /// `Some(reason)` when the lease branch is checked out in a registered
    /// worktree other than the lease's own (e.g. the worktree was moved and
    /// the lease path is stale), or the worktree listing cannot be read. The
    /// branch delete would orphan that live checkout — keep the session.
    // trace:BUG-1657 | ai:claude
    pub branch_checked_out_elsewhere: Option<String>,
    /// The worktree-GC safety facts, fed to the very same classifier
    /// `aida worktree gc` uses for its dirty / merged / unique-commit gates.
    pub worktree: AgentWorktreeFacts,
}

/// Should this finished-but-still-live session be NOTIFIED that it is safe to
/// exit? True in exactly the interactive near-miss the NOTIFY slice speaks to:
/// a session that would be reapable *were its process not still running* —
/// clean, unlocked, spec finished, branch merged (the shared classifier's
/// `Removable` verdict) — but whose process is ALIVE, so the reap pass correctly
/// leaves it in place (it owns its worktree as its cwd; AIDA never force-closes
/// it). The message tells the human/agent at that prompt "your spec merged, you
/// are free to exit; the worktree reaps on the next pass once you do."
///
/// Pure over the same pre-gathered facts as [`classify_session_reap`], for the
/// same reason: the whole matrix is unit-testable without a repo or a process.
/// The scan gates this on the session actually holding a worktree (an advisory,
/// worktree-less lease has nothing to exit for).
// trace:FR-284 | ai:claude
pub(crate) fn session_should_notify(facts: &ReapFacts) -> bool {
    !facts.worktree.dirty
        && !facts.locked
        && facts.head_on_branch
        && facts.branch_checked_out_elsewhere.is_none()
        && facts.spec_finished
        // The load-bearing distinction from a reap: the process is STILL ALIVE.
        && !facts.process_exited
        && matches!(
            classify_agent_worktree(&facts.worktree),
            AgentWorktreeVerdict::Removable(_)
        )
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
    // trace:BUG-1657 | ai:claude
    if !facts.head_on_branch {
        return ReapVerdict::Skip(
            "worktree is not checked out on its session branch (detached or switched) — \
             operator decision"
                .to_string(),
        );
    }
    // trace:BUG-1657 | ai:claude
    if let Some(reason) = &facts.branch_checked_out_elsewhere {
        return ReapVerdict::Skip(reason.clone());
    }
    // HARD BOUNDARY: a live session owns its worktree as its cwd. Leave it
    // running and leave its tree in place — AIDA never force-closes an agent.
    if !facts.process_exited {
        return ReapVerdict::Skip(
            "its process is still running — left untouched (never force-closed)".to_string(),
        );
    }
    // trace:TASK-1955 | ai:claude (BUG-2206)
    if facts.review_pr_terminal {
        return ReapVerdict::Reap("reviewed PR is merged or closed, process exited".to_string());
    }
    if !facts.spec_finished {
        return ReapVerdict::Skip("its spec is not finished (needs Done or Completed)".to_string());
    }
    match classify_agent_worktree(&facts.worktree) {
        AgentWorktreeVerdict::Removable(reason) => {
            ReapVerdict::Reap(format!("spec finished, process exited, {reason}"))
        }
        // trace:BUG-1719 | ai:codex
        AgentWorktreeVerdict::Keep { reason, .. } => ReapVerdict::Skip(reason),
    }
}

/// Has the session that minted this lease genuinely exited?
///
/// Three independent substrate signals must agree, and every one of them is a
/// state read — none of them looks at what a terminal printed:
///   * `owner_gone` — the [`aida_core::liveness::lease_owner_process_gone`]
///     tri-state over the lease's recorded pids. `None` (no pid was ever
///     recorded) normally means liveness is *undeterminable*, except for a
///     Dormant worktree lease where the shared session/status view has already
///     found no live process backing it.
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
    // BUG-1121: a Dormant normal worktree lease with no recorded pid is the
    // same no-live-process state `aida status <spec>` reports as safe to clear.
    // There is no pid to force-close, so reap may proceed once the worktree and
    // merge gates also pass.
    if lease_state == LeaseState::Dormant && owner_gone.is_none() && !worktree_has_live_process {
        return true;
    }
    owner_gone == Some(true)
        && !matches!(lease_state, LeaseState::Live)
        && !worktree_has_live_process
}

/// What the pass may do with a lease held on a protected checkout or branch.
// trace:BUG-2197 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProtectedLeaseVerdict {
    /// Every recorded pid is provably dead: release the lease file only.
    Release(String),
    /// No pid recorded: unknown owner. Release only on explicit request.
    UnknownOwner(String),
    /// Leave the lease exactly as it is.
    Keep(String),
}

/// Classify a lease on the protected floor. The checkout itself is never a
/// removal target, so the only question is whether the LEASE may go, and the
/// only evidence accepted is the lease's own recorded pids: whoever happens to
/// be running inside the main checkout (including the inspecting process) says
/// nothing about who owns the lease. Pids recorded on another host cannot be
/// probed here, and a worktree-less advisory lease (`aida claim` /
/// `aida review`) keeps its own lifecycle.
// trace:BUG-2197 | ai:claude
pub(crate) fn classify_protected_lease(
    has_worktree: bool,
    same_host: bool,
    owner: aida_core::liveness::RecordedLeaseOwner,
) -> ProtectedLeaseVerdict {
    use aida_core::liveness::RecordedLeaseOwner;
    const FLOOR: &str = "protected checkout/branch — never reaped";
    if !has_worktree {
        return ProtectedLeaseVerdict::Keep(FLOOR.to_string());
    }
    if !same_host {
        return ProtectedLeaseVerdict::Keep(format!(
            "{FLOOR}; lease was taken on another host, its pids cannot be checked here"
        ));
    }
    match owner {
        RecordedLeaseOwner::Dead => ProtectedLeaseVerdict::Release(
            "protected checkout — its recorded agent process is dead; lease released, checkout untouched"
                .to_string(),
        ),
        RecordedLeaseOwner::Unrecorded => ProtectedLeaseVerdict::UnknownOwner(
            "unknown owner — lease records no durable owner pid (neither live nor dead); \
             release with --release-unknown-owner"
                .to_string(),
        ),
        RecordedLeaseOwner::NotProvablyDead => ProtectedLeaseVerdict::Keep(format!(
            "{FLOOR}; a recorded pid is alive or cannot be proven dead"
        )),
    }
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

/// BUG-2143: the branch the reap reasons about for `lease`. A lease that holds
/// no worktree owns no branch: a claim/review lease that recorded the primary
/// checkout's protected branch (`main`) has nothing on disk to protect, so it
/// is treated as branchless — reapable once its spec is finished and its
/// process is gone — and the reap never touches that protected ref.
// trace:BUG-2143 | ai:claude
pub(crate) fn reap_branch<'a>(lease: &'a SessionLease, default_ref: Option<&str>) -> &'a str {
    let branch = lease.branch.trim();
    if lease.worktree_path.as_os_str().is_empty() && branch_is_protected(branch, default_ref) {
        ""
    } else {
        branch
    }
}

/// BUG-2143: the lease as the reap reasons about it. An ADVISORY lease (`aida
/// claim` / `aida review`) whose recorded path is blank or IS the project root
/// (`aida claim X --worktree .`) owns no tree: it is viewed as worktree-less, so
/// a dead claim on a finished spec is reapable (the lease is ended) and the
/// reap never touches the primary checkout.
// trace:BUG-2143 | ai:claude
pub(crate) fn reap_view(lease: &SessionLease, project_root: &std::path::Path) -> SessionLease {
    let mut view = lease.clone();
    if (view.claim_verb || view.review_verb) && !view.worktree_path.as_os_str().is_empty() {
        let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        if view.worktree_path.to_string_lossy().trim().is_empty()
            || canon(&view.worktree_path) == canon(project_root)
        {
            view.worktree_path = std::path::PathBuf::new();
        }
    }
    view
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
    /// The branch tip commit the merge and content proofs were run against.
    /// The reap deletes the branch only while it still points here.
    // trace:BUG-1657 | ai:claude
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_tip: Option<String>,
    /// What the execution leg did. `None` on a scan-only / dry run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

/// One finished-but-live session the pass will NOTIFY (FR-284 NOTIFY slice). A
/// session in this list is also present in `skipped` (its process is alive, so
/// it is not reapable) — this is the subset worth telling "you are free to exit".
// trace:FR-284 | ai:claude
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct NotifyRow {
    /// Lease id (the session id `aida ps` prints).
    pub session: String,
    /// The lease's scope — the finished spec.
    pub scope: String,
    /// Lease owner (git identity captured at session-start), recorded for the
    /// report; the mailbox recipient is resolved at write time.
    pub owner: String,
    /// Worktree the live session sits in.
    pub worktree: String,
    /// Branch the worktree is on.
    pub branch: String,
    /// What the execution leg did — `Some("notified …")` / `Some("already
    /// notified …")`, or `None` on a scan-only / dry run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

/// The machine-readable pass result.
// trace:TASK-1177 | ai:claude
#[derive(Debug, Clone, Default, serde::Serialize)]
pub(crate) struct ReapReport {
    /// Spec-linked branches ahead of main with no open PR and no live lease.
    /// Reported alongside reap state so `aida session reap` never removes or
    /// ignores work that still needs a PR/recovery drive.
    // trace:STORY-1043 | ai:codex
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unshipped_work: Vec<awaiting_you::UnshippedWorkItem>,
    pub reapable: Vec<ReapRow>,
    pub skipped: Vec<ReapRow>,
    /// Finished sessions whose process is still alive — left in place, but told
    /// (once) via a mailbox FYI that their spec merged and they may exit.
    // trace:FR-284 | ai:claude
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notifiable: Vec<NotifyRow>,
    /// Leases on a protected checkout (the main checkout, the store tree, a
    /// protected branch) whose recorded pids are all provably dead. Never
    /// reaped: only the lease file is released, the checkout is untouched.
    // trace:BUG-2197 | ai:claude
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub releasable: Vec<ReapRow>,
    /// Leases on a protected checkout with no durable owner: unknown owner,
    /// neither live nor dead. Released only with `--release-unknown-owner`.
    // trace:BUG-2197 | ai:claude
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown_owner: Vec<ReapRow>,
    /// CHAIN slice (TASK-1179): the next queued spec's display id, set only
    /// after the pass actually reaped ≥1 session AND a next spec is queued.
    /// Suggest-only — the operator runs the launch; the reap pass never spawns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_up: Option<String>,
    /// Seat-clone base pins in main that no `clone` lease accounts for. A pin
    /// whose clone directory is gone is released on a real pass; one whose
    /// clone still exists is reported and kept for the operator.
    // trace:TASK-2053 | ai:claude
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orphan_pins: Vec<crate::seat_clone::OrphanPin>,
}

/// Resolve every lease's scope to a spec status in ONE store open, so the pass
/// stays cheap even with a dozen leases. Returns an empty map when the project
/// has no distributed store (the legacy centralized layout is out of scope) —
/// with no completion signal available, nothing is finished and nothing reaps.
// trace:TASK-1177 | ai:claude
// Reads each scope from its stored YAML object (`get_requirement_by_spec_id`),
// never from the cache rows, so a reopened spec is never counted finished even
// when the cache is stale. trace:BUG-1670 | ai:claude
pub(crate) fn finished_scopes(
    project_root: &std::path::Path,
    scopes: &[String],
) -> HashSet<String> {
    scopes_with_status(project_root, scopes, |status| {
        matches!(
            status,
            RequirementStatus::Done | RequirementStatus::Completed
        )
    })
}

/// [`finished_scopes`] for any status predicate: the upper-cased scopes whose
/// stored spec object has a status `keep` accepts, in ONE store open.
// trace:STORY-1688 | ai:claude
pub(crate) fn scopes_with_status(
    project_root: &std::path::Path,
    scopes: &[String],
    keep: impl Fn(&RequirementStatus) -> bool,
) -> HashSet<String> {
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
            if keep(&req.status) {
                out.insert(scope.to_ascii_uppercase());
            }
        }
    }
    out
}

// trace:STORY-1043 | ai:codex
fn requirement_summaries(project_root: &std::path::Path) -> Vec<aida_core::RequirementSummary> {
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return Vec::new();
    };
    let Ok(dispenser) = load_dispenser(&store_path) else {
        return Vec::new();
    };
    let Ok(inner) = aida_core::GitBackend::new(&store_path).map(|b| b.with_dispenser(dispenser))
    else {
        return Vec::new();
    };
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let Ok(backend) = aida_core::CachedGitBackend::with_inner(inner, &cache_path) else {
        return Vec::new();
    };
    backend
        .list_summaries(&aida_core::ListFilter::default())
        .unwrap_or_default()
}

/// Does `default_ref` carry a landed commit that names `spec` as delivered,
/// since `branch` forked from it? This is the batched-integration merge signal:
/// an integration PR squash-merges several specs' work into one commit whose
/// body lists one line per spec ending in `(SPEC-ID)`, so the spec's own
/// branch is never an ancestor of the default branch and never has a PR of its
/// own. Recognition reuses the merge-time completion parsers (subject trailer
/// plus body-line trailers) rather than a second grammar, and skips plan
/// commits, whose trailer names what a plan is FOR, not what shipped.
///
/// The search is bounded to commits after the branch's merge-base — a landing
/// commit for this branch's work cannot predate the fork. Returns the first
/// such commit — the landing point recency checks use — and `None` (no signal)
/// when there is none or on any git failure. This is a merge SIGNAL only;
/// content safety is proven separately before anything is removed.
// trace:BUG-1657 | ai:claude
// trace:BUG-1718 | ai:codex
// trace:TASK-1581 | ai:antigravity — absorbed the unused bool wrapper.
pub(crate) fn spec_landing_commit(
    project_root: &std::path::Path,
    default_ref: &str,
    branch: &str,
    spec: &str,
) -> Option<String> {
    let spec = spec.trim();
    if spec.is_empty() || branch.trim().is_empty() {
        return None;
    }
    let run = |args: &[&str]| -> Option<String> {
        std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    let Some(merge_base) = run(&[
        "merge-base",
        crate::git_arg_guard::END_OF_OPTIONS,
        default_ref,
        branch,
    ]) else {
        return None;
    };
    let merge_base = merge_base.trim();
    if merge_base.is_empty() {
        return None;
    }
    // `--grep` is a cheap literal pre-filter (it also matches longer ids that
    // share the prefix); the parsers below make the exact decision.
    let Some(log) = run(&[
        "log",
        "--format=%H%x00%B%x00",
        "--fixed-strings",
        "--regexp-ignore-case",
        &format!("--grep={spec}"),
        &format!("{merge_base}..{default_ref}"),
    ]) else {
        return None;
    };
    let mut commits = log.split('\0');
    let mut matches = Vec::new();
    while let (Some(sha), Some(message)) = (commits.next(), commits.next()) {
        let sha = sha.trim();
        let message = message.trim();
        if sha.is_empty()
            || message.is_empty()
            || is_plan_commit_subject(message.lines().next().unwrap_or(""))
        {
            continue;
        }
        if extract_spec_ids_from_commit(message)
            .into_iter()
            .chain(extract_referenced_spec_ids_from_commit(message))
            .any(|id| id.eq_ignore_ascii_case(spec))
        {
            matches.push(sha.to_string());
        }
    }
    // `git log` is newest-first; the landing is the oldest qualifying commit.
    matches.pop()
}

/// Is `worktree`'s HEAD a symbolic ref to `refs/heads/<branch>`? A detached
/// HEAD, another branch, an empty lease branch, or any git failure → `false`.
// trace:BUG-1657 | ai:claude
pub(crate) fn worktree_head_on_branch(worktree: &std::path::Path, branch: &str) -> bool {
    let branch = branch.trim();
    if branch.is_empty() {
        return false;
    }
    std::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["symbolic-ref", "--quiet", "HEAD"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| {
            String::from_utf8_lossy(&o.stdout).trim() == format!("refs/heads/{branch}")
        })
}

/// Does `git status` fail on a worktree directory that exists? A missing
/// directory is not unreadable (there is nothing left to lose in it).
// trace:BUG-1657 | ai:claude
pub(crate) fn worktree_status_unreadable(worktree: &std::path::Path) -> bool {
    if worktree.as_os_str().is_empty() || !worktree.exists() {
        return false;
    }
    // trace:BUG-1985 | ai:claude
    !aida_core::git_ops::host_git_command(worktree)
        .args(["status", "--porcelain"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The scan's dirty fact, failing closed: uncommitted entries, OR a status
/// that cannot be read on a directory that exists (unknown counts as dirty,
/// because the reap removes the worktree with force).
// trace:BUG-1657 | ai:claude
pub(crate) fn worktree_status_dirty(worktree: &std::path::Path) -> bool {
    worktree_status_unreadable(worktree) || !worktree_dirty_entries(worktree).is_empty()
}

/// Is `branch` checked out in a registered worktree OTHER than `lease_path`
/// whose directory exists? Reads `git worktree list --porcelain`. Returns the
/// keep reason, or `None` when no other live checkout holds the branch. A
/// listing that cannot be read fails closed (a keep reason). Entries whose
/// directories are gone (prunable) do not count — nothing lives there.
// trace:BUG-1657 | ai:claude
pub(crate) fn branch_checked_out_elsewhere(
    project_root: &std::path::Path,
    branch: &str,
    lease_path: &std::path::Path,
) -> Option<String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return None;
    }
    let Some(listing) = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["worktree", "list", "--porcelain", "-z"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
    else {
        return Some("the worktree listing could not be read — operator decision".to_string());
    };
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let lease_canon = (!lease_path.as_os_str().is_empty()).then(|| canon(lease_path));
    let want = format!("branch refs/heads/{branch}");
    let text = String::from_utf8_lossy(&listing.stdout);
    let mut path: Option<std::path::PathBuf> = None;
    // `-z` terminates each attribute with NUL and each entry with an extra NUL.
    for field in text.split('\0') {
        if let Some(p) = field.strip_prefix("worktree ") {
            path = Some(std::path::PathBuf::from(p));
        } else if field == want {
            let Some(p) = path.as_deref() else { continue };
            if p.exists() && lease_canon.as_deref() != Some(canon(p).as_path()) {
                return Some(format!(
                    "branch is checked out in another worktree at {}",
                    p.display()
                ));
            }
        } else if field.is_empty() {
            path = None;
        }
    }
    None
}

/// The scan's `head_on_branch` fact: no worktree, a worktree directory that
/// was removed by hand (no checkout left to be off-branch — the merge and
/// content proofs still gate removal), or a worktree checked out on `branch`.
// trace:BUG-1657 | ai:claude
pub(crate) fn session_head_on_branch(worktree: &std::path::Path, branch: &str) -> bool {
    worktree.as_os_str().is_empty()
        || !worktree.exists()
        || worktree_head_on_branch(worktree, branch)
}

/// Gather the merge facts for one session branch — the probes the shared
/// worktree classifier needs. `worth_probing` is "the spec is finished and the
/// process exited"; the dearer probes (trailer scan, forge lookup, content
/// comparison) only run when they could turn a skip into a reap. `pr_merged`
/// is the forge lookup, injected so fixture tests need no network.
///
/// The merge signals, in order: the branch is an ancestor of the default
/// branch; a landed commit names the spec in a trailer (batched integration);
/// the forge reports a merged PR for the branch. A signal with commits the
/// default branch lacks by ancestry still needs content proof — patch-id
/// equivalence, or every path the branch touched already identical on the
/// default branch.
///
/// A failed ancestry count is UNKNOWN, not zero: no merge signal is probed and
/// the session is kept.
// trace:TASK-1177 | ai:claude
// trace:BUG-1657 | ai:claude
#[cfg(test)]
pub(crate) fn gather_merge_facts(
    project_root: &std::path::Path,
    default_ref: Option<&str>,
    branch: &str,
    spec: &str,
    dirty: bool,
    worth_probing: bool,
    pr_merged: impl FnOnce(&str) -> bool,
) -> AgentWorktreeFacts {
    gather_merge_facts_pinned(
        project_root,
        default_ref,
        branch,
        spec,
        dirty,
        worth_probing,
        pr_merged,
    )
    .0
}

/// Resolve the LOCAL branch `branch` to its tip commit, spelled
/// `refs/heads/<branch>` so a tag or remote ref of the same name can never
/// shadow it. `None` when the branch does not exist or git fails.
// trace:BUG-1657 | ai:claude
pub(crate) fn resolve_local_branch_tip(
    project_root: &std::path::Path,
    branch: &str,
) -> Option<String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return None;
    }
    std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            crate::git_arg_guard::END_OF_OPTIONS,
            &format!("refs/heads/{branch}^{{commit}}"),
        ])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|sha| !sha.is_empty())
}

/// [`gather_merge_facts`], also returning the branch tip commit every probe
/// was run against. The tip is resolved ONCE from `refs/heads/<branch>` and
/// every git probe then uses that commit id, so a same-named tag cannot shadow
/// the branch and the facts describe one fixed commit; the reap later deletes
/// the branch only if it still points there.
// trace:BUG-1657 | ai:claude
pub(crate) fn gather_merge_facts_pinned(
    project_root: &std::path::Path,
    default_ref: Option<&str>,
    branch: &str,
    spec: &str,
    dirty: bool,
    worth_probing: bool,
    pr_merged: impl FnOnce(&str) -> bool,
) -> (AgentWorktreeFacts, Option<String>) {
    let branch = branch.trim();
    let tip = resolve_local_branch_tip(project_root, branch);
    // A lease with no worktree and no branch has nothing on disk that could
    // carry unmerged work, so it counts as merged.
    let mut count_known = true;
    let (ancestor_of_main, unique_unmerged_commits) = match (branch.is_empty(), default_ref, &tip) {
        (true, _, _) => (true, 0),
        (false, Some(default_ref), Some(tip)) => {
            let n = std::process::Command::new("git")
                .arg("-C")
                .arg(project_root)
                .args(["rev-list", "--count", &format!("{default_ref}..{tip}")])
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .trim()
                        .parse::<u32>()
                        .ok()
                });
            // Fail closed: an unreadable count must never read as "nothing
            // unique". trace:BUG-1657 | ai:claude
            count_known = n.is_some();
            (n == Some(0), n.unwrap_or(u32::MAX))
        }
        // Without a resolvable default ref or branch tip merged-ness cannot be
        // proven, and no signal (not even a forge-merged PR) may make it
        // removable. trace:BUG-1657 | ai:claude
        _ => {
            count_known = false;
            (false, u32::MAX)
        }
    };
    let worth_probing = worth_probing && count_known && !ancestor_of_main && !dirty;
    let probe = match (worth_probing, default_ref, tip.as_deref()) {
        (true, Some(default_ref), Some(tip)) => Some((default_ref, tip)),
        _ => None,
    };
    // BUG-1657: a spec landed through a batched integration PR has no merge
    // of its own branch and no PR of its own; the landing commit on the
    // default branch names it in a trailer instead. Local and cheap, so it
    // runs before (and can spare) the forge lookup.
    let landing_commit = probe
        .and_then(|(default_ref, tip)| spec_landing_commit(project_root, default_ref, tip, spec));
    let spec_trailer_on_main = landing_commit.is_some();
    // Only pay for the forge lookup when the cheap probes were inconclusive
    // (the squash-merge case) AND everything else already points at a reap.
    // The forge is asked by branch NAME; the content proof below still runs
    // against the pinned tip.
    let pr_merged = probe.is_some() && !spec_trailer_on_main && pr_merged(branch);
    // BUG-1287: a squash-merged branch's own commits keep a different SHA
    // from the squash commit forever, so `unique_unmerged_commits` stays
    // positive whether or not anything is unshipped. Only pay for the content
    // probe when it could change the verdict. BUG-1657: a batched squash folds
    // other specs' work into the same commit, so patch-ids never match; the
    // per-path probe proves every file the branch touched is already there.
    let merge_signal = pr_merged || spec_trailer_on_main;
    let content_fully_landed = match probe {
        Some((default_ref, tip)) if merge_signal && unique_unmerged_commits > 0 => {
            branch_content_fully_landed(project_root, default_ref, tip)
                || branch_paths_match_default(project_root, default_ref, tip)
        }
        _ => false,
    };
    (
        AgentWorktreeFacts {
            dirty,
            ancestor_of_main,
            pr_merged,
            unique_unmerged_commits,
            content_fully_landed,
            spec_trailer_on_main,
        },
        tip,
    )
}

/// Gather the facts for every session lease and classify each. Read-only: git
/// ancestry probes, a forge merged-PR lookup for the squash case, a store read,
/// and the process-liveness probe. Nothing is mutated here.
// trace:TASK-1177 | ai:claude
pub(crate) fn scan_reapable(project_root: &std::path::Path) -> ReapReport {
    scan_reapable_with(project_root, |pr| {
        crate::queue_cmd::review_pr_is_merged_or_closed(
            project_root,
            pr,
            &mut crate::network_retry::NoopSink,
        )
    })
}

/// [`scan_reapable`] with the forge's PR-state lookup injected:
/// `pr_terminal(n)` is `Some(true)` when PR `n` is merged or closed, `None`
/// when unknown (never a reap).
// trace:TASK-1955 | ai:claude (BUG-2206)
pub(crate) fn scan_reapable_with(
    project_root: &std::path::Path,
    pr_terminal: impl Fn(u32) -> Option<bool>,
) -> ReapReport {
    let leases = list_leases(project_root);
    let mut report = ReapReport::default();
    let summaries = requirement_summaries(project_root);
    report.unshipped_work = collect_unshipped_work_items(project_root, &summaries, false, true);
    report.orphan_pins = crate::seat_clone::orphan_pins(project_root); // trace:TASK-2053 | ai:claude
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
    let this_host = crate::coordination::hostname();

    for lease in &leases {
        let lease = &reap_view(lease, project_root);
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
        let branch = reap_branch(lease, default_ref.as_deref());
        let protected_branch = branch_is_protected(branch, default_ref.as_deref());
        if is_project_or_store_tree || protected_branch {
            // trace:BUG-2197 | ai:claude
            let verdict = classify_protected_lease(
                has_worktree,
                lease.hostname == this_host,
                lease_recorded_owner(lease),
            );
            let (verdict_label, reason) = match &verdict {
                ProtectedLeaseVerdict::Release(r) => ("release", r.clone()),
                ProtectedLeaseVerdict::UnknownOwner(r) => ("unknown-owner", r.clone()),
                ProtectedLeaseVerdict::Keep(r) => ("skip", r.clone()),
            };
            let row = ReapRow {
                session: lease.id.clone(),
                scope: lease.scope.clone(),
                worktree: lease.worktree_path.display().to_string(),
                branch: lease.branch.clone(),
                verdict: verdict_label,
                reason,
                spec_finished: finished.contains(&lease.scope.to_ascii_uppercase()),
                branch_tip: None,
                outcome: None,
            };
            match verdict {
                ProtectedLeaseVerdict::Release(_) => report.releasable.push(row),
                ProtectedLeaseVerdict::UnknownOwner(_) => report.unknown_owner.push(row),
                ProtectedLeaseVerdict::Keep(_) => report.skipped.push(row),
            }
            continue;
        }

        let spec_finished = finished.contains(&lease.scope.to_ascii_uppercase());

        // TASK-2053: a seat clone is judged from the sandboxed probe and the
        // forge, never from host git in the clone or main's branch refs.
        // trace:TASK-2053 | ai:claude
        if lease.tree_kind == crate::seat_clone::TreeKind::Clone {
            let row = scan_clone_lease(project_root, lease, spec_finished, &live, &active, now);
            match row.verdict {
                "reap" => report.reapable.push(row),
                _ => report.skipped.push(row),
            }
            continue;
        }

        // Liveness — the shipped detection, not a second notion. An advisory
        // lease's worktree path is empty, so the in-worktree probe is skipped
        // for it (an empty path would otherwise resolve against the cwd).
        let worktree_has_live_process =
            has_worktree && worktree_is_active(&lease.worktree_path, &active);
        let owner_gone = aida_core::liveness::lease_owner_process_gone(
            lease.active_pid,
            lease.active_pid_start_time.as_deref(),
            lease.creator_pid,
            lease.creator_pid_start_time.as_deref(),
            process_probe::process_identity_is_alive,
        );
        let process_exited = session_process_exited(
            lease_state_for(lease, &live, now),
            owner_gone,
            worktree_has_live_process,
        );

        // Merge facts.
        let dirty = has_worktree && worktree_status_dirty(&lease.worktree_path);
        let (worktree, branch_tip) = gather_merge_facts_pinned(
            project_root,
            default_ref.as_deref(),
            branch,
            &lease.scope,
            dirty,
            spec_finished && process_exited,
            |branch| {
                matches!(
                    detect_merged_pr_for_branch_via_forge(project_root, branch),
                    PrLookup::Found(_)
                )
            },
        );

        // BUG-2206: a dead attested reviewer's PR is asked about only once its
        // process is gone. trace:TASK-1955 | ai:claude (BUG-2206)
        // TASK-2095: and only when the review branch holds nothing past the
        // PR head the review recorded (an unpushed fix-forward would be lost
        // with the branch), so the skipped merge gate loses no commit.
        // trace:TASK-1955 | ai:claude (TASK-2095)
        let review_pr_terminal = review_pr_terminal_gate(
            process_exited,
            lease,
            || review_branch_within_pr_head(project_root, branch_tip.as_deref(), lease),
            &pr_terminal,
        );
        let facts = ReapFacts {
            spec_finished,
            review_pr_terminal,
            process_exited,
            locked: has_worktree && worktree_is_locked(project_root, &lease.worktree_path),
            head_on_branch: session_head_on_branch(&lease.worktree_path, branch),
            branch_checked_out_elsewhere: branch_checked_out_elsewhere(
                project_root,
                branch,
                &lease.worktree_path,
            ),
            worktree,
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
            spec_finished: spec_finished || review_pr_terminal,
            branch_tip,
            outcome: None,
        };
        // FR-284 NOTIFY: a finished + merged session whose process is STILL
        // ALIVE is the interactive near-miss — not reapable (it owns its cwd),
        // but worth telling "your spec merged, safe to exit". Gated on the
        // session actually holding a worktree; an advisory lease has nothing to
        // exit for. This session is also in `skipped` (its verdict is a Skip on
        // the liveness boundary); `notifiable` is the subset we message.
        // trace:FR-284 | ai:claude
        if has_worktree && session_should_notify(&facts) {
            report.notifiable.push(NotifyRow {
                session: lease.id.clone(),
                scope: lease.scope.clone(),
                owner: lease.owner.clone(),
                worktree: lease.worktree_path.display().to_string(),
                branch: lease.branch.clone(),
                outcome: None,
            });
        }

        match verdict {
            ReapVerdict::Reap(_) => report.reapable.push(row),
            ReapVerdict::Skip(_) => report.skipped.push(row),
        }
    }

    report.reapable.sort_by(|a, b| a.scope.cmp(&b.scope));
    report.skipped.sort_by(|a, b| a.scope.cmp(&b.scope));
    report.notifiable.sort_by(|a, b| a.scope.cmp(&b.scope));
    report
}

/// BUG-2206: the PR an attested-reviewer lease reviews — a `PR-N`/`MR-N`
/// scope on a review-verb or reviewer-role lease.
// trace:TASK-1955 | ai:claude (BUG-2206)
fn review_lease_pr(lease: &SessionLease) -> Option<u64> {
    (lease.review_verb || lease.role.as_deref() == Some("reviewer"))
        .then(|| crate::status_prs::parse_pr_scope(&lease.scope))
        .flatten()
}

/// The dead-reviewer reap gate, cheapest check first: the lease's PR scope
/// (no I/O), then the review branch's git checks, then the forge.
// trace:TASK-1955 | ai:claude (TASK-2119)
fn review_pr_terminal_gate(
    process_exited: bool,
    lease: &SessionLease,
    branch_within_pr_head: impl FnOnce() -> bool,
    pr_terminal: impl FnOnce(u32) -> Option<bool>,
) -> bool {
    let Some(pr) = review_lease_pr(lease).and_then(|pr| u32::try_from(pr).ok()) else {
        return false;
    };
    process_exited && branch_within_pr_head() && pr_terminal(pr) == Some(true)
}

/// Does the review branch hold nothing the reap would lose? True when its
/// tip is the recorded PR head or behind it, or when every commit on it is
/// already on `origin` (a fix-forward the reviewer pushed to the PR, or a
/// lease that recorded no PR head). An unpushed local commit keeps it.
// trace:TASK-1955 | ai:claude (TASK-2095, TASK-2108)
fn review_branch_within_pr_head(
    project_root: &std::path::Path,
    branch_tip: Option<&str>,
    lease: &SessionLease,
) -> bool {
    let Some(tip) = branch_tip else {
        return false;
    };
    let within_recorded = lease.pr_head_sha.as_deref().is_some_and(|pr_head| {
        aida_core::git_ops::is_ancestor(project_root, tip, pr_head).unwrap_or(false)
    });
    within_recorded || commits_outside_origin(project_root, tip) == Some(0)
}

/// How many commits reachable from `tip` are on no `origin` remote-tracking
/// ref; `None` when git cannot say (never read as zero).
// trace:TASK-1955 | ai:claude (TASK-2108)
fn commits_outside_origin(project_root: &std::path::Path, tip: &str) -> Option<u64> {
    let out = aida_core::git_ops::host_git_command(project_root)
        .args(["rev-list", "--count", tip, "--not", "--remotes=origin"])
        .output()
        .ok()?;
    out.status.success().then_some(())?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
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
fn reap_one(
    project_root: &std::path::Path,
    lease: &SessionLease,
    checked_tip: Option<&str>,
) -> String {
    reap_one_with_clear_hooks(project_root, lease, checked_tip, || {}, || {})
}

// trace:BUG-1694 | ai:claude
fn reap_one_with_clear_hooks(
    project_root: &std::path::Path,
    lease: &SessionLease,
    checked_tip: Option<&str>,
    before_missing_worktree_clear: impl FnOnce(),
    after_final_registration_check: impl FnOnce(),
) -> String {
    let lease = &reap_view(lease, project_root);
    // trace:TASK-2053 | ai:claude
    if lease.tree_kind == crate::seat_clone::TreeKind::Clone {
        return reap_one_clone(project_root, lease, checked_tip);
    }
    let has_worktree = !lease.worktree_path.as_os_str().is_empty();
    let branch = reap_branch(lease, resolve_default_branch_ref(project_root).as_deref());

    // The scan proved a specific commit shipped. If the branch moved since
    // (or its tip was never pinned), those proofs say nothing about the new
    // commits — leave everything in place. trace:BUG-1657 | ai:claude
    if !branch.is_empty() && !branch_still_at(project_root, branch, checked_tip) {
        return format!("skipped — branch `{branch}` moved since the scan");
    }
    // The branch delete below does not refuse a branch checked out in another
    // worktree (e.g. the session's worktree was moved and the lease path is
    // stale), so re-check the live listing. trace:BUG-1657 | ai:claude
    if let Some(reason) = branch_checked_out_elsewhere(project_root, branch, &lease.worktree_path) {
        return format!("skipped — {reason}");
    }
    let worktree_missing = has_worktree && !lease.worktree_path.exists();

    if has_worktree && lease.worktree_path.exists() {
        // An unreadable status is not "clean": the teardown below forces
        // removal, so keep a tree whose contents cannot be checked.
        // trace:BUG-1657 | ai:claude
        if worktree_status_unreadable(&lease.worktree_path) {
            return "skipped — worktree status could not be read".to_string();
        }
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
        // BUG-2176: a reviewer's merge-hold marker lives in its review
        // worktree; move it to the primary before the tree goes, or keep the
        // tree. trace:TASK-1955 | ai:claude (BUG-2206)
        if let Some(pr) = review_lease_pr(lease) {
            if let Err(e) = crate::merge_hold::adopt_worktree_hold(
                project_root,
                &lease.worktree_path,
                pr,
                lease.started_at.into(),
            ) {
                return format!("skipped — could not adopt the PR-{pr} merge-hold marker ({e})");
            }
        }
        if let Err(e) = aida_core::worktree_pool_destroy::teardown_worktree_path(
            project_root,
            &lease.worktree_path,
            &worktree_pool_global_hooks("pre_destroy"),
        ) {
            // A live process inside the tree keeps the whole session: lease,
            // worktree, and branch. trace:BUG-1947 | ai:claude
            if let Some(in_use) = e.downcast_ref::<aida_core::worktree_holders::WorktreeInUse>() {
                return format!("skipped — worktree {}", in_use.summary());
            }
            return format!(
                "failed — could not remove worktree {}",
                lease.worktree_path.display()
            );
        }
    }

    release_lease_files(project_root, &lease.id);

    // Branch cleanup. The protected-ref floor is re-asserted here so the
    // destructive call can never be reached by a future caller that skipped
    // the scan's guard.
    if branch.is_empty()
        || branch_is_protected(branch, resolve_default_branch_ref(project_root).as_deref())
    {
        return "reaped — lease released".to_string();
    }
    // A worktree directory removed by hand leaves a prunable registration
    // behind; clear it so no dangling entry still names the deleted branch.
    // Scope removal to this lease path rather than a repo-wide prune so
    // another session's temporarily unavailable worktree keeps its registration.
    // trace:BUG-1657 trace:TASK-1543 | ai:antigravity
    if worktree_missing {
        // A decline is reported even when the path vanished again by the time
        // we return: the claim saw it occupied (or could not prove otherwise),
        // so the registration was kept. trace:BUG-1694 | ai:claude
        if clear_missing_worktree_registration(
            project_root,
            &lease.worktree_path,
            before_missing_worktree_clear,
            after_final_registration_check,
        ) == RegistrationClearOutcome::Declined
        {
            return "reaped — lease released (worktree path reappeared or could not be \
                    confirmed unoccupied; registration kept)"
                .to_string();
        }
    }
    if delete_branch_at(project_root, branch, checked_tip) {
        format!("reaped — worktree removed, lease released, branch `{branch}` deleted")
    } else {
        format!("reaped — worktree removed, lease released (branch `{branch}` kept: gone or moved)")
    }
}

/// The reap verdict for a seat clone, pure over the probe and the gates.
/// The cheap gates run first (liveness, then completion), so a live or
/// unfinished seat is never probed: a probe is two sandboxed git runs, and
/// every reap scan, interactive or from the drive loop and night shift,
/// visits every clone lease. Then the probe, in [`classify_session_reap`]'s
/// order (unreadable or uncommitted state, the branch, work origin lacks),
/// and last the merge signal (a merged PR on the forge: the clone's branch
/// is never in main).
// trace:TASK-2053 | ai:claude
pub(crate) fn classify_clone_reap(
    probe: impl FnOnce() -> crate::seat_clone::Probe,
    branch: &str,
    process_exited: bool,
    spec_finished: bool,
    merged: impl FnOnce() -> bool,
) -> ReapVerdict {
    if !process_exited {
        return ReapVerdict::Skip(
            "its process is still running — left untouched (never force-closed)".to_string(),
        );
    }
    if !spec_finished {
        return ReapVerdict::Skip("its spec is not finished (needs Done or Completed)".to_string());
    }
    let facts = match probe() {
        crate::seat_clone::Probe::Unknown(why) => {
            return ReapVerdict::Skip(format!("seat clone state could not be read ({why}) — kept"))
        }
        crate::seat_clone::Probe::HasSubmodules(reason) => return ReapVerdict::Skip(reason),
        crate::seat_clone::Probe::Known(facts) => facts,
    };
    if !facts.dirty.is_empty() {
        return ReapVerdict::Skip("uncommitted changes present — never auto-removed".to_string());
    }
    if facts.branch.as_deref() != Some(branch) {
        return ReapVerdict::Skip(
            "seat clone is not checked out on its session branch — operator decision".to_string(),
        );
    }
    if facts.unpushed > 0 {
        return ReapVerdict::Skip(format!(
            "seat clone holds {} commit(s) origin does not have ({})",
            facts.unpushed,
            facts.unpushed_refs.join("; ") // trace:BUG-2224 | ai:claude
        ));
    }
    if !merged() {
        return ReapVerdict::Skip(format!("no merged PR found for `{branch}`"));
    }
    ReapVerdict::Reap(
        "spec finished, process exited, PR merged, nothing beyond origin in the seat clone"
            .to_string(),
    )
}

/// Scan one `clone` lease. The row's `branch_tip` is the HEAD the probe saw;
/// [`reap_one_clone`] removes the clone only while HEAD is still there.
// trace:TASK-2053 | ai:claude
fn scan_clone_lease(
    project_root: &std::path::Path,
    lease: &SessionLease,
    spec_finished: bool,
    live: &[process_probe::LiveSession],
    active: &std::collections::HashSet<std::path::PathBuf>,
    now: chrono::DateTime<chrono::Utc>,
) -> ReapRow {
    let owner_gone = aida_core::liveness::lease_owner_process_gone(
        lease.active_pid,
        lease.active_pid_start_time.as_deref(),
        lease.creator_pid,
        lease.creator_pid_start_time.as_deref(),
        process_probe::process_identity_is_alive,
    );
    let process_exited = session_process_exited(
        lease_state_for(lease, live, now),
        owner_gone,
        worktree_is_active(&lease.worktree_path, active),
    );
    let mut branch_tip = None;
    let verdict = classify_clone_reap(
        || {
            // TASK-2110: a squash-merged PR's head counts as shipped.
            // trace:TASK-2110 | ai:claude
            let shipped = crate::seat_clone::merged_pr_heads(project_root, &lease.branch);
            let probe =
                crate::seat_clone::probe_with_shipped(project_root, &lease.worktree_path, &shipped);
            if let crate::seat_clone::Probe::Known(facts) = &probe {
                branch_tip = facts.head.clone();
            }
            probe
        },
        &lease.branch,
        process_exited,
        spec_finished,
        || {
            matches!(
                detect_merged_pr_for_branch_via_forge(project_root, &lease.branch),
                PrLookup::Found(_)
            )
        },
    );
    ReapRow {
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
        branch_tip,
        outcome: None,
    }
}

/// Reap ONE seat clone: [`crate::seat_clone::destroy`] re-probes it, keeps it
/// (with a salvage patch) if work appeared or HEAD moved since the scan, and
/// otherwise removes the clone, its cargo target directory and its base pin.
/// The lease goes only once the clone is gone.
// trace:TASK-2053 | ai:claude
pub(crate) fn reap_one_clone(
    project_root: &std::path::Path,
    lease: &SessionLease,
    checked_tip: Option<&str>,
) -> String {
    let Some(tip) = checked_tip else {
        return "skipped — the scan recorded no HEAD for the seat clone".to_string();
    };
    // trace:TASK-2110 | ai:claude
    let shipped = crate::seat_clone::merged_pr_heads(project_root, &lease.branch);
    match crate::seat_clone::destroy_with_shipped(
        project_root,
        &lease.worktree_path,
        crate::seat_clone::DestroyMode::KeepUnshipped,
        Some(tip),
        &shipped,
    ) {
        Ok(crate::seat_clone::Destroyed::Removed { .. }) => {
            release_lease_files(project_root, &lease.id);
            "reaped — seat clone removed, base pin released, lease released".to_string()
        }
        Ok(crate::seat_clone::Destroyed::Kept { reason, salvage }) => format!(
            "skipped — seat clone kept: {reason}{}",
            salvage
                .map(|p| format!(" (salvage patch: {})", p.display()))
                .unwrap_or_default()
        ),
        Err(error) => format!("failed — {error:#}"),
    }
}

/// Release every orphan pin whose clone directory is gone; record the outcome
/// on each row. Returns how many were released.
// trace:TASK-2053 | ai:claude
pub(crate) fn release_orphan_pins(
    project_root: &std::path::Path,
    pins: &[crate::seat_clone::OrphanPin],
) -> Vec<String> {
    pins.iter()
        .map(|orphan| {
            if orphan.clone_exists {
                format!(
                    "kept {} — its clone {} exists with no lease; `aida worktree dismiss {}` \
                     removes it",
                    orphan.pin,
                    orphan.clone.display(),
                    orphan.clone.display()
                )
            } else {
                match crate::seat_clone::release_orphan_pin(project_root, orphan) {
                    Ok(true) => format!("released {} (its clone is gone)", orphan.pin),
                    Ok(false) => format!(
                        "kept {} — its clone or lease reappeared since the scan",
                        orphan.pin
                    ),
                    Err(error) => format!("could not release {}: {error:#}", orphan.pin),
                }
            }
        })
        .collect()
}

/// BUG-2197: release ONE protected-checkout lease. Re-reads the lease from disk
/// and re-proves the verdict immediately before deleting, so a lease that was
/// re-stamped with a live pid since the scan is kept. Only the lease and its
/// companion files go; no worktree, branch, or checkout file is touched.
// trace:BUG-2197 | ai:claude
pub(crate) fn release_protected_lease(
    project_root: &std::path::Path,
    lease_id: &str,
    allow_unknown_owner: bool,
) -> String {
    let Some(lease) = list_leases(project_root)
        .into_iter()
        .find(|l| l.id == lease_id)
    else {
        return "skipped — lease already gone".to_string();
    };
    let lease = reap_view(&lease, project_root);
    let verdict = classify_protected_lease(
        !lease.worktree_path.as_os_str().is_empty(),
        lease.hostname == crate::coordination::hostname(),
        lease_recorded_owner(&lease),
    );
    match verdict {
        ProtectedLeaseVerdict::Release(_) => {
            release_lease_files(project_root, &lease.id);
            "released — recorded agent process dead; checkout untouched".to_string()
        }
        ProtectedLeaseVerdict::UnknownOwner(_) if allow_unknown_owner => {
            release_lease_files(project_root, &lease.id);
            "released — unknown owner (no durable pid); checkout untouched".to_string()
        }
        ProtectedLeaseVerdict::UnknownOwner(r) | ProtectedLeaseVerdict::Keep(r) => {
            format!("kept — {r}")
        }
    }
}

/// Delete a lease and its companions (activity log, manifest, headless child
/// grant). Reuses the same aggregation-then-delete ordering `aida session end`
/// uses so the session's activity is folded into the project's role activity
/// before its log goes away. Touches no worktree and no branch.
// trace:TASK-1784 | ai:claude
pub(crate) fn release_lease_files(project_root: &std::path::Path, lease_id: &str) {
    aggregate_session_activity_into_roles(project_root, lease_id);
    // BUG-2139: a reaped lease takes its headless child's grant with it.
    // trace:BUG-2139 | ai:claude
    crate::session::revoke_lease_child_grant(&lease_path(project_root, lease_id));
    let _ = std::fs::remove_file(lease_path(project_root, lease_id));
    let activity = session_activity_path(project_root, lease_id);
    if activity.exists() {
        let _ = std::fs::remove_file(&activity);
    }
    let manifest = session_manifest::manifest_path(project_root, lease_id);
    if manifest.exists() {
        let _ = std::fs::remove_file(&manifest);
    }
}

// How a registration-clear attempt for an absent lease path ended.
// trace:BUG-1694 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegistrationClearOutcome {
    /// The stale admin registration was removed.
    Cleared,
    /// The path could not be confirmed unoccupied (or the removal itself
    /// failed), so the registration was kept. Fail closed: a stale
    /// registration is strictly better than orphaning a live worktree.
    Declined,
    /// No admin registration names this path (or the repository state was
    /// unreadable); there was nothing to clear.
    NoRegistration,
}

// Clear only the administrative registration for an absent lease path. Never
// ask Git to remove a worktree here: the path can reappear after the early scan.
//
// An exists() check cannot close the window between itself and the removal
// (TASK-1543 left that residue open). Instead the path is CLAIMED with an
// atomic `mkdir`: every cooperating creator — a restore (`mv`/untar) or
// `git worktree add` — must materialize this same path, so either the claim
// wins and nothing can reappear while the admin dir is removed, or the claim
// loses (EEXIST, or any other error) and we decline. The claim deliberately
// uses the byte-identical path value the `gitdir` identity match uses, so the
// two cannot diverge; a relative lease path is declined outright rather than
// resolved against a process cwd. trace:BUG-1694 | ai:claude
pub(crate) fn clear_missing_worktree_registration(
    project_root: &std::path::Path,
    worktree_path: &std::path::Path,
    before_clear: impl FnOnce(),
    after_final_check: impl FnOnce(),
) -> RegistrationClearOutcome {
    if !worktree_path.is_absolute() {
        return RegistrationClearOutcome::Declined;
    }
    if worktree_path.exists() {
        return RegistrationClearOutcome::Declined;
    }
    before_clear();
    if worktree_path.exists() {
        return RegistrationClearOutcome::Declined;
    }

    // Unreadable repository state cannot establish that no registration names
    // this path — decline loudly rather than fall through to branch deletion.
    // trace:BUG-1694 | ai:claude
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--git-common-dir"])
        .output();
    let Ok(output) = output else {
        return RegistrationClearOutcome::Declined;
    };
    if !output.status.success() {
        return RegistrationClearOutcome::Declined;
    }
    let common_dir = std::path::PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let common_dir = if common_dir.is_absolute() {
        common_dir
    } else {
        project_root.join(common_dir)
    };
    let worktrees_dir = common_dir.join("worktrees");
    let expected_gitdir = worktree_path.join(".git");
    // A missing worktrees dir means no registrations exist at all (the common
    // already-pruned case); any OTHER read failure is unreadable state and
    // declines like the above. trace:BUG-1694 | ai:claude
    let entries = match std::fs::read_dir(worktrees_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return RegistrationClearOutcome::NoRegistration;
        }
        Err(_) => return RegistrationClearOutcome::Declined,
    };
    for entry in entries.flatten() {
        let admin_dir = entry.path();
        let Ok(gitdir) = std::fs::read_to_string(admin_dir.join("gitdir")) else {
            continue;
        };
        if worktree_gitdir_matches(gitdir.trim(), &expected_gitdir) {
            if worktree_path.exists() {
                return RegistrationClearOutcome::Declined;
            }
            after_final_check();
            if std::fs::create_dir(worktree_path).is_err() {
                return RegistrationClearOutcome::Declined;
            }
            let removed = std::fs::remove_dir_all(admin_dir);
            // Release the claim on both outcomes; non-recursive, so a creator
            // that already populated the empty directory keeps it (ENOTEMPTY).
            let _ = std::fs::remove_dir(worktree_path);
            return if removed.is_ok() {
                RegistrationClearOutcome::Cleared
            } else {
                RegistrationClearOutcome::Declined
            };
        }
    }
    RegistrationClearOutcome::NoRegistration
}

// Windows' Git for Windows and `std::fs::canonicalize` can spell the same
// worktree with different separator, case, or `\\?\` prefix forms. The path
// is already constrained to an absolute path from this lease; normalize those
// Windows spelling differences before matching the stale administrative link.
// trace:BUG-1777 | ai:codex
fn worktree_gitdir_matches(recorded: &str, expected: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        let key = |path: &str| {
            let path = path.replace('\\', "/");
            let path = path
                .strip_prefix("//?/UNC/")
                .map(|unc| format!("//{unc}"))
                .or_else(|| path.strip_prefix("//?/").map(str::to_string))
                .unwrap_or(path);
            path.trim_end_matches('/').to_ascii_lowercase()
        };
        return key(recorded) == key(&expected.to_string_lossy());
    }
    #[cfg(not(windows))]
    {
        std::path::Path::new(recorded) == expected
    }
}

/// Does local branch `branch` still point at `tip`? `None` (never pinned) or
/// any git failure → `false`.
// trace:BUG-1657 | ai:claude
pub(crate) fn branch_still_at(
    project_root: &std::path::Path,
    branch: &str,
    tip: Option<&str>,
) -> bool {
    match tip {
        Some(tip) => resolve_local_branch_tip(project_root, branch).as_deref() == Some(tip),
        None => false,
    }
}

/// Delete `refs/heads/<branch>` only if it still points at `tip`:
/// `git update-ref -d <ref> <old>` refuses when the ref moved, so a commit
/// added after the scan can never be lost. A squash-merged branch is not
/// "merged" to `git branch -d`, which is why this is not `-d`; the scan
/// already proved the pinned commit shipped. Returns whether it was deleted.
///
/// Unlike `git branch -D`, `update-ref -d` does NOT refuse a branch that is
/// checked out in another worktree. Callers must guard with
/// [`branch_checked_out_elsewhere`] first (`reap_one` does).
// trace:BUG-1657 | ai:claude
pub(crate) fn delete_branch_at(
    project_root: &std::path::Path,
    branch: &str,
    tip: Option<&str>,
) -> bool {
    let (branch, Some(tip)) = (branch.trim(), tip) else {
        return false;
    };
    if branch.is_empty() || tip.is_empty() {
        return false;
    }
    std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "update-ref",
            "-d",
            crate::git_arg_guard::END_OF_OPTIONS,
            &format!("refs/heads/{branch}"),
            tip,
        ])
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The once-per-session sentinel for the FR-284 NOTIFY slice. A file per lease
/// under `.aida/session-notices/` (runtime per-clone state, gitignored by the
/// deny-by-default `.aida/*` rule — no new allow-list entry needed) records that
/// the "safe to exit" FYI was already sent, so the notice fires once rather than
/// on every merge / every reap pass. Its content is the spec id it was sent for,
/// so the rare case of a worktree lease reused for a *different* spec re-notifies.
// trace:FR-284 | ai:claude
fn session_notice_path(project_root: &std::path::Path, lease_id: &str) -> std::path::PathBuf {
    // Lease ids are 12-char hex, but neutralize path separators defensively so a
    // crafted id can never escape the notices dir.
    let safe: String = lease_id
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == '.' {
                '_'
            } else {
                c
            }
        })
        .collect();
    project_root
        .join(".aida")
        .join("session-notices")
        .join(safe)
}

/// FR-284 NOTIFY: tell each finished-but-still-live session, once, that its spec
/// merged and it is safe to exit. Delivery is a mailbox FYI (intent = `fyi`,
/// "surface only, no action") addressed to the operator handle — the same
/// `current_user_id` the per-turn `aida awaiting` notice reads its OWN inbox as,
/// so the message lands in the live session's headline on its next prompt turn
/// (ADR-23). Idempotent via the per-session sentinel: a session already notified
/// for this spec is skipped. Best-effort — a mailbox write failure degrades to a
/// skip, never bubbling up to fail a reap pass or a landed PR.
///
/// Records the outcome on each row and returns the count actually sent this
/// pass (a fresh notice, not an already-sent one). Never touches the live
/// process: this is detect-and-NOTIFY, the same boundary the reap holds.
// trace:FR-284 | ai:claude
fn notify_finished_live_sessions(project_root: &std::path::Path, rows: &mut [NotifyRow]) -> usize {
    if rows.is_empty() {
        return 0;
    }
    // The recipient the target session's `aida awaiting` headline reads as. In
    // the common single-operator, same-machine case the reaper and the target
    // share this handle; cross-user sessions on one host (rare) would route to
    // the reaper's inbox instead — an acceptable, non-destructive miss.
    let recipient = current_user_id(None);
    let mut sent = 0usize;
    for row in rows.iter_mut() {
        let sentinel = session_notice_path(project_root, &row.session);
        // Already notified for THIS spec? Leave it — the notice fires once.
        if std::fs::read_to_string(&sentinel)
            .map(|prev| prev.trim() == row.scope.trim())
            .unwrap_or(false)
        {
            row.outcome = Some(format!("already notified — spec {}", row.scope));
            continue;
        }
        let now = chrono::Utc::now().timestamp_millis();
        let id = uuid::Uuid::new_v4().to_string();
        let body = format!(
            "Spec {spec} has merged and its branch is finished — you are free to exit this \
             session whenever you like. Its worktree ({wt}) will be reaped automatically once \
             your process exits (the post-merge pass, or `aida session reap`). Nothing here \
             needs you; this is an FYI, no action required.",
            spec = row.scope,
            wt = row.worktree,
        );
        let msg = aida_core::mailbox::Message {
            subject: None,
            id: id.clone(),
            thread_id: id,
            from: "aida-session-reap".to_string(),
            to: aida_core::mailbox::Recipient::Agent(recipient.clone()),
            timestamp: now,
            in_reply_to: None,
            body,
            urgent: false,
            intent: aida_core::mailbox::Intent::Fyi,
            retracted: false,
            deleted: false,
            archived: false,
            // A fixed system identity, not an ambiguous env fallback.
            // trace:BUG-1533 | ai:claude
            from_source: aida_core::mailbox::SenderSource::Explicit,
            from_role: None,
            relayed_from: None,
        };
        if let Err(e) = mailbox_store::write_message(project_root, &msg) {
            row.outcome = Some(format!("notify failed — {e}"));
            continue;
        }
        // Drop the sentinel only after the message landed, so a write failure
        // retries next pass rather than silently swallowing the notice.
        if let Some(parent) = sentinel.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&sentinel, row.scope.trim());
        row.outcome = Some(format!(
            "notified {} — spec {} safe to exit",
            recipient, row.scope
        ));
        sent += 1;
    }
    sent
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
    /// Also release protected-checkout leases that record no durable pid.
    // trace:BUG-2197 | ai:claude
    pub release_unknown_owner: bool,
    /// Run the protected-checkout release lane at all. Off for the
    /// post-merge pass inside `aida pr ship`: a lease is released only from an
    /// operator-typed `aida session reap`.
    // trace:BUG-2197 | ai:claude
    pub release_protected: bool,
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
    let candidates =
        crate::queue_cmd::auto_complete_head_candidates(&storage, &user_id, None).ok()?;
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
    // trace:TASK-1184 | ai:codex
    if !opts.dry_run {
        let _ = agent_registry::gc_dead_agents(&project_root, false, None);
    }
    let mut report = scan_reapable(&project_root);

    if report.reapable.is_empty()
        && report.skipped.is_empty()
        && report.unshipped_work.is_empty()
        && report.releasable.is_empty()
        && report.unknown_owner.is_empty()
        && report.orphan_pins.is_empty()
    {
        if opts.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else if !opts.quiet_when_empty {
            println!("No session leases found — nothing to reap.");
        }
        return Ok(());
    }

    // trace:STORY-1043 | ai:codex
    if !opts.json && !report.unshipped_work.is_empty() && !opts.quiet_when_empty {
        println!("Unshipped work detected ({}):", report.unshipped_work.len());
        // trace:TASK-1305 | ai:claude — this surface also renders `recovery`,
        // so it shares awaiting_you's helper rather than reading the field
        // straight (see TASK-1305 note on UnshippedWorkItem::recovery).
        for row in &report.unshipped_work {
            println!(
                "  {} {} on `{}` — {} commit{} ahead, age {} — `{}`",
                "recover".yellow(),
                row.spec_id.cyan(),
                row.branch,
                row.commits_ahead,
                if row.commits_ahead == 1 { "" } else { "s" },
                row.age,
                awaiting_you::unshipped_work_recovery_hint(row).cyan()
            );
        }
    }

    // FR-284 NOTIFY: before any reap decision, tell each finished-but-still-live
    // session (once) that its spec merged and it is safe to exit. Non-destructive
    // and best-effort, so it runs on every real pass regardless of --yes / TTY;
    // --dry-run only reports what WOULD be sent. This is detect-and-notify — the
    // live process is never touched. trace:FR-284 | ai:claude
    let notified = if opts.dry_run {
        0
    } else {
        notify_finished_live_sessions(&project_root, &mut report.notifiable)
    };

    // The post-merge hook asks for silence on the common no-op, so a pass that
    // found nothing to reap prints nothing at all for it.
    let stay_silent = opts.quiet_when_empty && report.reapable.is_empty();

    // A freshly-sent notice is a real action worth a line even under
    // `quiet_when_empty`; an already-notified session stays quiet there to avoid
    // repeating on every merge. On --dry-run the notify preview rides the same
    // verbose gate as the rest of the report.
    if !opts.json && !report.notifiable.is_empty() {
        if opts.dry_run {
            if !stay_silent {
                println!(
                    "Would notify {} finished-but-live session(s) they are safe to exit:",
                    report.notifiable.len()
                );
                for row in &report.notifiable {
                    println!(
                        "  {} {} — spec merged, safe to exit",
                        "notify".cyan(),
                        row.scope.cyan()
                    );
                }
            }
        } else if notified > 0 || !opts.quiet_when_empty {
            for row in &report.notifiable {
                let Some(outcome) = &row.outcome else {
                    continue;
                };
                let fresh = outcome.starts_with("notified");
                if fresh || !opts.quiet_when_empty {
                    let marker = if fresh {
                        crate::glyph(crate::glyphs::Glyph::Mailbox)
                            .green()
                            .to_string()
                    } else {
                        "·".dimmed().to_string()
                    };
                    println!("  {marker} {} — {outcome}", row.scope.cyan());
                }
            }
        }
    }
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

    // trace:BUG-2197 | ai:claude
    release_protected_leases(&project_root, &mut report, &opts)?;

    // TASK-2053: a pin whose clone is gone protects nothing; release it on a
    // real pass. A pin whose clone still exists is only reported.
    // trace:TASK-2053 | ai:claude
    if !report.orphan_pins.is_empty() && !opts.json {
        if opts.dry_run {
            for orphan in &report.orphan_pins {
                println!(
                    "  {} orphan seat-clone pin {}{}",
                    "pin".cyan(),
                    orphan.pin,
                    if orphan.clone_exists {
                        " (its clone exists with no lease — kept)"
                    } else {
                        " (its clone is gone — would be released)"
                    }
                );
            }
        } else {
            for line in release_orphan_pins(&project_root, &report.orphan_pins) {
                println!("  {} {line}", "pin".cyan());
            }
        }
    } else if !report.orphan_pins.is_empty() && !opts.dry_run {
        release_orphan_pins(&project_root, &report.orphan_pins);
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
        let outcome = reap_one(&project_root, lease, row.branch_tip.as_deref());
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
    let _ = crate::notify::passive_check(&project_root);
    Ok(())
}

/// BUG-2197: the protected-checkout release lane of `aida session reap`. Lists
/// releasable and unknown-owner leases, then (unless `--dry-run`, and behind
/// the same `--yes` / TTY confirmation as the reap) releases the dead-owner
/// ones, plus the unknown-owner ones when `--release-unknown-owner` is set.
/// Never runs when `release_protected` is off (the `aida pr ship` pass).
// trace:BUG-2197 | ai:claude
fn release_protected_leases(
    project_root: &std::path::Path,
    report: &mut ReapReport,
    opts: &ReapOptions,
) -> Result<()> {
    release_protected_leases_with(
        project_root,
        report,
        opts,
        &mut std::io::stdout(),
        |count| {
            if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                return Ok(None);
            }
            use std::io::Write;
            eprint!("\nRelease the {count} lease(s)? Only lease files are removed. [y/N] ");
            std::io::stderr().flush()?;
            let mut ans = String::new();
            std::io::stdin().read_line(&mut ans)?;
            Ok(Some(matches!(
                ans.trim().to_ascii_lowercase().as_str(),
                "y" | "yes"
            )))
        },
    )
}

/// The release lane with its output sink and its confirmation injected.
/// `confirm(count)` answers `None` when no one can be asked (stdin is not a
/// TTY), else the operator's yes/no. It is consulted only without `--yes`.
// trace:BUG-2197 | ai:claude
pub(crate) fn release_protected_leases_with(
    project_root: &std::path::Path,
    report: &mut ReapReport,
    opts: &ReapOptions,
    out: &mut dyn std::io::Write,
    confirm: impl FnOnce(usize) -> Result<Option<bool>>,
) -> Result<()> {
    if !opts.release_protected || (report.releasable.is_empty() && report.unknown_owner.is_empty())
    {
        return Ok(());
    }
    if !opts.json {
        for row in &report.releasable {
            writeln!(
                out,
                "  {} {} {} — {}",
                "release".cyan(),
                short_id(&row.session),
                row.scope.cyan(),
                row.reason
            )?;
        }
        // A quiet pass names only what it will act on.
        let unknown_rows: &[ReapRow] = if opts.quiet_when_empty && !opts.release_unknown_owner {
            &[]
        } else {
            &report.unknown_owner
        };
        for row in unknown_rows {
            writeln!(
                out,
                "  {} {} {} — {}",
                "unknown".yellow(),
                short_id(&row.session),
                row.scope.cyan(),
                row.reason
            )?;
        }
    }
    let mut targets: Vec<&mut ReapRow> = report.releasable.iter_mut().collect();
    if opts.release_unknown_owner {
        targets.extend(report.unknown_owner.iter_mut());
    }
    if targets.is_empty() {
        return Ok(());
    }
    if opts.dry_run {
        if !opts.json {
            writeln!(out, "--dry-run: no lease was released.")?;
        }
        return Ok(());
    }
    if !opts.yes {
        match confirm(targets.len())? {
            None => {
                if !opts.json {
                    writeln!(out, "\nRe-run with --yes to release them.")?;
                }
                return Ok(());
            }
            Some(false) => {
                if !opts.json {
                    writeln!(out, "Aborted — nothing released.")?;
                }
                return Ok(());
            }
            Some(true) => {}
        }
    }
    for row in targets {
        let outcome =
            release_protected_lease(project_root, &row.session, opts.release_unknown_owner);
        if !opts.json {
            writeln!(
                out,
                "  {} {} — {outcome}",
                short_id(&row.session),
                row.scope.cyan()
            )?;
        }
        row.outcome = Some(outcome);
    }
    Ok(())
}

fn short_id(id: &str) -> &str {
    &id[..id.len().min(8)]
}

/// STORY-1218: the night-shift tick's reap — the same scan and the same
/// per-lease reap as `aida session reap --yes`, returning the reaped count and
/// printing nothing. A live process is never touched (the predicate requires
/// an exited process); the worktree-gc predicate is unchanged.
// trace:STORY-1218 | ai:claude
pub(crate) fn reap_quiet(project_root: &std::path::Path) -> usize {
    reap_quiet_outcomes(project_root)
        .leases
        .iter()
        .filter(|(_, outcome)| outcome.starts_with("reaped"))
        .count()
}

/// What one quiet reap did: the agent-registry GC (removed count, or why it
/// failed) and every reapable lease's `(session, outcome)` line — `reaped
/// …`, `skipped …` or `failed …`, exactly as `reap_one` reports it.
// trace:TASK-1915 | ai:claude
#[derive(Debug)]
pub(crate) struct QuietReap {
    pub(crate) agent_gc: Result<usize, String>,
    pub(crate) leases: Vec<(String, String)>,
}

/// [`reap_quiet`] with every outcome kept, so the drive tick can ledger
/// what the reap did beyond releasing leases (TASK-1915 F4).
// trace:TASK-1915 | ai:claude
pub(crate) fn reap_quiet_outcomes(project_root: &std::path::Path) -> QuietReap {
    let agent_gc = agent_registry::gc_dead_agents(project_root, false, None)
        .map(|r| r.removed_count())
        .map_err(|e| format!("{e:#}"));
    let report = scan_reapable(project_root);
    release_orphan_pins(project_root, &report.orphan_pins); // trace:TASK-2053 | ai:claude
    if report.reapable.is_empty() {
        return QuietReap {
            agent_gc,
            leases: Vec::new(),
        };
    }
    let leases = list_leases(project_root);
    let leases = report
        .reapable
        .iter()
        .filter_map(|row| {
            leases
                .iter()
                .find(|l| l.id == row.session)
                .map(|lease| (lease, row.branch_tip.as_deref()))
        })
        .map(|(lease, tip)| (lease.id.clone(), reap_one(project_root, lease, tip)))
        .collect();
    QuietReap { agent_gc, leases }
}

// The reapable-predicate matrix + the process-exited derivation.
// trace:TASK-1177 | ai:claude
#[cfg(test)]
#[path = "tests/task_1177_session_reap_tests.rs"]
mod task_1177_session_reap_tests;

// The NOTIFY predicate matrix (finished-but-still-live → safe-to-exit notice).
// trace:FR-284 | ai:claude
#[cfg(test)]
#[path = "tests/fr_284_session_notify_tests.rs"]
mod fr_284_session_notify_tests;

// The CHAIN-slice suggest-block emission + no-op cases.
// trace:TASK-1179 | ai:claude
#[cfg(test)]
#[path = "tests/task_1179_chain_suggest_tests.rs"]
mod task_1179_chain_suggest_tests;

// Batched-integration merge signal, against fixture git repos.
// trace:BUG-1657 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1657_batched_reap_tests.rs"]
mod bug_1657_batched_reap_tests;

// Reap polish: scoped missing worktree removal and porcelain -z lock detection.
// trace:TASK-1543 | ai:antigravity
#[cfg(test)]
#[path = "tests/task_1543_reap_polish_tests.rs"]
mod task_1543_reap_polish_tests;

// Registration clear claims the worktree path atomically (fail closed).
// trace:BUG-1694 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1694_registration_clear_claim_tests.rs"]
mod bug_1694_registration_clear_claim_tests;

// Protected-checkout lease release + no-pid owner attribution.
// trace:BUG-2197 | ai:claude
#[cfg(test)]
#[path = "tests/bug_2197_dead_lease_release_tests.rs"]
mod bug_2197_dead_lease_release_tests;
