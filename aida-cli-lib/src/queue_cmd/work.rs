// trace:TASK-3426 | ai:codex
use super::*;
// trace:BUG-2509 | ai:codex
use crate::rework_direction::ReviewResult;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QueueFreshPickup {
    Pickable,
    Archived,
    Deferred,
    NeedsGuidedOrOperatorSession(aida_core::ExecutionMode),
    NeedsReleaseOperatorSession,
    AwaitingMerge,
    /// BUG-1515: `Done`, but the review that examined this branch is an
    /// OUTSTANDING refusal (RequestChanges/Rejected, never closed by a later
    /// merge — `review_verdict::is_outstanding_refusal`). Distinct from
    /// `AwaitingMerge`: nothing here is ready to ship, it needs a rework
    /// round first, and the two shipping-route hints (`--from-pr`,
    /// `integrate`) both refuse from this state.
    // trace:BUG-1515 | ai:claude
    AwaitingRework,
    Terminal(RequirementStatus),
    Blocked(aida_core::pickability::BlockedReason),
}

/// BUG-1017: one policy for fresh queue pickup surfaces. `Done` stays visible
/// as in-flight / awaiting-merge work, but it is not a valid fresh pickup.
/// `NeedsAttention` is blocked by normal pickability unless an explicit force
/// path is being resolved.
///
/// BUG-1515: a `Done` spec whose recorded review verdict is a still-live
/// refusal is a DIFFERENT ineligibility (`AwaitingRework`, not
/// `AwaitingMerge`) — `project_root` is `None` in callers that cannot cheaply
/// resolve one (e.g. isolated unit tests), which degrades to the pre-BUG-1515
/// `AwaitingMerge` reading rather than erroring.
// trace:BUG-1017 | ai:codex
// trace:BUG-1515 | ai:claude
pub(crate) fn queue_fresh_pickup_policy(
    req: &aida_core::Requirement,
    store: &aida_core::RequirementsStore,
    force_needs_attention: bool,
    project_root: Option<&std::path::Path>,
) -> QueueFreshPickup {
    // trace:BUG-1099 | ai:codex
    if req.deferred {
        return QueueFreshPickup::Deferred;
    }
    if req.archived {
        return QueueFreshPickup::Archived;
    }
    if matches!(req.status, RequirementStatus::Done) {
        if let Some(root) = project_root {
            // BUG-2403: an unselectable review is protective, not merge-ready.
            // trace:BUG-2403 | ai:claude
            if !matches!(
                done_spec_outstanding_refusal(
                    root,
                    req.agreed_id.as_deref().unwrap_or_default(),
                    req.spec_id.as_deref().unwrap_or_default(),
                ),
                RefusalCheck::Clear
            ) {
                return QueueFreshPickup::AwaitingRework;
            }
        }
        return QueueFreshPickup::AwaitingMerge;
    }
    if is_terminal_status(&req.status) {
        return QueueFreshPickup::Terminal(req.status.clone());
    }
    if force_needs_attention && matches!(req.status, RequirementStatus::NeedsAttention) {
        return QueueFreshPickup::Pickable;
    }
    match aida_core::pickability::pickability(req, store) {
        aida_core::pickability::Pickability::Pickable => QueueFreshPickup::Pickable,
        aida_core::pickability::Pickability::Blocked(reason) => QueueFreshPickup::Blocked(reason),
    }
}

pub(crate) fn queue_fresh_pickup_reason_label(policy: &QueueFreshPickup) -> Option<String> {
    match policy {
        QueueFreshPickup::Pickable => None,
        QueueFreshPickup::Archived => Some("archived — skipped".to_string()),
        QueueFreshPickup::Deferred => Some("deferred — skipped".to_string()),
        QueueFreshPickup::NeedsGuidedOrOperatorSession(mode) => Some(format!(
            "skipped — needs guided/operator session ({mode}); use `aida queue work --guided`, `aida do`, or de-risk it with `aida derisk <ID>`"
        )),
        QueueFreshPickup::NeedsReleaseOperatorSession => Some(
            "skipped — release prep is operator-guided; use `/aida-release` at the keyboard"
                .to_string(),
        ),
        QueueFreshPickup::AwaitingMerge => Some(
            "Done — awaiting merge; route via `aida queue work --from-pr` or `aida integrate`"
                .to_string(),
        ),
        // trace:BUG-1515 | ai:claude
        QueueFreshPickup::AwaitingRework => Some(
            "REWORK NEEDED — reviewer requested changes and it is still outstanding; route via \
             `aida queue rework <ID>` then \
             `aida queue work <ID> --auto-complete=through-ci --no-human=both`"
                .to_string(),
        ),
        QueueFreshPickup::Terminal(status) => Some(format!("{status} — already terminal")),
        QueueFreshPickup::Blocked(reason) => {
            Some(aida_core::pickability::pickability_reason_label(reason))
        }
    }
}

/// STORY-1436: which gate slug a fresh-pickup refusal records under — a
/// `BlockedBy` refusal is named distinctly from every other pickability hold.
// trace:STORY-1436 | ai:claude
pub(crate) fn pickup_refusal_gate(pickup: &QueueFreshPickup) -> &'static str {
    use aida_core::pickability::BlockedReason;
    match pickup {
        QueueFreshPickup::Blocked(
            BlockedReason::UnsatisfiedBlocker { .. } | BlockedReason::PermanentlyBlocked { .. },
        ) => crate::events::GATE_BLOCKED_BY_PICKUP,
        _ => crate::events::GATE_QUEUE_PICKUP,
    }
}

/// STORY-1436: record a refused fresh pickup — but never for a dry-run
/// preview (`drain preview`, `queue work --dry-run`), which refuses nothing
/// and would inflate the held-gate counts.
// trace:STORY-1436 | ai:claude
pub(crate) fn record_pickup_refusal(
    project_root: &std::path::Path,
    pickup: &QueueFreshPickup,
    spec: String,
    reason: &str,
    dry_run: bool,
) {
    if dry_run {
        return;
    }
    crate::events::record_gate_held(
        project_root,
        pickup_refusal_gate(pickup),
        Some(spec),
        None,
        reason,
    );
}

pub(crate) fn queue_drain_pickup_policy(
    req: &aida_core::Requirement,
    store: &aida_core::RequirementsStore,
    force_needs_attention: bool,
    project_root: Option<&std::path::Path>,
) -> QueueFreshPickup {
    // Release meta-tasks may be queued and tracked, but never picked up by an
    // unattended drain. Their driver is the at-keyboard `/aida-release` prep
    // flow, which stops before tag/push. trace:STORY-1125 | ai:codex
    if req
        .tags
        .iter()
        .flat_map(|tag| tag.split_whitespace())
        .any(crate::presence::is_release_operator_tag)
    {
        return QueueFreshPickup::NeedsReleaseOperatorSession;
    }
    // trace:BUG-1120 | ai:codex
    if matches!(
        req.execution_mode,
        Some(aida_core::ExecutionMode::Guided)
            | Some(aida_core::ExecutionMode::Operator)
            | Some(aida_core::ExecutionMode::Decide)
    ) {
        return QueueFreshPickup::NeedsGuidedOrOperatorSession(
            req.execution_mode.expect("matched Some execution_mode"),
        );
    }
    queue_fresh_pickup_policy(req, store, force_needs_attention, project_root)
}

// trace:TASK-1234 | ai:codex
pub(crate) fn queued_supervised_mode(
    req: &aida_core::Requirement,
) -> Option<aida_core::ExecutionMode> {
    match req.execution_mode {
        Some(
            mode @ (aida_core::ExecutionMode::Guided
            | aida_core::ExecutionMode::Operator
            | aida_core::ExecutionMode::Decide),
        ) => Some(mode),
        _ => None,
    }
}

/// Keep route inheritance separate from the caller's explicit reviewer intent.
/// Rework may preserve a stored route for pickup without authorizing coexistence.
// trace:TASK-1786 | ai:codex
#[derive(Clone, Copy)]
pub(crate) struct QueueWorkRoleInput<'a> {
    pickup: Option<&'a str>,
    explicit: Option<&'a str>,
}

impl<'a> QueueWorkRoleInput<'a> {
    pub(crate) fn direct(role: Option<&'a str>) -> Self {
        Self {
            pickup: role,
            explicit: role,
        }
    }

    pub(crate) fn review_coexistence<'p>(
        &self,
        plan: &'p QueueWorkPlan,
    ) -> Option<(&'a str, &'p str)> {
        plan.review_target?;
        self.explicit
            .map(|explicit| (explicit, plan.anchor_display.as_str()))
    }
}

/// Rework inherits the route while preserving the raw CLI intent separately.
// trace:TASK-1801 | ai:codex
pub(crate) fn rework_role_input<'a>(
    raw_for: Option<&'a str>,
    resolved_route: Option<&'a str>,
) -> QueueWorkRoleInput<'a> {
    QueueWorkRoleInput {
        pickup: resolved_route,
        explicit: raw_for,
    }
}

/// STORY-42: resolved pickup plan — which queue entries we're working,
/// in which mode, against which scope/branch, with which skill.
/// trace:STORY-42 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct QueueWorkPlan {
    /// "head" (no arg), "item" (single queued entry), or "cluster"
    /// (parent scope draining children).
    pub(crate) mode: QueueWorkMode,
    /// Pre-resolved view of each queued entry we're picking up, in
    /// queue order. Carries the display SPEC-ID and current status
    /// at plan time so manifest writing doesn't need a second store
    /// load (the second load races against `aida db sync --pull` /
    /// concurrent edits and can return a different requirements set
    /// from a sibling worktree's perspective).
    pub(crate) entries: Vec<QueueWorkEntry>,
    /// Resolved scope string for `--owns` (e.g. "EPIC-20", "PR-11",
    /// "BUG-82"). Always non-empty.
    pub(crate) scope: String,
    /// Forge + PR number when scope is a PR-N / MR-N. Lets the caller
    /// route to the review session flow without re-parsing.
    pub(crate) review_target: Option<(ReviewForge, u64)>,
    /// Display SPEC-ID of the "anchor" requirement (the first entry
    /// for head/item modes; the parent for cluster mode). Surfaced in
    /// human output so the user can confirm we resolved the right thing.
    pub(crate) anchor_display: String,
    /// Title of the anchor requirement (e.g., "Review PR-11: …" or the
    /// real spec title). Used to detect Review-PR shape for skill
    /// routing without re-querying.
    pub(crate) anchor_title: String,
}

/// STORY-42: per-entry resolved view used by manifest writing and role
/// tally. trace:STORY-42 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct QueueWorkEntry {
    /// The underlying queue entry (carries position, for_role,
    /// for_scope so the role tally and ordering work).
    pub(crate) queue: aida_core::QueueEntry,
    /// Provenance SPEC-ID for the manifest (spec_id → agreed_id → "?").
    /// Queue-work implementation surfaces must preserve node-qualified origin
    /// ids until the merge gate intentionally promotes them.
    pub(crate) spec_id: String,
    /// Status at plan time, formatted as the manifest field expects
    /// ("Approved", "In Progress", …).
    pub(crate) status_at_plan: String,
}

#[cfg(test)]
pub(crate) fn rework_pr_head_branch_from_lookup(lookup: PrLookup) -> Option<String> {
    match lookup {
        PrLookup::Found(pr) => pr.head_branch.filter(|branch| !branch.trim().is_empty()),
        _ => None,
    }
}

/// `completion_held` is the store-backed closure-hold check
/// ([`pr_ship::spec_completion_hold_reason`]). For rework matching it is
/// Match a spec against a candidate rework PR: an ID in the branch name takes
/// precedence; otherwise the PR title/body completion derivation is consulted.
// trace:BUG-1957 | ai:claude
// trace:BUG-1997 | ai:antigravity
pub(crate) fn rework_pr_head_matches_spec(
    spec: &str,
    head_branch: &str,
    pr_title: &str,
    pr_body: &str,
) -> bool {
    let wanted = spec.trim().to_ascii_uppercase();
    if wanted.is_empty() {
        return false;
    }

    let branch_ids = pr_ship::extract_spec_ids_from_text(head_branch);
    if !branch_ids.is_empty() {
        return branch_ids.iter().any(|id| id.eq_ignore_ascii_case(&wanted));
    }

    let meta_ids = pr_ship::derive_squash_subject_spec_ids(pr_title, head_branch, pr_body);
    meta_ids.iter().any(|id| id.eq_ignore_ascii_case(&wanted))
}

pub(crate) fn queue_work_plan_wants_pr_head_branch(plan: &QueueWorkPlan) -> bool {
    plan.review_target.is_none()
        && plan.entries.len() == 1
        && plan.entries.iter().any(|entry| {
            matches!(
                entry.status_at_plan.as_str(),
                "In Progress" | "InProgress" | "Done"
            )
        })
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PreclaimDecision {
    Proceed,
    Refuse(String),
}

/// BUG-1763: what the pre-claim branch arm knows about the spec's existing
/// local branch. BUG-1761's branch arm refused on *any* existing branch, but
/// this repository retains merged spec branches locally (761 of them as of
/// 2026-10-01 — `gh pr merge --delete-branch` removes only the remote), so
/// the refusal fired on re-pickup of almost every shipped spec. Only an
/// **unmerged** branch is a real collision; re-picking up a spec whose work
/// merged months ago (a follow-on fix, a revert, a rework) is legitimate.
///
/// Merged-ness is resolved through the forge (`gh pr list --head <branch>
/// --state merged`), never via `git merge-base --is-ancestor`: this
/// repository squash-merges, so an ancestry test reports NO for every
/// squash-merged branch and would classify all of them as unmerged —
/// leaving the over-broad refusal in place while appearing to fix it.
///
/// `MergeStateUnknown` is deliberately distinct from `Unmerged`: a forge
/// lookup failure means "cannot tell", not "unmerged", and refusing on it
/// would let a network blip block every offline pickup — the same trap
/// BUG-1761 avoided in the PR arm.
// trace:BUG-1763 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreclaimBranchState {
    /// No local branch for the spec — the branch arm has nothing to say.
    Absent,
    /// Branch exists and the forge confirmed a MERGED PR with it as head.
    Merged,
    /// Branch exists and the forge answered cleanly that no merged PR has
    /// it as head — the one true collision class.
    Unmerged,
    /// Branch exists but the forge could not answer (gh missing, failed,
    /// unreachable). Fail open.
    MergeStateUnknown,
}

/// BUG-1763: resolve the branch arm's state, invoking `merged_probe` (the
/// forge lookup) **only** when the branch actually exists — an ordinary
/// pickup of a spec with no local branch pays no `gh` call beyond the PR
/// arm it already paid before this bug.
// trace:BUG-1763 | ai:claude
pub(crate) fn preclaim_branch_state(
    branch_exists: bool,
    merged_probe: impl FnOnce() -> crate::PrLookup,
) -> PreclaimBranchState {
    if !branch_exists {
        return PreclaimBranchState::Absent;
    }
    match merged_probe() {
        crate::PrLookup::Found(_) => PreclaimBranchState::Merged,
        crate::PrLookup::NoOpenPr => PreclaimBranchState::Unmerged,
        crate::PrLookup::GhMissing
        | crate::PrLookup::GhFailed(_)
        | crate::PrLookup::GhUnreachable(_) => PreclaimBranchState::MergeStateUnknown,
    }
}

/// BUG-1761: the pre-claim collision decision for `aida queue work <SPEC>`.
///
/// The command used to signal "someone is already on this spec" only as a
/// branch-name suffix. It had the information and discarded it: it had to
/// check that `bug-1819` existed in order to pick `bug-1819-2`, so at that
/// moment it knew a branch for the spec existed — and it bumped the status,
/// took a lease and minted a worktree anyway, next to an open PR another
/// agent was actively implementing.
///
/// Pure by design, in the shape of `dup_pickup_recheck`: the caller runs the
/// forge and branch probes and passes primitives in, so the whole truth table
/// is unit-testable without a forge. Ordering matters — an open PR is the more
/// specific and more actionable collision, so it is reported in preference to
/// the branch that PR is built on.
///
/// `branch_state` must already encode forge ignorance: a merged-PR lookup
/// failure arrives as [`PreclaimBranchState::MergeStateUnknown`], never as
/// `Unmerged`. A lookup failure means "cannot tell", not "no PR", and
/// refusing on it would let a network blip block every pickup on the machine.
///
/// BUG-1763 narrowed the branch arm: only an **unmerged** branch refuses.
/// A merged local branch is the normal residue of this repo's retention
/// policy and must not block re-pickup. Session allocation uses a dated name
/// after confirming the merge and absence of an open change for that branch.
// trace:BUG-1761 | ai:codex
// trace:BUG-1763 | ai:claude
pub(crate) fn preclaim_collision_check(
    spec_display: &str,
    open_pr: Option<(u64, &str, &str)>,
    branch: &str,
    branch_state: PreclaimBranchState,
    bypass: bool,
) -> PreclaimDecision {
    if bypass {
        return PreclaimDecision::Proceed;
    }
    if let Some((number, title, url)) = open_pr {
        return PreclaimDecision::Refuse(format!(
            "{spec_display} has open PR #{number} ({title}) at {url}. The PR is OPEN. Pick this spec up individually with `aida queue work {spec_display}` to retain its head branch; a cluster cannot choose a different branch.",
        ));
    }
    if branch_state == PreclaimBranchState::Unmerged {
        return PreclaimDecision::Refuse(format!(
            "Branch `{branch}` exists, no open PR was found for it, and the forge reports no merged PR with it as head — it looks like unfinished work. `queue work` will not silently allocate `{branch}-2`. Pass `--branch <name>` to choose deliberately, or delete the stale branch."
        ));
    }
    PreclaimDecision::Proceed
}

// Preserve the distinction between a child that could not start and a host
// publication failure after the child ran.
// trace:TASK-3608 | ai:codex
#[derive(Debug)]
pub(crate) struct ResumePublicationFailure(pub anyhow::Error);
impl std::fmt::Display for ResumePublicationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "resumed implementer publication failed: {:#}", self.0)
    }
}
impl std::error::Error for ResumePublicationFailure {}

/// Explicit queue resumes must publish before returning control to the
/// launcher's process exit, under the same vendor/role gate as fresh lanes.
// trace:STORY-2475 | ai:codex
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_queue_resume_to_exit(
    host_main: Option<&std::path::Path>,
    target: &session::HeadlessResume<'_>,
    prompt: &str,
    log: &std::path::Path,
    tree: &std::path::Path,
    tee: &headless_tee::TeeOptions,
    contained: bool,
    role: &str,
    publication: (&std::path::Path, &str, &str),
    record_no_change: bool,
) -> Result<std::process::ExitStatus> {
    // trace:STORY-2640 | ai:codex — explicit resumes retain the same host gate.
    let implementing = crate::canonical_role_name(role) == "implementer";
    let host_published =
        target.vendor == session::HeadlessVendor::Codex && contained && implementing;
    let mut publish_native = host_published;
    let main = crate::main_worktree_root_from(publication.0);
    let mut _fast_gate = None;
    let branch = if implementing {
        if let Some(lease) = crate::list_leases(&main)
            .into_iter()
            .find(|l| l.id == publication.2)
        {
            _fast_gate =
                Some(crate::lane_fast_gate::reserve(&main, &lease.branch)?.with_result_line());
            Some(lease.branch)
        } else {
            crate::lane_fast_gate::missing_lease(&main, None);
            anyhow::ensure!(
                !host_published,
                "publication lease {} is missing",
                publication.2
            );
            publish_native = false;
            None
        }
    } else {
        None
    };
    let mut status = session::spawn_vendor_headless_resume(
        host_main, target, prompt, log, tree, tee, contained, role,
    )?;
    // trace:BUG-2488 | ai:codex
    if implementing {
        crate::standalone_implementer::reconcile_signals(publication.0, tree, publication.1)?;
    }
    // trace:STORY-2640 | ai:codex
    let branch =
        if implementing && !host_published && !crate::lane_fast_gate::veto(&main, publication.1) {
            match _fast_gate.as_mut() {
                Some(reservation) => Some(reservation.exit_branch(tree)),
                None => branch,
            }
        } else {
            branch
        };
    if let Some(branch) = branch.as_deref() {
        if host_published {
            publish_native = crate::lane_fast_gate::native(
                &main,
                tree,
                publication.1,
                publication.2,
                branch,
                status.success(),
                |p| {
                    status = session::spawn_vendor_headless_resume(
                        host_main,
                        target,
                        p,
                        &log.with_extension("fast-fix.jsonl"),
                        tree,
                        tee,
                        contained,
                        role,
                    )?;
                    crate::standalone_implementer::reconcile_signals(
                        publication.0,
                        tree,
                        publication.1,
                    )?;
                    Ok(status.success())
                },
            )?;
        } else if let Some((pr, _)) = crate::standalone_implementer::lookup_pr(&main, branch) {
            let queued = crate::lane_fast_gate::pushed(
                &main,
                tree,
                publication.1,
                publication.2,
                branch,
                pr,
                target.vendor.as_str(),
            )?;
            eprintln!("fast_gate={}", crate::lane_fast_gate::marker(&main, branch));
            anyhow::ensure!(
                !queued,
                "fast gate failed; one rework round queued before review"
            );
        }
    }
    if publish_native {
        let result = crate::ship_from_seat::publish_queue_exit_recorded(
            publication.0,
            tree,
            publication.1,
            publication.2,
            status.success(),
            record_no_change,
        );
        if let Some(branch) = branch.as_deref() {
            crate::lane_fast_gate::publication_result(&main, branch, result)
                .map_err(ResumePublicationFailure)?;
        } else {
            result.map_err(ResumePublicationFailure)?;
        }
    }
    Ok(status)
}

/// Resolve the host merge result before any vendor is launched. Successful
/// conflicts require advisor delivery; an existing non-Claude thread refuses.
// trace:STORY-2475 | ai:codex
pub(crate) fn rework_conflict_route(
    prepared: Result<Vec<String>>,
    vendor: session::HeadlessVendor,
    resume: bool,
    scope: &str,
    pr: u64,
    branch: &str,
    notify: impl FnOnce(&str, &str) -> Result<()>,
) -> Result<(session::HeadlessVendor, Vec<String>)> {
    let files = prepared?;
    if files.is_empty() {
        return Ok((vendor, files));
    }
    notify(
        "Rework merge conflicts require Claude",
        &format!(
            "{scope} PR #{pr} conflicts with origin/main in: {}. The host aborted the merge. \
         Conflict resolution requires Claude on branch {branch}. Fresh route: \
         aida queue work {scope} --vendor claude --fresh --no-human.",
            files.join(", ")
        ),
    )?;
    if resume && vendor != session::HeadlessVendor::Claude {
        anyhow::bail!(
            "conflicting rework requires a fresh Claude lane: \
            aida queue work {scope} --vendor claude --fresh --no-human"
        );
    }
    Ok((session::HeadlessVendor::Claude, files))
}

/// The launch facts [`apply_rework_conflict_route`] needs from the pickup.
// trace:STORY-2475 | ai:claude
pub(crate) struct ReworkConflictLaunch<'a> {
    pub(crate) vendor: session::HeadlessVendor,
    pub(crate) resume: bool,
    pub(crate) interactive: bool,
    pub(crate) contained: bool,
    pub(crate) scope: &'a str,
    pub(crate) pr: u64,
    pub(crate) branch: &'a str,
}

/// Apply the host merge result to a pickup whose seat (linked worktree or
/// clone) and lease already exist. A clean merge keeps the planned vendor,
/// model and prompt. A conflict reroutes the launch to Claude in the same
/// seat: the override is installed, the vendor is resolved and preflighted
/// again (binary, `[agents] enabled`, containment), the vendor-specific model
/// pin is dropped, and the conflict task is appended to the prompt. An
/// interactive pickup (`!no_human`) gets the same reroute: the launch below
/// starts interactive Claude, not Codex. Any refusal returns before
/// `agent.starting()`, so the BUG-2428 setup rollback releases the seat.
// trace:STORY-2475 | ai:claude
pub(crate) fn apply_rework_conflict_route(
    project_root: &Path,
    prepared: Result<Vec<String>>,
    launch: ReworkConflictLaunch<'_>,
    prompt: &mut String,
    resolved_model: &mut Option<String>,
    notify: impl FnOnce(&str, &str) -> Result<()>,
) -> Result<session::HeadlessVendor> {
    let (route_vendor, files) = rework_conflict_route(
        prepared,
        launch.vendor,
        launch.resume,
        launch.scope,
        launch.pr,
        launch.branch,
        notify,
    )?;
    if files.is_empty() {
        return Ok(launch.vendor);
    }
    session::install_headless_vendor_override(route_vendor.as_str());
    let vendor = session::resolve_enabled_headless_vendor(project_root)?;
    session::preflight_launch_vendor(vendor, launch.interactive)?;
    session::require_vendor_containment(vendor, launch.contained, "conflict rework")?;
    std::env::remove_var("AIDA_AGENT_MODEL");
    *resolved_model = None;
    prompt.push_str(&format!(
        "\n\n{}",
        crate::rework_direction::conflict_task(&crate::rework_direction::MainConflict {
            pr: launch.pr,
            base: "main".into(),
            files,
        })
    ));
    Ok(vendor)
}

// trace:BUG-2451 | ai:codex
pub(crate) fn rework_pr_head_branch_override(
    forge: &dyn crate::forge::Forge,
    plan: &QueueWorkPlan,
    explicit_branch: Option<&str>,
) -> Result<Option<(u64, String)>> {
    if plan.review_target.is_some() {
        return Ok(None);
    }
    let mut existing = None;
    for entry in &plan.entries {
        if let Some(change) =
            crate::session_start::pickup_pr_branch(forge, &entry.spec_id, explicit_branch)?
        {
            if plan.entries.len() != 1 {
                anyhow::bail!("{} already has open change #{} on `{}`; pick it up individually to continue that branch", entry.spec_id, change.0, change.1);
            }
            existing = Some(change);
        }
    }
    Ok(existing)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueWorkMode {
    Head,
    Item,
    Cluster,
}

// BUG-1082: a live implementer lease means the worker is already assigned.
// Queue consumers inside that worktree must surface the lease scope instead
// of rediscovering whatever currently sits at the queue head.
// trace:BUG-1082 | ai:codex
pub(crate) fn leased_implementer_queue_head(
    entries: &[aida_core::QueueEntry],
    store: &aida_core::RequirementsStore,
    user_id: &str,
    active_lease: Option<&SessionLease>,
) -> Option<aida_core::QueueEntry> {
    let lease = active_lease?;
    if !lease
        .role
        .as_deref()
        .map(|r| r.eq_ignore_ascii_case("implementer"))
        .unwrap_or(false)
    {
        return None;
    }
    let req = store
        .requirements
        .iter()
        .find(|r| spec_matches(r, &lease.scope))?;
    // A lease can outlive the moment `queue done` flips the spec to Done. Do
    // not synthesize that finished scope as the next fresh assignment.
    // trace:TASK-1-136 | ai:codex
    if req.archived
        || req.deferred
        || matches!(
            req.status,
            RequirementStatus::Done
                | RequirementStatus::Completed
                | RequirementStatus::Rejected
                | RequirementStatus::Superseded
        )
    {
        return None;
    }
    if let Some(entry) = entries.iter().find(|e| e.requirement_id == req.id) {
        return Some(entry.clone());
    }
    Some(aida_core::QueueEntry {
        user_id: user_id.to_string(),
        requirement_id: req.id,
        position: i64::MIN,
        added_by: user_id.to_string(),
        note: Some(format!("pinned by active implementer lease {}", lease.id)),
        added_at: chrono::Utc::now(),
        for_role: Some("implementer".to_string()),
        for_scope: None,
        for_session: Some(lease.id.clone()),
        added_by_machine: None,
    })
}

/// STORY-42: resolve the user's queue-work argument into a concrete plan.
///
///   - `arg = None`            → head-pickup mode: top item from the
///     active role's queue (with the same
///     role + scope filters `queue next` uses).
///   - `arg` matches a queued  → item-pickup mode: just that entry.
///     entry's spec_id/agreed_id/uuid
///   - `arg` resolves to a req → cluster-pickup mode: every queued entry
///     that isn't queued but   whose for_scope == <id> OR whose req's
///     has queued children    derived parent EPIC == <id>.
///
/// `type_filter` only applies in cluster mode (filters drained children
/// by req type, case-insensitive). trace:STORY-42 | ai:claude
/// TASK-560: `--auto-complete` and `--resume` are mutually exclusive
/// (auto-complete drives a FRESH implementer→CI→reviewer→merge pipeline;
/// --resume continues an EXISTING session — the two can't both own the run).
/// Returns the helpful rejection message when both are set, else `None`.
/// Pure so the message (the WHY + both recovery paths) is unit-testable
/// without running the launcher. trace:TASK-560 | ai:claude
pub(crate) fn resume_autocomplete_conflict_message(
    resume: bool,
    auto_complete: bool,
) -> Option<String> {
    if resume && auto_complete {
        Some(
            "--auto-complete cannot be used with --resume.\n\
             auto-complete drives a FRESH implementer→CI→reviewer→merge→pull→build \
             pipeline; --resume continues an EXISTING session — the two can't both \
             own the run.\n\n  \
             To continue the existing session:    re-run with --resume alone.\n  \
             To start a fresh orchestrator drain:  end the existing session first \
             (`aida session end <id>`), then re-run with --auto-complete."
                .to_string(),
        )
    } else {
        None
    }
}

pub(crate) fn resolve_queue_work_plan(
    storage: &Storage,
    user_id: &str,
    arg: Option<&str>,
    type_filter: Option<&str>,
    strict: bool,
    // TASK-1053: under a dry-run, the convenience auto-queue of an explicit
    // Approved-but-unqueued spec must NOT persist — we synthesize the queue
    // entry in-memory so the plan resolves identically to a real pickup while
    // mutating nothing. trace:TASK-1053 | ai:claude
    dry_run: bool,
    force_needs_attention: bool,
    // BUG-1195: the explicit `--role` (if any) — the routing role for a
    // non-review scope; ignored for PR/MR scopes, which imply `reviewer`.
    role_hint: Option<&str>,
) -> Result<QueueWorkPlan> {
    // A `--for <role>` routing lands in the ROUTING user's queue file, so a
    // pickup that read only our own file could not work a spec a peer routed
    // to our role. Widen the read to the active/default work role's cross-user
    // routings so coordinator writes to `role:implementer` are drivable by a
    // bare default drain.
    // trace:BUG-774 | ai:claude
    // trace:BUG-900 | ai:codex
    let session_role_for_work =
        queue_role_fallback::session_role_env().unwrap_or_else(|| "implementer".to_string());
    // BUG-1195 (round 2): a `PR-N` / `MR-N` scope IS a review by construction,
    // so the cross-user routing it reads through is the reviewer route — not
    // whatever role the calling shell happens to carry. A dry-run from an
    // advisor shell (`AIDA_SESSION_ROLE=advisor`) used to derive `advisor`
    // here, never merged the reviewer-routed story another user had filed,
    // and reported "no queued review story" while `queue work STORY-N`
    // resolved fine. An explicit `--role` still wins for non-review scopes.
    // trace:BUG-1195 | ai:claude
    let work_fallback_role = if arg.and_then(parse_review_scope).is_some() {
        Some("reviewer".to_string())
    } else {
        queue_role_fallback::fallback_role(role_hint, Some(session_role_for_work.as_str()))
    };
    let mut entries = queue_role_fallback::queue_list_with_role_fallback(
        storage,
        user_id,
        work_fallback_role.as_deref(),
        /* include_completed */ false,
    )?;
    let store = storage.load()?;

    if let Some(arg_str) = arg {
        if let Some(req) = store.requirements.iter().find(|r| spec_matches(r, arg_str)) {
            // A deferred spec is an explicit operator hold. Refuse before
            // queue synthesis or other pickup side effects. trace:BUG-1793
            if let Some(refusal) = crate::gate::operator_hold::operator_hold_refusal(
                req.spec_id.as_deref().unwrap_or(arg_str),
                req.deferred,
                req.deferred_reason.as_deref(),
                req.deferred_until.as_deref(),
            ) {
                return Err(refusal);
            }
            // trace:BUG-1106 | ai:codex
            // A phase child is assigned by the live orchestrator, not by its
            // temporary phase role. If the reviewer phase is asked to pick up
            // the current drain spec, pull that exact queued row into view even
            // when the row lives in a different queue identity or is routed for
            // the implementer. The live token + drain-state current spec are
            // the authorization; ordinary/stale reviewer sessions still see
            // the normal role-gated view.
            if orchestrator_authorizes_explicit_queue_pickup(storage, arg_str, req) {
                merge_current_spec_queue_entries(storage, &mut entries, req.id);
            }
            let is_queued = entries.iter().any(|e| e.requirement_id == req.id);
            if !is_queued && req.status == RequirementStatus::Approved && !strict {
                let role = std::env::var("AIDA_SESSION_ROLE")
                    .unwrap_or_else(|_| "implementer".to_string());
                let entry = aida_core::QueueEntry {
                    user_id: user_id.to_string(),
                    requirement_id: req.id,
                    position: i64::MAX,
                    added_by: user_id.to_string(),
                    note: None,
                    added_at: chrono::Utc::now(),
                    for_role: Some(role.clone()),
                    for_scope: None,
                    for_session: None,
                    added_by_machine: None,
                };
                if dry_run {
                    // TASK-1053: preview only — push the synthesized entry so the
                    // Item-pickup match below resolves the same plan a real pickup
                    // would, but persist nothing (no queue_add, no role-activity,
                    // no "queued …" line). trace:TASK-1053 | ai:claude
                    entries.push(entry);
                } else {
                    storage.queue_add(entry)?;
                    let display_id = req
                        .agreed_id
                        .as_deref()
                        .or(req.spec_id.as_deref())
                        .unwrap_or(arg_str);
                    record_role_activity(display_id, "queue-add");
                    println!("queued {} for role:{}", display_id, role);
                    // Reload entries to include the auto-queued entry.
                    // trace:TASK-547 | ai:antigravity
                    entries = queue_role_fallback::queue_list_with_role_fallback(
                        storage,
                        user_id,
                        work_fallback_role.as_deref(),
                        /* include_completed */ false,
                    )?;
                }
            }
        }
    }

    // Head-pickup: pick the top item for the active role, honoring the
    // same filters as `queue next` (role routing + active-role scope +
    // terminal-status skip). We don't re-implement that whole filter
    // chain here; queue work is best-effort and the user can pass an
    // explicit id when the head heuristic picks the wrong thing.
    if arg.is_none() {
        let role_filter: Option<String> = std::env::var("AIDA_SESSION_ROLE")
            .ok()
            .filter(|s| !s.is_empty());
        let active_lease_for_head = find_project_root().ok().and_then(|root| {
            std::env::current_dir()
                .ok()
                .and_then(|cwd| active_lease_for_cwd(&root, &cwd))
        });
        // STORY-333: collect un-pickable specs (blocked-by / human-only)
        // skipped during head resolution so the kickoff banner can name
        // them. Silent skipping looks like "queue is empty" — we surface
        // the reason instead. trace:STORY-333 | ai:claude
        let mut skipped_unpickable: Vec<(String, String)> = Vec::new();
        let head = leased_implementer_queue_head(
            &entries,
            &store,
            user_id,
            active_lease_for_head.as_ref(),
        )
        .or_else(|| {
            entries
            .iter()
            .filter(|e| match &role_filter {
                Some(r) => e.for_role.as_deref() == Some(r.as_str()),
                None => true,
            })
            .filter(|e| {
                // STORY-333: pre-pickup gate. Skip un-pickable specs so the
                // orchestrator never spawns a doomed phase-1 implementer on
                // them. Reasons are recorded for the banner.
                let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id) else {
                    return true;
                };
                match queue_fresh_pickup_policy(
                    req,
                    &store,
                    force_needs_attention,
                    storage.path().parent(),
                ) {
                    QueueFreshPickup::Pickable => true,
                    other => {
                        let display = req
                            .agreed_id
                            .clone()
                            .or_else(|| req.spec_id.clone())
                            .unwrap_or_else(|| "?".to_string());
                        let reason = queue_fresh_pickup_reason_label(&other)
                            .unwrap_or_else(|| "not pickable".to_string());
                        skipped_unpickable.push((
                            display,
                            reason,
                        ));
                        false
                    }
                }
            })
            .min_by_key(|e| e.position)
            .cloned()
        })
            .ok_or_else(|| {
                let suffix = if skipped_unpickable.is_empty() {
                    String::new()
                } else {
                    let listed = skipped_unpickable
                        .iter()
                        .map(|(s, r)| format!("{} ({})", s, r))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(
                        "\n  Skipped {} un-pickable spec(s): {}\n  \
                         (Run `aida queue list` to see the Blocked section.)",
                        skipped_unpickable.len(),
                        listed
                    )
                };
                // BUG-465: on a FRESH project (no real, non-META specs yet) the
                // queue is legitimately empty — guide the new user's first
                // action instead of dead-ending with a bare error. EPIC-37:
                // fresh init -> first `queue work` must be boring and correct.
                // trace:BUG-465 | ai:claude
                let has_real_specs = store
                    .requirements
                    .iter()
                    .any(|r| !matches!(r.req_type, aida_core::RequirementType::Meta));
                // STORY-737 (delight #5): an empty queue on day one is the
                // EXPECTED state, not a failure. On the HUMAN path, render a
                // soft, forward-pointing signpost (info glyph — NOT a red
                // `Error:`) and return the `SoftSignpostShown` sentinel so the
                // top-level handler suppresses the error render (exit still
                // non-zero so scripts keep gating). Agent mode keeps the
                // structured error it parses. trace:STORY-737 | ai:claude
                if !agent_output_mode() {
                    if !has_real_specs && skipped_unpickable.is_empty() {
                        eprintln!(
                            "{} Your queue is empty — looks like a fresh project. Get started:\n  \
                             1. File a spec:  aida add --title \"<what you're building>\" --type task --status approved\n  \
                             2. Queue it:     aida queue add <SPEC-ID>   (the id printed by step 1)\n  \
                             3. Work it:      aida queue work\n  \
                             Browse anytime with `aida list`.",
                            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
                        );
                    } else {
                        eprintln!(
                            "{} Nothing queued yet — that's the expected day-one state. \
                             Approve a draft and queue it in one step: \
                             `aida add \"<what you're building>\" --queue`, \
                             or queue an existing spec: `aida queue add <id>`.{}",
                            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                            suffix
                        );
                    }
                    return anyhow::Error::new(SoftSignpostShown);
                }
                if !has_real_specs && skipped_unpickable.is_empty() {
                    anyhow::anyhow!(
                        "Your queue is empty — looks like a fresh project. Get started:\n  \
                         1. File a spec:  aida add --title \"<what you're building>\" --type task --status approved\n  \
                         2. Queue it:     aida queue add <SPEC-ID>   (the id printed by step 1)\n  \
                         3. Work it:      aida queue work\n  \
                         Browse anytime with `aida list`."
                    )
                } else {
                    anyhow::anyhow!(
                        "queue is empty for {}; pass an id explicitly or run `aida queue list`{}",
                        role_filter.as_deref().unwrap_or("any role"),
                        suffix
                    )
                }
            })?;
        if !skipped_unpickable.is_empty() {
            let listed = skipped_unpickable
                .iter()
                .map(|(s, r)| format!("{} ({})", s, r))
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!(
                "  {} skipped {} un-pickable spec(s) ahead of head: {}",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                skipped_unpickable.len(),
                listed
            );
        }
        let req = store
            .requirements
            .iter()
            .find(|r| r.id == head.requirement_id)
            .ok_or_else(|| anyhow::anyhow!("queue head's requirement is missing from the store"))?;
        let (scope, review_target) = derive_scope_from_entry(&head, req);
        let anchor_display = req
            .agreed_id
            .clone()
            .or_else(|| req.spec_id.clone())
            .unwrap_or_else(|| "?".to_string());
        let resolved = build_resolved_entry(head, req);
        return Ok(QueueWorkPlan {
            mode: QueueWorkMode::Head,
            entries: vec![resolved],
            scope,
            review_target,
            anchor_display,
            anchor_title: req.title.clone(),
        });
    }

    let arg = arg.unwrap();

    // Item-pickup: look for a queue entry whose req matches the arg by
    // uuid / spec_id / agreed_id (case-insensitive).
    let matched_item: Option<aida_core::QueueEntry> = entries
        .iter()
        .find(|e| {
            let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id) else {
                return false;
            };
            spec_matches(req, arg)
        })
        .cloned();

    if let Some(entry) = matched_item {
        let req = store
            .requirements
            .iter()
            .find(|r| r.id == entry.requirement_id)
            .unwrap();
        let pickup =
            queue_fresh_pickup_policy(req, &store, force_needs_attention, storage.path().parent());
        if let Some(reason) = queue_fresh_pickup_reason_label(&pickup) {
            // STORY-1436: the pickup gate HELD — record it so a seat refused
            // work is distinguishable from an idle one. trace:STORY-1436 | ai:claude
            if let Some(root) = storage.path().parent() {
                record_pickup_refusal(root, &pickup, req.display_id(), &reason, dry_run);
            }
            anyhow::bail!(
                "`{}` is not pickable for fresh work: {}",
                req.friendly_id(),
                reason
            );
        }
        let (scope, review_target) = derive_scope_from_entry(&entry, req);
        let anchor_display = req
            .agreed_id
            .clone()
            .or_else(|| req.spec_id.clone())
            .unwrap_or_else(|| arg.to_string());
        let resolved = build_resolved_entry(entry, req);
        return Ok(QueueWorkPlan {
            mode: QueueWorkMode::Item,
            entries: vec![resolved],
            scope,
            review_target,
            anchor_display,
            anchor_title: req.title.clone(),
        });
    }

    // TASK-85: PR-N / MR-N pickup by review-story title. The auto-queued
    // review stories live in the queue with titles like "Review PR-14:
    // ...", but their spec_id (e.g., STORY-103) isn't memorable. Let
    // `aida queue work PR-14` find the corresponding queue entry by
    // title prefix so the user doesn't have to look up the story id.
    // Composes with STORY-66/STORY-90 (auto-queue at PR-create) which
    // produce these stories. trace:TASK-85 | ai:claude
    if let Some((forge, n)) = parse_review_scope(arg) {
        // Auto-queue writes under the producer's queue identity. Reviewers may
        // run under a role-scoped or ordinary identity, so resolve PR/MR
        // hand-offs from every queue file using the same reviewer-route
        // semantics as the writer. trace:BUG-1817 | ai:codex
        let review_entries = queue_role_fallback::queue_entries_routed_to_role(
            storage, "reviewer", /* include_completed */ false,
        )?;
        let matches: Vec<aida_core::QueueEntry> = review_entries
            .iter()
            .filter(|e| {
                let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id) else {
                    return false;
                };
                if !matches!(
                    queue_fresh_pickup_policy(
                        req,
                        &store,
                        force_needs_attention,
                        storage.path().parent(),
                    ),
                    QueueFreshPickup::Pickable
                ) {
                    return false;
                }
                review_title_matches(&req.title, forge, n)
            })
            .cloned()
            .collect();
        // BUG-2150: an explicit reviewer pickup with no queued review story
        // reviews the PR against its backing spec directly — the review
        // lease is PR-scoped, so it needs neither a queue entry nor the
        // implementer's spec lease released. trace:BUG-2150 | ai:claude
        // BUG-2208: a fallback that cannot resolve the backing spec says why
        // (ambiguous, none) instead of only "no queued review story".
        // trace:TASK-1955 | ai:claude (BUG-2208)
        let mut fallback_refusal: Option<String> = None;
        if matches.is_empty() && reviewer_pickup_keeps_pr_scope(role_hint) {
            if let Some(project_root) = storage.path().parent() {
                match resolve_pr_to_spec(project_root, n as u32, &store) {
                    Ok(spec) => {
                        if let Some(req) =
                            store.requirements.iter().find(|r| spec_matches(r, &spec))
                        {
                            return Ok(standalone_review_plan(forge, n, req, user_id));
                        }
                        fallback_refusal = Some(format!("backing spec {spec} is not in the store"));
                    }
                    Err(err) => fallback_refusal = Some(format!("{err:#}")),
                }
            }
        }
        match matches.len() {
            0 => {
                let label = format_review_label(forge, n);
                // BUG-1195: name the filter that dropped it — the same
                // classifier the review-envelope guard uses.
                let why =
                    crate::classify_review_story_lookup(&entries, &store, forge, n).describe();
                let identities = storage.queue_users().unwrap_or_default();
                let searched = if identities.is_empty() {
                    "no persisted queue identities".to_string()
                } else {
                    format!(
                        "queue identities [{}] for role reviewer",
                        identities.join(", ")
                    )
                };
                let fallback = fallback_refusal
                    .map(|why| format!("; reviewing without a story failed too: {why}"))
                    .unwrap_or_default();
                anyhow::bail!(
                    "no queued review story for {} ({}) after searching {}{}; check `gh pr view {}` (or `glab mr view {}`) and run `aida pr auto-queue-review --branch <branch>` if needed",
                    label,
                    why,
                    searched,
                    fallback,
                    n,
                    n
                );
            }
            _ => {
                // Legacy retries may have left duplicates. Select the same
                // stable canonical story every time instead of blocking phase
                // 3; auto-queue will stop creating new duplicates.
                let persisted = find_main_worktree_root()
                    .ok()
                    .and_then(|root| drain_state::DrainState::read(&root))
                    .and_then(|state| state.review_spec);
                let canonical =
                    canonical_review_story(&store, forge, n, None, persisted.as_deref())
                        .and_then(|req| matches.iter().find(|e| e.requirement_id == req.id))
                        .or_else(|| matches.iter().min_by_key(|e| e.requirement_id))
                        .cloned()
                        .expect("non-empty matches");
                let entry = canonical;
                let req = store
                    .requirements
                    .iter()
                    .find(|r| r.id == entry.requirement_id)
                    .unwrap();
                // PR review pickup: scope + review_target come straight from
                // the (forge, n) parse_review_scope just gave us. We don't
                // delegate to derive_scope_from_entry because its title-based
                // path is case-sensitive on `strip_prefix("Review ")` while
                // review_title_matches is case-insensitive — a lowercase
                // "review pr-14: …" title would pass the matcher but fall
                // through to the spec_id fallback, losing the PR scope and
                // the --pr N reviewer-skill route. trace:TASK-85 | ai:claude
                let scope = format_review_label(forge, n);
                let review_target = Some((forge, n));
                let anchor_display = req
                    .agreed_id
                    .clone()
                    .or_else(|| req.spec_id.clone())
                    .unwrap_or_else(|| arg.to_string());
                let resolved = build_resolved_entry(entry, req);
                return Ok(QueueWorkPlan {
                    mode: QueueWorkMode::Item,
                    entries: vec![resolved],
                    scope,
                    review_target,
                    anchor_display,
                    anchor_title: req.title.clone(),
                });
            }
        }
    }

    // Cluster-pickup: arg names a req that isn't queued itself but has
    // queued children. Find them via for_scope match OR derived parent
    // EPIC match. The arg must resolve to a req in the store.
    let anchor_req = store
        .requirements
        .iter()
        .find(|r| spec_matches(r, arg))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "`{}` doesn't match any queued entry or known requirement",
                arg
            )
        })?;
    let anchor_id_upper = anchor_req
        .agreed_id
        .as_deref()
        .or(anchor_req.spec_id.as_deref())
        .map(|s| s.to_uppercase())
        .unwrap_or_default();
    let type_filter_lower = type_filter.map(|s| s.to_ascii_lowercase());

    // BUG-1126: an anchor with a large archived cluster (e.g. a long-lived
    // EPIC whose shipped children were archived) previously printed one
    // "skipping un-pickable … (archived — skipped)" line per member — dozens
    // of lines of noise before the real result. Archived skips carry no
    // action, so tally them and print a single summary; keep the other
    // un-pickable reasons (blocked-by, human-only, …) enumerated since those
    // are rare and tell the user why a member was held back.
    // trace:BUG-1126 | ai:claude
    let mut archived_skipped: usize = 0;
    let mut cluster: Vec<aida_core::QueueEntry> = Vec::new();
    for e in entries.iter() {
        let Some(req) = store.requirements.iter().find(|r| r.id == e.requirement_id) else {
            continue;
        };
        // STORY-333: cluster drains must skip un-pickable members so
        // the orchestrator never spawns phase 1 on a blocked-by /
        // human-only spec. Same gate as head pickup + batch drain.
        // trace:STORY-333 | ai:claude
        match queue_fresh_pickup_policy(req, &store, force_needs_attention, storage.path().parent())
        {
            QueueFreshPickup::Pickable => {}
            QueueFreshPickup::Archived => {
                archived_skipped += 1;
                continue;
            }
            other => {
                let reason = queue_fresh_pickup_reason_label(&other)
                    .unwrap_or_else(|| "not pickable".to_string());
                eprintln!(
                    "  {} cluster {} — skipping un-pickable {} ({})",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    anchor_id_upper,
                    req.friendly_id(),
                    reason,
                );
                continue;
            }
        }
        // Match by explicit for_scope.
        let scope_match = e
            .for_scope
            .as_deref()
            .map(|s| s.eq_ignore_ascii_case(&anchor_id_upper))
            .unwrap_or(false);
        // Match by derived parent EPIC (when anchor is itself an
        // EPIC; otherwise the function returns None so this branch
        // is harmless).
        let parent_match = derive_parent_epic_label(req, &store)
            .map(|p| p.eq_ignore_ascii_case(&anchor_id_upper))
            .unwrap_or(false);
        if !(scope_match || parent_match) {
            continue;
        }
        // Type filter (cluster only).
        if let Some(want) = &type_filter_lower {
            let actual = format!("{:?}", req.req_type).to_ascii_lowercase();
            if actual != *want {
                continue;
            }
        }
        cluster.push(e.clone());
    }
    if archived_skipped > 0 {
        eprintln!(
            "  {} cluster {} — skipped {} archived member{}",
            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
            anchor_id_upper,
            archived_skipped,
            if archived_skipped == 1 { "" } else { "s" },
        );
    }

    if cluster.is_empty() {
        // TASK-217: status-aware recovery hint. The user typed an id that
        // resolves to a known spec but isn't queued (and has no queued
        // children). Branch on the anchor's current status to suggest the
        // recovery the user most likely wants — re-queueing after a
        // Done→rework cycle, promoting Planned to Approved, etc. — instead
        // of the previous opaque "no queued children match" hint.
        // trace:TASK-217 | ai:claude
        let role = std::env::var("AIDA_SESSION_ROLE")
            .ok()
            .filter(|s| !s.is_empty());
        let display_id = anchor_req
            .agreed_id
            .as_deref()
            .or(anchor_req.spec_id.as_deref())
            .unwrap_or(&anchor_id_upper);
        // Before guessing at a lost lease, check whether the spec is simply
        // sitting in ANOTHER user's queue file (the routing user's, not ours).
        // "Not in YOUR queue" and "not queued at all" are different states and
        // the old message conflated them. trace:BUG-774 | ai:claude
        let elsewhere =
            queue_role_fallback::queued_by_other_users(storage, user_id, &anchor_req.id);
        if !elsewhere.is_empty() {
            // BUG-1126: this already fails with a one-line role hint
            // ("aida role enter <role>"), which is the resolution the bug's
            // acceptance calls for. (A "--force-claim does not cross role
            // routing" clarification belongs in the flag's help text, not
            // here — the CLI error renderer only surfaces the first line plus
            // help lines, so an extra note appended to the message is
            // dropped.) trace:BUG-1126 | ai:claude
            anyhow::bail!(queue_role_fallback::format_queued_by_other_user_error(
                display_id,
                &elsewhere,
                role.as_deref(),
            ));
        }
        anyhow::bail!(format_queue_work_not_queued_error(
            display_id,
            anchor_req,
            role.as_deref(),
        ));
    }

    // Sort cluster by queue position so the manifest reflects intent.
    let mut cluster_sorted = cluster;
    cluster_sorted.sort_by_key(|e| e.position);

    // Pre-resolve each entry's display id + status so manifest writing
    // doesn't need a second store load.
    let resolved_entries: Vec<QueueWorkEntry> = cluster_sorted
        .into_iter()
        .filter_map(|entry| {
            let req = store
                .requirements
                .iter()
                .find(|r| r.id == entry.requirement_id)?;
            Some(build_resolved_entry(entry, req))
        })
        .collect();

    let scope = anchor_id_upper.clone();
    let review_target = parse_review_scope(&scope);
    Ok(QueueWorkPlan {
        mode: QueueWorkMode::Cluster,
        entries: resolved_entries,
        scope,
        review_target,
        anchor_display: anchor_id_upper,
        anchor_title: anchor_req.title.clone(),
    })
}

pub(crate) fn orchestrator_authorizes_explicit_queue_pickup(
    storage: &Storage,
    arg: &str,
    req: &aida_core::Requirement,
) -> bool {
    if !spec_matches(req, arg) {
        return false;
    }
    let Ok(auto_complete) = std::env::var(orchestrator::AUTO_COMPLETE_ENV) else {
        return false;
    };
    if auto_complete.is_empty() {
        return false;
    }
    let Ok(token) = std::env::var(orchestrator::TOKEN_ENV) else {
        return false;
    };
    orchestrator_authorizes_explicit_queue_pickup_with_token(storage, arg, req, &token)
}

pub(crate) fn orchestrator_authorizes_explicit_queue_pickup_with_token(
    storage: &Storage,
    arg: &str,
    req: &aida_core::Requirement,
    token: &str,
) -> bool {
    if !spec_matches(req, arg) {
        return false;
    }
    if token.is_empty() {
        return false;
    }
    let mut roots = Vec::new();
    if let Some(root) = storage.path().parent().map(std::path::Path::to_path_buf) {
        roots.push(root);
    }
    if let Ok(root) = find_main_worktree_root() {
        if !roots.iter().any(|existing| existing == &root) {
            roots.push(root);
        }
    }
    roots.into_iter().any(|project_root| {
        if !orchestrator::run_is_live(&project_root, &token) {
            return false;
        }
        let Some(state) = drain_state::DrainState::read(&project_root) else {
            return false;
        };
        state
            .current
            .as_deref()
            .map(|current| spec_matches(req, current))
            .unwrap_or(false)
    })
}

pub(crate) fn merge_current_spec_queue_entries(
    storage: &Storage,
    entries: &mut Vec<aida_core::QueueEntry>,
    requirement_id: uuid::Uuid,
) {
    if entries.iter().any(|e| e.requirement_id == requirement_id) {
        return;
    }
    let Ok(users) = storage.queue_users() else {
        return;
    };
    for user in users {
        let Ok(user_entries) = storage.queue_list(&user, /* include_completed */ false) else {
            continue;
        };
        for entry in user_entries {
            if entry.requirement_id == requirement_id
                && !entries.iter().any(|e| e.requirement_id == requirement_id)
            {
                entries.push(entry);
                return;
            }
        }
    }
}

/// STORY-42: zip a queue entry with its current requirement state so
/// later steps (role tally, manifest write) don't need to re-look-up
/// against a potentially-different Storage handle.
/// trace:STORY-42 | ai:claude
pub(crate) fn build_resolved_entry(
    queue: aida_core::QueueEntry,
    req: &aida_core::Requirement,
) -> QueueWorkEntry {
    let spec_id = req
        .spec_id
        .clone()
        .or_else(|| req.agreed_id.clone())
        .unwrap_or_else(|| "?".to_string());
    let status_at_plan = format!("{}", req.status);
    QueueWorkEntry {
        queue,
        spec_id,
        status_at_plan,
    }
}

/// TASK-217: build a status-aware recovery hint when `aida queue work <id>`
/// resolves a spec but finds no queue entry. The current status of the
/// resolved spec tells us which recovery path the user most likely wants.
/// trace:TASK-217 | ai:claude
pub(crate) fn format_queue_work_not_queued_error(
    display_id: &str,
    anchor_req: &aida_core::Requirement,
    role: Option<&str>,
) -> String {
    use RequirementType::*;
    let role_display = role.unwrap_or("<role>");
    let is_container = matches!(anchor_req.req_type, Epic | Folder | Sprint);
    if is_container {
        return format!(
            "`{display_id}` has no queued children. (Status: {status})\n  \
             Inspect the cluster: `aida queue list --tree` or `aida list --parent {display_id}`\n  \
             To queue more work under {display_id}: `aida queue add <child-id> --for {role_display}`",
            status = anchor_req.status,
        );
    }
    match anchor_req.status {
        RequirementStatus::Draft | RequirementStatus::Approved => format!(
            "`{display_id}` isn't queued. Status is {status}.\n  \
             To queue and start: `aida queue add {display_id} --for {role_display}` then `aida queue work {display_id}`",
            status = anchor_req.status,
        ),
        RequirementStatus::Planned => format!(
            "`{display_id}` isn't queued. Status is Planned (still in planning).\n  \
             To work it: `aida edit {display_id} --status approved` then `aida queue add {display_id} --for {role_display}`"
        ),
        // No queue file — yours or anyone else's — holds this spec (the caller
        // checks the cross-user case first), so a lost lease is the honest
        // remaining explanation. trace:BUG-774 | ai:claude
        RequirementStatus::InProgress => format!(
            "`{display_id}` is genuinely unqueued (no user's queue holds it), but status is In Progress.\n  \
             The lease may have been lost. Inspect with `aida queue list --all` or `aida queue list --all-users`.\n  \
             To re-queue: `aida queue add {display_id} --for {role_display}`"
        ),
        // The suggested command must run verbatim for *any* Done spec —
        // including one with no recorded claude session — so it stays
        // `--work` (a fresh session), not `--work --resume` (which needs a
        // prior session and bounces when there isn't one). trace:BUG-236
        // TASK-240: a Done spec usually means "PR open, awaiting merge". If the
        // user authored that PR they may want to merge it NOW rather than wait —
        // point them at the merge path. We can't name the PR number here (no gh
        // lookup in this pure builder), so route via `aida show` which prints the
        // PR linkage, keeping every suggested command honest/runnable.
        // No --delete-branch in the merge suggestion — a worktree may still
        // hold the branch — and `;` so the auto-bump pull cannot be dropped.
        // trace:TASK-240 trace:BUG-758 | ai:claude
        RequirementStatus::Done => format!(
            "`{display_id}` isn't queued. Status is Done (work finished on a branch).\n  \
             If review found issues and more commits are needed: `aida queue rework {display_id} --work`\n  \
             If the PR is yours and you want to merge now (CI green): find it with `aida show {display_id}`, then `gh pr merge <PR> --squash; aida pull`.\n  \
             Otherwise nothing to do — auto-bump fires when the PR merges."
        ),
        RequirementStatus::Completed => format!(
            "`{display_id}` is Completed (already shipped). Nothing to work on.\n  \
             To re-open: `aida edit {display_id} --status in-progress --force`"
        ),
        RequirementStatus::Rejected => format!(
            "`{display_id}` is Rejected. Pick a different spec, or re-open with `aida edit {display_id} --status approved --force`."
        ),
        // trace:TASK-1176 | ai:claude — superseded is terminal-but-adopted:
        // the work moved to a successor spec, so point at the successor rather
        // than offering a re-open the way Rejected does.
        RequirementStatus::Superseded => format!(
            "`{display_id}` is Superseded — it was adopted and then replaced by a later spec.\n  \
             Find the successor with `aida show {display_id}` (the superseded-by link) and work that instead."
        ),
        // STORY-332: a punted spec is paused awaiting triage — it should be
        // resolved by a human/advisor, not silently re-queued.
        RequirementStatus::NeedsAttention => format!(
            "`{display_id}` is paused (Needs Attention) — an agent punted a design-fork it couldn't resolve.\n  \
             Review the reason with `aida show {display_id}`, then triage:\n  \
             resume with `aida edit {display_id} --status in-progress`, or drop it with `--status rejected`."
        ),
    }
}

/// TASK-630: the held-state re-entry decision, isolated so it is unit-testable
/// without a Storage handle, a worktree, or a launcher.
///
/// A deliberate *push-branch, hold-PR* finish (BUG-250) leaves the spec **Done**
/// + dequeued with a persisted hold marker at `.aida/pr-holds/<spec>.json`. The
///   plain `aida queue work <spec>` recovery hints (rework / wait-for-merge) don't
///   fit that state — the operator is neither reworking nor waiting for a merge;
///   they're re-entering the dormant implementer worktree to run the manual gate
///   and *then* open the deferred PR. `--resume` against such a spec should flow
///   through the verb's normal lease/pull/worktree bookkeeping instead of bailing.
///
/// The decision is deliberately narrow: re-entry is allowed **only** when the
/// operator explicitly asked to resume (`resume == true`), the spec is in the
/// Done state (the state a deliberate hold parks it in), AND a hold marker is
/// present. A queued / Completed / NeedsAttention spec, or a Done spec with no
/// hold marker, is left to the existing recovery hints — a held re-entry is a
/// distinct state, not a blanket "resume any Done spec". trace:TASK-630 | ai:claude
pub(crate) fn held_resume_reentry_allowed(
    resume: bool,
    status: &RequirementStatus,
    hold_marker_present: bool,
) -> bool {
    resume && hold_marker_present && *status == RequirementStatus::Done
}

/// TASK-630: build the Item-mode plan for a held-spec `--resume` re-entry.
///
/// A held spec is Done + dequeued, so there is no real `QueueEntry` to anchor
/// the plan on. We synthesise one (scoped to the spec's own id, matching the
/// implementer worktree the held session lives in) so the rest of
/// `handle_queue_work` — lease bookkeeping, worktree resolution, the
/// `QueueWorkLaunch::Resume` re-entry — runs unchanged. trace:TASK-630 | ai:claude
pub(crate) fn held_resume_plan(req: &aida_core::Requirement, user_id: &str) -> QueueWorkPlan {
    let synthetic = aida_core::QueueEntry {
        user_id: user_id.to_string(),
        requirement_id: req.id,
        position: 0,
        added_by: user_id.to_string(),
        note: None,
        added_at: chrono::Utc::now(),
        for_role: Some("implementer".to_string()),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    let (scope, review_target) = derive_scope_from_entry(&synthetic, req);
    let anchor_display = req
        .agreed_id
        .clone()
        .or_else(|| req.spec_id.clone())
        .unwrap_or_else(|| scope.clone());
    let resolved = build_resolved_entry(synthetic, req);
    QueueWorkPlan {
        mode: QueueWorkMode::Item,
        entries: vec![resolved],
        scope,
        review_target,
        anchor_display,
        anchor_title: req.title.clone(),
    }
}

/// BUG-2150: does this `queue work PR-N` pickup ask for the reviewer seat
/// explicitly (`--role reviewer`)? Only then does a PR scope with no queued
/// review story stay a PR-scoped review instead of resolving to the backing
/// spec (TASK-518's convenience for a human who wants to keep implementing).
// trace:BUG-2150 | ai:claude
pub(crate) fn reviewer_pickup_keeps_pr_scope(role_flag: Option<&str>) -> bool {
    role_flag.is_some_and(|r| canonical_role_name(r.trim()).eq_ignore_ascii_case("reviewer"))
}

/// BUG-2150: the plan for a standalone reviewer pickup of PR-N whose backing
/// spec is `req` and which has no queued review story. The scope is the PR
/// (`PR-N` / `MR-N`), so `session_start` takes a PR-scoped review lease that
/// does not collide with the implementer's spec-scoped lease, while a second
/// reviewer on the same PR still hits the PR-scope conflict. The entry is
/// synthesized at position 0, so nothing is written to the queue.
// trace:BUG-2150 | ai:claude
pub(crate) fn standalone_review_plan(
    forge: ReviewForge,
    n: u64,
    req: &aida_core::Requirement,
    user_id: &str,
) -> QueueWorkPlan {
    let synthetic = aida_core::QueueEntry {
        user_id: user_id.to_string(),
        requirement_id: req.id,
        position: 0,
        added_by: user_id.to_string(),
        note: None,
        added_at: chrono::Utc::now(),
        for_role: Some("reviewer".to_string()),
        for_scope: None,
        for_session: None,
        added_by_machine: None,
    };
    let anchor_display = req
        .agreed_id
        .clone()
        .or_else(|| req.spec_id.clone())
        .unwrap_or_else(|| format_review_label(forge, n));
    QueueWorkPlan {
        mode: QueueWorkMode::Item,
        entries: vec![build_resolved_entry(synthetic, req)],
        scope: format_review_label(forge, n),
        review_target: Some((forge, n)),
        anchor_display,
        anchor_title: req.title.clone(),
    }
}

/// STORY-42: case-insensitive match against a requirement's uuid,
/// spec_id, or agreed_id. trace:STORY-42 | ai:claude
/// A friendly node alias (`TYPE-<LABEL>-NNN`) matches its canonical id,
/// literal id first. trace:BUG-B6B1278890774EF993D2EBC327862470-015 | ai:claude
pub(crate) fn spec_matches(req: &aida_core::Requirement, query: &str) -> bool {
    if let Ok(uuid) = uuid::Uuid::parse_str(query) {
        return req.id == uuid;
    }
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude — numeric
    // identity: TASK-0042 matches a spec stored as TASK-42.
    let q = aida_core::node_alias::expand_or_keep(query);
    [req.spec_id.as_deref(), req.agreed_id.as_deref()]
        .into_iter()
        .flatten()
        .any(|s| aida_core::id_identity::same_id(s, &q))
}

/// STORY-42: pick the scope string for a single queued entry.
/// Preference order:
///   1. PR-N / MR-N parsed from "Review PR-N: …" title → "PR-N"
///   2. entry.for_scope (explicit STORY-57 tag)
///   3. fall back to the req's own display id
///      Also returns the parsed review target (if any) so the caller can
///      thread it into session_start without re-parsing.
///      trace:STORY-42 | ai:claude
// BUG-431 #1: a child story no longer inherits its parent epic's scope (the
// removed step 3). Session scope is derived purely from the entry's
// `for_scope`, the review-title shape, and the req's own id — so same-epic
// siblings drained together each get an independent scope / worktree / branch
// instead of contending for one epic scope. The store param is gone with the
// parent lookup. trace:BUG-431
pub(crate) fn derive_scope_from_entry(
    entry: &aida_core::QueueEntry,
    req: &aida_core::Requirement,
) -> (String, Option<(ReviewForge, u64)>) {
    // Review-PR shape — title carries the PR ref; e.g. "Review PR-11: …".
    if let Some(rest) = req.title.strip_prefix("Review ") {
        let pr_token = rest.split([':', ' ']).next().unwrap_or("");
        if let Some(target) = parse_review_scope(pr_token) {
            return (pr_token.to_uppercase(), Some(target));
        }
    }
    if let Some(s) = &entry.for_scope {
        // BUG-739: `harness-worktree` is a generic Claude harness lease, not a
        // work scope. Older queue entries may have been auto-stamped with it;
        // ignore that legacy value so `queue work` falls back to the spec id
        // instead of trying to start inside the shared harness checkout.
        // trace:BUG-739 | ai:codex
        if s.eq_ignore_ascii_case(worktree_lease::HARNESS_WORKTREE_SCOPE) {
            return derive_scope_from_req_id(req);
        }
        let target = parse_review_scope(s);
        return (s.to_uppercase(), target);
    }
    derive_scope_from_req_id(req)
}

pub(crate) fn derive_scope_from_req_id(
    req: &aida_core::Requirement,
) -> (String, Option<(ReviewForge, u64)>) {
    // BUG-431 #1: a child story's session scopes to the STORY'S OWN id, not
    // its parent epic. Previously this fell back to `derive_parent_epic_label`,
    // so every same-epic story under one drain (`aida queue work next N`) tried
    // to own the SAME epic scope — the worktree (`proj-epic-11`) and branch
    // (`epic-11`) collided, the first story's lease blocked all siblings at
    // phase 1 ("scope EPIC-11 is owned by lease …"), and the epic-leading PR
    // title backed two specs (epic + child), breaking the headless reviewer.
    // Scoping to the story gives each sibling its own scope / worktree / branch
    // so they drain without contention. An operator who genuinely wants an
    // epic-wide session still gets it via an explicit `for_scope` (above) or
    // `aida queue work EPIC-N`. The parent-epic label is still used for
    // *display* clustering elsewhere (planned-cluster manifest, queue
    // grouping) — that is separate from the lease scope and unaffected.
    // trace:BUG-431 | ai:claude
    // trace:BUG-888 | ai:codex
    let fallback = req
        .spec_id
        .clone()
        .or_else(|| req.agreed_id.clone())
        .unwrap_or_else(|| "scope".to_string());
    let target = parse_review_scope(&fallback);
    (fallback, target)
}

pub(crate) fn queue_add_for_scope_routing(
    no_scope: bool,
    explicit_scope: Option<&str>,
    for_session: Option<&str>,
    active_lease: Option<&SessionLease>,
) -> Option<String> {
    if no_scope {
        None
    } else if let Some(s) = explicit_scope {
        Some(s.to_string())
    } else if for_session.is_some() {
        // --for-session is the more specific axis; don't also auto-add a scope
        // filter unless the user asked for it.
        None
    } else {
        // BUG-739: the generic harness-worktree lease only means "this shell is
        // in a shared Claude harness checkout"; it must not become queue
        // routing state. Explicit `--scope harness-worktree` remains honored by
        // the explicit_scope branch above.
        // trace:BUG-739 | ai:codex
        active_lease
            .filter(|l| {
                !l.scope
                    .eq_ignore_ascii_case(worktree_lease::HARNESS_WORKTREE_SCOPE)
            })
            .map(|l| l.scope.clone())
    }
}

/// STORY-42: pick the role for a resolved queue-work plan.
/// trace:STORY-42 | ai:claude
pub(crate) fn infer_queue_work_role(
    plan: &QueueWorkPlan,
    override_role: Option<&str>,
) -> (String, &'static str, Option<Vec<String>>) {
    if let Some(r) = override_role {
        return (r.to_string(), "--role flag", None);
    }
    // Tally for_role across entries. None counts as "unrouted".
    let mut tally: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for e in &plan.entries {
        if let Some(r) = &e.queue.for_role {
            *tally.entry(r.to_ascii_lowercase()).or_default() += 1;
        }
    }
    let total_routed: usize = tally.values().sum();
    let mut warnings: Vec<String> = Vec::new();
    let chosen = if let Some((best, count)) = tally
        .iter()
        .max_by_key(|(_, c)| **c)
        .map(|(k, c)| (k.clone(), *c))
    {
        if count < total_routed {
            warnings.push(format!(
                "cluster has {} item{} routed to other role(s); using majority `{}` ({}/{})",
                total_routed - count,
                if total_routed - count == 1 { "" } else { "s" },
                best,
                count,
                total_routed
            ));
        }
        (best, "cluster-derived")
    } else {
        // No entry has for_role set — fall through to scope default
        // (PR-N → reviewer; else implementer) then env.
        let scope_default = if plan.review_target.is_some() {
            "reviewer"
        } else {
            "implementer"
        };
        let env_role = std::env::var("AIDA_SESSION_ROLE")
            .ok()
            .filter(|s| !s.is_empty());
        match env_role {
            Some(r) if !r.eq_ignore_ascii_case(scope_default) => {
                warnings.push(format!(
                    "no for_role on queued items; scope default `{}` overrides shell role `{}` (pass --role {} to keep shell role)",
                    scope_default, r, r
                ));
                (scope_default.to_string(), "scope-default")
            }
            _ => (scope_default.to_string(), "scope-default"),
        }
    };
    let warns = if warnings.is_empty() {
        None
    } else {
        Some(warnings)
    };
    (chosen.0, chosen.1, warns)
}

/// STORY-42: build the initial-prompt string fed to `claude <prompt>`.
/// Routes by role:
///   - reviewer → `/aida-review --pr N` (when scope parses as PR-N/MR-N)
///     else `/aida-review`
///   - implementer / other → `/aida-pickup [<ITEM-ID> | --auto-first]`
///     Both forms skip the skill's Step 2 confirm — the
///     argument IS the consent signal. Item mode focuses a
///     single id (the operator typed it, so the SPEC-ID is
///     the commitment — TASK-548 generalised TASK-86's
///     argument-as-consent rule to cover this case).
///     Cluster / head mode passes `--auto-first` to ride the
///     queue-work pre-flight summary as its consent point.
///     Only a bare `/aida-pickup` from the conversation (no
///     argument) still pauses to confirm.
///     trace:TASK-86 trace:TASK-548 | ai:claude
///     trace:STORY-42 | ai:claude
/// BUG-809: the reviewer's verdict-file anchor, carried in the PROMPT TEXT.
/// The env anchor (`AIDA_DRIVE_ROOT` + PATH prepend) does not reliably
/// survive a vendor's tool-call sandbox — codex in particular can hand the
/// skill's shell a sanitized environment — so a reviewer that checks the PR
/// out elsewhere resolves ITS checkout as project root and writes the verdict
/// where the orchestrator can't see it. Prompt text cannot be stripped by a
/// sandbox, so the absolute path rides in the instructions themselves.
// trace:BUG-809 | ai:claude
pub(crate) fn reviewer_verdict_anchor_suffix() -> Option<String> {
    let vf = std::env::var("AIDA_REVIEW_VERDICT_FILE").ok()?;
    if vf.trim().is_empty() {
        return None;
    }
    // <root>/.aida/review-verdicts/PR-N.json → <root>
    let root = std::path::Path::new(&vf)
        .ancestors()
        .nth(3)
        .map(|p| p.display().to_string())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| ".".to_string());
    Some(format!(
        "\n\nVerdict anchor (do not relocate): the verdict JSON must land at exactly `{vf}`. \
         If you use `aida review record`, run it from `{root}` — cd there first if you checked \
         the PR out somewhere else. A verdict written under any other directory is invisible \
         to the orchestrator and shelves the spec."
    ))
}

/// The standalone reviewer's verdict anchor. Its launcher collects the
/// verdict from the review worktree it launched (BUG-2149), and a contained
/// reviewer cannot write the primary checkout, so `aida review record` runs
/// from `worktree` — never from the root that holds `verdict_file`.
// trace:BUG-b6b1278890774ef993d2ebc327862470-045 | ai:claude
pub(crate) fn standalone_reviewer_record_anchor(
    verdict_file: &str,
    worktree: &std::path::Path,
) -> String {
    let root = std::path::Path::new(verdict_file)
        .ancestors()
        .nth(3)
        .map(|p| p.display().to_string())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| ".".to_string());
    let tree = worktree.display();
    format!(
        "\n\nVerdict anchor (do not relocate): record the verdict with `aida review record`, run \
         from your review worktree `{tree}` (cd there first if you left it). Do not cd to the \
         primary checkout `{root}`: a contained reviewer cannot write there, and the launcher \
         collects the verdict from `{tree}` onto the PR's gate ledger (canonical file \
         `{verdict_file}`)."
    )
}

/// BUG-868: a `--from-pr` reviewer is reviewing implementation that shipped
/// outside the orchestrator. The owning spec being Done is expected, so the
/// prompt must keep the session on the PR verdict contract instead of queue
/// hygiene.
/// BUG-1186: is the orchestrator's review envelope (`AIDA_FROM_PR_REVIEW` +
/// `AIDA_FROM_PR_NUMBER`) set for exactly this PR? The routing chokepoint uses
/// it to refuse the PR→backing-spec fallback that swapped the reviewer seat
/// for an implementer pickup.
// trace:BUG-1186 | ai:claude
pub(crate) fn orchestrator_review_envelope_for_pr(pr: u32) -> bool {
    review_envelope_targets_pr(
        std::env::var("AIDA_FROM_PR_REVIEW").ok().as_deref(),
        std::env::var("AIDA_FROM_PR_NUMBER").ok().as_deref(),
        pr,
    )
}

/// Pure half of [`orchestrator_review_envelope_for_pr`].
// trace:BUG-1186 | ai:claude
pub(crate) fn review_envelope_targets_pr(
    review: Option<&str>,
    number: Option<&str>,
    pr: u32,
) -> bool {
    let enabled = review.is_some_and(|v| matches!(v.trim(), "1" | "true" | "yes"));
    enabled && number.and_then(|n| n.trim().parse::<u32>().ok()) == Some(pr)
}

// trace:BUG-868 | ai:codex
pub(crate) fn reviewer_from_pr_contract_suffix() -> Option<String> {
    let enabled = std::env::var("AIDA_FROM_PR_REVIEW")
        .ok()
        .is_some_and(|v| matches!(v.trim(), "1" | "true" | "yes"));
    if !enabled {
        return None;
    }
    let pr = std::env::var("AIDA_FROM_PR_NUMBER")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let head_sha = std::env::var("AIDA_FROM_PR_HEAD_SHA")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "unavailable".to_string());
    let verdict_path = std::env::var("AIDA_REVIEW_VERDICT_FILE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "<AIDA_REVIEW_VERDICT_FILE>".to_string());

    Some(format!(
        "\n\nFrom-PR review contract: review PR #{pr} at head `{head_sha}` and write the \
         verdict file at `{verdict_path}`. The owning spec being Done is expected (and so is \
         In Progress under a rework re-drive): you REVIEW the PR, you never implement — a \
         `[rework]` / `--reason` note on the spec is the implementer's instruction, not yours. \
         Make no commits and no pushes to the PR branch; do not run `aida queue done`; complete \
         this phase by writing the review verdict file."
    ))
}

pub(crate) fn append_reviewer_prompt_suffixes(prompt: &mut String) {
    append_reviewer_prompt_suffixes_with(prompt, reviewer_verdict_anchor_suffix());
}

/// [`append_reviewer_prompt_suffixes`] for a standalone reviewer, whose
/// verdict anchor names its own review worktree as the directory to run
/// `aida review record` from (see [`standalone_reviewer_record_anchor`]).
// trace:BUG-b6b1278890774ef993d2ebc327862470-045 | ai:claude
pub(crate) fn append_standalone_reviewer_prompt_suffixes(
    prompt: &mut String,
    worktree: &std::path::Path,
) {
    let anchor = std::env::var("AIDA_REVIEW_VERDICT_FILE")
        .ok()
        .filter(|vf| !vf.trim().is_empty())
        .map(|vf| standalone_reviewer_record_anchor(&vf, worktree));
    append_reviewer_prompt_suffixes_with(prompt, anchor);
}

pub(crate) fn append_reviewer_prompt_suffixes_with(prompt: &mut String, anchor: Option<String>) {
    if let Some(suffix) = reviewer_from_pr_contract_suffix() {
        prompt.push_str(&suffix);
    }
    if let Some(suffix) = anchor {
        prompt.push_str(&suffix);
    }
    if let Ok(context) = std::env::var("AIDA_GRADED_REVIEW_CONTEXT") {
        if !context.trim().is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&context);
        }
    }
}

/// TASK-1290: round 1 of the headless reviewer prompt CITES the untraced
/// acceptance criteria for every spec the PR covers instead of rediscovering
/// them during review — reuses `aida criteria`'s tracer
/// (`crate::criteria::build_criteria_report`) via
/// `criteria_gate::reviewer_prompt_block`; no second implementation.
/// Resolves the PR's covered specs from its commit trailers, the same
/// resolution `generate_review_prompt` uses for `--pr`. Best-effort: any
/// failure to resolve the PR/commits/store degrades to no block — this
/// never blocks the reviewer launch.
// trace:TASK-1290 | ai:claude
pub(crate) fn append_untraced_criteria_prompt_block(
    prompt: &mut String,
    project_root: &std::path::Path,
    store: &aida_core::RequirementsStore,
    forge: ReviewForge,
    pr_n: u64,
) {
    // Only round 1 cites the list; a rework round already has the reviewer's
    // own findings to work from. trace:TASK-1290 | ai:claude
    let round = std::env::var("AIDA_REVIEW_ROUND")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1);
    if round != 1 {
        return;
    }
    let Ok((base, head)) = pr_base_head(project_root, forge, pr_n) else {
        return;
    };
    // BUG-035: an unreadable range still never blocks the launch, but it is
    // not silent: the operator learns which range failed and why the prompt
    // carries no criteria list.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-035 | ai:claude
    let messages = match git_log_messages(project_root, &base, &head) {
        Ok(messages) => messages,
        Err(err) => {
            eprintln!(
                "warning: cannot read commits {base}..{head} for PR-{pr_n} ({err:#}); \
                 the reviewer prompt omits the untraced acceptance criteria list"
            );
            return;
        }
    };
    let mut spec_ids: Vec<String> = Vec::new();
    for msg in &messages {
        for id in extract_spec_ids_from_commit(msg) {
            if !spec_ids
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&id))
            {
                spec_ids.push(id);
            }
        }
    }
    let mut reports: Vec<(String, crate::criteria::CriteriaReport)> = Vec::new();
    for id in &spec_ids {
        let Some(req) = store.requirements.iter().find(|r| spec_matches(r, id)) else {
            continue;
        };
        let display = req.spec_id.as_deref().unwrap_or(id.as_str());
        if let Ok(report) =
            crate::criteria::build_criteria_report(project_root, display, &req.description)
        {
            reports.push((display.to_string(), report));
        }
    }
    if let Some(block) = crate::criteria_gate::reviewer_prompt_block(&reports) {
        prompt.push_str(&block);
    }
    // STORY-1386: cite the pre-implementation red run recorded at phase 1.
    // trace:STORY-1386 | ai:claude
    let red_root = crate::main_worktree_root_from(project_root);
    let records: Vec<_> = reports
        .iter()
        .filter_map(|(display, _)| crate::criteria_red_run::load_record(&red_root, display))
        .collect();
    if let Some(block) = crate::criteria_red_run::reviewer_prompt_block(&records) {
        prompt.push_str(&block);
    }
}

// trace:BUG-1063 | ai:codex
pub(crate) fn headless_waiting_rule_suffix(role: &str) -> String {
    let artifact = if role.eq_ignore_ascii_case("reviewer") {
        "verdict file"
    } else if role.eq_ignore_ascii_case("advisor") {
        "response JSON"
    } else {
        "result"
    };
    // trace:STORY-2697 | ai:codex
    let reviewer_ci_contract = if role.eq_ignore_ascii_case("reviewer") {
        "\n\nReviewer CI contract: approve a defect-free head while CI is pending; \
         WAITING only for independent evidence or a named test the judgment depends on."
    } else {
        ""
    };
    format!(
        "\n\nHEADLESS WAITING RULE: AIDA_HEADLESS=1 means there is no next \
         conversational turn. Never end your turn waiting for a notification, \
         watcher, monitor, or CI callback. Never block in one tool call longer \
         than 60 seconds. Poll long waits in bounded steps (`sleep <= 60s` per \
         tool call) and print the current state each step so stream-json emits \
         progress for the watchdog. If the wait would exceed the phase's \
         remaining budget, write the {artifact} with the current wait state and \
         exit instead.{reviewer_ci_contract}"
    )
}

// trace:TASK-1581 | ai:antigravity — test-only: the production caller derives
// the real rework round and calls `derive_queue_work_prompt_with_round`.
#[cfg(test)]
pub(crate) fn derive_queue_work_prompt(
    plan: &QueueWorkPlan,
    role: &str,
    plan_only: bool,
    guided: bool,
    review_findings: Option<&str>,
) -> String {
    // Tests only: the first rework is round 2 by definition.
    derive_queue_work_prompt_with_round(plan, role, plan_only, guided, review_findings, 2)
}

// BUG-1213: the real re-drive round is derived from the spec's durable
/// history (one recorded findings block per rework) by the production caller.
// trace:BUG-1213 | ai:claude
pub(crate) fn derive_queue_work_prompt_with_round(
    plan: &QueueWorkPlan,
    role: &str,
    plan_only: bool,
    guided: bool,
    review_findings: Option<&str>,
    rework_round: usize,
) -> String {
    let role_lower = role.to_ascii_lowercase();
    // Agent gates wear a custom role but must still produce a review verdict
    // for the orchestrator, so they reuse the review skill with a tighter
    // gate prompt. trace:STORY-1155 | ai:codex
    if let Ok(gate_name) = std::env::var("AIDA_AGENT_GATE_NAME") {
        if let Some((_, n)) = plan.review_target {
            let gate_name = gate_name.trim();
            let gate_role = std::env::var("AIDA_AGENT_GATE_ROLE").unwrap_or_else(|_| role.into());
            return format!(
                "/aida-review --pr {}\n\nAgent gate: {}\nGate role: {}\nTreat this as a required pre-merge quality gate. Write the verdict file named by AIDA_REVIEW_VERDICT_FILE.",
                n,
                if gate_name.is_empty() { "agent-gate" } else { gate_name },
                gate_role.trim()
            );
        }
    }
    if role_lower == "reviewer" {
        if let Some((_, n)) = plan.review_target {
            let round = std::env::var("AIDA_REVIEW_ROUND")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(1);
            return reviewer_prompt_for_round(n, round);
        }
        return "/aida-review".to_string();
    }
    // STORY-735: guided keystone mode runs the structured decision-dialog
    // skill instead of pickup. It is interactive-only and conflicts with the
    // autonomous drain at the CLI; the spec id is the anchor it dialogs over.
    // trace:STORY-735 | ai:claude
    if guided {
        return format!("/aida-guided-implement {}", plan.anchor_display);
    }
    // STORY-265: plan-only mode runs the planning skill instead of pickup —
    // produce a docs/plans/ file, no code. (Reviewer handled above; plan-only
    // is an implementer-side mode.)
    if plan_only {
        if plan.mode == QueueWorkMode::Item {
            return format!("/aida-plan {}", plan.anchor_display);
        }
        return "/aida-plan".to_string();
    }
    // Implementer (and unknown roles): /aida-pickup with optional focus.
    if plan.mode == QueueWorkMode::Item {
        let mut pickup = format!("/aida-pickup {}", plan.anchor_display);
        // A groomed SPIKE is executed by the normal CI/review machinery, but
        // phase 1 is research rather than product implementation.
        // trace:TASK-1276 | ai:codex
        if plan
            .anchor_display
            .to_ascii_uppercase()
            .starts_with("SPIKE-")
        {
            pickup.push_str(
                "\n\nResearch-lane contract: produce the source-grounded deliverable as a dated `docs/spikes/<date>-<slug>.md` report, commit it, and open a PR for normal CI and review. Do not make or apply a product/design decision inside the report; surface any decision for the advisor. This is the research lane, not the implementer lane."
            );
        }
        if let Some(findings) = review_findings {
            return rework_pickup_prompt(findings, &pickup, rework_round);
        }
        return pickup;
    }
    // Cluster / Head: drain-intent already confirmed by the queue work
    // pre-flight; pass --auto-first so the skill skips its own confirm.
    let pickup = "/aida-pickup --auto-first".to_string();
    if let Some(findings) = review_findings {
        return rework_pickup_prompt(findings, &pickup, rework_round);
    }
    pickup
}

// trace:TASK-1291 | ai:codex
pub(crate) fn reviewer_prompt_for_round(pr: u64, round: usize) -> String {
    let base = format!("/aida-review --pr {pr}");
    if round != 1 {
        return base;
    }
    format!(
        "{base}\n\nROUND 1 FULL-SWEEP CONTRACT: Before choosing a verdict, enumerate EVERY acceptance criterion from every covered spec and write one status line per criterion: `verified by <test/evidence>`, `unverified`, or `not-applicable-because <reason>`. Inspect the whole diff and list EVERY defect you find, not only the first. The verdict file's `findings` array must carry the complete actionable defect list so one rework round can address the full review surface."
    )
}

// A review starts at round 1. Each durable findings block records one
// completed non-approving review and advances the next review by one round.
// trace:TASK-1291 | ai:codex
pub(crate) fn review_round_from_comments(comments: &[aida_core::Comment]) -> usize {
    comments
        .iter()
        .filter(|comment| crate::review_verdict_display::is_findings_block(&comment.content))
        .count()
        + 1
}

/// STORY-281 / TASK-480: whether the reviewer pre-flight checks
/// (stale-base, intermediate-only) apply to this queue-work plan, and if
/// so, the PR/MR number to check against.
///
/// BUG-1609: this used to be inlined at each call site as a match on
/// `Some((ReviewForge::GitHub, pr_n))`, which silently skipped every GitLab
/// MR — not because the checks themselves are GitHub-only
/// (`preflight_stale_base_check` / `preflight_intermediate_only_check` read
/// metadata through the forge-routed `fetch_change_info_via_forge`,
/// GitLab-safe since STORY-621 slice 2), but because the call sites never
/// reached them for GitLab. Pulling the predicate out to a named,
/// independently-testable function makes "any forge with a review target,
/// not just GitHub" a pinned invariant instead of an inlined pattern that
/// can silently narrow again.
// trace:BUG-1609 | ai:claude
pub(crate) fn reviewer_preflight_pr_n(review_target: Option<(ReviewForge, u64)>) -> Option<u64> {
    review_target.map(|(_, n)| n)
}

// BUG-1213: keep durable-history round derivation on the same path used to
/// assemble the production pickup prompt, so callers cannot accidentally
/// reintroduce a constant round.
// trace:BUG-1213 | ai:codex
pub(crate) fn derive_rework_queue_work_prompt(
    plan: &QueueWorkPlan,
    role: &str,
    plan_only: bool,
    guided: bool,
    review_findings: Option<&str>,
    comments: &[aida_core::Comment],
) -> String {
    derive_queue_work_prompt_with_round(
        plan,
        role,
        plan_only,
        guided,
        review_findings,
        rework_round_from_comments(comments),
    )
}

// BUG-1213: the review-findings block that was recorded LAST on the spec
/// (by comment time), if any.
// trace:BUG-1213 | ai:claude
pub(crate) fn latest_findings_block(comments: &[aida_core::Comment]) -> Option<&str> {
    comments
        .iter()
        .filter(|c| crate::review_verdict_display::is_findings_block(&c.content))
        .max_by_key(|c| c.created_at)
        .map(|c| c.content.as_str())
}

/// The tag that marks a spec comment as an advisor ruling. Rulings are
/// written by hand (`aida comment add ID "Advisor ruling …"`) as well as by
/// `aida rework --ruling`, so the match is on the comment's first line.
// trace:BUG-B6B1278890774EF993D2EBC327862470-021 | ai:claude
pub(crate) const RULING_TAG: &str = "Advisor ruling";

/// Whether a comment is an advisor ruling: its first non-empty line starts
/// with `Ruling`, `Advisor ruling`, `Advisor signoff` or `Advisor amendment`
/// (any case, after markdown emphasis or a `[` bracket). Signoffs and
/// amendments participate in the same timestamp ordering as rulings.
// trace:BUG-B6B1278890774EF993D2EBC327862470-021 | ai:claude
// trace:BUG-2523 | ai:codex
pub(crate) fn is_ruling_comment(content: &str) -> bool {
    let first = crate::rework_direction::lead_line(content);
    [
        "ruling",
        "advisor ruling",
        "advisor signoff",
        "advisor amendment",
    ]
    .iter()
    .any(|prefix| first.starts_with(prefix))
}

/// `--ruling` text as stored: tagged unless the caller already tagged it.
// trace:BUG-B6B1278890774EF993D2EBC327862470-021 | ai:claude
pub(crate) fn ruling_comment_text(ruling: &str) -> String {
    if is_ruling_comment(ruling) {
        ruling.to_string()
    } else {
        format!("{RULING_TAG}: {ruling}")
    }
}

/// The advisor ruling recorded LAST on the spec (by comment time), replies
/// included. A later ruling supersedes an earlier one. Production pickups
/// use [`crate::rework_direction::resolve_direction`], which also reads
/// briefs (BUG-057).
// trace:BUG-B6B1278890774EF993D2EBC327862470-021 | ai:claude
#[cfg(test)]
pub(crate) fn latest_ruling_comment(
    comments: &[aida_core::Comment],
) -> Option<&aida_core::Comment> {
    crate::rework_direction::latest_matching(comments, is_ruling_comment)
}

/// Lead a pickup prompt with the spec's latest advisor ruling. The pickup
/// line stays last. Production pickups go through
/// [`crate::rework_direction::lead_rework_prompt`] (BUG-057).
// trace:BUG-B6B1278890774EF993D2EBC327862470-021 | ai:claude
#[cfg(test)]
pub(crate) fn prepend_ruling(prompt: String, ruling: Option<&aida_core::Comment>) -> String {
    let direction = crate::rework_direction::AdvisorDirection {
        brief: None,
        ruling,
    };
    crate::rework_direction::lead_rework_prompt(prompt, &direction, None, None)
}

// BUG-1213: the rework round a pickup is entering: the original
/// implementation is round 1, and every recorded findings block (one per
/// `queue rework`) opens the next round — so N recorded blocks ⇒ round N+1.
// trace:BUG-1213 | ai:claude
pub(crate) fn rework_round_from_comments(comments: &[aida_core::Comment]) -> usize {
    let recorded = comments
        .iter()
        .filter(|c| crate::review_verdict_display::is_findings_block(&c.content))
        .count();
    recorded.max(1) + 1
}

// BUG-1213: do this round's findings repeat the immediately previous round's
/// verbatim? Only then is a further blind requeue a loop. A → B → A returns
/// false (the last recorded block is B).
// trace:BUG-1213 | ai:claude
pub(crate) fn findings_recur_consecutively(comments: &[aida_core::Comment], block: &str) -> bool {
    latest_findings_block(comments).is_some_and(|last| last.trim() == block.trim())
}

/// BUG-1213: make the current review delta, rather than the original spec
/// description or an earlier commit, the first and authoritative instruction
/// a rework implementer sees.
// trace:BUG-1213 | ai:codex
/// STORY-1226: lead a seat's pickup prompt with its due `[schedule]` jobs.
/// `due_block` is `maintenance_schedule::render_due_jobs_block` output (empty
/// → the prompt is returned unchanged). Same construction as
/// [`rework_pickup_prompt`]: the block comes first, the pickup line last, so
/// the skill invocation stays the final line. Never applied to an agent-gate
/// prompt (those wear a tighter verdict contract).
// trace:STORY-1226 | ai:claude
pub(crate) fn prepend_due_jobs(prompt: String, due_block: &str) -> String {
    let block = due_block.trim_end();
    if block.is_empty() || std::env::var("AIDA_AGENT_GATE_NAME").is_ok() {
        return prompt;
    }
    format!("{block}\n\n{prompt}")
}

pub(crate) fn rework_pickup_prompt(findings: &str, pickup: &str, round: usize) -> String {
    format!(
        "ROUND {round} — ITEMS STILL OPEN (AUTHORITATIVE TASK):\n{findings}\n\n\
         The previous round's commit is already on the PR branch and does not count for this \
         round. Produce a new commit addressing every item below, or punt explicitly naming \
         the finding you dispute.\n\n{pickup}"
    )
}

pub(crate) fn review_ref_from_verdict_filename(path: &std::path::Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("review");
    if let Some(n) = stem.strip_prefix("PR-") {
        return format!("PR #{n}");
    }
    if let Some(n) = stem.strip_prefix("MR-") {
        return format!("MR #{n}");
    }
    stem.to_string()
}

/// BUG-2403: is the change-keyed verdict `key` (`PR-N`/`MR-N`) PROVED to
/// concern one of `labels`? Only the gate ledger's own binding counts: the
/// spec's `by-spec` index names that change (written when the change was
/// opened or recorded for the spec), or the change's newest verdict event
/// is recorded for the spec. Prose that merely mentions an ID, a file name
/// or a sort order never binds a review to a spec.
// trace:BUG-2403 | ai:claude
pub(crate) fn change_verdict_bound_to(
    project_root: &std::path::Path,
    key: &str,
    labels: &[String],
) -> bool {
    let Some(change) = crate::gate::ledger::ChangeKey::parse(key) else {
        return false;
    };
    let Ok(store) = crate::gate::ledger::store_root(project_root) else {
        return false;
    };
    let indexed = labels.iter().any(|label| {
        crate::gate::ledger::project::changes_for_spec(&store, label).contains(&change)
    });
    indexed
        || crate::gate::ledger::project::newest_verdict_event(project_root, key).is_some_and(|e| {
            e.spec
                .as_deref()
                .is_some_and(|spec| labels.iter().any(|l| l.eq_ignore_ascii_case(spec)))
        })
}

/// The findings block a rework pickup leads with: the CURRENT review of the
/// spec, selected once across its native and friendly labels.
///
/// BUG-2403: this used to try the friendly label first and return its record
/// however stale, then fall back to any blocking verdict whose prose
/// mentioned the ID, in file-name order. A current approval therefore
/// replayed an obsolete refusal's findings, and the recurrence guard compared
/// that stale block. Now the selected current record answers (an approval
/// yields no block); ambiguous or refused evidence yields no block rather
/// than a guess; only when no spec-keyed record exists does a change-keyed
/// review answer, and only one whose ledger binding proves it concerns this
/// spec — two such changes are ambiguous, not sorted to a winner.
// trace:BUG-2403 | ai:claude
pub(crate) fn rework_findings_block_for_spec(
    project_root: &std::path::Path,
    display_id: &str,
    spec_id: &str,
) -> Option<String> {
    match resolve_rework_findings(project_root, display_id, spec_id, None) {
        Ok(block) => block,
        Err(why) => {
            // trace:BUG-2507 | ai:codex
            eprintln!("warning: no review findings imported for {display_id}: {why}. Read the review comment before starting rework.");
            None
        }
    }
}

/// BUG-2430: the change key (`PR-N`) of the spec's open pull request, if the
/// forge names exactly one. A failed lookup is `None`: the ledger bindings
/// still answer, and an unknown PR never binds a review to the spec.
// trace:BUG-2430 | ai:claude
pub fn spec_open_change_key(project_root: &std::path::Path, ids: &[&str]) -> Option<String> {
    let forge = Ok(crate::forge::resolve_forge_kind(project_root));
    let mut ids: Vec<String> = ids.iter().map(|s| s.trim().to_string()).collect();
    ids.dedup();
    crate::reconcile_evidence::check_open_changes(project_root, &forge, ids, false)
        .into_iter()
        .find_map(|(_, check)| match check {
            crate::reconcile_evidence::OpenChangeCheck::Open(n) => {
                Some(crate::gate::ledger::ChangeKey::pr(n).as_key())
            }
            _ => None,
        })
}

/// BUG-2430: the time a verdict was recorded, for ordering a spec-keyed
/// record against a change-keyed one: its own `recorded_at`, else when the
/// reader saw it written.
// trace:BUG-2430 | ai:claude
pub(crate) fn verdict_recorded_time(
    verdict: &crate::gate::review_verdict::RecordedVerdict,
    fallback: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    verdict
        .recorded_at
        .as_deref()
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at.trim()).ok())
        .map(|at| at.with_timezone(&chrono::Utc))
        .or(fallback)
}

/// The findings block a rework pickup leads with, or why it cannot be read.
///
/// BUG-2430: a review recorded under the change key (`PR-N`) with no spec
/// binding in the ledger was invisible here, so the rework lane launched
/// with only older rulings. `open_change` (the spec's open PR, from
/// [`spec_open_change_key`]) now binds that change's verdict to the spec,
/// and when both a spec-keyed and a change-keyed record answer, the newer
/// one wins. `Err` means a request-changes verdict exists but its findings
/// cannot be selected; a rework launch must refuse rather than send the
/// lane out on stale instructions.
// trace:BUG-2430 | ai:claude
pub(crate) fn resolve_rework_findings(
    project_root: &std::path::Path,
    display_id: &str,
    spec_id: &str,
    open_change: Option<&str>,
) -> Result<Option<String>, String> {
    resolve_rework_review(project_root, display_id, spec_id, open_change)
        .map(|review| match review {
            ReviewResult::Available(findings) => Some(findings),
            _ => None,
        })
        .map_err(|error| match error {
            ReworkReviewError::Waiting(why) | ReworkReviewError::Unavailable(why) => why,
        })
}

// trace:BUG-2500 | ai:codex
pub fn refuse_waiting_review(
    root: &Path,
    display: &str,
    spec: &str,
    change: Option<&str>,
) -> Result<(), String> {
    match resolve_rework_review(root, display, spec, change) {
        Err(ReworkReviewError::Waiting(why)) => Err(why),
        // Existing call sites retain their own unavailable-evidence policy.
        _ => Ok(()),
    }
}

// trace:BUG-2500 | ai:codex
pub(crate) fn refuse_waiting_pickup(
    root: &Path,
    plan: &QueueWorkPlan,
    role: &str,
    list_sessions: bool,
    plan_only: bool,
    guided: bool,
) -> Result<(), String> {
    if role.eq_ignore_ascii_case("reviewer") || list_sessions || plan_only || guided {
        return Ok(());
    }
    for entry in &plan.entries {
        let display = if plan.entries.len() == 1 {
            &plan.anchor_display
        } else {
            &entry.spec_id
        };
        let open = spec_open_change_key(root, &[display, &entry.spec_id]);
        refuse_waiting_review(root, display, &entry.spec_id, open.as_deref())?;
    }
    Ok(())
}

// trace:BUG-2500 | ai:codex
#[derive(Debug)]
pub(crate) enum ReworkReviewError {
    Waiting(String),
    Unavailable(String),
}

// Preserve a known wait even when competing records cannot be ordered. An
// unavailable-evidence warning must never turn a known wait into admission.
// trace:BUG-2500 | ai:codex
fn waiting_rework_error(
    shown: &str,
    verdict: &crate::gate::review_verdict::RecordedVerdict,
) -> Result<(), ReworkReviewError> {
    if verdict.kind == crate::gate::review_verdict::VerdictKind::Waiting && !verdict.is_closed() {
        return Err(ReworkReviewError::Waiting(format!(
            "{shown}: WAITING — {}. Resume with a fresh review at the current head; no implementer rework is queued.",
            verdict.summary.as_deref().unwrap_or("external evidence required")
        )));
    }
    Ok(())
}

pub(crate) fn resolve_rework_review(
    project_root: &Path,
    display_id: &str,
    spec_id: &str,
    open_change: Option<&str>,
) -> Result<ReviewResult<String>, ReworkReviewError> {
    use crate::gate::review_alias::{select_verdict, spec_labels, AliasSelection};
    use crate::gate::review_verdict::RecordedVerdict;
    let ids = [display_id, spec_id];
    let labels = spec_labels(&ids);
    // Refs-only batches do not complete the spec, but their terminal reviews
    // must not block the next batch. Reconcile only this spec's proved keys.
    // trace:BUG-2525 | ai:codex
    // BUG-2540: the keys bound to this spec, computed once. Resolving a key's
    // current verdict costs git subprocesses, so the readers below resolve
    // only these instead of every key in the store (thousands: 180 s+).
    // trace:BUG-2540 | ai:claude
    let all_keys = crate::gate::review_verdict::verdict_keys(project_root);
    let (bound_keys, rest): (Vec<String>, Vec<String>) = all_keys
        .into_iter()
        .partition(|key| change_verdict_bound_to(project_root, key, &labels));
    let open_keys: Vec<String> = rest
        .into_iter()
        .filter(|key| open_change.is_some_and(|open| open.eq_ignore_ascii_case(key)))
        .collect();
    let mut review_keys = labels.clone();
    review_keys.extend(bound_keys.iter().cloned());
    review_keys.extend(open_keys.iter().cloned());
    crate::review_waiting::reconcile_waiting_verdicts(project_root, Some(&review_keys));
    let shown = labels
        .first()
        .cloned()
        .unwrap_or_else(|| display_id.to_string());
    // Ignore terminal waits before alias ordering as well as PR/spec ordering.
    // Otherwise a late annotation can hide an active review on another label.
    // Refused or unreadable labels remain in the strict selector.
    // trace:BUG-2525 | ai:codex
    let active_ids: Vec<&str> = ids
        .iter()
        .copied()
        .filter(|id| {
            !crate::gate::review_verdict::current_verdict(project_root, id)
                .and_then(|c| crate::gate::review_verdict::parse_recorded_verdict(&c.body))
                .is_some_and(|v| {
                    v.kind == crate::gate::review_verdict::VerdictKind::Waiting && v.is_closed()
                })
        })
        .collect();
    let spec_keyed: Option<(RecordedVerdict, Option<chrono::DateTime<chrono::Utc>>)> =
        match select_verdict(project_root, &active_ids) {
            AliasSelection::Selected(s) => {
                let at = verdict_recorded_time(
                    &s.verdict,
                    s.order_time.map(|t| t.with_timezone(&chrono::Utc)),
                );
                Some((s.verdict, at))
            }
            AliasSelection::Absent => None,
            unavailable => {
                // Substantive findings remain actionable even beside a wait.
                // Inspect all candidates before refusing a waiting-only review.
                // trace:BUG-2500 | ai:codex
                let mut candidates = Vec::new();
                for label in &labels {
                    if let Some(current) =
                        crate::gate::review_verdict::current_verdict(project_root, label)
                    {
                        if let Some(v) =
                            crate::gate::review_verdict::parse_recorded_verdict(&current.body)
                        {
                            candidates.push((label.clone(), v));
                        }
                    }
                }
                candidates.extend(crate::gate::review_verdict::active_verdicts_for(
                    project_root,
                    bound_keys.iter().chain(&open_keys).cloned(),
                ));
                if candidates.iter().any(|(_, v)| {
                    !v.is_closed() && v.kind == crate::gate::review_verdict::VerdictKind::Waiting
                }) {
                    let findings = candidates
                        .iter()
                        .filter(|(_, v)| !v.is_closed() && v.kind.requires_changes())
                        .filter_map(|(key, v)| {
                            crate::review_verdict_display::rework_findings_comment(&shown, key, v)
                        })
                        .collect::<Vec<_>>();
                    if !findings.is_empty() {
                        return Ok(ReviewResult::Available(findings.join("\n\n")));
                    }
                    for (_, v) in &candidates {
                        waiting_rework_error(&shown, v)?;
                    }
                }
                let reason = unavailable
                    .unavailable_reason()
                    .unwrap_or_default()
                    .to_string();
                if unavailable.fail_closed().is_some() {
                    return Err(ReworkReviewError::Unavailable(format!(
                        "a request-changes review of {shown} exists but its current review cannot be selected ({reason})"
                    )));
                }
                eprintln!(
                    "warning: no review findings imported for {shown}: its current review cannot be selected ({reason}). Read the review comment before starting rework."
                );
                return Ok(ReviewResult::Unavailable);
            }
        };

    // A closed wait has no selection priority over any active review.
    // trace:BUG-2525 | ai:codex
    let spec_keyed = spec_keyed.filter(|(v, _)| {
        !(v.kind == crate::gate::review_verdict::VerdictKind::Waiting && v.is_closed())
    });

    let open_key = open_change
        .and_then(crate::gate::ledger::ChangeKey::parse)
        .map(|k| k.as_key());
    let change_keyed: Option<(String, RecordedVerdict)> = match open_key.as_deref() {
        // The spec's open PR answers for its change key, approval included:
        // a newer approval there means no findings are outstanding.
        Some(key) => match crate::gate::review_verdict::current_verdict(project_root, key) {
            None => None,
            Some(current) => {
                match crate::gate::review_verdict::parse_recorded_verdict(&current.body) {
                    Some(v) => Some((key.to_string(), v)),
                    None => {
                        return Err(ReworkReviewError::Unavailable(format!(
                            "the review recorded for {key} (the open pull request of {shown}) cannot be read"
                        )))
                    }
                }
            }
        },
        None => None,
    };
    let change_keyed = change_keyed.filter(|(_, v)| {
        !(v.kind == crate::gate::review_verdict::VerdictKind::Waiting && v.is_closed())
    });
    let change_keyed = match change_keyed {
        Some(found) => Some(found),
        None => {
            // M1: every key with a verdict, the ledger's included, each read
            // as the gates weigh it.
            // trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
            // trace:TASK-1191 | ai:codex
            let mut bound =
                crate::gate::review_verdict::active_verdicts_for(project_root, bound_keys.clone())
                    .into_iter()
                    .filter(|(_, verdict)| verdict.kind.blocks_done() && !verdict.is_closed())
                    .collect::<Vec<_>>();
            // A wait cannot hide actionable findings in another bound change.
            // trace:BUG-2500 | ai:codex
            if bound.len() > 1
                && (bound.iter().any(|(_, v)| {
                    !v.is_closed() && v.kind == crate::gate::review_verdict::VerdictKind::Waiting
                }) || spec_keyed.as_ref().is_some_and(|(v, _)| {
                    !v.is_closed() && v.kind == crate::gate::review_verdict::VerdictKind::Waiting
                }))
            {
                let mut findings = bound
                    .iter()
                    .filter(|(_, v)| !v.is_closed() && v.kind.requires_changes())
                    .filter_map(|(key, v)| {
                        crate::review_verdict_display::rework_findings_comment(&shown, key, v)
                    })
                    .collect::<Vec<_>>();
                if let Some((v, _)) = &spec_keyed {
                    if !v.is_closed() && v.kind.requires_changes() {
                        if let Some(block) = crate::review_verdict_display::rework_findings_comment(
                            &shown, &shown, v,
                        ) {
                            findings.push(block);
                        }
                    }
                }
                if !findings.is_empty() {
                    return Ok(ReviewResult::Available(findings.join("\n\n")));
                }
                for (_, v) in &bound {
                    waiting_rework_error(&shown, v)?;
                }
            }
            if bound.len() > 1 && spec_keyed.is_some() {
                // The spec's own record answers, as before BUG-2430.
                bound.clear();
            } else if bound.len() > 1 {
                return Err(ReworkReviewError::Unavailable(format!(
                    "{} changes each hold a current refusal for {shown} ({})",
                    bound.len(),
                    bound
                        .iter()
                        .map(|(k, _)| k.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            bound.pop()
        }
    };

    let change_time = |key: &str, v: &RecordedVerdict| {
        let written = crate::gate::review_verdict::current_verdict(project_root, key)
            .and_then(|c| c.written_at)
            .map(chrono::DateTime::<chrono::Utc>::from);
        verdict_recorded_time(v, written)
    };
    // Only an explicit approval clears durable findings; an unknown verdict
    // supplies no authoritative clearance.
    // trace:BUG-2509 | ai:codex
    let classify = |v: &RecordedVerdict, findings: Option<String>| match findings {
        Some(findings) => ReviewResult::Available(findings),
        None if v.kind.approves() => ReviewResult::ClearedByApproval,
        None => ReviewResult::Unavailable,
    };
    let render_change = |key: &str, v: &RecordedVerdict| {
        let review_ref = review_ref_from_verdict_filename(
            &crate::gate::review_verdict::verdict_path(project_root, key),
        );
        classify(
            v,
            crate::review_verdict_display::rework_findings_comment(display_id, &review_ref, v),
        )
    };
    // Named by the caller's first label, not the answering partition's: the
    // recurrence guard compares whole blocks, so the same findings must
    // render the same whichever label recorded them.
    let render_spec = |v: &RecordedVerdict| {
        classify(
            v,
            crate::review_verdict_display::rework_findings_comment(&shown, &shown, v),
        )
    };
    // trace:BUG-2500 | ai:codex
    let refuse_wait = |v: &RecordedVerdict| waiting_rework_error(&shown, v);
    Ok(match (spec_keyed, change_keyed) {
        (None, None) => ReviewResult::Unavailable,
        (Some((v, _)), None) => {
            refuse_wait(&v)?;
            render_spec(&v)
        }
        (None, Some((key, v))) => {
            refuse_wait(&v)?;
            render_change(&key, &v)
        }
        (Some((sv, s_at)), Some((key, cv))) => {
            // Closing a terminal batch appends an annotation, not a fresh
            // review. Its new ledger timestamp cannot clear code findings.
            // trace:BUG-2525 | ai:codex
            if sv.kind == crate::gate::review_verdict::VerdictKind::Waiting
                && sv.is_closed()
                && !cv.is_closed()
                && cv.kind.requires_changes()
            {
                return Ok(render_change(&key, &cv));
            }
            if cv.kind == crate::gate::review_verdict::VerdictKind::Waiting
                && cv.is_closed()
                && !sv.is_closed()
                && sv.kind.requires_changes()
            {
                return Ok(render_spec(&sv));
            }
            let c_at = change_time(&key, &cv);
            // A wait carries no findings and must not replace substantive work.
            // trace:BUG-2500 | ai:codex
            let mixed_wait = !sv.is_closed()
                && !cv.is_closed()
                && ((sv.kind == crate::gate::review_verdict::VerdictKind::Waiting
                    && cv.kind.requires_changes())
                    || (cv.kind == crate::gate::review_verdict::VerdictKind::Waiting
                        && sv.kind.requires_changes()));
            let change_newer = match (s_at, c_at) {
                // A fresh review supersedes older evidence, including code
                // findings. Only tied or unordered evidence needs priority.
                (Some(s), Some(c)) if s != c || !mixed_wait => c > s,
                _ if mixed_wait => cv.kind.requires_changes(),
                // Unordered: fail closed toward the refusal so its findings
                // are not dropped.
                _ => cv.kind.blocks_done() && !sv.kind.blocks_done(),
            };
            if change_newer {
                refuse_wait(&cv)?;
                render_change(&key, &cv)
            } else {
                refuse_wait(&sv)?;
                render_spec(&sv)
            }
        }
    })
}

/// BUG-225: render the copy-pasteable `claude` command line for a
/// headless launch deferred by `--no-launch`. Built from
/// `session::claude_headless_args_with_posture` — the exact argv `exec_claude_headless`
/// feeds to `claude` — so the printed hint can never drift from the real
/// launch (correct flag order, `--session-id` included, prompt last).
/// STORY-278: prefix `AIDA_HEADLESS=1` so the copy-paste hint also sets
/// the env var `exec_claude_headless` puts on the child via `.env(...)`.
/// trace:BUG-225 | ai:claude
pub(crate) fn headless_launch_hint(prompt: &str, session_id: &str, contained: bool) -> String {
    // BUG-1982: a refused contained launch shows the refusal, not a command.
    // trace:BUG-1982 | ai:claude
    // TASK-2032: a `--no-launch` hint renders before any chdir into a seat.
    // trace:TASK-2032 | ai:claude
    match session::claude_headless_args_with_posture(
        crate::seat_clone::launcher_main().as_deref(),
        prompt,
        session_id,
        contained,
    ) {
        Ok(argv) => format!("AIDA_HEADLESS=1 claude {}", shell_join_display(&argv)),
        Err(err) => format!("{err:#}"),
    }
}

// BUG-913: deferred queue-work launch instructions must reflect the resolved
// vendor. The Claude arm deliberately reuses the exact historical helper so its
// round-trip invariant stays unchanged; non-Claude arms name the command the
// equivalent launch path would use. trace:BUG-913 | ai:codex
pub(crate) fn deferred_headless_launch_hint(
    vendor: session::HeadlessVendor,
    prompt: &str,
    session_id: &str,
    contained: bool,
    model: Option<&str>,
) -> String {
    match vendor {
        session::HeadlessVendor::Claude => headless_launch_hint(prompt, session_id, contained),
        session::HeadlessVendor::Codex => {
            let argv = session::codex_headless_args_with_model(prompt, model);
            format!("AIDA_HEADLESS=1 codex {}", shell_join_display(&argv))
        }
        session::HeadlessVendor::Agy => {
            // A manual paste is also a launch path: never offer the legacy
            // bypass command without the shared wrapper's applied receipt.
            // trace:TASK-3048 | ai:codex
            match crate::launch::agy::AGY.require_conformance() {
                Err(error) => format!("{error:#}"),
                Ok(()) => "agy headless launch requires AIDA's shared wrapper".into(),
            }
        }
    }
}

// BUG-913: same vendor-aware rendering for `--no-launch` interactive handoff
// text. Without this, a codex-only project correctly prepared a codex session
// but still told the operator to run `claude /aida-pickup ...`.
// trace:BUG-913 | ai:codex
pub(crate) fn deferred_interactive_launch_hint(
    vendor: session::HeadlessVendor,
    prompt: &str,
    permission_mode: Option<&str>,
    contained: bool,
    model: Option<&str>,
) -> String {
    match vendor {
        session::HeadlessVendor::Claude => {
            let mut args = match session::claude_session_args(
                crate::seat_clone::launcher_main().as_deref(),
                permission_mode,
                None,
                Some(prompt),
                None,
                contained,
                model,
            ) {
                Ok(args) => args,
                // trace:BUG-1982 | ai:claude
                Err(err) => return format!("{err:#}"),
            };
            if contained && !args.iter().any(|arg| arg == "--permission-mode") {
                args.splice(
                    0..0,
                    ["--permission-mode".to_string(), "dontAsk".to_string()],
                );
            }
            format!("claude {}", shell_join_display(&args))
        }
        session::HeadlessVendor::Codex => {
            let bypass = permission_mode == Some("bypassPermissions");
            // trace:BUG-2394 | ai:claude
            let read_only = session::codex_plan_read_only(permission_mode);
            format!(
                "codex {}",
                shell_join_display(&session::codex_interactive_session_args(
                    prompt, bypass, read_only, model
                ))
            )
        }
        session::HeadlessVendor::Agy => {
            format!("agy {}", shell_join_display(&[prompt.to_string()]))
        }
    }
}

pub(crate) fn claude_posture_display(permission_mode: Option<&str>, contained: bool) -> String {
    if contained {
        "contained sandbox".to_string()
    } else {
        permission_mode
            .map(|m| format!("permission-mode {}", m))
            .unwrap_or_else(|| "native permission posture".to_string())
    }
}

/// BUG-1517: `--resume-dry-run` is a read-only preview of the reconciled
/// re-entry plan — it never re-enters the drain — so it is disposition, not
/// dispatch, and must not require `has_dispatch_authority()`. A live (non
/// dry-run) launch stays gated exactly as before. Pure so the two halves of
/// the acceptance criteria are unit-testable without spinning up a store.
// trace:BUG-1517 | ai:claude
pub(crate) fn auto_complete_dispatch_authority_ok(
    resume_dry_run: bool,
    has_dispatch_authority: bool,
) -> Result<(), &'static str> {
    if resume_dry_run || has_dispatch_authority {
        Ok(())
    } else {
        Err(
            "starting an autonomous drain needs dispatch authority (product, advisor, \
             or integrator role, or a live orchestrator)",
        )
    }
}

// trace:TASK-1337 | ai:codex
pub(crate) fn queue_work_role_mismatch(
    role: &str,
    origin: &str,
    shell_role: Option<&str>,
) -> Option<String> {
    let shell = shell_role.filter(|value| !value.trim().is_empty())?;
    (!canonical_role_name(shell).eq_ignore_ascii_case(&canonical_role_name(role))).then(||
        format!("shell role `{shell}` differs from pickup role `{role}`; {origin} wins before setup (use --role {shell} to choose the shell role)"))
}

/// A standalone `aida queue work <PR-N> --role reviewer`: a reviewer pickup on
/// a PR scope that no `--auto-complete` orchestrator spawned (the orchestrator
/// sets `AIDA_REVIEW_VERDICT_FILE` on its reviewer child). Launches through
/// `run_standalone_reviewer`, which mints a `reviewer` child grant on both the
/// interactive and the `--no-human` path.
// trace:BUG-226, BUG-1808 | ai:claude
pub(crate) fn is_standalone_reviewer(role: &str, has_review_target: bool) -> bool {
    has_review_target
        && role.eq_ignore_ascii_case("reviewer")
        && std::env::var("AIDA_REVIEW_VERDICT_FILE")
            .ok()
            .filter(|s| !s.is_empty())
            .is_none()
}

/// Whether this `queue work` invocation will later mint a child seat grant
/// via `seat_authority::issue_child`, and so must preflight delegation before
/// any mutation. Mirrors the launch arms of `handle_queue_work`:
/// - `--no-launch` launches nothing, so no child grant;
/// - every other launch — interactive, headless resume, standalone reviewer
///   and (BUG-2139) a fresh headless pickup — mints one for the pickup role.
///   A fresh headless pickup never runs under the launcher's grant.
// trace:BUG-1808 | ai:claude
// trace:BUG-2139 | ai:claude
pub(crate) fn queue_work_issues_child_grant(no_launch: bool) -> bool {
    !no_launch
}

/// BUG-2139: whether this launch is a fresh headless pickup — the arm that
/// execs the vendor through `session::exec_vendor_headless`. It is the one
/// arm where a launcher that cannot delegate a BASELINE seat still launches,
/// with the child at baseline authority (an empty
/// [`crate::seat_authority::GRANT_ENV`]); every other
/// arm requires delegation.
// trace:BUG-2139 | ai:claude
pub(crate) fn queue_work_is_fresh_headless(
    no_human: bool,
    resume: bool,
    standalone_reviewer: bool,
) -> bool {
    no_human && !resume && !standalone_reviewer
}

/// BUG-2139: the one line a baseline headless launch prints.
// trace:BUG-2139 | ai:claude
pub(crate) fn headless_baseline_notice(seat: &str, cause: &str) -> String {
    // trace:TASK-1752 | ai:claude
    let grant_env = crate::gate::seat_authority::GRANT_ENV;
    format!(
        "headless `{seat}` child runs ungranted at baseline authority ({grant_env}=\"\"): \
         the launcher cannot delegate `{seat}` ({cause})"
    )
}

/// The refusal printed when the launcher's grant cannot delegate the child
/// seat the pickup would launch. It names the decision that failed (child
/// delegation) separately from the one that succeeded (queue selection), and
/// tells a coordinating launcher (advisor / product / integrator) to choose a
/// seat its grant delegates with an explicit `--role`.
// trace:BUG-1808 | ai:claude
pub(crate) fn queue_work_delegation_refusal(
    child_seat: &str,
    role_origin: &str,
    launcher: Option<(&str, &[String])>,
    cause: &str,
) -> String {
    // One line: agent-mode error rendering keeps only the first line.
    let mut message = format!(
        "child-seat delegation refused before any pickup state changed: queue selection \
         succeeded ({role_origin} chose role `{child_seat}`), but this launcher may not start a \
         `{child_seat}` child session: {cause}"
    );
    let Some((seat, delegable)) = launcher else {
        return message;
    };
    let coordinator = matches!(
        crate::canonical_role_name(seat).as_str(),
        "advisor" | "product" | "integrator"
    );
    if !coordinator {
        return message;
    }
    let choices: Vec<&str> = delegable
        .iter()
        .map(String::as_str)
        .filter(|s| *s != child_seat)
        .collect();
    if choices.is_empty() {
        message.push_str(&format!(
            "; the `{seat}` grant delegates no seat, so re-enter it with `--delegate-seat {child_seat}`"
        ));
    } else {
        message.push_str(&format!(
            "; choose a seat the `{seat}` grant delegates with an explicit role selector: {}",
            choices
                .iter()
                .map(|s| format!("`--role {s}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    message
}

// trace:TASK-1337 | ai:codex
// trace:BUG-2396 | ai:claude — the launch line follows the resolved host vendor.
pub(crate) fn queue_work_manual_continuation(
    id: &str,
    guided: bool,
    vendor: crate::session::HeadlessVendor,
) -> String {
    let skill = if guided {
        "aida-guided-implement"
    } else {
        "aida-pickup"
    };
    format!(
        "Pickup setup exists; manual continuation:\n  aida worktree enter {id}\n  {}",
        crate::vendor_hints::launch_with_skill(vendor, skill, id)
    )
}

#[cfg(test)]
mod bug2396_manual_continuation_tests {
    use crate::session::HeadlessVendor;

    // trace:BUG-2396 | ai:claude
    #[test]
    fn manual_continuation_renders_codex_and_claude() {
        let codex = super::queue_work_manual_continuation("BUG-1", false, HeadlessVendor::Codex);
        assert!(codex.ends_with("\n  codex '$aida-pickup BUG-1'"), "{codex}");
        let claude = super::queue_work_manual_continuation("BUG-1", true, HeadlessVendor::Claude);
        assert!(
            claude.ends_with("\n  claude '/aida-guided-implement BUG-1'"),
            "{claude}"
        );
    }
}

/// Explicit/rework branches must be available before any setup mutation.
/// An idle, clean, unleased checkout is offered for manual retirement; it is
/// never removed automatically, and active/dirty/locked checkouts stay protected.
// trace:TASK-1337 | ai:codex
pub(crate) fn preflight_queue_work_branch(
    root: &std::path::Path,
    branch: Option<&str>,
) -> Result<()> {
    preflight_queue_work_branch_adopting(root, branch, None)
}

/// A finished or crashed review may leave its disposable checkout. An ended
/// reviewer lease additionally requires current forge-head coverage.
/// Retire only the default review tree, so ordinary occupied branches
/// and explicitly placed worktrees retain the usual refusal.
// trace:BUG-2457 | ai:codex
pub(crate) fn retire_idle_review_worktree(
    root: &std::path::Path,
    forge: ReviewForge,
    n: u64,
) -> Result<()> {
    let branch = forge.local_branch_for(n);
    let path = crate::session_start::pickup_worktree_path(root, &branch)?;
    let output = aida_core::git_ops::host_git_command(root)
        .args(["worktree", "list", "--porcelain"])
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "could not inspect review worktree ownership"
    );
    let inventory = String::from_utf8_lossy(&output.stdout);
    let candidate = crate::worktree_reclaim::parse_worktree_entries(&inventory)
        .into_iter()
        .any(|wt| {
            wt.branch.as_deref() == Some(branch.as_str())
                && crate::worktree::canonical_or_self(&wt.path)
                    == crate::worktree::canonical_or_self(&path)
        });
    if !candidate {
        return Ok(());
    }
    // A finished review may already have released ownership; a crashed one
    // needs positive death proof. Only one exact reviewer lease is eligible.
    // trace:BUG-2498 BUG-2469 | ai:codex
    let leases: Vec<_> = list_leases(root)
        .into_iter()
        .filter(|l| l.branch == branch || l.worktree_path == path)
        .collect();
    let reclaim = match leases.as_slice() {
        [] => None,
        [lease]
            if lease.branch == branch
                && crate::worktree::canonical_or_self(&lease.worktree_path)
                    == crate::worktree::canonical_or_self(&path)
                && crate::pr_lease_takeover::reviewer_target(lease) == Some((forge, n))
                && crate::ownership::reader::review_lease_ended(root, lease) =>
        {
            Some(lease)
        }
        _ => return preflight_queue_work_branch(root, Some(&branch)),
    };
    let locked = inventory.split("\n\n").any(|record| {
        record
            .lines()
            .any(|line| line == format!("branch refs/heads/{branch}"))
            && record.lines().any(|line| line.starts_with("locked"))
    });
    let live = aida_core::liveness::probe_live_agent_processes()
        .iter()
        .any(|p| !p.stale_cwd && p.cwd.starts_with(&path));
    let inside = std::env::current_dir().is_ok_and(|cwd| cwd.starts_with(&path));
    if locked
        || live
        || inside
        || working_tree_clean(&path) != Some(true)
        || aida_core::worktree_holders::check_worktree_removal(&path).is_err()
    {
        return preflight_queue_work_branch(root, Some(&branch));
    }
    // Require both the recorded review head and every actual seat commit to
    // survive at the fresh forge head. Force-pushes and local fixes retain it.
    // trace:BUG-2469 | ai:codex
    let lease_snapshot = if let Some(lease) = reclaim {
        let eligible = (|| -> Result<()> {
            let old = lease
                .pr_head_sha
                .as_deref()
                .context("review lease has no recorded head")?;
            let head = crate::pr_lease_takeover::fetch_review_head(root, forge, n)?;
            anyhow::ensure!(
                crate::pr_lease_takeover::is_ancestor(root, old, &head),
                "recorded review head is not covered"
            );
            anyhow::ensure!(
                crate::seat_clone::remote_missing_commits_with_review_head(
                    root,
                    &path,
                    false,
                    &branch,
                    Some(&head)
                )? == 0,
                "review checkout has unpublished commits"
            );
            Ok(())
        })();
        // Preserve the failed evidence check in the operator-facing refusal,
        // including nested fetch/coverage diagnostics. No retirement is safe.
        // trace:TASK-b6b1278890774ef993d2ebc327862470-900 | ai:codex
        if let Err(reason) = eligible {
            anyhow::bail!(
                "branch `{branch}` is held by worktree {} before pickup setup. \
                 Ended reviewer lease {} was not reclaimed: {reason:#}. \
                 Keep this checkout; inspect its review head and unpublished commits, then retry pickup.",
                path.display(),
                lease.id
            );
        }
        let snapshot = std::fs::read(crate::lease_path(root, &lease.id))?;
        let current: SessionLease = toml::from_str(std::str::from_utf8(&snapshot)?)?;
        anyhow::ensure!(
            toml::to_string(&current)? == toml::to_string(lease)?,
            "review lease changed during retirement checks"
        );
        Some(snapshot)
    } else {
        None
    };
    // Collect a legacy hold before removal, even if it predates this pickup.
    // Unknown markers and unreadable directories retain the checkout.
    let markers = path.join(".aida/merge-holds");
    for dir in [path.join(".aida"), markers.clone()] {
        match std::fs::symlink_metadata(&dir) {
            Ok(metadata) => anyhow::ensure!(
                metadata.is_dir(),
                "review state directory is not a directory: {}",
                dir.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    crate::merge_hold::adopt_worktree_hold(root, &path, n, std::time::UNIX_EPOCH)?;
    match std::fs::symlink_metadata(&markers) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_dir(),
                "review merge-hold directory is not a directory"
            );
            if std::fs::read_dir(&markers)?.next().transpose()?.is_some() {
                return preflight_queue_work_branch(root, Some(&branch));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    // Revalidate the owner and exact lease before tearing down its tree.
    // trace:BUG-2498 | ai:codex
    // trace:BUG-2469 | ai:codex
    if let Some(lease) = reclaim {
        anyhow::ensure!(
            crate::ownership::reader::review_lease_ended(root, lease)
                && Some(std::fs::read(crate::lease_path(root, &lease.id))?) == lease_snapshot
                && working_tree_clean(&path) == Some(true),
            "review ownership or working tree changed before retirement"
        );
        crate::session_end::guard_remote_commits(root, lease, false)?;
        let current: Vec<_> = list_leases(root)
            .into_iter()
            .filter(|l| l.branch == branch || l.worktree_path == path)
            .collect();
        anyhow::ensure!(
            current.len() == 1 && current[0].id == lease.id,
            "review lease ownership became ambiguous"
        );
    }
    // trace:STORY-2325 | ai:codex
    if let Some(lease) = reclaim {
        crate::ownership::reader::journal_review_reclaim(root, lease).with_context(|| {
            format!(
                "ownership reclaim could not end lease {}; inspect `aida session end {}`",
                lease.id, lease.id
            )
        })?;
    }
    aida_core::worktree_holders::check_worktree_removal(&path)?;
    aida_core::mailbox_lifecycle::guard::remove_worktree_retained(
        root,
        &path,
        false,
        "next-head review pickup",
    )?;
    if let Some(lease) = reclaim {
        crate::session_reap::release_lease_files(root, &lease.id);
        anyhow::ensure!(
            !crate::lease_path(root, &lease.id).exists(),
            "retired review tree but could not release lease {}",
            lease.id
        );
    }
    Ok(())
}

/// [`preflight_queue_work_branch`], except that the orphan's tree `adopting`
/// (`aida queue work --resume`) holds its own branch: that is the tree the
/// pickup re-enters, and session start re-verifies it.
// trace:BUG-2223 | ai:claude
pub(crate) fn preflight_queue_work_branch_adopting(
    root: &std::path::Path,
    branch: Option<&str>,
    adopting: Option<&std::path::Path>,
) -> Result<()> {
    preflight_queue_work_branch_reclaiming(root, branch, adopting, false)
}

/// Explicit force-claim can retire an exited lane before the branch occupancy
/// refusal. Resume adoption and ordinary preflights remain read-only.
// trace:BUG-2455 | ai:codex
pub(crate) fn preflight_queue_work_branch_reclaiming(
    root: &std::path::Path,
    branch: Option<&str>,
    adopting: Option<&std::path::Path>,
    force_claim: bool,
) -> Result<()> {
    let Some(branch) = branch else {
        return Ok(());
    };
    let adopted = |path: &std::path::Path| {
        adopting.is_some_and(|a| {
            crate::worktree::canonical_or_self(a) == crate::worktree::canonical_or_self(path)
        })
    };
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .context("could not inspect branch worktree ownership")?;
    if !out.status.success() {
        anyhow::bail!(
            "could not inspect branch worktree ownership: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    // BUG-2169 slice S4: a contained seat's private clone on the branch holds
    // it too. It is never offered for `git worktree remove` (not a linked
    // worktree); ending its session salvages and removes it.
    // trace:TASK-2053 | ai:claude
    let clones: Vec<_> = clone_seat_trees(root)
        .into_iter()
        .filter(|wt| wt.branch.as_deref() == Some(branch) && !adopted(&wt.path))
        .collect();
    // Validate the entire set before retiring any clone. Teardown still
    // rechecks each owner and working tree to protect against later races.
    // trace:BUG-2455 | ai:codex
    if force_claim {
        for clone in &clones {
            check_exited_branch_clone(root, branch, &clone.path)?;
        }
    }
    for clone in &clones {
        if force_claim {
            reclaim_exited_branch_clone(root, branch, &clone.path)?;
            continue;
        }
        anyhow::bail!(
            "branch `{branch}` is held by a contained seat's private clone at {} before pickup \
             setup. Continue it in that seat, end that seat's session first (`aida session end`), \
             then retry pickup on this branch.",
            clone.path.display()
        );
    }
    let inventory = String::from_utf8_lossy(&out.stdout);
    for wt in crate::worktree_reclaim::parse_worktree_entries(&inventory) {
        if wt.branch.as_deref() != Some(branch) || adopted(&wt.path) {
            continue;
        }
        let leased = list_leases(root)
            .iter()
            .any(|lease| lease.branch == branch || lease.worktree_path == wt.path);
        let live = aida_core::liveness::probe_live_agent_processes()
            .iter()
            .any(|process| !process.stale_cwd && process.cwd.starts_with(&wt.path));
        let locked = inventory.split("\n\n").any(|record| {
            record
                .lines()
                .any(|line| line == format!("branch refs/heads/{branch}"))
                && record.lines().any(|line| line.starts_with("locked"))
        });
        let clean = working_tree_clean(&wt.path) == Some(true);
        let recovery = if clean && !leased && !live && !locked && wt.path != root {
            format!("It is idle, clean, and unleased. To retire it, run `git worktree remove {}` then retry pickup.", shell_quote(&wt.path.to_string_lossy()))
        } else {
            "Keep this checkout; inspect its lease/process/dirty/lock state, then retry pickup on this branch.".to_string()
        };
        anyhow::bail!(
            "branch `{branch}` is held by worktree {} before pickup setup. {recovery}",
            wt.path.display()
        );
    }
    Ok(())
}

/// Read-only admission check shared by the all-clone preflight and teardown.
// trace:BUG-2455 | ai:codex
pub(crate) fn check_exited_branch_clone(
    root: &std::path::Path,
    branch: &str,
    path: &std::path::Path,
) -> Result<SessionLease> {
    let leases: Vec<_> = list_leases(root)
        .into_iter()
        .filter(|l| {
            crate::worktree::canonical_or_self(&l.worktree_path)
                == crate::worktree::canonical_or_self(path)
        })
        .collect();
    anyhow::ensure!(
        !leases.is_empty(),
        "cannot force-claim branch `{branch}`: private clone {} has no matching lease; inspect its sessions before retrying",
        path.display()
    );
    anyhow::ensure!(
        leases.len() == 1,
        "cannot force-claim branch `{branch}`: private clone {} has several matching leases (ambiguous lease ownership); inspect its sessions before retrying",
        path.display()
    );
    let lease = &leases[0];
    anyhow::ensure!(
        lease.tree_kind == crate::seat_clone::TreeKind::Clone && lease.branch == branch,
        "cannot force-claim branch `{branch}`: private clone {} no longer has the expected lease",
        path.display()
    );
    match crate::pr_lease_takeover::check_pr_lease(root, lease, branch) {
        crate::pr_lease_takeover::PrLeaseCheck::SafeToEnd => {}
        other => {
            let reason = match other {
                crate::pr_lease_takeover::PrLeaseCheck::Live => {
                    "its owning session is still live".to_string()
                }
                crate::pr_lease_takeover::PrLeaseCheck::NotProvable(why)
                | crate::pr_lease_takeover::PrLeaseCheck::Refuse(why) => why,
                crate::pr_lease_takeover::PrLeaseCheck::SafeToEnd => unreachable!(),
            };
            let stat = crate::seat_clone::pending_diff_stat(root, path)
                .unwrap_or_else(|e| format!("unavailable ({e:#})"));
            anyhow::bail!(
                "cannot force-claim branch `{branch}`: retained lease {} and private clone {}: {reason}. Diff stat: {stat}. Commit and push pending work there, or resume that session, then retry",
                lease.id, path.display()
            );
        }
    }
    Ok(leases.into_iter().next().unwrap())
}

/// Reuse the PR relaunch evidence and conservative clone teardown. Unlike
/// --steal, force-claim never removes a clone holding unpublished work.
// trace:BUG-2455 | ai:codex
pub(crate) fn reclaim_exited_branch_clone(
    root: &std::path::Path,
    branch: &str,
    path: &std::path::Path,
) -> Result<()> {
    let lease = check_exited_branch_clone(root, branch, path)?;
    // KeepUnshipped re-probes immediately before teardown and checks live
    // holders. Any new dirty or unpushed work retains the clone and lease.
    match crate::seat_clone::destroy(
        root,
        path,
        crate::seat_clone::DestroyMode::KeepUnshipped,
        None,
    )? {
        crate::seat_clone::Destroyed::Removed { .. } => {
            crate::session_reap::release_lease_files(root, &lease.id);
            anyhow::ensure!(
                !crate::lease_path(root, &lease.id).exists(),
                "retired private clone {} but could not release lease {}",
                path.display(),
                lease.id
            );
            eprintln!(
                "  retired exited lease {} and clean, pushed private clone {} (--force-claim)",
                lease.id,
                path.display()
            );
            Ok(())
        }
        crate::seat_clone::Destroyed::Kept { reason, .. } => {
            anyhow::bail!("cannot force-claim branch `{branch}`: retained lease {} and private clone {}: {reason}", lease.id, path.display())
        }
    }
}

#[cfg(test)]
mod force_claim_ownership_tests {
    use super::*;

    // Admission must refuse before touching an orphan or an ambiguous clone.
    // trace:BUG-2455 | ai:codex
    #[test]
    fn bug_2455_missing_or_multiple_leases_keep_clone() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("main");
        let clone = temp.path().join("clone");
        std::fs::create_dir_all(&root).unwrap();
        assert!(crate::test_git::git_command()
            .arg("-C")
            .arg(&root)
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success());
        assert!(crate::test_git::git_command()
            .args(["clone", "-q"])
            .arg(&root)
            .arg(&clone)
            .status()
            .unwrap()
            .success());
        std::fs::write(clone.join("pending.txt"), "keep me").unwrap();
        let err = reclaim_exited_branch_clone(&root, "rework", &clone)
            .unwrap_err()
            .to_string();
        assert!(err.contains("no matching lease"), "{err}");
        assert!(err.contains(&clone.display().to_string()), "{err}");
        assert_eq!(
            std::fs::read_to_string(clone.join("pending.txt")).unwrap(),
            "keep me"
        );

        for id in ["first", "second"] {
            let path = crate::lease_path(&root, id);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, format!(
                "id = \"{id}\"\nscope = \"BUG-2455\"\nslug = \"bug-2455\"\nowner = \"test\"\nworktree_path = {}\nbranch = \"rework\"\nstarted_at = \"2026-11-05T00:00:00Z\"\nhostname = \"h\"\ntree_kind = \"clone\"\n",
                aida_core::toml_quote::toml_string(&clone.display().to_string())
            )).unwrap();
        }
        let before: Vec<_> = ["first", "second"]
            .map(|id| std::fs::read(crate::lease_path(&root, id)).unwrap())
            .into();
        let err = reclaim_exited_branch_clone(&root, "rework", &clone)
            .unwrap_err()
            .to_string();
        assert!(err.contains("several matching leases"), "{err}");
        assert_eq!(
            std::fs::read_to_string(clone.join("pending.txt")).unwrap(),
            "keep me"
        );
        for (id, bytes) in ["first", "second"].into_iter().zip(before) {
            assert_eq!(std::fs::read(crate::lease_path(&root, id)).unwrap(), bytes);
        }
    }
}

/// BUG-2145: `aida queue work --auto-complete --sandbox` contains the whole
/// drain. `--sandbox` exports `AIDA_CONTAINED=1` before the drain starts, so
/// the drain's own contained flag ([`session::drain_contained`]) and every
/// phase child it spawns resolve contained. Without `--sandbox` this is a
/// no-op: the env / `[agents] contained` knob still apply as before.
// trace:BUG-2145 | ai:claude
pub(crate) fn honour_auto_complete_sandbox(
    sandbox: bool,
    permission_mode: Option<&str>,
) -> Result<()> {
    // BUG-2166: a contained parent's env always wins (monotonic containment,
    // BUG-2151 ruling). `--permission-mode` must be refused under a contained parent.
    // trace:BUG-2166 | ai:antigravity
    let contained_env = session::aida_contained_env()?;
    // trace:BUG-2172 | ai:codex
    crate::session::refuse_contained_permission_override(permission_mode, contained_env)?;
    if !sandbox {
        return Ok(());
    }
    if permission_mode.is_some_and(|m| !m.is_empty()) {
        anyhow::bail!(
            "--sandbox and --permission-mode are mutually exclusive Claude launch postures"
        );
    }
    apply_queue_work_contained_env(true);
    Ok(())
}

// trace:BUG-2006 | ai:antigravity
// The launcher keeps its own posture: the export is for its children only.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn apply_queue_work_contained_env(contained: bool) {
    session::set_contained_env_for_children(contained);
}

/// BUG-2010: true when caller is working the lease itself (self-claim under `--no-launch`).
/// false when launching/spawning another agent.
// trace:BUG-2010 | ai:antigravity
pub(crate) fn queue_work_self_claim(no_launch: bool) -> bool {
    no_launch
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueueWorkPosture {
    pub(crate) contained: bool,
    pub(crate) permission_mode: Option<String>,
    pub(crate) permission_mode_origin: &'static str,
}

// BUG-2166: monotonic containment — a contained parent's env always wins
// (BUG-2151 ruling). `--plan-only` must not disable containment, and
// `--permission-mode` must be refused under a contained parent.
// trace:BUG-2166 | ai:antigravity
pub(crate) fn resolve_queue_work_posture(
    permission_mode: Option<&str>,
    sandbox: bool,
    contained_env: bool,
    contained_knob: bool,
    bypass_knob: bool,
    plan_only: bool,
    env_mode: Option<&str>,
    config_mode: Option<&str>,
) -> Result<QueueWorkPosture> {
    if sandbox && permission_mode.is_some() {
        anyhow::bail!(
            "--sandbox and --permission-mode are mutually exclusive Claude launch postures"
        );
    }
    // trace:BUG-2172 | ai:codex
    crate::session::refuse_contained_permission_override(permission_mode, contained_env)?;
    let contained = sandbox || contained_env || contained_knob;
    if contained && bypass_knob {
        anyhow::bail!("[agents] bypass and contained mode are mutually exclusive launch postures");
    }
    let (permission_mode, permission_mode_origin) = if contained {
        // Plan mode and containment are independent: retain the sandbox flags
        // while requesting the planning permission posture.
        // trace:TASK-1794 | ai:codex
        let mode = if plan_only { "plan" } else { "dontAsk" };
        let origin = if plan_only {
            "contained sandbox + --plan-only"
        } else {
            "contained sandbox"
        };
        (Some(mode.to_string()), origin)
    } else {
        resolve_queue_work_permission_mode(
            permission_mode,
            env_mode,
            config_mode,
            bypass_knob,
            plan_only,
        )
    };
    Ok(QueueWorkPosture {
        contained,
        permission_mode,
        permission_mode_origin,
    })
}

// trace:BUG-2166 | ai:antigravity
pub(crate) fn build_queue_work_claude_command(
    posture: &QueueWorkPosture,
    name: Option<&str>,
    prompt: &str,
    session_id: &str,
    model: Option<&str>,
    grant_id: Option<&str>,
) -> Result<std::process::Command> {
    session::build_claude_exec_command(
        crate::seat_clone::launcher_main().as_deref(),
        posture.permission_mode.as_deref(),
        name,
        Some(prompt),
        Some(session_id),
        posture.contained,
        model,
        grant_id,
    )
}

/// BUG-2223: the sentence a scope-conflict refusal adds about keeping the
/// conflicting lease's tree. It offers `--resume` only when `--resume` would
/// adopt that lease (`refusal` from [`crate::leases::lease_adoption_refusal`]
/// is `None` and no other orphan is being adopted), so this refusal and the
/// `--resume` path never point at each other. A seat clone (`clone`) is
/// re-entered only by a contained launch, so its hint names `--sandbox`.
// trace:BUG-2223 | ai:claude
pub(crate) fn scope_conflict_resume_hint(
    scope: &str,
    conflict_id: &str,
    refusal: Option<&str>,
    adopting: Option<&str>,
    clone: bool,
) -> String {
    // The FULL lease id: an 8-hex prefix is a timestamp and is routinely
    // shared by two sessions, so `aida session end <prefix>` was ambiguous.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-023 | ai:claude
    let id = conflict_id;
    match (refusal, adopting) {
        (Some(why), _) => format!(
            " `--resume` cannot re-enter it in place: {why} (`aida session show {id}` shows \
             its liveness). To keep the work, commit or stash it there; `aida session end \
             {id} --force` discards it."
        ),
        (None, Some(adopted)) => {
            format!(
                " `--resume` re-enters only the newest orphaned lease ({adopted}), not this \
                 one: commit or stash its work there, or `aida session end {id} --force` \
                 discards it, then re-run."
            )
        }
        (None, None) => {
            let (flags, why) = if clone {
                (
                    " --sandbox",
                    ", contained, since it is a seat's private clone",
                )
            } else {
                ("", "")
            };
            format!(
                " To keep its tree and work: `aida queue work {scope} --resume{flags}` \
                 re-enters it in place{why} (a new session starts there when none was \
                 recorded), or `aida session end {id} --force` discards it."
            )
        }
    }
}

// trace:STORY-2325 | ai:codex
pub(crate) fn refuse_retired_steal(steal: bool) -> Result<()> {
    anyhow::ensure!(!steal, "`--steal` is retired: ownership decides reclaim automatically; live or unknown owners retain their lane. Ask the owner to use `aida session end <id>` for voluntary handoff");
    Ok(())
}

/// STORY-42: the orchestrator. Resolves the plan, optionally pulls,
/// runs session_start in non-launch mode (so we can write the manifest
/// from the freshly minted lease), then either prints next-steps or
/// chdirs + execs claude with the skill prompt.
/// trace:STORY-42 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_queue_work(
    storage: &Storage,
    // Invocation-local host check from the dispatcher; direct callers pass None.
    // A changed rework vendor still needs its own check. trace:STORY-2642 | ai:codex
    mut preflight_vendor: Option<session::HeadlessVendor>,
    user_id: &str,
    arg: Option<&str>,
    permission_mode: Option<&str>,
    sandbox: bool,
    no_launch: bool,
    role_input: QueueWorkRoleInput<'_>,
    no_pull: bool,
    type_filter: Option<&str>,
    branch_override: Option<&str>,
    path_override: Option<&str>,
    // STORY-248: stacked-branch base resolution. `stack` auto-detects the
    // freshest un-merged in-flight implementer lease; `base` takes an
    // explicit branch name. CLI clap-level enforces they're mutually
    // exclusive. `force_base` opts out of the merged-base safety check.
    // trace:STORY-248 | ai:claude
    stack: bool,
    base: Option<&str>,
    force_base: bool,
    steal: bool,
    force: bool,
    force_claim: bool,
    resume: Option<&str>,
    fresh: bool,
    list_sessions: bool,
    session_id: Option<&str>,
    // TASK-895: which vendor CLI hosts the interactive session — `claude`
    // (default) or `codex`. The AIDA TUI passes `codex` to host a Codex tab.
    // Only affects the interactive (non-headless) launch path; the headless
    // drain resolves its own vendor via STORY-683. trace:TASK-895 | ai:claude
    vendor: &str,
    no_human: bool,
    // TASK-1060 / ADR-10: the autonomy mode, resolved ONCE at dispatch and
    // threaded in as a typed value so in-process logic (the pre-flight banner)
    // never re-reads the bare `AIDA_ZEN` env var. trace:TASK-1060 | ai:claude
    autonomy: AutonomyMode,
    // TASK-272: the batch this pickup belongs to, when resolved from
    // `aida queue work --batch NAME`. Recorded on the session manifest so
    // /aida-pickup can detect batch context. `None` for a plain pickup.
    batch_name: Option<&str>,
    // BUG-226: suppress the end-of-command summary for a standalone
    // reviewer run (`--quiet`). Ignored for non-reviewer / orchestrator
    // launches, which never print one.
    quiet: bool,
    // STORY-281: opt out of the reviewer pre-flight stale-base refusal.
    // Only meaningful when scope resolves to a PR + role is reviewer;
    // ignored on every other pickup. trace:STORY-281 | ai:claude
    allow_stale_base: bool,
    // TASK-480: opt out of the reviewer pre-flight intermediate-only
    // refusal. Same scoping as allow_stale_base — only meaningful for a
    // reviewer pickup of a GitHub PR. trace:TASK-480 | ai:claude
    allow_intermediate_only: bool,
    // STORY-439: pickup-time complexity + assistance estimate. Each
    // value writes a slot to `.aida/complexity-calibration/<SPEC>.yaml`
    // AND stamps a `complexity:<level>` / `estimated-assistance:<level>`
    // tag on the spec for tag-based queries. trace:STORY-439 | ai:claude
    complexity: Option<complexity_calibration::ComplexityLevel>,
    assist_est: Option<complexity_calibration::AssistanceLevel>,
    // STORY-451: post-plan/pickup effort estimate. Captured as the plan
    // touchpoint and stamped as `effort:plan:<bucket>`.
    effort: Option<effort_calibration::EffortBucket>,
    // trace:TASK-547 | ai:antigravity
    strict: bool,
    // STORY-265: plan-only mode — run /aida-plan (not /aida-pickup) in `plan`
    // permission mode so the session writes a docs/plans/ file without
    // touching code; promote Approved -> Planned afterward via
    // `aida plan promote`. trace:STORY-265 | ai:claude
    plan_only: bool,
    // STORY-735: guided keystone-implementation mode — launch
    // /aida-guided-implement (a structured decision dialog) instead of
    // /aida-pickup. Interactive-only; the spec's major architectural forks
    // are decided up front and recorded as ADRs before any code.
    // trace:STORY-735 | ai:claude
    guided: bool,
    // TASK-1053: single-spec preview. Resolve the full plan (scope, role,
    // skill, branch, worktree path, session id, lease) and print it, then
    // return WITHOUT creating a worktree, taking a lease, or launching a
    // session. The `--batch` form is handled at the dispatch site (it
    // returns there before reaching this function). trace:TASK-1053 | ai:claude
    dry_run: bool,
    // STORY-1003: opaque vendor model override for this pickup/drain.
    model_override: Option<&str>,
) -> Result<()> {
    // Record admission before any pickup gate; previews and interactive sessions
    // do not claim a headless lane attempt. trace:STORY-2690 | ai:codex
    let mut attempt = if no_human && resume.is_none() && !dry_run && !no_launch && !list_sessions {
        Some(crate::lane_result::Attempt::begin(
            &requeue_project_root(storage),
            arg.unwrap_or("queue-head"),
            branch_override,
        )?)
    } else {
        None
    };
    let result = (|| -> Result<()> {
        refuse_retired_steal(steal)?;
        let role_override = role_input.pickup;
        // STORY-132: validate a caller-minted --session-id up front — before
        // any side effect — so a malformed id fails clean with a clear
        // message rather than surfacing deep in session setup.
        if let Some(sid) = session_id {
            uuid::Uuid::parse_str(sid)
                .with_context(|| format!("--session-id `{}` is not a valid UUID", sid))?;
        }
        // trace:STORY-1133 | ai:codex
        if current_role_instance_is_companion() {
            anyhow::bail!(
                "companion sessions cannot drive queued work. Use the authoritative driver seat \
             for this role, or launch a doer role for implementation/review work."
            );
        }
        // TASK-630: a deliberate PR-hold (BUG-250) parks the spec Done + dequeued
        // with a persisted marker at `.aida/pr-holds/<spec>.json`. Normal queue
        // resolution then bails ("isn't queued. Status is Done") — wrong for a
        // re-entry. When the operator explicitly `--resume`s such a spec, recognise
        // the hold as a re-enterable state and synthesise an Item-mode plan so the
        // rest of the verb (lease/pull/worktree + QueueWorkLaunch::Resume) runs
        // unchanged. We try the normal resolution first; only on its failure do we
        // consult the marker, so a still-queued or non-held spec keeps its existing
        // behaviour exactly. trace:TASK-630 | ai:claude
        let plan = match resolve_queue_work_plan(
            storage,
            user_id,
            arg,
            type_filter,
            strict,
            /* read-only synthesis until launch preflight passes */ true,
            force || force_claim,
            role_override,
        ) {
            Ok(plan) => plan,
            Err(e) => match arg.filter(|_| resume.is_some()) {
                Some(arg_str) => {
                    let store = storage.load()?;
                    let held = store
                        .requirements
                        .iter()
                        .find(|r| spec_matches(r, arg_str))
                        .and_then(|req| {
                            let main_root = find_main_worktree_root().ok()?;
                            let display = req
                                .agreed_id
                                .as_deref()
                                .or(req.spec_id.as_deref())
                                .unwrap_or(arg_str);
                            let marker = punt::hold_signal_path(&main_root, display);
                            let present = punt::read_hold_signal(&marker).is_some();
                            held_resume_reentry_allowed(true, &req.status, present)
                                .then(|| held_resume_plan(req, user_id))
                        });
                    match held {
                        Some(plan) => plan,
                        None => return Err(e),
                    }
                }
                None => return Err(e),
            },
        };

        // Resolve once for both receipt applicability and the actual launch.
        // Provisional capture above retains failures before a plan is available.
        // trace:STORY-2690 | ai:codex
        let (role, role_origin, warnings) = infer_queue_work_role(&plan, role_override);
        let implementer_route =
            resolve_pickup_attempt(&mut attempt, &plan, &role, no_human, branch_override)?;

        // Recheck the resolved anchor, including no-argument queue-head pickup.
        // This runs before launch resolution, calibration, worktree creation, or
        // lease acquisition, so held work cannot be picked indirectly. trace:BUG-1793
        if let Some(req) = storage
            .load()?
            .requirements
            .into_iter()
            .find(|r| spec_matches(r, &plan.anchor_display))
        {
            let display = req.spec_id.as_deref().unwrap_or(&plan.anchor_display);
            if let Some(refusal) = crate::gate::operator_hold::operator_hold_refusal(
                display,
                req.deferred,
                req.deferred_reason.as_deref(),
                req.deferred_until.as_deref(),
            ) {
                return Err(refusal);
            }
        }

        // TASK-304: on a no-arg head pickup, surface the ultraplan suggestion
        // for a chunky head spec under `[ultraplan] mode = "suggested"`. Only
        // the no-arg form (the operator hasn't already chosen a spec) gets the
        // nudge; an explicit `aida queue work SPEC` is already a deliberate
        // choice. Best-effort and read-only — never perturbs the pickup.
        // trace:TASK-304 | ai:claude
        if arg.is_none() && !list_sessions {
            if let Ok(root) = find_project_root() {
                if let Ok(store) = storage.load() {
                    if let Some(req) = store.get_requirement_by_spec_id(&plan.anchor_display) {
                        print_ultraplan_suggestion_hint(&root, req);
                    }
                }
            }
        }

        // TASK-516: warn before picking up a spec whose imported plan is still
        // awaiting master review (tagged `plan-review:pending`). The plan is
        // archived but NOT yet canonical — the master hasn't signed off, so
        // riding it into the implementer session as the brief would treat an
        // unreviewed plan as load-bearing. Best-effort, read-only; never blocks
        // the pickup (this minimal slice warns, it doesn't gate).
        // trace:TASK-516 | ai:claude
        if !list_sessions {
            if let Ok(store) = storage.load() {
                if let Some(req) = store.get_requirement_by_spec_id(&plan.anchor_display) {
                    if let Some(msg) = plan_review_warning(&req.tags, &plan.anchor_display) {
                        eprintln!("{msg}");
                    }
                }
            }
        }

        // TASK-112: decide cold-launch-vs-resume up front, *before*
        // session_start mints a worktree — so a bad `--resume <id>` (or a
        // bare `--resume` with no prior session) fails clean with nothing to
        // unwind. `None` when `--no-launch` (setup-only, no conversation).
        // BUG-2223: `--resume` on a scope whose lease is orphaned (owner
        // verifiably gone, tree still on disk) re-enters that tree in place:
        // the conflict sweep below leaves the lease alone and session start
        // adopts it. With no recorded claude session the launch cold-starts
        // there instead of refusing.
        // trace:BUG-2223 | ai:claude
        let adopt_orphan: Option<SessionLease> = if resume.is_some() && !list_sessions {
            find_main_worktree_root()
                .ok()
                .and_then(|root| crate::leases::adoptable_orphan_lease(&root, &plan.scope))
        } else {
            None
        };
        let launch: Option<QueueWorkLaunch> = if no_launch {
            None
        } else {
            Some(resolve_queue_work_launch(
                &plan.anchor_display,
                resume,
                fresh,
                session_id,
                adopt_orphan.as_ref().map(|l| l.worktree_path.as_path()),
            )?)
        };

        let project_root_for_config = find_main_worktree_root().ok();
        // BUG-898/BUG-902/BUG-913: resolve + validate the launch/display vendor
        // before any write below (calibration tags, leases, worktrees), for both
        // launching and non-launching previews. A codex-only profile should
        // auto-pick codex; disabled defaults must not leak into dry-run or
        // no-launch output. trace:BUG-898 BUG-902 BUG-913 | ai:codex
        // BUG-2386: a `--resume`'s recorded vendor and resumable id, resolved
        // once here (before any lease is minted) and reused by the launch arms.
        // trace:BUG-2386 | ai:claude
        // Infer the recorded vendor type without adding vendor-ratchet debt.
        // trace:STORY-2642 | ai:codex
        let mut resume_target = None;
        let resolved_launch_vendor = if !list_sessions {
            let root = project_root_for_config
                .clone()
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            // BUG-2386: a resume runs on the vendor that recorded the session
            // (a Codex thread resumes on Codex), gated on that vendor being
            // resumable and enabled — not on the configured vendor being Claude.
            // trace:BUG-2386 | ai:claude
            match (&launch, no_launch) {
                (Some(QueueWorkLaunch::Resume(id)), false) => {
                    let (recorded, resume_id) =
                        crate::codex_resume::recorded_resume(&root, id, Some(plan.scope.as_str()))?;
                    session::require_resume_vendor(&root, recorded)?;
                    resume_target = Some((recorded, resume_id));
                    Some(recorded)
                }
                _ => Some(session::resolve_enabled_headless_vendor(&root)?),
            }
        } else {
            None
        };
        let launch_vendor = resolved_launch_vendor.unwrap_or_else(|| {
            session::HeadlessVendor::parse(vendor).unwrap_or(session::HeadlessVendor::Claude)
        });
        // BUG-2146: a malformed agents.toml is a refusal naming the file, not a
        // silent "uncontained". trace:BUG-2146 | ai:claude
        let contained_knob = project_root_for_config
            .as_deref()
            .map(load_agents_contained)
            .transpose()?
            .unwrap_or(false);
        // trace:STORY-2642 | ai:codex — a resume uses its recorded vendor.
        if crate::lane_host::needs_preflight(no_human, dry_run, no_launch, list_sessions)
            && preflight_vendor != Some(launch_vendor)
        {
            let root = project_root_for_config
                .as_deref()
                .context("cannot resolve lane host root")?;
            crate::lane_host::preflight(root, launch_vendor.as_str(), sandbox)?;
            preflight_vendor = Some(launch_vendor);
        }
        // trace:BUG-2146 | ai:claude
        let contained_env = session::aida_contained_env()?;
        // BUG-2166: a contained parent's env always wins (monotonic containment,
        // BUG-2151 ruling). `--plan-only` must not disable containment, and
        // `--permission-mode` must be refused under a contained parent.
        // trace:BUG-2166 | ai:antigravity
        if sandbox && permission_mode.is_some() {
            anyhow::bail!(
                "--sandbox and --permission-mode are mutually exclusive Claude launch postures"
            );
        }
        // trace:BUG-2172 | ai:codex
        crate::session::refuse_contained_permission_override(permission_mode, contained_env)?;
        let contained = sandbox || contained_env || contained_knob;
        // BUG-2144: containment is monotonic (BUG-2133 ruling). A contained launch
        // on a vendor that cannot apply it is refused HERE — before the queue_add
        // row, the calibration/effort tags, any lease or worktree — rather than
        // silently running uncontained. `contained` below uses the same inputs.
        // trace:BUG-2144 | ai:claude
        // trace:BUG-2166 | ai:antigravity
        if !no_launch && !list_sessions {
            session::require_vendor_containment(launch_vendor, contained, "queue-work session")?;
        }
        let model_root = project_root_for_config
            .clone()
            .or_else(|| std::path::Path::new(".").canonicalize().ok());
        let resolved_model = model_override
            .filter(|m| !m.trim().is_empty())
            .map(str::to_string)
            .or_else(|| {
                model_root
                    .as_deref()
                    .and_then(|r| session::resolve_vendor_model(r, launch_vendor))
            });
        if let Some(model) = &resolved_model {
            std::env::set_var("AIDA_AGENT_MODEL", model);
        }

        // trace:TASK-1337 | ai:codex
        // Resolve role, branch, and delegation before calibration, cleanup, pull,
        // lease creation, or status writes. Shell roles route; grants authorize.
        let project_root = find_main_worktree_root()?;
        // trace:BUG-2500 | ai:codex — admission precedes calibration and setup.
        refuse_waiting_pickup(
            &project_root,
            &plan,
            &role,
            list_sessions,
            plan_only,
            guided,
        )
        .map_err(|why| anyhow::anyhow!(why))?;
        // trace:BUG-2451 | ai:codex — session listing allocates nothing.
        let rework_pr = if list_sessions {
            None
        } else {
            rework_pr_head_branch_override(
                &*crate::forge::forge_for_open_change(&project_root),
                &plan,
                branch_override,
            )?
        };
        if let Some(attempt) = &attempt {
            attempt.resolve_scope(
                &plan.anchor_display,
                branch_override.or_else(|| rework_pr.as_ref().map(|(_, branch)| branch.as_str())),
                rework_pr.as_ref().map(|(pr, _)| *pr),
            )?;
        }
        // BUG-022: say which existing branch the rework must continue.
        // trace:BUG-b6b1278890774ef993d2ebc327862470-022 | ai:claude
        if let Some((pr, branch)) = rework_pr.as_ref().filter(|_| !list_sessions) {
            eprintln!(
                "  rework: continuing open PR #{pr} on its branch `{branch}`; \
             this pickup must keep the existing PR branch"
            );
        }
        let resolved_pickup_pr =
            (plan.entries.len() == 1 && plan.review_target.is_none()).then(|| rework_pr.clone());
        let rework_branch_override = rework_pr.as_ref().map(|(_, branch)| branch.clone());
        let session_branch_override = branch_override.or(rework_branch_override.as_deref());

        // BUG-2223: an adopted orphan fixes the tree and branch; an explicit
        // --path or --branch naming another one refuses rather than guess.
        // trace:BUG-2223 | ai:claude
        let adopt_path = adopt_orphan
            .as_ref()
            .map(|l| l.worktree_path.to_string_lossy().into_owned());
        let (path_override, session_branch_override) = match &adopt_orphan {
            Some(orphan) => {
                let id = &orphan.id[..orphan.id.len().min(8)];
                if let Some(p) = path_override.filter(|p| {
                    std::path::Path::new(p).canonicalize().ok()
                        != orphan.worktree_path.canonicalize().ok()
                }) {
                    anyhow::bail!(
                        "--resume re-enters orphaned lease {id}'s tree {}, but --path names {p}; \
                     drop --path to resume there",
                        orphan.worktree_path.display()
                    );
                }
                if let Some(b) = branch_override.filter(|b| *b != orphan.branch) {
                    anyhow::bail!(
                    "--resume re-enters orphaned lease {id} on branch `{}`, but --branch names \
                     `{b}`; drop --branch to resume there",
                    orphan.branch
                );
                }
                // trace:BUG-2223 | ai:claude — no --branch was passed here: the
                // open PR's head picked the branch, so name what works instead.
                if let Some(b) = rework_branch_override
                    .as_deref()
                    .filter(|b| *b != orphan.branch)
                {
                    anyhow::bail!(
                        "--resume re-enters orphaned lease {id} on branch `{}`, but the open PR \
                     for `{}` is on branch `{b}`, so its rework belongs there. Commit or stash \
                     the work in {}, then `aida session end {id} --force` and re-run \
                     `aida queue work {}` without --resume.",
                        orphan.branch,
                        plan.scope,
                        orphan.worktree_path.display(),
                        plan.scope,
                    );
                }
                (adopt_path.as_deref(), Some(orphan.branch.as_str()))
            }
            None => (path_override, session_branch_override),
        };
        // STORY-1662: a fresh headless implementer pickup outside a drain waits
        // for its child and reports the outcome. Its concurrency slot is taken
        // before any setup and held until this command returns.
        // trace:STORY-1662 | ai:claude
        let standalone_implementer =
            !no_launch && matches!(launch, Some(QueueWorkLaunch::Fresh(_))) && implementer_route;
        // TASK-619: capture the anchor spec's status from the *local* (pre-pull)
        // view so we can detect whether the pull below reveals that another
        // machine claimed the spec under us. Best-effort + read-only — a load
        // failure just yields None, which makes the guard a no-op.
        // trace:TASK-619 | ai:claude
        let status_before_pull: Option<RequirementStatus> = storage.load().ok().and_then(|store| {
            store
                .get_requirement_by_spec_id(&plan.scope)
                .map(|r| r.status.clone())
        });

        let mut _standalone_slot = None;
        if !list_sessions {
            eprintln!("  pickup role: {role} ({role_origin}); --role overrides queue routing, then scope default");
            if let Some(message) = queue_work_role_mismatch(
                &role,
                role_origin,
                std::env::var("AIDA_SESSION_ROLE").ok().as_deref(),
            ) {
                eprintln!("  {message}");
            }
            if !dry_run {
                let review_branch = plan
                    .review_target
                    .map(|(forge, n)| forge.local_branch_for(n));
                // trace:BUG-1808 | ai:claude
                // Preflight every launch path that later mints a child grant —
                // including the `--no-human` resume and standalone-reviewer
                // launches, which used to skip this check and refuse in
                // `issue_child` only after the status bump, queue row, worktree,
                // lease and manifest had landed.
                if queue_work_issues_child_grant(no_launch) {
                    let child_seat = crate::canonical_role_name(&role);
                    // BUG-2139: a fresh headless pickup now mints a child grant
                    // too. Its baseline seats may fall back to an ungranted child;
                    // every other arm and seat must be delegated.
                    let preflight = if queue_work_is_fresh_headless(
                        no_human,
                        matches!(launch, Some(QueueWorkLaunch::Resume(_))),
                        is_standalone_reviewer(&role, plan.review_target.is_some()),
                    ) {
                        crate::gate::seat_authority::preflight_headless_child(
                            &project_root,
                            &child_seat,
                        )
                        .map(|authority| {
                            matches!(
                                authority,
                                crate::gate::seat_authority::HeadlessChildAuthority::Delegated
                            )
                        })
                    } else {
                        crate::gate::seat_authority::validate_child_delegation(
                            &project_root,
                            &child_seat,
                        )
                        .map(|_| true)
                    };
                    let mints_child = match preflight {
                        Ok(mints_child) => mints_child,
                        Err(cause) => {
                            let launcher =
                                crate::gate::seat_authority::current_grant(&project_root);
                            anyhow::bail!(queue_work_delegation_refusal(
                                &child_seat,
                                role_origin,
                                launcher
                                    .as_ref()
                                    .map(|g| (g.seat.as_str(), g.delegable_seats.as_slice())),
                                &format!("{cause:#}"),
                            ));
                        }
                    };
                    // BUG-2428: a sandboxed child whose grant key this launcher
                    // cannot put in a session keyring is refused at mint time —
                    // after the worktree and lease exist. Probe the keyring now,
                    // before any setup, so that refusal leaves nothing behind.
                    // trace:BUG-2428 | ai:claude
                    if is_standalone_reviewer(&role, plan.review_target.is_some()) {
                        refuse_unpresentable_contained_reviewer(contained)?;
                    } else if mints_child
                    // trace:TASK-2708 | ai:claude
                    && session::sandboxed_child_grant(launch_vendor, contained)?
                    {
                        crate::gate::seat_authority::preflight_sandboxed_child_key(&child_seat)?;
                    }
                }
                // Pull after read-only launch preflights and before session setup
                // or store writes. A stale view cannot authorize pickup after failure.
                // trace:BUG-2490 | ai:codex
                if !no_pull {
                    run_aida_db_sync_pull(storage.path()).context(
                    "pre-pickup pull failed; pickup stopped before session setup and store writes; \
                     resolve the store sync failure and retry `aida db sync --pull`, or use `--no-pull` for an intentional offline pickup",
                )?;
                }

                // trace:BUG-2530 | ai:codex — after sync, before any pickup mutation.
                if !no_launch && role == "reviewer" {
                    if let Some(pr_n) = reviewer_preflight_pr_n(plan.review_target) {
                        // trace:BUG-2530 | ai:codex
                        let head = standalone_review_pr_head(&project_root, pr_n);
                        crate::gate::review_marker::refuse_conclusive_head(
                            &crate::main_worktree_root_from(&project_root),
                            pr_n,
                            head.as_deref(),
                        )?;
                    }
                }

                // trace:STORY-2705 | ai:codex — before worktree/queue mutation.
                if role == "reviewer" {
                    if let Some((forge, n)) = plan.review_target {
                        if let Some(reason) = crate::review_implementer_gate::deferred(
                            &project_root,
                            n,
                            forge == ReviewForge::GitLab,
                        )? {
                            anyhow::bail!("{reason}");
                        }
                    }
                }

                // trace:STORY-1662 | ai:claude
                // trace:STORY-2660 | ai:codex
                if standalone_implementer {
                    _standalone_slot = Some(crate::standalone_implementer::acquire_slot(
                        &project_root,
                        &plan.scope,
                        crate::standalone_implementer::configured_cap(&project_root)?,
                    )?);
                }
                // Force-claim may retire a dead clone, so delegation and launch
                // capacity must succeed before this preflight mutates anything.
                // trace:BUG-2455 | ai:codex
                // trace:BUG-2457 | ai:codex
                if role == "reviewer"
                    && adopt_orphan.is_none()
                    && session_branch_override.is_none()
                    && path_override.is_none()
                {
                    if let Some((forge, n)) = plan.review_target {
                        retire_idle_review_worktree(&project_root, forge, n)?;
                    }
                }
                preflight_queue_work_branch_reclaiming(
                    &project_root,
                    session_branch_override.or(review_branch.as_deref()),
                    adopt_orphan.as_ref().map(|l| l.worktree_path.as_path()),
                    force_claim,
                )?;
                // Persist only the synthesized explicit row, after all three
                // preflight decisions. Do not reselect a different queue head.
                for entry in &plan.entries {
                    if entry.queue.position == i64::MAX
                        && !storage
                            .queue_list(&entry.queue.user_id, false)?
                            .iter()
                            .any(|queued| queued.requirement_id == entry.queue.requirement_id)
                    {
                        storage.queue_add(entry.queue.clone())?;
                        record_role_activity(&entry.spec_id, "queue-add");
                        println!(
                            "queued {} for role:{}",
                            entry.spec_id,
                            entry.queue.for_role.as_deref().unwrap_or(&role)
                        );
                    }
                }
            }
        }

        // STORY-439: capture pickup-time complexity + assistance estimate
        // ASAP after plan resolution — we know the anchor spec, the project
        // root is reachable via `find_project_root`, and the capture is a
        // local FS write that won't perturb the rest of the pickup flow.
        // Best-effort: a write error logs and continues. Cluster mode uses
        // the anchor (the parent scope) as the captured spec; the
        // operator's estimate is for the cluster as a whole.
        // trace:STORY-439 | ai:claude
        if (complexity.is_some() || assist_est.is_some()) && !list_sessions && !dry_run {
            if let Ok(project_root) = find_project_root() {
                let main_root = main_worktree_root_from(&project_root);
                let spec = plan.anchor_display.as_str();
                if let Err(e) =
                    complexity_calibration::upsert_pickup(&main_root, spec, complexity, assist_est)
                {
                    eprintln!(
                        "  {} could not record pickup calibration for {spec}: {e}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                    );
                }
                // Stamp the tags on the spec so existing tag tooling works
                // on the new dimension. Best-effort — a load/save failure
                // doesn't fail the pickup.
                apply_calibration_tags(storage, spec, complexity, assist_est);
            }
        }
        if effort.is_some() && !list_sessions && !dry_run {
            if let Ok(project_root) = find_project_root() {
                let main_root = main_worktree_root_from(&project_root);
                let spec = plan.anchor_display.as_str();
                if let Err(e) = effort_calibration::upsert_plan(
                    &main_root,
                    spec,
                    effort,
                    Some(current_user_id(None)),
                ) {
                    eprintln!(
                        "  {} could not record plan effort for {spec}: {e}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                    );
                }
                apply_effort_tag(
                    storage,
                    spec,
                    effort_calibration::EffortTouchpoint::Plan,
                    effort,
                );
            }
        }

        // TASK-112: `--list-sessions` is a pure read — print the recorded
        // claude conversations for this scope and exit before any side
        // effects (no worktree, no lease, no pull).
        if list_sessions {
            return print_scope_sessions(&plan.anchor_display);
        }

        // Permission mode resolution (TASK-83 → TASK-84). Order:
        //   1. --permission-mode flag (explicit override always wins)
        //   2. AIDA_PERMISSION_MODE env var (process-level opt-out)
        //   3. .aida/config.toml [behavior] permission_mode (project policy)
        //   4. AIDA-managed worktree default → `bypassPermissions` (TASK-84:
        //      worktree is sandboxed git state; the prompt flood was eating
        //      autonomous overnight runs and circuit breakers stay in place)
        //   5. Non-AIDA default → `acceptEdits` (safe default for foreign cwd)
        // The pass-through to `claude --permission-mode` is unvalidated, so
        // users can pick `auto`, `plan`, etc. trace:STORY-42, TASK-83, TASK-84
        // | ai:claude
        let env_mode = std::env::var("AIDA_PERMISSION_MODE")
            .ok()
            .filter(|s| !s.is_empty());
        let config_mode = project_root_for_config
            .as_deref()
            .and_then(read_behavior_permission_mode);
        // STORY-495: the interactive worktree default is now faithful (native).
        // The uniform `[agents] bypass` knob is the single opt-in that restores
        // bypass posture — read it from user-base + project agents.toml.
        let bypass_knob = project_root_for_config
            .as_deref()
            .map(|r| load_agents_bypass(r).unwrap_or(false))
            .unwrap_or(false);
        let posture = resolve_queue_work_posture(
            permission_mode,
            sandbox,
            contained_env,
            contained_knob,
            bypass_knob,
            // The headless launcher never runs Claude plan mode (it owns its own
            // posture), so only an interactive plan lane resolves to `plan`; the
            // headless plan phase must not report a mode it will not run.
            // trace:TASK-1794 | ai:claude
            plan_only && !no_human,
            env_mode.as_deref(),
            config_mode.as_deref(),
        )?;
        let contained = posture.contained;
        // trace:BUG-2131 | ai:claude
        if contained {
            aida_core::git_ops::ensure_git_supports_contained()?;
        }
        apply_queue_work_contained_env(contained);
        let explicit_permission_mode = permission_mode.is_some();
        let mut bypass_prompt_declined = false;
        let (mut permission_mode, permission_mode_origin) =
            (posture.permission_mode, posture.permission_mode_origin);
        // TASK-1500: interactive queue work owns a human consent prompt. Corroborated
        // orchestrated children already passed dispatch authority and must not hang.
        // BUG-2166: `--plan-only` must not skip the bypass consent prompt if an
        // uncontained interactive session resolves to bypassPermissions.
        // trace:BUG-2166 | ai:antigravity
        if !no_human && permission_mode.as_deref() == Some("bypassPermissions") {
            let orchestrated = project_root_for_config
                .as_deref()
                .is_some_and(|root| crate::orchestrator::detect(root).is_orchestrated());
            if !orchestrated {
                if let Some(root) = project_root_for_config.as_deref() {
                    let mut argv = vec![
                        "--permission-mode".to_string(),
                        "bypassPermissions".to_string(),
                    ];
                    crate::gate_agent_bypass(
                        root,
                        vendor,
                        &mut argv,
                        false,
                        explicit_permission_mode,
                    )?;
                    if !argv.iter().any(|a| a == "bypassPermissions") {
                        permission_mode = None;
                        bypass_prompt_declined = true;
                    }
                }
            }
        }
        if permission_mode.is_none() && !contained {
            maybe_show_faithful_launcher_notice();
        }

        // BUG-2430: the findings come from the newest review of the spec, its
        // open PR's change-keyed record included; a refusal that exists but
        // cannot be read stops the launch instead of sending the lane out on
        // older rulings. trace:BUG-2430 | ai:claude
        let review_findings = match project_root_for_config
            .as_deref()
            .filter(|_| !role.eq_ignore_ascii_case("reviewer") && !plan_only && !guided)
        {
            None => ReviewResult::Unavailable,
            Some(root) => {
                let spec_id = plan
                    .entries
                    .first()
                    .map(|entry| entry.spec_id.as_str())
                    .unwrap_or(plan.anchor_display.as_str());
                let open_change =
                    spec_open_change_key(root, &[plan.anchor_display.as_str(), spec_id]);
                resolve_rework_review(root, &plan.anchor_display, spec_id, open_change.as_deref())
                .map_err(|error| {
                    let (ReworkReviewError::Waiting(why) | ReworkReviewError::Unavailable(why)) = error;
                    anyhow::anyhow!(
                        "refusing to launch {}: {why}. Read the review with `aida review verdict {}` \
                         or post its findings as an advisor ruling, then relaunch.",
                        plan.anchor_display,
                        open_change.as_deref().unwrap_or(&plan.anchor_display)
                    )
                })?
            }
        };
        // BUG-1213: the pickup leads with the ACTUAL rework round, derived from
        // the spec's recorded findings blocks (one per `queue rework`), not a
        // constant. trace:BUG-1213 | ai:claude
        // An advisor ruling or brief left as a spec comment leads the implementer
        // pickup; a lane that read only the description repeated the overruled
        // change. trace:BUG-B6B1278890774EF993D2EBC327862470-021 | ai:claude
        // BUG-057: a brief newer than the last verdict is the task; the verdict's
        // findings become superseded context instead of the authoritative round.
        // The assembly is a pure function so the whole path is testable.
        // trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude
        let implementer_item = !role.eq_ignore_ascii_case("reviewer")
            && !plan_only
            && !guided
            && plan.mode == QueueWorkMode::Item;
        let spec_comments = if review_findings.findings().is_some() || implementer_item {
            let spec_id = plan
                .entries
                .first()
                .map(|entry| entry.spec_id.as_str())
                .unwrap_or(plan.anchor_display.as_str());
            storage
                .load()
                .ok()
                .and_then(|store| {
                    store
                        .requirements
                        .iter()
                        .find(|r| spec_matches(r, spec_id))
                        .map(|r| r.comments.clone())
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        // A rework lane is contained and cannot download CI logs, so the launcher
        // leads its prompt with the PR head's failing tests and panic lines.
        // trace:BUG-2380 | ai:claude
        let ci_failures = review_findings
            .findings()
            .and(project_root_for_config.as_deref())
            .and_then(|root| {
                let spec_id = plan
                    .entries
                    .first()
                    .map(|entry| entry.spec_id.as_str())
                    .unwrap_or(plan.anchor_display.as_str());
                crate::ci_failures::section_for_spec(root, spec_id)
            });
        let mut prompt = crate::rework_direction::assemble_pickup_prompt(
            &crate::rework_direction::PickupInputs {
                plan: &plan,
                role: &role,
                plan_only,
                guided,
                review_findings: review_findings.as_deref(),
                spec_comments: &spec_comments,
                conflict: None, // Host prepares the merge before launch (STORY-2475).
                ci_failures: ci_failures.as_deref(),
            },
        );
        // Interactive implementers and every headless phase receive the same
        // resolved, capped type+lane protocol. Reviewers are included headlessly
        // because there is no conversational pickup hook to inject it later.
        // trace:STORY-1221 trace:TASK-1278 | ai:codex
        if no_human || !role.eq_ignore_ascii_case("reviewer") {
            let spec_id = plan
                .entries
                .first()
                .map(|entry| entry.spec_id.as_str())
                .unwrap_or(plan.anchor_display.as_str());
            if let Ok(store) = storage.load() {
                if let Some(req) = store.requirements.iter().find(|r| spec_matches(r, spec_id)) {
                    prompt =
                        crate::protocol_cmd::prepend_resolved_pickup_protocol(&store, req, prompt);
                }
            }
        }
        // STORY-1226: due seat jobs for this pickup's role lead the prompt (the
        // Codex/Antigravity delivery channel; a nudge on Claude). File-only read.
        // trace:STORY-1226 | ai:claude
        {
            let due_root = project_root_for_config
                .clone()
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let due = maintenance_schedule::due_seat_jobs(&due_root, Some(&role));
            prompt = prepend_due_jobs(
                prompt,
                &maintenance_schedule::render_due_jobs_block(&due, &role),
            );
        }
        // BUG-809: orchestrated reviewer child — the parent set the verdict-file
        // env; bake the absolute anchor into the prompt text as well.
        if role.eq_ignore_ascii_case("reviewer") {
            append_reviewer_prompt_suffixes(&mut prompt);
            // TASK-1277: every review round carries the type protocol's
            // machine-checkable checklist verbatim, resolved from the plan's own
            // spec entries (no forge call), so rounds cite protocol items by name.
            // trace:TASK-1277 | ai:claude
            if let Ok(store) = storage.load() {
                let typed: Vec<(String, aida_core::RequirementType)> = plan
                    .entries
                    .iter()
                    .filter_map(|entry| {
                        let req = store
                            .requirements
                            .iter()
                            .find(|r| spec_matches(r, &entry.spec_id))?;
                        Some((req.display_id().to_string(), req.req_type.clone()))
                    })
                    .collect();
                if let Some(block) = crate::protocol_gate::reviewer_prompt_block(&typed) {
                    prompt.push_str(&block);
                }
            }
            // trace:TASK-1290 | ai:claude
            if let Some((forge, pr_n)) = plan.review_target {
                if let Some(root) = project_root_for_config.as_deref() {
                    if let Ok(store) = storage.load() {
                        append_untraced_criteria_prompt_block(
                            &mut prompt,
                            root,
                            &store,
                            forge,
                            pr_n,
                        );
                    }
                }
            }
            // The reviewer gets the same failing-test digest the rework lane does.
            // trace:BUG-2380 | ai:claude
            if let Some((ReviewForge::GitHub, pr_n)) = plan.review_target {
                if let Some(root) = project_root_for_config.as_deref() {
                    if let Some(section) = crate::ci_failures::section_for_pr(root, pr_n) {
                        prompt.push_str("\n\n");
                        prompt.push_str(section.trim_end());
                    }
                }
            }
        }
        if no_human {
            prompt.push_str(&headless_waiting_rule_suffix(&role));
        }

        // STORY-281: reviewer pre-flight stale-base check. Fires whenever the
        // resolved scope is ANY forge's PR/MR AND the inferred role is the
        // reviewer — every other pickup (implementer, dialog, architect) skips
        // this branch. The check is also no-op'd when the pickup is
        // `--no-launch` (no reviewer session about to run) and when the user
        // passed `--list-sessions` (already exited above).
        //
        // BUG-1609: this used to match only `(ReviewForge::GitHub, pr_n)`, so a
        // GitLab MR silently skipped the check entirely — not because the check
        // itself was GitHub-only (`preflight_stale_base_check` reads metadata
        // through the forge-routed `fetch_change_info_via_forge`, GitLab-safe
        // since STORY-621 slice 2), but because this call site never reached it
        // for GitLab. trace:BUG-1609 | ai:claude
        //
        // Behaviour mirrors the orchestrator's phase-3 pre-flight:
        //   Current        → silent proceed
        //   StaleNoOverlap → warning, proceed
        //   StaleOverlap   → refuse (anyhow::bail!) unless allow_stale_base
        //   Err (gh/glab / fetch) → warning, proceed (never block on transient infra)
        //
        // trace:STORY-281 | ai:claude
        if !no_launch && role == "reviewer" {
            if let Some(pr_n) = reviewer_preflight_pr_n(plan.review_target) {
                if let Some(root) = project_root_for_config.as_deref() {
                    match preflight_stale_base_check(root, pr_n) {
                        Ok(pr_rebase::StaleBaseOutcome::Current) => {}
                        Ok(pr_rebase::StaleBaseOutcome::StaleNoOverlap { behind }) => {
                            eprintln!(
                                "  {} {}",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                                pr_rebase::stale_base_warn_message(pr_n, behind).yellow()
                            );
                        }
                        Ok(pr_rebase::StaleBaseOutcome::StaleOverlap {
                            behind,
                            overlap_files,
                            ..
                        }) => {
                            let msg =
                                pr_rebase::stale_base_block_message(pr_n, behind, &overlap_files);
                            if allow_stale_base {
                                eprintln!(
                                    "  {} stale-base + overlap detected; \
                                 `--allow-stale-base` is set, proceeding.\n{}",
                                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                                    msg.yellow()
                                );
                            } else {
                                anyhow::bail!(msg);
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "  {} pre-flight stale-base check for PR-{pr_n} failed \
                             ({e}); proceeding with reviewer",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                            );
                        }
                    }
                }
            }
        }

        // TASK-480: reviewer pre-flight intermediate-only check. Sibling
        // substrate-as-bouncer gate to the STORY-281 stale-base refusal
        // above — same scoping (reviewer + any forge's PR/MR + launching).
        // Refuses a PR/MR whose diff is exclusively intermediate/generated paths
        // (build outputs, gitignored files, lockfiles with no source change)
        // because such a fix is not reproducible. The check is a
        // PROGRAMMATIC GATE here, not skill-template instruction text
        // (BUG-280-class lesson). Fails open on infra error.
        //
        // BUG-1609: was gated on `ReviewForge::GitHub` like its stale-base
        // sibling above, so GitLab MRs never ran it even though
        // `preflight_intermediate_only_check` is itself forge-routed.
        // trace:BUG-1609 | ai:claude
        //
        //   Clean                   → silent proceed
        //   SourcePlusIntermediate  → warning, proceed (flag-but-allow)
        //   IntermediateOnly        → refuse unless --allow-intermediate-only
        //
        // trace:TASK-480 | ai:claude
        if !no_launch && role == "reviewer" {
            if let Some(pr_n) = reviewer_preflight_pr_n(plan.review_target) {
                if let Some(root) = project_root_for_config.as_deref() {
                    let allow = allow_intermediate_only
                        || std::env::var("AIDA_ALLOW_INTERMEDIATE_ONLY")
                            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                            .unwrap_or(false);
                    match preflight_intermediate_only_check(root, pr_n) {
                        Ok(pr_rebase::IntermediateOnlyOutcome::Clean) => {}
                        Ok(pr_rebase::IntermediateOnlyOutcome::SourcePlusIntermediate {
                            intermediate,
                        }) => {
                            eprintln!(
                                "  {} {}",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                                pr_rebase::intermediate_only_warn_message(pr_n, &intermediate)
                                    .yellow()
                            );
                        }
                        Ok(pr_rebase::IntermediateOnlyOutcome::IntermediateOnly {
                            intermediate,
                        }) => {
                            let msg =
                                pr_rebase::intermediate_only_block_message(pr_n, &intermediate);
                            if allow {
                                eprintln!(
                                    "  {} intermediate-only diff detected; \
                                 `--allow-intermediate-only` is set, proceeding.\n{}",
                                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                                    msg.yellow()
                                );
                            } else {
                                anyhow::bail!(msg);
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "  {} pre-flight intermediate-only check for PR-{pr_n} failed \
                             ({e}); proceeding with reviewer",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                            );
                        }
                    }
                }
            }
        }

        // Pre-flight summary so the user sees what we resolved before any
        // side effects fire (worktree create, lease, exec). Goes to stderr.
        eprintln!();
        eprintln!(
            "{} queue work {} mode",
            crate::glyph(crate::glyphs::Glyph::FlowActive)
                .green()
                .bold(),
            format!("({:?})", plan.mode).to_lowercase().cyan()
        );
        let line = |label: &str, value: String| {
            // Pad label to 8 chars so anchor/scope/role/mode/skill/cluster
            // line up vertically. The bold() call doesn't affect width as
            // seen by humans (terminal renders the same column).
            eprintln!("  {:<8} {}", format!("{}:", label).bold(), value);
        };
        line(
            "anchor",
            format!(
                "{}  {}",
                plan.anchor_display.cyan(),
                plan.anchor_title.dimmed()
            ),
        );
        line("scope", plan.scope.cyan().to_string());
        line(
            "role",
            format!("{} {}", role.cyan(), format!("({})", role_origin).dimmed()),
        );
        line(
            "mode",
            format!(
                "{} {}",
                permission_mode.as_deref().unwrap_or("native").cyan(),
                format!(
                    "({}{})",
                    permission_mode_origin,
                    if bypass_prompt_declined {
                        "; prompt declined; native"
                    } else {
                        ""
                    }
                )
                .dimmed()
            ),
        );
        line("skill", prompt.cyan().to_string());
        // STORY-287: surface the `--zen` autonomy mode in the pre-flight so the
        // user sees the flag took. ADR-10 / TASK-1060: consult the typed `autonomy`
        // value (resolved ONCE at dispatch and threaded in) instead of re-reading
        // the bare `AIDA_ZEN` env var — the env var stays as the cross-process
        // transport to phase children, not the in-process source of truth. This
        // also subsumes the old TASK-327 value-semantics guard (`AIDA_ZEN=0`/`false`
        // must not enable zen): `resolve_autonomy_mode` already handles it.
        // The full three-mode pre-launch banner is TASK-306's job.
        // trace:STORY-287 trace:ADR-10 trace:TASK-1060 | ai:claude
        if autonomy.is_zen() {
            // BUG-232: `--zen`'s end-of-session behavior differs by whether an
            // orchestrator is present to hand off to. With one, `--auto-complete`
            // drives PR → CI → review → merge; without one, `--zen` auto-opens
            // the PR at end-of-session then pauses on the grab-next/stop fork.
            // Corroborate via `orchestrator::detect` — BUG-233: never key off
            // the bare `AIDA_AUTO_COMPLETE` var, a stray value would lie here.
            // trace:BUG-232 | ai:claude
            let orch_root = project_root_for_config
                .clone()
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let (mode, note) = if orchestrator::detect(&orch_root).is_orchestrated() {
                (
                    "zen + auto-complete",
                    "(orchestrator drives end-of-session: PR → CI → review → merge)",
                )
            } else {
                (
                    "zen",
                    "(no orchestrator — confirmations auto-resolve; at end-of-session the PR \
                 auto-opens, then grab-next/stop pauses)",
                )
            };
            line("autonomy", format!("{} {}", mode.cyan(), note.dimmed()));
        }
        // TASK-112: surface a resume in the pre-flight summary.
        if let Some(QueueWorkLaunch::Resume(id)) = &launch {
            line(
                "resume",
                format!(
                    "{} {}",
                    id[..id.len().min(8)].cyan(),
                    "(continuing prior conversation)".dimmed()
                ),
            );
        }
        if plan.entries.len() > 1 {
            line(
                "cluster",
                format!(
                    "{} item{}",
                    plan.entries.len().to_string().cyan(),
                    if plan.entries.len() == 1 { "" } else { "s" }
                ),
            );
        }
        if let Some(warns) = &warnings {
            for w in warns {
                eprintln!(
                    "  {} {}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                    w.yellow()
                );
            }
        }

        // BUG-233: if this session carries `AIDA_AUTO_COMPLETE` without a live
        // corroboration token, surface it once — informationally. It is NOT a
        // leak to hunt (BUG-233's corrected diagnosis): the session simply runs
        // interactive. A corroborated orchestrator child stays silent here.
        // trace:BUG-233 | ai:claude
        {
            let root_for_orch = project_root_for_config
                .clone()
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            if let Some(note) = orchestrator::detect(&root_for_orch).informational_note() {
                eprintln!(
                    "  {} {}",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    note.dimmed()
                );
            }
        }

        // TASK-1053: single-spec dry-run preview. The plan is now fully resolved —
        // the pre-flight summary above already printed anchor/scope/role/mode/skill;
        // here we add the branch, worktree path, session id, and the lease the
        // pickup WOULD create, then return. The earlier pre-pickup pull and
        // admission writes are guarded by !dry_run; we return before the
        // scope-conflict sweep and session_start, so a dry run creates no
        // worktree, takes no lease, writes no
        // manifest, and launches no claude. The branch + worktree derivation
        // mirror `session_start` exactly so the preview matches a real pickup.
        // The `--batch` dry-run is a separate path that returns at the dispatch
        // site before this function is even called. trace:TASK-1053 | ai:claude
        // trace:BUG-2490 | ai:codex
        if dry_run {
            let dry_line = |label: &str, value: String| {
                eprintln!("  {:<8} {}", format!("{}:", label).bold(), value);
            };
            let slug = slugify(&plan.scope);
            let branch = if let Some(b) = session_branch_override {
                b.to_string()
            } else if let Some((forge, n)) = plan.review_target {
                forge.local_branch_for(n)
            } else {
                // Best-effort: on the rare all-candidates-taken error, fall back to
                // the bare slug rather than failing a read-only preview.
                resolve_session_branch(&project_root, &slug, "auto")
                    .unwrap_or_else(|_| slug.clone())
            };
            // trace:BUG-1628 | ai:claude — the shared pickup resolver.
            let worktree_path = match path_override {
                Some(p) => std::path::PathBuf::from(p),
                None => pickup_worktree_path(&project_root, &slug)
                    .unwrap_or_else(|_| std::path::PathBuf::from(&slug)),
            };
            let executable = session::resolve_agent_program(launch_vendor.program());
            dry_line("branch", branch.cyan().to_string());
            dry_line(
                "worktree",
                worktree_path.display().to_string().cyan().to_string(),
            );
            dry_line("vendor", launch_vendor.as_str().cyan().to_string());
            dry_line("exec", executable.cyan().to_string());
            if let Some(model) = &resolved_model {
                dry_line("model", model.cyan().to_string());
            }
            if launch_vendor == session::HeadlessVendor::Claude {
                let session_render = match &launch {
                    Some(l) => format!(
                        "{} {}",
                        l.session_id().cyan(),
                        "(claude session id it would launch / resume)".dimmed()
                    ),
                    None => "(deferred — --no-launch)".dimmed().to_string(),
                };
                dry_line("session", session_render);
            }
            dry_line(
                "lease",
                format!(
                    "{} {}",
                    "would be created".cyan(),
                    format!("(owner {}, scope {}, role {})", user_id, plan.scope, role).dimmed()
                ),
            );
            eprintln!();
            eprintln!(
                "{} dry run — nothing created (no worktree, no lease, no session). \
             Re-run without --dry-run to pick up.",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold()
            );
            return Ok(());
        }

        // Re-list after each guarded end: a prior owner may exit during pickup,
        // but PID scans and lease age cannot authorize reclaim. Ownership's judge
        // supplies death proof; tree coverage and ordinary session_end protect WIP.
        // trace:STORY-2325 | ai:codex
        {
            let project_root_for_conflict = find_main_worktree_root()?;
            // trace:STORY-2325 | ai:codex — the judge alone decides ownership.
            // Tree coverage and session_end still guard every destructive step.
            let mut remaining = 16usize;
            let adopting = adopt_orphan.as_ref().map(|o| o.id.as_str());
            while let Some(conflict) = crate::leases::next_scope_conflict(
                list_leases(&project_root_for_conflict),
                &plan.scope,
                adopting,
            ) {
                let conflict_id = conflict.id.as_str();
                let pr_branch = crate::pr_lease_takeover::pr_branch_for_lease(
                    &project_root_for_conflict,
                    &conflict,
                );
                match crate::pr_lease_takeover::plan_lease_end(
                    &project_root_for_conflict,
                    &conflict,
                    pr_branch.as_deref(),
                ) {
                    crate::pr_lease_takeover::LeaseEndAction::SafeEnd => {
                        eprintln!(
                        "  {} lease {}'s session has exited and its seat on `{}` is clean and fully covered by origin — ending it",
                        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                        &conflict.id[..conflict.id.len().min(8)],
                        conflict.branch
                    );
                    }
                    crate::pr_lease_takeover::LeaseEndAction::Bail(message) => {
                        anyhow::bail!(
                            "ownership reclaim could not end lease {}: {}",
                            conflict_id,
                            message
                        );
                    }
                };
                crate::ownership::reader::journal_reclaim(&project_root_for_conflict, &conflict)
                    .with_context(|| {
                        format!("ownership reclaim could not end lease {}", conflict.id)
                    })?;
                session_end(
                Some(&conflict.id),
                /* spec */ None,
                /* branch */ None,
                /* yes */ true,
                /* force */ false, // trace:STORY-2325 | ai:codex
                /* purge_cc */ false,
                /* wait_ci */ false,
                /* watch_ci */ false,
                // Reclaim skips CI, while keeping tree and holder safeguards.
                /* skip_ci */ true,
                /* return_to_pool */ false,
                /* remove */ false,
                /* include_in_use */
                false,
            )
            .map_err(|error| anyhow::anyhow!(
                "ownership reclaim could not end lease {}: {error:#}; retain the lane and resolve its tree or mail safeguards before retrying",
                conflict_id,
            ))?;
                remaining -= 1;
                if remaining == 0 {
                    anyhow::bail!(
                        "ownership reclaim sweep gave up after 16 iterations on scope `{}` — \
                     the lease store may be corrupt; inspect `.aida/sessions/`",
                        plan.scope
                    );
                }
            }
            // trace:BUG-2223 | ai:claude
            if let Some(orphan) = &adopt_orphan {
                eprintln!(
                    "  {} re-entering orphaned lease {}'s tree in place (--resume): {}",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    (&orphan.id[..orphan.id.len().min(8)]).yellow(),
                    orphan.worktree_path.display()
                );
            }
        }

        // TASK-619: cross-machine duplicate-pickup guard. Session leases are
        // machine-local (.aida/sessions/, gitignored), so the only shared
        // "someone is on this" signal across machines is the git-canonical
        // status flip — which is eventually-consistent. Now that we've pulled
        // the latest store, re-read the anchor spec's status and refuse if it
        // was claimed/shipped elsewhere in the window between planning this
        // pickup and now. The existing BUG-379 preflight in `session_start`
        // catches stuck-InProgress-without-lease; this guard specifically names
        // the cross-machine dup-pickup case with a recovery hint and respects
        // --force-claim / orchestrator-corroboration / review sessions.
        // trace:TASK-619 | ai:claude
        if !no_pull {
            let status_after_pull: Option<RequirementStatus> =
                storage.load().ok().and_then(|store| {
                    store
                        .get_requirement_by_spec_id(&plan.scope)
                        .map(|r| r.status.clone())
                });
            let orchestrator_corroborated = {
                let root = project_root_for_config
                    .clone()
                    .or_else(|| std::env::current_dir().ok())
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                orchestrator::detect(&root).is_orchestrated()
            };
            let is_review_session = plan.review_target.is_some()
                || role == "reviewer"
                || std::env::var("AIDA_REVIEW_VERDICT_FILE").is_ok();
            if let DupPickupDecision::Refuse(msg) = dup_pickup_recheck(
                &plan.anchor_display,
                status_before_pull.as_ref(),
                status_after_pull.as_ref(),
                force_claim,
                orchestrator_corroborated,
                is_review_session,
            ) {
                anyhow::bail!("{}", msg);
            }
        }

        // BUG-1761: the suffix allocator already knows when the spec branch is
        // occupied; expose that collision before claiming status, lease, or tree.
        // trace:BUG-1761 | ai:codex
        let preclaim_orchestrator_corroborated = {
            let root = project_root_for_config
                .clone()
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            orchestrator::detect(&root).is_orchestrated()
        };
        let preclaim_review_session = plan.review_target.is_some()
            || role == "reviewer"
            || std::env::var("AIDA_REVIEW_VERDICT_FILE").is_ok();
        let preclaim_bypass = force_claim
        || queue_work_plan_wants_pr_head_branch(&plan)
        // BUG-022: the open PR is the one this pickup continues, not a
        // collision. trace:BUG-b6b1278890774ef993d2ebc327862470-022 | ai:claude
        || rework_branch_override.is_some()
        || preclaim_review_session
        || preclaim_orchestrator_corroborated
        || resume.is_some()
        || branch_override.is_some();
        if !preclaim_bypass {
            let (open_pr, branch_probe_allowed) =
                match crate::detect_open_pr_for_spec_via_forge(&project_root, &plan.scope) {
                    crate::PrLookup::Found(info) => (Some(info), false),
                    crate::PrLookup::NoOpenPr => (None, true),
                    // Forge failures mean unknown, not "no PR". Skip the branch
                    // refusal too, so an offline or gh-less pickup proceeds.
                    crate::PrLookup::GhMissing
                    | crate::PrLookup::GhFailed(_)
                    | crate::PrLookup::GhUnreachable(_) => (None, false),
                };
            let branch = slugify(&plan.scope);
            let branch_exists =
                branch_probe_allowed && crate::branch_exists_anywhere(&project_root, &branch);
            // BUG-1763: an existing branch only collides when it is UNMERGED.
            // Merged-ness comes from the forge (`gh pr list --head <branch>
            // --state merged`) — never from a git ancestry test, which reports
            // NO for every squash-merged branch. The probe runs only when the
            // branch exists, and a probe failure fails open.
            // trace:BUG-1763 | ai:claude
            let branch_state = preclaim_branch_state(branch_exists, || {
                crate::detect_merged_pr_for_branch_via_forge(&project_root, &branch)
            });
            if let PreclaimDecision::Refuse(msg) = preclaim_collision_check(
                &plan.scope,
                open_pr
                    .as_ref()
                    .map(|info| (info.number, info.title.as_str(), info.url.as_str())),
                &branch,
                branch_state,
                preclaim_bypass,
            ) {
                anyhow::bail!("{}", msg);
            };
        }

        // BUG-1607: preflight the resolved launch vendor BEFORE `session_start`
        // below mints a lease/worktree — both vendor SUPPORT (an interactive
        // launch refuses Agy; this used to be checked only after session_start,
        // at the `launch_vendor == Agy && !no_human` bail further down, leaving
        // an orphaned lease/worktree behind exactly like the bug this fixes) and
        // binary reachability (`launch_vendor` above, via
        // `resolve_enabled_headless_vendor`, only checks the `[agents] enabled`
        // config, not whether the binary is installed — the observed failure:
        // `vendor: codex` resolved correctly, then the launch itself hardcoded
        // `claude`, or on a genuinely codex-only machine `codex` was never
        // installed). `--no-launch` is exempt: it deliberately defers the
        // launch, so neither check should block the setup-only prep.
        // trace:BUG-1607 | ai:claude
        if !no_launch {
            session::preflight_launch_vendor(launch_vendor, !no_human)?;
            // BUG-014: a contained Claude reviewer needs the egress allowlist and
            // the reviewer allow entries on the PR branch it checks out; refuse
            // before any lease or worktree.
            // trace:BUG-b6b1278890774ef993d2ebc327862470-014 | ai:claude
            if let Some((forge, pr)) = plan.review_target.filter(|_| {
                contained
                    && launch_vendor == session::HeadlessVendor::Claude
                    && role.eq_ignore_ascii_case("reviewer")
            }) {
                crate::reviewer_preflight::preflight_contained_reviewer(&project_root, forge, pr)?;
            }
        }

        // The selected role is launch intent, never a process-wide authority
        // mutation. Each child receives it together with its own grant below.
        // trace:STORY-2326 | ai:codex

        // STORY-248: resolve the stacked base BEFORE session_start so the
        // worktree is forked from the right place and the SHA capture has a
        // real base to record. Both `--stack` and `--base` flow through the
        // same `session_start(base: Option<&str>)` parameter.
        // trace:STORY-248 | ai:claude
        let project_root_for_base = project_root.clone();
        let cwd_for_base =
            std::env::current_dir().unwrap_or_else(|_| project_root_for_base.clone());
        let resolved_base = resolve_stack_base(
            &project_root_for_base,
            &cwd_for_base,
            stack,
            base,
            force_base,
        )?;
        if let Some(b) = resolved_base.as_deref() {
            eprintln!(
                "  {} base: {} {}",
                crate::glyph(crate::glyphs::Glyph::FlowActive).cyan().bold(),
                b.cyan(),
                if stack {
                    "(detected via --stack)".dimmed().to_string()
                } else {
                    "(--base)".dimmed().to_string()
                }
            );
        }

        // session_start handles worktree creation, lease persistence, env
        // shim, conflict detection. We always pass launch=false so we can
        // (a) write the cluster manifest from the new lease and (b) emit
        // a queue-work-specific launch summary before exec.
        // BUG-1918: when this command launches the worker, the claim is made FOR
        // it — the ledger must name the worker, not this launcher.
        // trace:BUG-1918 | ai:claude
        let start = || {
            crate::session_start::session_start_with_receipt(
                &plan.scope,
                session_branch_override,
                resolved_base.as_deref(),
                /* reuse_branch */ false,
                path_override,
                /* forge_override */ None,
                /* branch_style */ "auto",
                /* launch */ false,
                /* launch_title */ None,
                /* launch_set_title */ false,
                /* launch_name */ None,
                // STORY-495: inert here (launch=false); the real launch below threads
                // the resolved `permission_mode` into the exec call directly.
                /* permission_mode */
                permission_mode.as_deref(),
                /* launch_contained */
                false,
                /* role */ Some(role.clone()),
                /* force_claim */ force_claim,
                // STORY-714: config-driven ([worktree_pool] enabled). The pool is used
                // only for non-stacked spawns (resolved_base is None unless --stack /
                // --base), so stacked work keeps a fresh `git worktree add`.
                /* use_pool */
                None,
                /* self_claim */
                queue_work_self_claim(no_launch),
                // trace:TASK-1786 | ai:codex
                role_input.review_coexistence(&plan),
                // trace:BUG-2169 | ai:claude
                /* contained_seat */
                contained,
                // BUG-2428: a launching pickup keeps the setup's rollback armed
                // until it knows whether an agent was launched (below).
                // trace:BUG-2428 | ai:claude
                /* rollback_failed_setup */
                !no_launch,
                // BUG-2223: re-enter the orphan's tree in place. Session start
                // re-checks it and bypasses nothing else: a live claim on the
                // scope still refuses. trace:BUG-2223 | ai:claude
                if adopt_orphan.is_some() {
                    crate::session_start::PathReentry::AdoptOrphan
                } else {
                    crate::session_start::PathReentry::None
                },
                resolved_pickup_pr.clone(),
            )
        };
        let setup_receipt = if no_launch {
            start()?
        } else {
            crate::gate::review_authority::as_launcher(start)?
        };

        // The lease this pickup minted, for the refused-delegation release below.
        let mut minted_lease_id: Option<String> = None;
        // trace:TASK-1337 | ai:codex
        // TASK-2807: the setup rollback stays armed only until `agent` marks a
        // call that can start an agent. trace:TASK-2807 | ai:claude
        let launch_agent = |agent: &AgentLaunchMark| -> Result<()> {
            // Look up the lease we just minted: by scope, by owner=us, freshest.
            let lease = list_leases(&project_root)
        .into_iter()
        .filter(|l| l.scope.eq_ignore_ascii_case(&plan.scope))
        .max_by_key(|l| l.started_at)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "session_start completed but no lease for scope `{}` is visible — try `aida session leases`",
                plan.scope
            )
        })?;
            minted_lease_id = Some(lease.id.clone());
            // The PR branch has been seated by session_start. Prepare it before
            // any vendor, red run, child grant or resume is started.
            // trace:STORY-2475 | ai:codex
            let mut launch_vendor = launch_vendor;
            let mut resolved_model = resolved_model.clone();
            let mut prompt = prompt.clone();
            if !no_launch && !plan_only && !guided && role == "implementer" {
                if let Some((pr, branch)) = rework_pr.as_ref() {
                    let files = crate::seat_clone::prepare_rework_merge(
                        &project_root,
                        &lease.worktree_path,
                        branch,
                        matches!(launch, Some(QueueWorkLaunch::Resume(_))),
                    );
                    launch_vendor = apply_rework_conflict_route(
                        &project_root,
                        files,
                        ReworkConflictLaunch {
                            vendor: launch_vendor,
                            resume: matches!(launch, Some(QueueWorkLaunch::Resume(_))),
                            interactive: !no_human,
                            contained,
                            scope: &plan.scope,
                            pr: *pr,
                            branch,
                        },
                        &mut prompt,
                        &mut resolved_model,
                        |subject, body| {
                            crate::ship_from_seat::advisor_request(&project_root, subject, body)
                        },
                    )?;
                }
            }
            // Rework may select another vendor after the early host gate. Recheck
            // its actual policy before spawning; ordinary cleanup retains the lease
            // recovery path if this late check refuses. trace:STORY-2642 | ai:codex
            if crate::lane_host::needs_preflight(no_human, dry_run, no_launch, list_sessions)
                && preflight_vendor != Some(launch_vendor)
            {
                crate::lane_host::preflight(&project_root, launch_vendor.as_str(), sandbox)?;
            }
            let headless_vendor = no_human.then_some(launch_vendor);

            // STORY-1386: opt-in pre-implementation red run, in this lane's own
            // freshly created worktree and BEFORE the implementer agent launches.
            // `lane_is_fresh` skips a lane already carrying commits (retry/rework).
            // trace:STORY-1386 | ai:claude
            if role == "implementer" && plan.review_target.is_none() {
                crate::criteria_red_run::after_lane_created(
                    &project_root,
                    &lease.worktree_path,
                    &plan.anchor_display,
                );
            }

            // TASK-99: warn (don't auto-pull) when the base the new worktree forked
            // from is behind origin/main. Closes the visibility half of the
            // 2026-05-13 stale-base pain cheaply: the operator sees the drift at
            // pickup and can `aida rebase` before the session accumulates work on a
            // stale base. We deliberately do NOT auto-pull here — that risks
            // surprising the worktree; the orchestrator drain (fresh main per-phase)
            // and the rebase verb own divergence handling. Best-effort + silent on
            // missing data (no origin/main → fresh clone / offline).
            // trace:TASK-99 | ai:claude
            {
                let base_ref = resolved_base.as_deref().unwrap_or("main");
                if let Some(behind) = commits_behind_origin_main(&project_root, base_ref) {
                    if let Some(msg) = behind_origin_warning(behind, "main") {
                        eprintln!(
                            "  {} {}",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                            msg.yellow()
                        );
                    }
                }
            }

            // STORY-248: register the stacked-branch entry in `.aida/stacks.json`
            // so `aida pull --auto`'s cascade can find it when the parent merges.
            // session_start records the parent fields on the lease itself; we
            // mirror them into the dedicated graph file because the cascade
            // needs to consult them AFTER the lease has been removed by
            // `aida queue done`. Best-effort — a write failure logs but doesn't
            // fail the pickup. trace:STORY-248 | ai:claude
            if let (Some(parent), Some(sha)) = (
                lease.parent_branch.as_deref(),
                lease.parent_branch_sha.as_deref(),
            ) {
                let mut graph = stacks::load(&project_root);
                stacks::add(
                    &mut graph,
                    stacks::StackEntry {
                        branch: lease.branch.clone(),
                        parent_branch: parent.to_string(),
                        parent_branch_sha: sha.to_string(),
                        spec_id: Some(plan.scope.clone()),
                        created_at: chrono::Utc::now(),
                    },
                );
                if let Err(e) = stacks::save(&project_root, &graph) {
                    eprintln!(
                        "  {} {}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                        format!(
                            "stack graph save failed: {} (cascade may miss this branch)",
                            e
                        )
                        .yellow()
                    );
                } else {
                    eprintln!(
                        "  {} stacked: {} → {}",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        lease.branch.cyan(),
                        parent.cyan()
                    );
                }
            }

            // Cluster manifest: pre-populate items so /aida-pickup can walk
            // them top-down. Skip for head/item modes (single item → no plan
            // needed beyond the queue head). trace:STORY-98 | ai:claude
            //
            // TASK-95: discover the plan brief for the anchor spec from any
            // owning docs/plans/ file. Cluster mode always writes a manifest, so
            // it just gains the brief; head/item modes write a manifest only
            // when a plan file exists (no plan file → today's no-op behavior).
            // trace:TASK-95 | ai:claude
            let plan_context = discover_plan_context(&project_root, &plan.anchor_display);
            // TASK-112: the claude session id to record in the manifest — the
            // UUID minted for a fresh launch, or the id being resumed.
            // STORY-132: a caller-minted `--session-id` is recorded even under
            // `--no-launch` (the TUI sets up a session then hosts the launch
            // itself), so the manifest carries the id either way.
            let claude_session_id: Option<String> = match (&launch, session_id) {
                (Some(l), _) => Some(l.session_id().to_string()),
                // --no-launch + caller-minted id: record it so the manifest
                // carries it (already validated as a UUID at function entry).
                (None, Some(sid)) => Some(sid.to_string()),
                // BUG-225: --no-launch --no-human defers a headless launch, and
                // `claude -p` requires a `--session-id`. Mint one when the caller
                // didn't supply it so the manifest carries the id and the printed
                // hint is a complete, round-trippable command.
                (None, None) if no_human => Some(uuid::Uuid::now_v7().to_string()),
                (None, None) => None,
            };
            // Write the manifest when there are planned cluster items, a plan
            // brief was discovered, there's a claude session id to record so
            // a later `--resume` can find this conversation, or this is a batch
            // pickup whose batch marker /aida-pickup needs.
            // trace:STORY-98, TASK-95, TASK-112, TASK-272 | ai:claude
            if plan.mode == QueueWorkMode::Cluster
                || plan_context.is_some()
                || claude_session_id.is_some()
                || batch_name.is_some()
            {
                write_queue_work_manifest(
                    &project_root,
                    &lease,
                    &plan,
                    plan_context.clone(),
                    claude_session_id.clone(),
                    batch_name,
                )?;
                if plan.mode == QueueWorkMode::Cluster {
                    eprintln!(
                        "  {} wrote manifest with {} planned item(s)",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        plan.entries.len()
                    );
                }
                if let Some(ctx) = &plan_context {
                    eprintln!(
                        "  {} attached plan brief from {}",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        ctx.plan_file.cyan()
                    );
                }
            }

            // BUG-1485: publish the parent/child correlation while the lease is
            // definitely present. This receipt deliberately lives outside the lease
            // directory, so a fast `queue done` or `pr ship` cannot erase the only
            // mapping before the orchestrator resumes after waitpid.
            // trace:BUG-1485 | ai:codex
            publish_orchestrated_lease_receipt_from_env(claude_session_id.as_deref(), &lease)?;

            if no_launch {
                eprintln!();
                eprintln!(
                    "{} setup complete; launch deferred (`--no-launch`).",
                    crate::glyph(crate::glyphs::Glyph::Check).green().bold()
                );
                eprintln!(
                    "  {}",
                    format!("cd {}", lease.worktree_path.display()).cyan()
                );
                // Never point at a raw `source` of the worktree's session-env file (a
                // branch can commit it); the eval'd enter applies the filtered env.
                // trace:BUG-1627 | ai:claude
                eprintln!(
                    "  {}    {}",
                    format!("aida worktree enter {}", lease.scope).cyan(),
                    "# cd in + warm build cache (filtered session env)".dimmed()
                );
                if no_human {
                    // STORY-263: mirror the headless launch the non-`--no-launch`
                    // path would have run. BUG-225: render it from
                    // `claude_headless_args_with_posture` (via `headless_launch_hint`) so the
                    // copy-pasteable command can't drift from `exec_claude_headless`
                    // — same flag order, `--session-id` included, prompt last.
                    // STORY-278: helper also prefixes `AIDA_HEADLESS=1` to match
                    // the env `exec_claude_headless` sets, so the copy-pasted hint
                    // launches an equivalent process.
                    let sid = claude_session_id.as_deref().unwrap_or_default();
                    eprintln!(
                        "  {}",
                        deferred_headless_launch_hint(
                            launch_vendor,
                            &prompt,
                            sid,
                            contained,
                            resolved_model.as_deref(),
                        )
                        .cyan()
                    );
                } else {
                    eprintln!(
                        "  {}",
                        deferred_interactive_launch_hint(
                            launch_vendor,
                            &prompt,
                            permission_mode.as_deref(),
                            contained,
                            resolved_model.as_deref(),
                        )
                        .cyan()
                    );
                }
                // BUG-673: next-step breadcrumb after a `queue work` pickup. The lease
                // is taken and the worktree is set up (only the launch is deferred), so
                // this IS a genuine pickup — the natural next move is to finish the work
                // (`aida queue done <id>`). Closes the gap where the next[] block
                // dropped out at `queue work`. Emit the TOON `next[]` block in agent
                // mode, the human `Next:` block otherwise. trace:BUG-673 | ai:claude
                {
                    let next = crate::help_next::queue_work_next(&plan.anchor_display);
                    let rendered = if agent_output_mode() {
                        crate::help_next::render(&next)
                    } else {
                        crate::help_next::render_human(&next)
                    };
                    if let Some(block) = rendered {
                        println!("{block}");
                    }
                }
                return Ok(());
            }

            // Chdir + source session-env shim + exec claude with the skill prompt.
            // Mirrors session_start's launch path (TASK-63 env sourcing).
            let env_shim = lease.worktree_path.join(".aida").join("session-env.sh");
            let applied_vars: Vec<String> = match std::fs::read_to_string(&env_shim) {
                Ok(body) => apply_session_env_to_process(&body),
                Err(_) => Vec::new(),
            };
            if !applied_vars.is_empty() {
                eprintln!(
                    "  {} sourced .aida/session-env.sh ({})",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    applied_vars.join(", ").dimmed()
                );
            }
            // TASK-2032: the host main, resolved once before the chdir into the
            // seat and threaded explicitly into the launch below.
            // trace:TASK-2032 | ai:claude
            let host_main = crate::seat_clone::launcher_main();
            std::env::set_current_dir(&lease.worktree_path).with_context(|| {
                format!("failed to chdir into {}", lease.worktree_path.display())
            })?;
            // BUG-1918: the launched worker (exec'd or spawned below) carries its
            // lease id, so it stays its lease's author wherever it later runs.
            // trace:BUG-1918 | ai:claude
            crate::gate::review_authority::export_worker_session_env(&lease.id);

            // TASK-112: exec — resume the prior conversation, or cold-launch
            // with the minted session id so this conversation is itself
            // resumable later. trace:TASK-112 | ai:claude
            let launch = launch.expect("launch decision is set when !no_launch");

            // BUG-226: a standalone `aida queue work <PR-N> --role reviewer` — a
            // reviewer session on a PR scope NOT spawned by the `--auto-complete`
            // orchestrator. The orchestrator sets `AIDA_REVIEW_VERDICT_FILE` on
            // its phase-3 child, so its absence is the standalone signal. For a
            // standalone run, point the `/aida-review` skill at a verdict file and
            // route the launch through `run_standalone_reviewer` (spawn + wait +
            // end-of-command summary) instead of `exec`'ing claude — so the
            // command no longer exits silently to the shell prompt.
            // trace:BUG-226 | ai:claude
            let standalone_reviewer: Option<(u64, std::path::PathBuf)> =
                if is_standalone_reviewer(&role, plan.review_target.is_some()) {
                    plan.review_target.map(|(forge, n)| {
                        (
                            n,
                            project_root
                                .join(".aida")
                                .join("review-verdicts")
                                .join(format!("{}.json", format_review_label(forge, n))),
                        )
                    })
                } else {
                    None
                };
            if let Some((pr, verdict_path)) = &standalone_reviewer {
                // A watch launch revalidates head, overlap and its reservation
                // before the destructive verdict reset, so a refused launch keeps
                // the prior verdict. Outside a watch launch this is inert.
                // trace:TASK-1993 | ai:claude
                crate::review_watch::reviewer_launch_context(&project_root, *pr)?;
                // A contained reviewer whose grant key cannot be held is refused
                // here, before the reset, so the refusal keeps the prior verdict.
                // trace:BUG-b6b1278890774ef993d2ebc327862470-045 | ai:claude
                refuse_unpresentable_contained_reviewer(contained)?;
                clear_standalone_reviewer_verdict(verdict_path);
                // The `/aida-review` skill writes the verdict here — it keys off
                // this env var. Setting it for standalone runs too is what makes
                // the skill "always write the verdict file" (BUG-226 acceptance).
                std::env::set_var("AIDA_REVIEW_VERDICT_FILE", verdict_path);
                // BUG-809: same prompt-text anchor for the standalone reviewer —
                // the env var was absent at prompt-derivation time above.
                // trace:BUG-b6b1278890774ef993d2ebc327862470-045 | ai:claude
                append_standalone_reviewer_prompt_suffixes(&mut prompt, &lease.worktree_path);
            }

            eprintln!();
            if let Some((pr, verdict_path)) = standalone_reviewer {
                return run_standalone_reviewer(
                    &project_root,
                    pr,
                    launch,
                    &prompt,
                    permission_mode.as_deref(),
                    &plan.scope,
                    &lease.branch,
                    &role,
                    &lease.worktree_path,
                    no_human,
                    quiet,
                    &verdict_path,
                    contained,
                    launch_vendor,
                    host_main.as_deref(),
                );
            }
            // trace:TASK-1594 | ai:claude
            let mut child_grant_id = None;
            // BUG-2139: the fresh headless child's own authority — minted here,
            // never inherited from the launcher. `None` on every other arm.
            let mut headless_child_grant = None;
            if !no_human {
                // trace:BUG-b6b1278890774ef993d2ebc327862470-045 | ai:claude
                let child_grant = crate::launch::issue_launch_child(
                    &project_root,
                    &crate::canonical_role_name(&role),
                    &crate::current_user_id(None),
                    // trace:TASK-2631 | ai:codex
                    // trace:TASK-2708 | ai:claude
                    session::sandboxed_child_grant(launch_vendor, contained)?,
                )?;
                child_grant_id = Some(child_grant.id);
            } else if matches!(launch, QueueWorkLaunch::Fresh(_)) {
                // trace:BUG-2139 | ai:claude
                // A contained child of any seat whose key cannot be held is
                // refused here, before it spawns.
                // trace:TASK-b6b1278890774ef993d2ebc327862470-144 | ai:claude
                let seat = crate::canonical_role_name(&role);
                let grant = crate::gate::seat_authority::issue_headless_child(
                    &project_root,
                    &seat,
                    &crate::current_user_id(None),
                    // trace:TASK-2631 | ai:codex
                    // trace:TASK-2708 | ai:claude
                    session::sandboxed_child_grant(launch_vendor, contained)?,
                )?;
                match &grant {
                    crate::gate::seat_authority::HeadlessChildGrant::Issued(issued) => {
                        // Stamped beside `active_pid`, so a lease end or reap
                        // revokes it even if this launcher dies before the child.
                        if let Err(e) = session::stamp_lease_child_grant_at(
                            &project_root,
                            &lease.id,
                            &issued.id,
                        ) {
                            session::revoke_headless_child_grant(&issued.id);
                            return Err(e.context(
                                "could not record the headless child grant on its lease",
                            ));
                        }
                    }
                    crate::gate::seat_authority::HeadlessChildGrant::Baseline { cause } => {
                        eprintln!(
                            "  {} {}",
                            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                            headless_baseline_notice(&seat, cause)
                        )
                    }
                }
                headless_child_grant = Some(grant);
            }
            // STORY-1662: spawn the standalone implementer, wait, and report its
            // outcome instead of exec'ing the vendor.
            // trace:STORY-1662 | ai:claude
            if standalone_implementer {
                if let (QueueWorkLaunch::Fresh(id), Some(child_grant)) =
                    (&launch, headless_child_grant.as_ref())
                {
                    crate::standalone_implementer::record_child_authority(
                        &project_root,
                        &lease.id,
                        child_grant,
                        crate::gate::seat_authority::current_grant_id().as_deref(),
                    );
                    eprintln!();
                    // Mark setup ownership only after an actual spawn succeeds.
                    // trace:STORY-2690 | ai:codex
                    return crate::standalone_implementer::run_recorded(
                        crate::standalone_implementer::Launch {
                            project_root: &project_root,
                            vendor: launch_vendor,
                            prompt: &prompt,
                            session_id: id,
                            spec: &plan.scope,
                            lease_id: &lease.id,
                            branch: &lease.branch,
                            contained,
                            child_grant,
                            host_main: host_main.as_deref(),
                        },
                        attempt.as_ref(),
                        Some(&|| agent.starting()),
                    );
                }
            }
            // TASK-895: a Codex tab hosts a fresh interactive Codex session. Codex has
            // no caller-minted session id / AIDA-addressable resume, and the interactive
            // tab launch is never `--no-human` (the headless drain resolves its own
            // vendor via STORY-683), so this branch handles only the interactive Codex
            // launch and leaves the entire Claude `match launch` below byte-identical.
            // trace:TASK-895 | ai:claude
            if launch_vendor == session::HeadlessVendor::Codex && !no_human {
                // BUG-743: queue/do's interactive Codex path used to ignore the
                // STORY-495 `[agents] bypass` resolver and launch bare `codex
                // /aida-pickup`, leaving operators in prompt-per-command posture even
                // after opting the supervised fleet into bypass. Map the resolved
                // uniform bypass posture to Codex's actual flag here; `None` and every
                // non-bypass Claude permission mode keep Codex native.
                let codex_bypass = permission_mode.as_deref() == Some("bypassPermissions");
                // BUG-2394: `--plan-only` resolves to `plan`, which Codex has no
                // permission-mode for — map it to the read-only sandbox instead of
                // falling through to the ordinary write posture. A resumed plan
                // lane keeps it too (BUG-2386).
                // trace:BUG-2394 | ai:claude
                let codex_read_only = session::codex_plan_read_only(permission_mode.as_deref());
                // BUG-2386: `--resume` continues the recorded Codex thread instead
                // of silently starting a fresh session. trace:BUG-2386 | ai:claude
                if let QueueWorkLaunch::Resume(id) = &launch {
                    let (_, thread) = match resume_target.clone() {
                        Some(target) => target,
                        None => crate::codex_resume::recorded_resume(
                            &project_root,
                            id,
                            Some(lease.scope.as_str()),
                        )?,
                    };
                    eprintln!(
                        "{} {}",
                        crate::glyph(crate::glyphs::Glyph::FlowActive)
                            .green()
                            .bold(),
                        format!(
                            "resuming codex session {} in {} ({})",
                            &thread[..thread.len().min(8)],
                            lease.worktree_path.display(),
                            if codex_read_only {
                                "read-only sandbox"
                            } else if codex_bypass {
                                "bypass"
                            } else {
                                "native"
                            },
                        )
                        .cyan()
                    );
                    // trace:TASK-2712 | ai:claude — the resumed lane's own tree and role.
                    return session::exec_codex_resume(
                        &thread,
                        codex_bypass,
                        codex_read_only,
                        child_grant_id.as_deref(),
                        contained,
                        session::CodexLane {
                            tree: &lease.worktree_path,
                            role: Some(&crate::canonical_role_name(&role)),
                            // trace:TASK-2032 | ai:claude
                            host_main: host_main.as_deref(),
                        },
                    );
                }
                eprintln!(
                    "{} {}",
                    crate::glyph(crate::glyphs::Glyph::FlowActive)
                        .green()
                        .bold(),
                    format!(
                        "launching codex in {} ({}, prompt `{}`)",
                        lease.worktree_path.display(),
                        if codex_read_only {
                            "read-only sandbox"
                        } else if codex_bypass {
                            "bypass"
                        } else {
                            "native"
                        },
                        prompt
                    )
                    .cyan()
                );
                // trace:TASK-2712 | ai:claude — the lane's role picks its write set.
                // trace:TASK-2032 | ai:claude — the host main pinned before the chdir.
                return session::exec_codex_session(
                    host_main.as_deref(),
                    &prompt,
                    codex_bypass,
                    codex_read_only,
                    resolved_model.as_deref(),
                    child_grant_id.as_deref(),
                    Some(&crate::canonical_role_name(&role)),
                );
            }
            // BUG-1607: an interactive Agy launch is now refused by
            // `preflight_launch_vendor` above, BEFORE `session_start` minted the
            // lease/worktree this function is already holding by this point — so
            // `launch_vendor == Agy && !no_human` can no longer reach here. No
            // per-arm Agy handling needed below either: `match launch` only spawns
            // Claude.
            debug_assert!(
        !(launch_vendor == session::HeadlessVendor::Agy && !no_human),
        "BUG-1607: preflight_launch_vendor must refuse an interactive Agy launch before this point"
    );
            match launch {
                QueueWorkLaunch::Resume(id) => {
                    if no_human {
                        let log_path = project_root
                            .join(".aida")
                            .join("headless-logs")
                            .join(format!("{}-{}.jsonl", lease.branch, id));
                        eprintln!(
                            "{} {}",
                            crate::glyph(crate::glyphs::Glyph::FlowActive)
                                .green()
                                .bold(),
                            format!(
                                "resuming {} headless session {} in {} ({}, prompt `{}`)",
                                launch_vendor.as_str(),
                                &id[..id.len().min(8)],
                                lease.worktree_path.display(),
                                match launch_vendor {
                                    session::HeadlessVendor::Codex =>
                                        "codex exec resume".to_string(),
                                    _ => format!(
                                        "claude -p, {}",
                                        claude_posture_display(
                                            permission_mode.as_deref(),
                                            contained
                                        )
                                    ),
                                },
                                prompt
                            )
                            .cyan()
                        );
                        eprintln!(
                            "  {} headless output → {}",
                            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                            log_path.display().to_string().dimmed()
                        );
                        // BUG-342: no-human resumes must use the same structural
                        // AskUserQuestion denial as fresh headless launches. Plain
                        // `claude --resume` bypasses `claude_headless_resume_args`.
                        // trace:BUG-342 | ai:codex
                        let tee_opts = headless_tee::TeeOptions::from_env_and_flag(false)
                            .with_label(&lease.branch);
                        // BUG-2386: resume on the recorded vendor — a Codex thread
                        // resumes as `codex exec resume`. trace:BUG-2386 | ai:claude
                        let (vendor, resume_id) = match resume_target.clone() {
                            Some(target) => target,
                            None => crate::codex_resume::recorded_resume(
                                &project_root,
                                &id,
                                Some(lease.scope.as_str()),
                            )?,
                        };
                        // Counted by `AgentLaunchMark` once the child spawned:
                        // a failed spawn launched nobody. trace:TASK-2807 | ai:claude
                        let status = run_queue_resume_to_exit(
                            host_main.as_deref(),
                            &session::HeadlessResume {
                                vendor,
                                session_id: &id,
                                resume_id: &resume_id,
                            },
                            &prompt,
                            &log_path,
                            &lease.worktree_path,
                            &tee_opts,
                            contained,
                            // trace:BUG-1808 | ai:claude — the seat the preflight validated.
                            &crate::canonical_role_name(&role),
                            (&project_root, &plan.scope, &lease.id),
                            false,
                        )?;
                        std::process::exit(status.code().unwrap_or(1));
                    }
                    eprintln!(
                        "{} {}",
                        crate::glyph(crate::glyphs::Glyph::FlowActive)
                            .green()
                            .bold(),
                        format!(
                            "resuming claude session {} in {} ({})",
                            &id[..id.len().min(8)],
                            lease.worktree_path.display(),
                            claude_posture_display(permission_mode.as_deref(), contained)
                        )
                        .cyan()
                    );
                    let mut cmd = session::build_claude_resume_exec_command(
                        host_main.as_deref(),
                        &id,
                        permission_mode.as_deref(),
                        contained,
                        child_grant_id.as_deref(),
                    )?;
                    crate::launch::assign_seat(
                        &mut cmd,
                        aida_core::launch_policy::LaunchKind::Resume,
                        crate::launch::SeatDecision::Child {
                            role: &crate::canonical_role_name(&role),
                            grant: child_grant_id.as_deref().unwrap_or(""),
                        },
                    )?;
                    crate::launch::spawn::exec_agent(&mut cmd)
                }
                QueueWorkLaunch::Fresh(id) => {
                    let name = session::derive_session_name(&plan.scope, &lease.branch, &role);
                    if no_human {
                        // STORY-263: headless launch — `claude -p`, single-turn,
                        // exits on its own (no Ctrl+D). `bypassPermissions` is forced
                        // (SPIKE-7 Q2 — `acceptEdits` leaves Bash gated); the
                        // stream-json output goes to a log file the watchdog
                        // (TASK-298) can tail. trace:STORY-263 | ai:claude
                        let log_path = project_root
                            .join(".aida")
                            .join("headless-logs")
                            .join(format!("{}-{}.jsonl", lease.branch, id));
                        // BUG-705: the banner names the RESOLVED headless vendor —
                        // before, it always said claude even when the drain was
                        // routed to codex, hiding the unrouted-exec bug.
                        let headless_vendor = headless_vendor.unwrap_or_else(|| {
                            crate::session::resolve_headless_vendor(&project_root)
                        });
                        let launch_detail = match headless_vendor {
                            crate::session::HeadlessVendor::Claude => format!(
                                "claude -p, {}",
                                claude_posture_display(permission_mode.as_deref(), contained)
                            ),
                            crate::session::HeadlessVendor::Codex => "codex exec".to_string(),
                            // trace:TASK-1048 | ai:claude
                            crate::session::HeadlessVendor::Agy => "agy -p".to_string(),
                        };
                        eprintln!(
                            "{} {}",
                            crate::glyph(crate::glyphs::Glyph::FlowActive)
                                .green()
                                .bold(),
                            format!(
                                "launching {} headless in {} ({}, prompt `{}`)",
                                headless_vendor.as_str(),
                                lease.worktree_path.display(),
                                launch_detail,
                                prompt
                            )
                            .cyan()
                        );
                        // BUG-1928: name the tree the session ACTUALLY runs in (the
                        // lease's worktree — a warm-pool tree when the pool served
                        // one) and the contained write boundary derived from it, so
                        // the operator never sees only the orchestrator's requested
                        // path. trace:BUG-1928 | ai:claude
                        // BUG-1971: both lines come from one helper, so the
                        // boundary line reads the launch's own roots.
                        // trace:BUG-1971 | ai:claude
                        let phase_env = std::env::var(crate::orchestrator::PHASE_ENV).ok();
                        for line in crate::contained_boundary::queue_work_launch_lines(
                            phase_env.as_deref(),
                            &lease.worktree_path,
                            &lease.branch,
                            contained && headless_vendor == crate::session::HeadlessVendor::Claude,
                        ) {
                            eprintln!(
                                "  {} {}",
                                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                                line
                            );
                        }
                        eprintln!(
                            "  {} headless output → {}",
                            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                            log_path.display().to_string().dimmed()
                        );
                        // TASK-307: tee high-signal events so the operator can
                        // follow the headless run without opening a second terminal
                        // to tail the JSONL. Disable with `--no-tee-headless` or
                        // `AIDA_TEE_HEADLESS=0`; failure events stream regardless.
                        // trace:TASK-307 | ai:claude
                        let tee_opts = headless_tee::TeeOptions::from_env_and_flag(false)
                            .with_label(&lease.branch);
                        // trace:BUG-2139 | ai:claude
                        let child_grant = headless_child_grant
                            .as_ref()
                            .expect("BUG-2139: a fresh headless launch minted its child authority");
                        // `exec_vendor_headless` spawns and waits (it is not an
                        // exec). trace:TASK-2807 | ai:claude
                        agent.starting();
                        return session::exec_vendor_headless(
                            host_main.as_deref(),
                            headless_vendor,
                            &prompt,
                            &id,
                            &log_path,
                            &tee_opts,
                            contained,
                            Some(&lease.id),
                            &crate::canonical_role_name(&role),
                            child_grant,
                            // BUG-050: the lane's seat, for the resume safety net.
                            // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
                            Some((&lease.worktree_path, &lease.branch)),
                            preclaim_orchestrator_corroborated,
                        );
                    }
                    eprintln!(
                        "{} {}",
                        crate::glyph(crate::glyphs::Glyph::FlowActive)
                            .green()
                            .bold(),
                        format!(
                            "launching claude in {} ({}, prompt `{}`)",
                            lease.worktree_path.display(),
                            claude_posture_display(permission_mode.as_deref(), contained),
                            prompt
                        )
                        .cyan()
                    );
                    let mut cmd = session::build_claude_exec_command(
                        host_main.as_deref(),
                        permission_mode.as_deref(),
                        name.as_deref(),
                        Some(&prompt),
                        Some(&id),
                        contained,
                        resolved_model.as_deref(),
                        child_grant_id.as_deref(),
                    )?;
                    crate::launch::assign_seat(
                        &mut cmd,
                        aida_core::launch_policy::LaunchKind::Exec,
                        crate::launch::SeatDecision::Child {
                            role: &crate::canonical_role_name(&role),
                            grant: child_grant_id.as_deref().unwrap_or(""),
                        },
                    )?;
                    crate::launch::spawn::exec_agent(&mut cmd)
                }
            }
        };
        let (setup_result, rolled_back) =
            run_pickup_launch(&project_root, setup_receipt, launch_agent);
        // BUG-2428 / TASK-2807: a launch that failed before any agent could start
        // launched nobody, so `run_pickup_launch` rolled back everything this
        // invocation's setup created (worktree, branch, lease, claim and status
        // write), and neither the tree nor the lease blocks the next attempt with
        // "held by worktree ... before pickup setup". Any other outcome kept it.
        // trace:BUG-2428 TASK-2807 | ai:claude
        if rolled_back {
            eprintln!(
            "  no agent was launched: rolled back the {} pickup setup (worktree, lease, claim; anything kept is named above)",
            plan.scope
        );
            return setup_result;
        }
        // Advisor comment 35: a reviewer launch refused at child-seat delegation
        // launched nobody, so its PR-N lease would only block the next pickup
        // until --force-claim. Release it. (A setup this invocation owned was
        // rolled back above; this covers a re-entered lease.)
        // trace:TASK-1955 | ai:claude (advisor comment 35)
        if release_refused_review_lease(
            &project_root,
            plan.review_target.is_some(),
            minted_lease_id.as_deref(),
            &setup_result,
        ) {
            eprintln!(
                "  released the {} review lease: no reviewer was launched",
                plan.scope
            );
            return setup_result;
        }
        // A standalone implementer's outcome (punted, held, no PR) or a reviewer
        // that recorded no verdict is a result, not a failed setup to continue
        // by hand. trace:STORY-1662 | ai:claude
        if setup_result
            .as_ref()
            .is_err_and(pickup_failure_offers_manual_continuation)
        {
            eprintln!(
                "{}",
                // The vendor actually launched (honours `[agents] enabled`),
                // not the raw flag/default token. trace:BUG-2396 | ai:claude
                queue_work_manual_continuation(&plan.anchor_display, guided, launch_vendor)
            );
        }
        setup_result
    })();
    if let Some(attempt) = &attempt {
        attempt.finish_result(&result)?;
    }
    result
}

// Bind admission to the resolved role and scope, returning the decision for
// launch to consume even when receipt capture is disabled (for example previews).
// trace:STORY-2690 | ai:codex
pub(crate) fn resolve_pickup_attempt(
    attempt: &mut Option<crate::lane_result::Attempt>,
    plan: &QueueWorkPlan,
    role: &str,
    no_human: bool,
    branch: Option<&str>,
) -> Result<bool> {
    let implementer_route = crate::standalone_implementer::is_standalone_implementer(
        role,
        plan.review_target.is_none(),
        no_human,
    );
    if !implementer_route {
        if let Some(provisional) = attempt.take() {
            provisional.discard()?;
        }
    }
    if let Some(attempt) = attempt {
        attempt.resolve_scope(&plan.anchor_display, branch, None)?;
    }
    Ok(implementer_route)
}

/// Advisor comment 35: release the lease `lease_id` a failed pickup minted
/// when [`refused_review_launch_releases_lease`] says so. Returns whether it
/// was released.
// trace:TASK-1955 | ai:claude (TASK-2256)
pub(crate) fn release_refused_review_lease(
    project_root: &std::path::Path,
    review_scoped: bool,
    lease_id: Option<&str>,
    setup_result: &Result<()>,
) -> bool {
    let (Err(err), Some(lease_id)) = (setup_result, lease_id) else {
        return false;
    };
    if !refused_review_launch_releases_lease(review_scoped, err) {
        return false;
    }
    crate::session_reap::release_lease_files(project_root, lease_id);
    true
}

/// A standalone reviewer has exited: once its verdict is on the ledger, its
/// review (`scope` over this review's own `worktree`) is reaped — tree,
/// lease, and review branch — so neither the lease nor the tree blocks the
/// next review of the PR. Exit records a lease-bound completion receipt even
/// without a verdict. The retained tree and lease remain available for recovery;
/// next-head pickup may retire them once clean, published and idle. A refused
/// reap also retains them under the same guards. Returns
/// `(lease id, Ok(outcome) | Err(why kept))` per matching lease.
// trace:BUG-b6b1278890774ef993d2ebc327862470-024 | ai:claude
pub(crate) fn end_finished_review(
    project_root: &std::path::Path,
    scope: &str,
    worktree: &std::path::Path,
    verdict_collected: bool,
) -> Vec<(String, std::result::Result<String, String>)> {
    list_leases(project_root)
        .into_iter()
        .filter(|l| l.scope.eq_ignore_ascii_case(scope) && l.worktree_path == worktree)
        .map(|l| {
            // Exit is positive lifecycle evidence even when no verdict exists.
            // Write before teardown; mail-retention stamps preserve this binding.
            // trace:BUG-2498 | ai:codex
            let receipt = crate::ownership::reader::review_finished(project_root, &l);
            let outcome = if verdict_collected {
                crate::session_reap::reap_finished_review(project_root, &l).map_err(|why| {
                    match receipt {
                        Ok(()) => why,
                        Err(error) => format!("{why}; completion receipt refused: {error:#}"),
                    }
                })
            } else {
                Err(match receipt {
                    Ok(()) => "no verdict was collected; review exit recorded for guarded next-head cleanup".to_string(),
                    Err(error) => format!("no verdict was collected; completion receipt refused: {error:#}"),
                })
            };
            (l.id, outcome)
        })
        .collect()
}

/// BUG-2428: whether a pickup's launch failed before any agent process
/// started, so the setup it created serves nothing and is rolled back.
/// `agent_started` is whether the launch reached a call that can start one
/// ([`AgentLaunchMark`]): any error before that is a pre-agent refusal (a
/// watch-launch refusal, a lease-lookup miss, a failed chdir, a failed exec).
/// A refused child grant is minted before its worker spawns, so it is one
/// wherever it surfaces.
// trace:BUG-2428 TASK-2807 | ai:claude
pub(crate) fn launch_refused_before_agent(setup_result: &Result<()>, agent_started: bool) -> bool {
    setup_result.as_ref().is_err_and(|err| {
        !agent_started || crate::gate::seat_authority::ChildGrantRefused::is_in(err)
    })
}

/// TASK-2807: whether a pickup's launch can have started an agent. A spawn
/// through `review_watch::reviewer_status_on_spawn` (every reviewer and
/// resume runner) counts only once it returned a child, so a spawn that
/// failed (ENOENT, EACCES, a pre-spawn helper error) launched nobody. A
/// runner that spawns elsewhere is marked with [`Self::starting`] before
/// the call. An `exec` is not marked: it either replaces this process (no
/// rollback runs) or fails, which launched nobody.
// trace:TASK-2807 | ai:claude
pub(crate) struct AgentLaunchMark {
    marked: std::cell::Cell<bool>,
    spawns_at_start: u64,
}

impl Default for AgentLaunchMark {
    fn default() -> Self {
        Self {
            marked: std::cell::Cell::new(false),
            spawns_at_start: crate::review_watch::agent_spawn_count(),
        }
    }
}

impl AgentLaunchMark {
    pub(crate) fn starting(&self) {
        self.marked.set(true);
    }
    pub(crate) fn started(&self) -> bool {
        self.marked.get() || crate::review_watch::agent_spawn_count() != self.spawns_at_start
    }
}

/// TASK-2807: run a pickup's launch with its setup rollback armed only until
/// an agent can start. A launch that fails before that (or a refused child
/// grant) rolls back the setup `receipt` owns — worktree, branch, lease,
/// claim — and returns `(result, true)`. Any other outcome keeps the setup.
/// A panic is caught only to decide the same way (after the mark the receipt
/// is disarmed, so the agent's tree and lease survive the unwind) and is
/// then resumed.
// trace:TASK-2807 | ai:claude
pub(crate) fn run_pickup_launch(
    project_root: &std::path::Path,
    mut receipt: crate::session_start::SessionStartReceipt,
    launch: impl FnOnce(&AgentLaunchMark) -> Result<()>,
) -> (Result<()>, bool) {
    let mark = AgentLaunchMark::default();
    let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| launch(&mark))) {
        Ok(result) => result,
        Err(panic) => {
            if mark.started() {
                receipt.disarm();
            } else if receipt.owns_setup() {
                leave_setup_tree(project_root, &receipt);
            }
            drop(receipt);
            std::panic::resume_unwind(panic);
        }
    };
    if launch_refused_before_agent(&result, mark.started()) && receipt.owns_setup() {
        leave_setup_tree(project_root, &receipt);
        drop(receipt);
        return (result, true);
    }
    receipt.disarm();
    (result, false)
}

/// The launcher may have entered the setup's tree (the launch `chdir`s into
/// it); leave it for `project_root` before the rollback removes it. A
/// process already elsewhere stays where it is.
// trace:BUG-2428 TASK-2807 | ai:claude
pub(crate) fn leave_setup_tree(
    project_root: &std::path::Path,
    receipt: &crate::session_start::SessionStartReceipt,
) {
    let Some(tree) = receipt.lease_id.as_deref().and_then(|id| {
        list_leases(project_root)
            .into_iter()
            .find(|l| l.id == id)
            .map(|l| l.worktree_path)
    }) else {
        return;
    };
    let tree = std::fs::canonicalize(&tree).unwrap_or(tree);
    if std::env::current_dir().is_ok_and(|cwd| cwd.starts_with(&tree)) {
        let _ = std::env::set_current_dir(project_root);
    }
}

/// Advisor comment 35: whether a failed pickup's lease is released — only a
/// PR-scoped review pickup whose child grant was refused. An implementer's
/// lease stays for the manual continuation.
// trace:TASK-1955 | ai:claude (advisor comment 35)
pub(crate) fn refused_review_launch_releases_lease(
    review_scoped: bool,
    err: &anyhow::Error,
) -> bool {
    review_scoped && crate::gate::seat_authority::ChildGrantRefused::is_in(err)
}

/// The specs a reviewed PR credits for calibration: the same title / branch
/// derivation `aida pr ship` uses, including its store-backed closure hold, so
/// a held spec ship would not complete is not credited either. The hold reads
/// `project_root`'s store; with no readable store nothing is known to be held.
// trace:BUG-1957 | ai:claude
pub(crate) fn review_calibration_spec_ids(
    project_root: &std::path::Path,
    pr_title: &str,
    branch: &str,
    pr_body: &str,
) -> Vec<String> {
    let store = crate::load_store_for_lookup(project_root);
    review_calibration_spec_ids_in(store.as_ref(), pr_title, branch, pr_body)
}

/// [`review_calibration_spec_ids`] over an already-loaded store.
// trace:BUG-1957 | ai:claude
pub(crate) fn review_calibration_spec_ids_in(
    store: Option<&aida_core::RequirementsStore>,
    pr_title: &str,
    branch: &str,
    pr_body: &str,
) -> Vec<String> {
    pr_ship::derive_squash_subject_spec_ids_held(pr_title, branch, pr_body, |id| {
        store.and_then(|s| pr_ship::spec_completion_hold_reason(s, id))
    })
}

/// BUG-226: drive a standalone `aida queue work <PR-N> --role reviewer`
/// launch — spawn `claude` (not `exec`) so this process survives to read
/// the verdict file + headless JSONL log and print an end-of-command
/// summary. The bug was the silent exit to the shell prompt: a headless
/// reviewer ran to completion, posted a real review, and left no terminal
/// trace of pass/fail, cost, or where the artifacts landed.
///
/// The `--auto-complete` orchestrator's phase-3 reviewer does NOT come
/// here — it sets `AIDA_REVIEW_VERDICT_FILE`, which clears the standalone
/// signal — so the orchestrator keeps owning its own progress output.
/// trace:BUG-226 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_standalone_reviewer(
    project_root: &std::path::Path,
    pr: u64,
    launch: QueueWorkLaunch,
    prompt: &str,
    // STORY-495: `None` → faithful native launch (no `--permission-mode`).
    permission_mode: Option<&str>,
    scope: &str,
    branch: &str,
    role: &str,
    worktree: &std::path::Path,
    no_human: bool,
    quiet: bool,
    verdict_path: &std::path::Path,
    contained: bool,
    // BUG-1607: the ALREADY-RESOLVED launch vendor (flag > `[agents]` config >
    // default, filtered to an enabled+installed profile — the same contract
    // `handle_queue_work` resolves for the implementer path). Every arm below
    // reuses this value instead of re-deriving or hardcoding `claude`, so a
    // codex-routed standalone reviewer actually launches `codex`.
    // trace:BUG-1607 | ai:claude
    launch_vendor: session::HeadlessVendor,
    // TASK-2032: the host main `handle_queue_work` resolved before its chdir
    // into the seat; every launch arm below builds its boundary from it.
    // trace:TASK-2032 | ai:claude
    host_main: Option<&std::path::Path>,
) -> Result<()> {
    // TASK-2807: every arm spawns through `reviewer_status_on_spawn`, which
    // `AgentLaunchMark` counts only once a child exists, so a reviewer spawn
    // that fails (a missing `codex`) rolls the pickup's setup back and one
    // that ran keeps it. trace:TASK-2807 | ai:claude
    // Watch context only restricts an ordinary attested launch; it cannot
    // authorize a verdict or a reviewer seat. trace:TASK-1993 | ai:codex
    // Every arm below (fresh or resume, headless or interactive) refuses a
    // contained reviewer whose grant key cannot be held before it spawns.
    // trace:BUG-b6b1278890774ef993d2ebc327862470-045 | ai:claude
    refuse_unpresentable_contained_reviewer(contained)?;
    let watch_prompt = crate::review_watch::reviewer_launch_prompt(project_root, pr, prompt)?;
    let prompt = watch_prompt.as_deref().unwrap_or(prompt);
    // Keep a recoverable final draft even if a tool refuses the recording.
    // No command from the draft is automatically replayed by this launcher.
    // Headless only: an interactive reviewer has a human watching, and its
    // prompt is echoed in the launch line. trace:BUG-2187 | ai:codex
    // trace:BUG-2187 | ai:claude
    let recovery_prompt;
    let prompt = if no_human {
        recovery_prompt = format!(
            "{prompt}\n\nIf recording the verdict fails, include your final verdict draft and the exact \
             `aida review record` command you attempted (including the reviewed SHA, summary, and findings) \
             in your final response. Do not report a successful recording when it failed."
        );
        recovery_prompt.as_str()
    } else {
        prompt
    };
    // BUG-2149: only verdict files written after the reviewer starts belong
    // to this run. trace:BUG-2149 | ai:claude
    let launched_at = reviewer_spawn_instant(worktree);
    // BUG-2185: armed before the launch, so the reviewer's store writes are
    // committed on every exit from here, an `Err` after the child ran
    // included. trace:BUG-2185 | ai:claude
    let store_commit = crate::child_store_commit::ChildStoreCommit::arm(project_root, "reviewer");
    // trace:TASK-1752 | ai:claude
    let launcher_grant = crate::gate::seat_authority::current_grant_id();
    // Spawn the resolved vendor, wait, and capture the headless JSONL log
    // path (None for an interactive review — there is no stream-json log).
    // BUG-2149: every arm returns the id of the child reviewer grant it
    // issued, so the collected verdict can be bound to exactly this launch.
    // trace:BUG-2149 | ai:claude
    let (status, log_path, child_grant): (
        std::process::ExitStatus,
        Option<std::path::PathBuf>,
        Option<String>,
    ) = match launch {
        QueueWorkLaunch::Resume(id) => {
            if no_human {
                let log_path = project_root
                    .join(".aida")
                    .join("headless-logs")
                    .join(format!("{branch}-{id}.jsonl"));
                eprintln!(
                    "{} {}",
                    crate::glyph(crate::glyphs::Glyph::FlowActive)
                        .green()
                        .bold(),
                    format!(
                        "resuming claude headless reviewer session {} in {} (claude -p, {})",
                        &id[..id.len().min(8)],
                        worktree.display(),
                        claude_posture_display(permission_mode, contained)
                    )
                    .cyan()
                );
                eprintln!(
                    "  {} headless output → {}",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    log_path.display().to_string().dimmed()
                );
                // BUG-342: a no-human reviewer resume is still unattended;
                // route it through the shared headless resume argv so
                // AskUserQuestion is structurally denied.
                // trace:BUG-342 | ai:codex
                let tee_opts =
                    headless_tee::TeeOptions::from_env_and_flag(false).with_label("reviewer");
                // BUG-2386: resume on the recorded vendor. trace:BUG-2386 | ai:claude
                let (vendor, resume_id) =
                    crate::codex_resume::recorded_resume(project_root, &id, Some(scope))?;
                let (status, grant) = session::spawn_vendor_headless_resume_granted(
                    host_main,
                    &session::HeadlessResume {
                        vendor,
                        session_id: &id,
                        resume_id: &resume_id,
                    },
                    prompt,
                    &log_path,
                    worktree,
                    &tee_opts,
                    contained,
                    "reviewer",
                )?;
                (status, Some(log_path), Some(grant))
            } else {
                // BUG-1607 / BUG-2386: `handle_queue_work` resolves the launch
                // vendor to the vendor that recorded the session. Guard the
                // pairing anyway — this function is unit-tested directly, so the
                // invariant must hold even when this arm is reached without
                // going through that outer gate.
                // trace:BUG-1607 | ai:claude
                // trace:BUG-2386 | ai:claude
                let (recorded, resume_id) =
                    crate::codex_resume::recorded_resume(project_root, &id, Some(scope))?;
                if launch_vendor != recorded {
                    anyhow::bail!(
                        "this reviewer session was recorded by `{}`, but the resolved launch \
                         vendor is `{}`. Recovery: start a fresh `{}` reviewer instead of \
                         `--resume`.",
                        recorded.as_str(),
                        launch_vendor.as_str(),
                        launch_vendor.as_str()
                    );
                }
                if recorded == session::HeadlessVendor::Codex {
                    // A Codex reviewer resumes its thread interactively.
                    let grant = crate::launch::issue_launch_child(
                        project_root,
                        "reviewer",
                        &crate::current_user_id(None),
                        // trace:TASK-2708 | ai:claude
                        session::sandboxed_child_grant(recorded, contained)?,
                    )?;
                    // A reviewer never runs a plan lane; the seat comes from
                    // the reviewer grant, set by `finalize_codex_resume`.
                    // trace:TASK-2712 | ai:claude — the reviewer's own tree:
                    // its write set carries the gate ledger.
                    let mut cmd = session::build_codex_resume_command(
                        &resume_id,
                        permission_mode == Some("bypassPermissions"),
                        false,
                        Some(&grant.id),
                        contained,
                        session::CodexLane {
                            tree: worktree,
                            role: Some("reviewer"),
                            host_main, // trace:TASK-2032 | ai:claude
                        },
                    )?;
                    cmd.current_dir(worktree);
                    // trace:BUG-2386 | ai:claude
                    if let Err(error) = session::finalize_codex_resume(&mut cmd, project_root) {
                        session::revoke_headless_child_grant(&grant.id);
                        return Err(error);
                    }
                    eprintln!(
                        "{} {}",
                        crate::glyph(crate::glyphs::Glyph::FlowActive)
                            .green()
                            .bold(),
                        format!(
                            "resuming codex reviewer session {} in {}",
                            &resume_id[..resume_id.len().min(8)],
                            worktree.display(),
                        )
                        .cyan()
                    );
                    let status = crate::review_watch::reviewer_status(&mut cmd)
                        .context("failed to spawn codex")?;
                    (status, None, Some(grant.id))
                } else {
                    eprintln!(
                        "{} {}",
                        crate::glyph(crate::glyphs::Glyph::FlowActive)
                            .green()
                            .bold(),
                        format!(
                            "resuming claude reviewer session {} in {} ({})",
                            &id[..id.len().min(8)],
                            worktree.display(),
                            claude_posture_display(permission_mode, contained)
                        )
                        .cyan()
                    );
                    // trace:BUG-b6b1278890774ef993d2ebc327862470-045 | ai:claude
                    let grant = crate::launch::issue_launch_child(
                        project_root,
                        "reviewer",
                        &crate::current_user_id(None),
                        contained || session::aida_contained_env()?,
                    )?;
                    (
                        session::spawn_claude_resume_with_grant(
                            host_main,
                            &id,
                            permission_mode,
                            contained,
                            Some(&grant.id),
                        )?,
                        None,
                        Some(grant.id),
                    )
                }
            }
        }
        QueueWorkLaunch::Fresh(id) => {
            if no_human {
                let log_path = project_root
                    .join(".aida")
                    .join("headless-logs")
                    .join(format!("{branch}-{id}.jsonl"));
                eprintln!(
                    "{} {}",
                    crate::glyph(crate::glyphs::Glyph::FlowActive)
                        .green()
                        .bold(),
                    format!(
                        "launching {} headless reviewer in {} ({}, {})",
                        launch_vendor.as_str(),
                        worktree.display(),
                        match launch_vendor {
                            session::HeadlessVendor::Claude => "claude -p",
                            session::HeadlessVendor::Codex => "codex exec",
                            session::HeadlessVendor::Agy => "agy -p",
                        },
                        claude_posture_display(permission_mode, contained)
                    )
                    .cyan()
                );
                eprintln!(
                    "  {} headless output → {}",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    log_path.display().to_string().dimmed()
                );
                // TASK-307: tee high-signal events for the standalone
                // reviewer too. Label = "reviewer" so a future concurrent
                // batch can disambiguate `│ [headless:reviewer]` lines.
                // trace:TASK-307 | ai:claude
                let tee_opts =
                    headless_tee::TeeOptions::from_env_and_flag(false).with_label("reviewer");
                let (status, grant) = session::spawn_vendor_headless_with_seat_granted(
                    host_main,
                    launch_vendor,
                    aida_core::agents_config::AgentSeat::Reviewer,
                    prompt,
                    &id,
                    &log_path,
                    &tee_opts,
                    contained,
                )?;
                (status, Some(log_path), Some(grant))
            } else {
                let name = session::derive_session_name(scope, branch, role);
                // BUG-1607: build the launch plan through the ONE shared
                // resolver instead of hardcoding `claude` — this was the
                // actual bug: the resolver upstream correctly printed
                // `vendor: codex`, but this arm always spawned `claude`
                // regardless, failing after the lease + worktree already
                // existed. trace:BUG-1607 | ai:claude
                let plan = session::interactive_reviewer_launch_plan(
                    host_main,
                    launch_vendor,
                    permission_mode,
                    name.as_deref(),
                    prompt,
                    &id,
                    contained,
                )?;
                let posture = match launch_vendor {
                    session::HeadlessVendor::Claude => {
                        claude_posture_display(permission_mode, contained)
                    }
                    session::HeadlessVendor::Codex | session::HeadlessVendor::Agy => {
                        if permission_mode == Some("bypassPermissions") {
                            "bypass".to_string()
                        } else {
                            "native".to_string()
                        }
                    }
                };
                eprintln!(
                    "{} {}",
                    crate::glyph(crate::glyphs::Glyph::FlowActive)
                        .green()
                        .bold(),
                    format!(
                        "launching {} reviewer in {} ({}, prompt `{}`)",
                        launch_vendor.as_str(),
                        worktree.display(),
                        posture,
                        prompt
                    )
                    .cyan()
                );
                let (status, grant) =
                    session::spawn_reviewer_launch_plan_granted(&plan, project_root)?;
                (status, None, Some(grant))
            }
        }
    };

    // TASK-1460: the skill wrote the verdict file with a heredoc, outside
    // the record path — archive its round by reviewed commit the same way a
    // recorded verdict is. Best-effort; the current file is left as written.
    // trace:TASK-1460 | ai:claude
    let _ = crate::gate::review_verdict::adopt_direct_write(verdict_path);

    // BUG-2180: a contained reviewer's request-changes shelve (and any other
    // store write it made) is left uncommitted for its host — this launcher.
    // Commit it now, or the mandate's fail-closed store snapshot refuses
    // every ship until someone commits by hand. trace:BUG-2180 | ai:claude
    store_commit.commit();

    let pr_spec_ids: Vec<String> = fetch_pr_ship_metadata_via_forge(project_root, pr)
        // trace:BUG-2515 | ai:codex
        // Standalone PR reviewers can lack local forge configuration (for
        // example in a scratch launch root). Preserve their GitHub recovery
        // lookup without redirecting a configured GitLab MR to GitHub.
        .or_else(|err| {
            if crate::forge::resolve_forge_kind(project_root) != crate::forge::ForgeKind::None {
                return Err(err);
            }
            crate::forge::forge_for_kind(project_root, crate::forge::ForgeKind::GitHub)
                .change_title_and_body(pr)
                .map(|(title, body)| crate::pr_cmd::PrShipMetadata {
                    title,
                    body,
                    number: Some(pr),
                })
        })
        .map(|meta| review_calibration_spec_ids(project_root, &meta.title, branch, &meta.body))
        .unwrap_or_default();

    // BUG-2149: a contained reviewer can only write inside its review
    // worktree, so its `aida review record` lands there and never reaches
    // the primary side. Collect it onto the PR's gate ledger, where `aida
    // review verdict` and `aida pr ship` read: the launcher is the ledger's
    // only writer for a contained seat. trace:BUG-2149 | ai:claude
    // trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
    let head_sha = standalone_review_pr_head(project_root, pr);
    // trace:BUG-2187 | ai:claude
    let record_template =
        reviewer_summary::review_record_template(&pr_spec_ids, pr, head_sha.as_deref());
    let checks = StandaloneVerdictChecks {
        project_root,
        pr,
        head_sha,
        launcher_grant,
        child_grant,
    };
    let collected = crate::gate::review_verdict::collect_standalone_verdict(
        project_root,
        worktree,
        verdict_path,
        &pr_spec_ids,
        launched_at,
        &checks,
    );
    match &collected {
        Ok(c) if c.collected_from_worktree && !quiet => eprintln!(
            "  {} collected the reviewer's verdict from {} onto the PR-{pr} gate ledger",
            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
            worktree.display(),
        ),
        Ok(_) => {}
        Err(why) => eprintln!(
            "  {} no verdict collected for PR-{pr}: {why}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
        ),
    }

    // BUG-2176 / M1 slice 2: a contained reviewer cannot write hold state,
    // so its refusal's Rework hold is armed here, primary-side, from the
    // verdict just collected onto the PR's gate ledger. A hold only tightens.
    // trace:BUG-2176 | ai:claude
    // trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
    // A legacy marker a pre-ledger reviewer left is collected too.
    if let Err(err) =
        crate::merge_hold::adopt_worktree_hold(project_root, worktree, pr, launched_at)
    {
        eprintln!(
            "  {} could not collect the legacy merge-hold marker for PR-{pr} from {} ({err})",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
            worktree.display()
        );
    }
    if let Ok(c) = &collected {
        match crate::merge_hold::arm_collected_refusal(project_root, pr, c) {
            Ok(true) if !quiet => eprintln!(
                "  {} armed PR-{pr}'s merge hold for the reviewer's refusal",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
            ),
            Ok(_) => {}
            Err(err) => eprintln!(
                "  {} could not arm PR-{pr}'s merge hold for the reviewer's refusal ({err}) — run `aida merge-hold add {pr}` before shipping",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
            ),
        }
    }

    // The reviewer has exited and its verdict is collected (or known to be
    // missing): its finished review is reaped now, before any exit path
    // below. A launcher still sitting in the tree steps out of it first; any
    // other cwd is left alone (it is process-wide state).
    // trace:BUG-b6b1278890774ef993d2ebc327862470-024 | ai:claude
    if std::env::current_dir().is_ok_and(|cwd| {
        cwd.starts_with(worktree)
            || worktree
                .canonicalize()
                .is_ok_and(|tree| cwd.starts_with(tree))
    }) {
        let _ = std::env::set_current_dir(project_root);
    }
    for (id, outcome) in end_finished_review(project_root, scope, worktree, collected.is_ok()) {
        let id = &id[..id.len().min(8)];
        match outcome {
            Ok(_) if !quiet => eprintln!(
                "  {} ended reviewer session {id} for PR-{pr} and removed its review tree",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
            ),
            Err(why) if !quiet => eprintln!(
                "  {} kept reviewer session {id} for PR-{pr}: {why}",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
            ),
            _ => {}
        }
    }

    // STORY-439: tag-along reviewer-side calibration capture. Resolve
    // every spec the PR credits (via the existing title / branch / body
    // precedence used by the squash-subject repair) and write a review
    // slot per spec. Best-effort; never blocks the summary print.
    // trace:STORY-439 | ai:claude
    for spec in &pr_spec_ids {
        capture_review_calibration_for_spec(project_root, verdict_path, spec);
    }

    // End-of-command summary, assembled from the verdict file the
    // `/aida-review` skill wrote and (headless only) the JSONL log.
    // `--quiet` suppresses it for scripted consumers that read the
    // verdict file directly. trace:BUG-226 | ai:claude
    // Failure evidence is available in quiet mode too. It is diagnostic only;
    // the collector above remains the sole acceptance decision.
    // trace:BUG-2187 | ai:codex
    let log_body = if !quiet || collected.is_err() {
        log_path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
    } else {
        None
    };
    let result_event = log_body
        .as_deref()
        .and_then(reviewer_summary::parse_result_event);
    let recovery = collected.as_ref().err().map(|_| {
        reviewer_summary::unrecorded_review_recovery(
            worktree,
            log_path.as_deref(),
            log_body.as_deref(),
            &record_template,
        )
    });
    if !quiet {
        // A collected verdict is on the ledger only: the file may be absent,
        // or a previous round's, so the newer of the two is this review's.
        // trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
        let verdict_json = standalone_summary_verdict_body(collected.is_ok(), verdict_path);
        let summary = reviewer_summary::format_reviewer_summary(
            pr,
            verdict_json.as_deref(),
            result_event.as_ref(),
            verdict_path,
            log_path.as_deref(),
            status.code(),
        );
        println!();
        println!("{summary}");
    }

    // BUG-2201: a reviewer stopped by the permission layer is named as such.
    // trace:BUG-2201 | ai:claude
    let denials = log_body
        .as_deref()
        .map(reviewer_summary::parse_permission_denials)
        .unwrap_or_default();
    let outcome = standalone_reviewer_outcome(pr, collected.err(), verdict_path, &denials)
        .map_err(|err| format!("{err:#}\n{}", recovery.as_deref().unwrap_or_default()));
    // Propagate claude's exit code so a scripted consumer can branch on
    // it; a clean exit returns `Ok(())` so `main` records telemetry.
    // A fresh trusted verdict can settle incomplete AGY execution evidence;
    // actual process/terminal failure still propagates. trace:TASK-2838 | ai:codex
    if let Some(code) = reviewer_vendor_exit_code(launch_vendor, status, outcome.is_ok()) {
        // No verdict: return the vendor's code with the recovery text, so
        // the draft reaches the caller like any other error.
        // trace:BUG-2187 | ai:claude
        if let Err(message) = outcome {
            return Err(anyhow::Error::new(ReviewerVendorExit { code, message }));
        }
        std::process::exit(code);
    }
    outcome.map_err(|message| anyhow::Error::new(NoReviewVerdict { message }))
}

// trace:TASK-2838 | ai:codex
pub(crate) fn reviewer_vendor_exit_code(
    vendor: session::HeadlessVendor,
    status: std::process::ExitStatus,
    collected_verdict: bool,
) -> Option<i32> {
    status.code().filter(|code| {
        *code != 0
            && !(collected_verdict
                && crate::agy_outcome::reviewer_may_check_verdict(vendor, status))
    })
}

/// The verdict body the standalone summary shows: this run's collected
/// verdict, or none. A failed collection never falls back to the file or the
/// ledger, whose newest record is a previous round's (another review's PASS
/// printed under a review that recorded nothing).
// trace:BUG-b6b1278890774ef993d2ebc327862470-045 | ai:claude
pub(crate) fn standalone_summary_verdict_body(
    collected: bool,
    verdict_path: &std::path::Path,
) -> Option<String> {
    collected
        .then(|| crate::gate::review_verdict::current_verdict_body_at(verdict_path))
        .flatten()
}

/// Refuse a contained standalone reviewer (by flag or inherited
/// `AIDA_CONTAINED`) whose grant key this launcher could not hold in a
/// session keyring: its sandboxed `aida review record` could never present
/// the grant, so the review would be spent for a verdict that is refused.
// trace:BUG-b6b1278890774ef993d2ebc327862470-045 | ai:claude
pub(crate) fn refuse_unpresentable_contained_reviewer(contained: bool) -> Result<()> {
    if contained || session::aida_contained_env()? {
        crate::gate::seat_authority::preflight_sandboxed_child_key("reviewer")?;
    }
    Ok(())
}

/// Whether a failed pickup should print the manual-continuation hint. A
/// finished standalone run (implementer outcome, or a reviewer that recorded
/// no verdict) is a result, not a setup to continue by hand.
// trace:STORY-1662 | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-045 | ai:claude
pub(crate) fn pickup_failure_offers_manual_continuation(err: &anyhow::Error) -> bool {
    err.downcast_ref::<crate::standalone_implementer::StandaloneImplementerExit>()
        .is_none()
        && err.downcast_ref::<NoReviewVerdict>().is_none()
        && err.downcast_ref::<ReviewerVendorExit>().is_none()
}

/// BUG-2187: a reviewer that exited non-zero without a collectable verdict.
/// `exit_code_for_error` maps it to the vendor's own `code`; the message
/// carries the unrecorded-review recovery text.
// trace:BUG-2187 | ai:claude
#[derive(Debug)]
pub(crate) struct ReviewerVendorExit {
    pub(crate) code: i32,
    pub(crate) message: String,
}

impl std::fmt::Display for ReviewerVendorExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ReviewerVendorExit {}

/// BUG-2149: the launch command's result once the reviewer exited cleanly. A
/// review that produced no verifiable verdict is a failure — a caller waiting
/// on the exit status must never read a missing verdict as an approval.
///
/// BUG-2201: `denials` are the tool calls the reviewer's permission layer
/// refused (see [`reviewer_summary::parse_permission_denials`]); when any
/// exist the message names them, since a denied `aida review record` or
/// verdict write is the usual reason no verdict exists.
// trace:BUG-2149 | ai:claude
// trace:BUG-2201 | ai:claude
pub(crate) fn standalone_reviewer_outcome(
    pr: u64,
    rejection: Option<String>,
    verdict_path: &std::path::Path,
    denials: &[String],
) -> Result<()> {
    let Some(why) = rejection else {
        return Ok(());
    };
    // A run with dozens of denials must not produce a screen-long error
    // line: name the first few and count the rest. trace:TASK-1955 | ai:claude (TASK-1674)
    const SHOWN_DENIALS: usize = 5;
    let denied = if denials.is_empty() {
        String::new()
    } else {
        let mut listed = denials
            .iter()
            .take(SHOWN_DENIALS)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("; ");
        if denials.len() > SHOWN_DENIALS {
            listed.push_str(&format!("; …{} more", denials.len() - SHOWN_DENIALS));
        }
        format!(
            " The permission layer denied {} of the reviewer's tool call(s): {listed}.",
            denials.len(),
        )
    };
    Err(anyhow::Error::new(NoReviewVerdict {
        message: format!(
            "the reviewer for PR-{pr} exited without recording a verifiable verdict ({why}); \
             nothing was collected into {}.{denied} Treat this as no approval and re-run the review.",
            verdict_path.display()
        ),
    }))
}

/// BUG-2149: exit code of a standalone review that ended without a verified
/// verdict — distinct from 1 (a crash or any other failure) so a caller can
/// tell "no approval" from "the launch broke".
// trace:BUG-2149 | ai:claude
pub(crate) const NO_REVIEW_VERDICT_EXIT_CODE: i32 = 30;

/// BUG-2149: the error a standalone review with no verified verdict returns;
/// `exit_code_for_error` maps it to [`NO_REVIEW_VERDICT_EXIT_CODE`].
// trace:BUG-2149 | ai:claude
#[derive(Debug)]
pub(crate) struct NoReviewVerdict {
    pub(crate) message: String,
}

impl std::fmt::Display for NoReviewVerdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for NoReviewVerdict {}

/// BUG-2149: the reviewer's start as the FILESYSTEM sees it — the mtime of a
/// marker written just before the spawn, so a verdict file's mtime compares
/// on the same (possibly coarse) clock. Falls back to the wall clock.
// trace:BUG-2149 | ai:claude
pub(crate) fn reviewer_spawn_instant(worktree: &std::path::Path) -> std::time::SystemTime {
    let dir = worktree.join(".aida");
    let marker = dir.join(format!(".review-spawn-{}", std::process::id()));
    let stamped = std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(&marker, b""))
        .and_then(|()| std::fs::metadata(&marker))
        .and_then(|m| m.modified());
    let _ = std::fs::remove_file(&marker);
    stamped.unwrap_or_else(|_| std::time::SystemTime::now())
}

/// BUG-2149: PR-N's head commit at collection time, read from the pinned
/// forge repo. `None` when it cannot be resolved — which refuses collection.
// trace:BUG-2149 | ai:claude
pub(crate) fn standalone_review_pr_head(project_root: &std::path::Path, pr: u64) -> Option<String> {
    #[cfg(test)]
    if let Ok(sha) = std::env::var("AIDA_TEST_PR_HEAD_SHA") {
        return Some(sha);
    }
    merge_hold::fetch_pinned_change(
        project_root,
        crate::forge::resolve_open_change_forge_kind(project_root),
        pr,
    )
    .ok()
    .flatten()
    .and_then(|change| change.head_sha)
}

/// BUG-2149: what a collected standalone verdict must satisfy: a recognized
/// verdict for THIS PR at its CURRENT head, recorded by the child reviewer
/// grant this launch issued — not the launcher's grant, not an implementer's.
// trace:BUG-2149 | ai:claude
pub(crate) struct StandaloneVerdictChecks<'a> {
    pub(crate) project_root: &'a std::path::Path,
    pub(crate) pr: u64,
    pub(crate) head_sha: Option<String>,
    pub(crate) launcher_grant: Option<String>,
    /// The grant this launch issued to its reviewer child — the ONLY grant a
    /// collected verdict may be recorded under.
    pub(crate) child_grant: Option<String>,
}

impl crate::gate::review_verdict::VerdictCollectionChecks for StandaloneVerdictChecks<'_> {
    fn check_record(
        &self,
        c: &crate::gate::review_verdict::VerdictCandidate,
    ) -> Result<(), String> {
        if c.verdict.kind == crate::gate::review_verdict::VerdictKind::Unknown {
            return Err("not a recognized verdict record".to_string());
        }
        if let Some(pr) = c.obj.get("pr") {
            let n = pr
                .as_u64()
                .or_else(|| pr.as_str().and_then(|s| s.trim().parse().ok()));
            if n != Some(self.pr) {
                return Err(format!("it names PR {pr}, not PR-{}", self.pr));
            }
        }
        let Some(head) = self.head_sha.as_deref() else {
            return Err(format!(
                "PR-{}'s current head could not be resolved",
                self.pr
            ));
        };
        match c.verdict.reviewed_sha.as_deref() {
            Some(sha) if crate::gate::review_verdict::same_reviewed_sha(sha, head) => Ok(()),
            Some(sha) => Err(format!(
                "it reviewed {sha}, but PR-{}'s head is now {head}",
                self.pr
            )),
            None => Err("it names no reviewed commit".to_string()),
        }
    }

    fn check_identity(&self, grant_id: &str) -> Result<(), String> {
        if self.launcher_grant.as_deref() == Some(grant_id) {
            return Err("it was recorded under the launcher's own grant".to_string());
        }
        let Some(child) = self.child_grant.as_deref() else {
            return Err("this launch issued no reviewer grant to verify against".to_string());
        };
        if grant_id != child {
            return Err(format!(
                "it was recorded under grant {grant_id}, not this launch's reviewer grant"
            ));
        }
        match crate::gate::seat_authority::grant_check_for(
            self.project_root,
            grant_id,
            &crate::current_user_id(None),
        ) {
            crate::gate::seat_authority::GrantCheck::Valid(g) if g.seat == "reviewer" => Ok(()),
            crate::gate::seat_authority::GrantCheck::Valid(g) => {
                Err(format!("it was recorded by a `{}` seat", g.seat))
            }
            _ => Err(format!("its recording grant {grant_id} is not valid")),
        }
    }
}

// ===========================================================================
// STORY-246: `aida queue work --auto-complete` — full lifecycle orchestrator.
// The phase-sequencing logic lives in `auto_complete.rs`; this is the real
// `PhaseDriver` — it spawns Claude sessions, polls CI, and shells out to
// `gh` / `aida` / `cargo`. trace:STORY-246 | ai:claude
// ===========================================================================

/// Partial view of a `SessionLease` TOML — just the fields the orchestrator
/// needs to discover after spawning a session. trace:STORY-246 | ai:claude
#[derive(serde::Deserialize)]
pub(crate) struct LeasePeek {
    // trace:BUG-2425 | ai:codex
    #[serde(default)]
    pub(crate) pid_namespace: Option<u64>,
    pub(crate) id: String,
    pub(crate) branch: String,
    /// Optional for older leases and minimal test fixtures. Used only by the
    /// phase watchdog's local descendant scan.
    // trace:BUG-749 | ai:codex
    #[serde(default)]
    pub(crate) creator_pid: Option<u32>,
    /// PID of the hosted agent child for process-backed headless launches.
    /// Preferred by vendor-neutral liveness; older leases omit it.
    // trace:STORY-1054 | ai:codex
    #[serde(default)]
    pub(crate) active_pid: Option<u32>,
    /// Kernel start times recorded beside the pids, so a liveness check can
    /// reject a recycled pid. Absent on older leases.
    // trace:TASK-1750 | ai:claude
    #[serde(default)]
    pub(crate) creator_pid_start_time: Option<String>,
    #[serde(default)]
    pub(crate) active_pid_start_time: Option<String>,
    /// Worktree path — re-read by the BUG-223 branch-swap reconciliation to
    /// recover the live branch when `/aida-pr` swapped it mid-phase.
    /// `#[serde(default)]` so a lease (or hand-written test fixture) without
    /// the field still parses. trace:BUG-223 | ai:claude
    #[serde(default)]
    pub(crate) worktree_path: std::path::PathBuf,
}

/// Statuses the `--auto-complete` orchestrator can drive from scratch
/// (TASK-292). The orchestrator runs a full implementer → CI → reviewer →
/// merge lifecycle starting at phase 1, so it can only begin a spec that the
/// advisor has disposed into the pipeline: Approved / Planned. A queued Draft
/// is still undisposed and must never become implicit approval in a batch.
/// In Progress and Done are mid-flight; Completed and Rejected are terminal.
// trace:TASK-292 trace:STORY-1353 | ai:codex
pub(crate) fn auto_complete_head_drivable(status: &RequirementStatus) -> bool {
    matches!(
        status,
        RequirementStatus::Approved | RequirementStatus::Planned
    )
}

/// Pure pickup-order resolution for the `aida queue work --auto-complete` head
/// (TASK-292): given queued `(display_id, status)` pairs already in pickup
/// order, return the first orchestrator-drivable spec together with the items
/// skipped to reach it, or `Err(skipped)` when none is drivable. Split out of
/// [`resolve_auto_complete_head`] so the skip / empty-queue logic is
/// unit-testable without a storage fixture. trace:TASK-292 | ai:claude
#[allow(clippy::type_complexity)]
pub(crate) fn pick_auto_complete_head(
    candidates: &[(String, RequirementStatus)],
) -> std::result::Result<(String, Vec<(String, RequirementStatus)>), Vec<(String, RequirementStatus)>>
{
    let mut skipped: Vec<(String, RequirementStatus)> = Vec::new();
    for (id, status) in candidates {
        if auto_complete_head_drivable(status) {
            return Ok((id.clone(), skipped));
        }
        skipped.push((id.clone(), status.clone()));
    }
    Err(skipped)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AutoCompleteHeadCandidate {
    pub(crate) id: String,
    pub(crate) status: RequirementStatus,
    pub(crate) for_role: Option<String>,
    pub(crate) deferred: bool,
    pub(crate) deferred_reason: Option<String>,
    pub(crate) deferred_until: Option<String>,
    pub(crate) execution_mode: Option<aida_core::ExecutionMode>,
    pub(crate) tags: std::collections::HashSet<String>,
    /// BUG-1608: the STORY-333 pickability verdict, resolved against the
    /// store at candidate-build time. `Some(label)` (e.g. `blocked-by STORY-52
    /// (Needs Attention)`) means the head picker must skip it; `None` means
    /// pickable. Built fresh on every head resolution, so a blocker shelved
    /// earlier in the same drain is seen.
    // trace:BUG-1608 | ai:claude
    pub(crate) blocked: Option<String>,
}

impl AutoCompleteHeadCandidate {
    // trace:STORY-1125 | ai:codex
    pub(crate) fn is_release_task(&self) -> bool {
        self.tags
            .iter()
            .flat_map(|tag| tag.split_whitespace())
            .any(crate::presence::is_release_operator_tag)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AutoCompleteHeadPick {
    pub(crate) spec: String,
    pub(crate) status_skipped: Vec<(String, RequirementStatus)>,
    pub(crate) role_skipped: Vec<(String, String)>,
    pub(crate) deferred_skipped: Vec<String>,
    pub(crate) guided_or_operator_skipped: Vec<(String, aida_core::ExecutionMode)>,
    pub(crate) release_skipped: Vec<String>,
    /// BUG-1608: drivable-status candidates skipped because the pickability
    /// gate refused them — `(id, reason label)`, e.g. an unmet `BlockedBy`.
    pub(crate) blocked_skipped: Vec<(String, String)>,
}

/// The auto-complete engine always starts with a phase-1 implementer unless
/// the caller explicitly asked for another role. Treat an absent shell role as
/// implementer so reviewer-routed queue rows are not accidentally driven by an
/// implementer lifecycle.
// trace:BUG-862 | ai:codex
pub(crate) fn effective_auto_complete_role(role_override: Option<&str>) -> String {
    role_override
        .filter(|s| !s.is_empty())
        .map(canonical_role_name)
        .or_else(|| {
            std::env::var("AIDA_SESSION_ROLE")
                .ok()
                .filter(|s| !s.is_empty())
                .map(|s| canonical_role_name(&s))
                // Dispatch seats (advisor, human) LAUNCH drains for the doers
                // — an advisor shell's drain works the implementer lane, the
                // same principle as the dispatch-seat queue-add default above.
                // Without this, AIDA_SESSION_ROLE=advisor makes the drain
                // skip reviewer- AND implementer-routed entries and report
                // "no drivable queued items". An explicit --role still wins.
                // trace:BUG-862 | ai:claude
                .map(|role| match role.as_str() {
                    "advisor" | "human" => "implementer".to_string(),
                    _ => role,
                })
        })
        .unwrap_or_else(|| "implementer".to_string())
}

/// Role-aware head selection for auto-complete drains. Entries explicitly
/// routed to a different role are skipped before status drivability is tested,
/// because the phase-1 lifecycle this command launches would be the wrong
/// session type for them. Unrouted entries remain drivable by the effective
/// role for backwards compatibility.
// trace:BUG-862 | ai:codex
// trace:BUG-1793 | ai:antigravity
pub(crate) fn pick_auto_complete_head_for_role(
    candidates: &[AutoCompleteHeadCandidate],
    effective_role: &str,
) -> anyhow::Result<Option<AutoCompleteHeadPick>> {
    let effective_role = canonical_role_name(effective_role);
    let mut status_skipped = Vec::new();
    let mut role_skipped = Vec::new();
    let mut deferred_skipped = Vec::new();
    let mut guided_or_operator_skipped = Vec::new();
    let mut release_skipped = Vec::new();
    let mut blocked_skipped = Vec::new();
    for candidate in candidates {
        if let Some(for_role) = candidate.for_role.as_deref() {
            let routed = canonical_role_name(for_role);
            if routed != effective_role {
                role_skipped.push((candidate.id.clone(), routed));
                continue;
            }
        }
        if let Some(refusal) = crate::gate::operator_hold::operator_hold_refusal(
            &candidate.id,
            candidate.deferred,
            candidate.deferred_reason.as_deref(),
            candidate.deferred_until.as_deref(),
        ) {
            return Err(refusal);
        }
        if candidate.deferred {
            deferred_skipped.push(candidate.id.clone());
            continue;
        }
        if candidate.is_release_task() {
            release_skipped.push(candidate.id.clone());
            continue;
        }
        // trace:BUG-1120 | ai:codex
        if let Some(
            mode @ (aida_core::ExecutionMode::Guided
            | aida_core::ExecutionMode::Operator
            | aida_core::ExecutionMode::Decide),
        ) = candidate.execution_mode
        {
            guided_or_operator_skipped.push((candidate.id.clone(), mode));
            continue;
        }
        if auto_complete_head_drivable(&candidate.status) {
            // BUG-1608: an Approved/Planned status is necessary but not
            // sufficient — a `BlockedBy` dependent stays unpickable until its
            // prerequisite is Completed (Done, NeedsAttention, a pushed branch
            // do not count). The queue-wide drain previously checked status
            // only, so it launched NFR-56 right after its blocker STORY-52
            // shelved. trace:BUG-1608 | ai:claude
            if let Some(reason) = &candidate.blocked {
                blocked_skipped.push((candidate.id.clone(), reason.clone()));
                continue;
            }
            return Ok(Some(AutoCompleteHeadPick {
                spec: candidate.id.clone(),
                status_skipped,
                role_skipped,
                deferred_skipped,
                guided_or_operator_skipped,
                release_skipped,
                blocked_skipped,
            }));
        }
        status_skipped.push((candidate.id.clone(), candidate.status.clone()));
    }
    Ok(None)
}

pub(crate) fn auto_complete_head_candidates_with_roles(
    storage: &Storage,
    user_id: &str,
    fallback_role: Option<&str>,
) -> Result<Vec<AutoCompleteHeadCandidate>> {
    let entries = queue_role_fallback::queue_list_with_role_fallback(
        storage,
        user_id,
        fallback_role,
        /* include_completed */ false,
    )?;
    let store = storage.load()?;
    let mut ordered: Vec<&aida_core::QueueEntry> = entries.iter().collect();
    ordered.sort_by_key(|e| e.position);

    Ok(ordered
        .iter()
        .filter_map(|e| {
            store
                .requirements
                .iter()
                .find(|r| r.id == e.requirement_id)
                .map(|r| AutoCompleteHeadCandidate {
                    id: r.display_id(),
                    status: r.status.clone(),
                    for_role: e.for_role.clone(),
                    deferred: r.deferred,
                    deferred_reason: r.deferred_reason.clone(),
                    deferred_until: r.deferred_until.clone(),
                    execution_mode: r.execution_mode,
                    tags: r.tags.clone(),
                    // BUG-1608: PRIN-5 fail closed — `pickability` treats a
                    // dangling or non-Completed `BlockedBy` target as blocked,
                    // so unknown dependency state never reads as pickable.
                    // trace:BUG-1608 | ai:claude
                    blocked: match aida_core::pickability::pickability(r, &store) {
                        aida_core::pickability::Pickability::Pickable => None,
                        aida_core::pickability::Pickability::Blocked(reason) => {
                            Some(aida_core::pickability::pickability_reason_label(&reason))
                        }
                    },
                })
        })
        .collect())
}

/// Build the `(display_id, status, blocked)` candidate list for the active
/// role's queue in pickup order (queue position ascending, same as `aida
/// queue next`) — the shared input to [`auto_complete_head_candidates`]
/// (status only, for [`pick_auto_complete_head`]) and, since TASK-1490, to
/// the drain preview and drain-state member list, both of which need the
/// carried BUG-1608 pickability verdict rather than the status alone.
// trace:TASK-292 TASK-293 TASK-1490 | ai:claude
pub(crate) fn auto_complete_head_candidates_with_blocked(
    storage: &Storage,
    user_id: &str,
    role_override: Option<&str>,
) -> Result<Vec<(String, RequirementStatus, Option<String>)>> {
    let effective_role = effective_auto_complete_role(role_override);
    let mut out = Vec::new();
    for candidate in
        auto_complete_head_candidates_with_roles(storage, user_id, Some(&effective_role))?
    {
        if matches!(
            candidate.execution_mode,
            Some(
                aida_core::ExecutionMode::Guided
                    | aida_core::ExecutionMode::Operator
                    | aida_core::ExecutionMode::Decide
            )
        ) {
            continue;
        }
        if candidate.is_release_task() {
            continue;
        }
        if let Some(for_role) = candidate.for_role.as_deref() {
            if canonical_role_name(for_role) != effective_role {
                continue;
            }
        }
        if let Some(refusal) = crate::gate::operator_hold::operator_hold_refusal(
            &candidate.id,
            candidate.deferred,
            candidate.deferred_reason.as_deref(),
            candidate.deferred_until.as_deref(),
        ) {
            return Err(refusal);
        }
        if candidate.deferred {
            continue;
        }
        out.push((candidate.id, candidate.status, candidate.blocked));
    }
    Ok(out)
}

/// Build the `(display_id, status)` candidate list for the active role's
/// queue in pickup order (queue position ascending, same as `aida queue
/// next`) — the shared input to [`pick_auto_complete_head`] for both
/// single-head pickup (TASK-292) and the `nextN` drain (TASK-293).
///
/// This view drops the BUG-1608 pickability verdict; a caller that must
/// distinguish a blocked dependent from a merely undrivable status (the
/// drain preview and drain-state member list, TASK-1490) should call
/// [`auto_complete_head_candidates_with_blocked`] instead.
/// trace:TASK-292 TASK-293 | ai:claude
pub(crate) fn auto_complete_head_candidates(
    storage: &Storage,
    user_id: &str,
    role_override: Option<&str>,
) -> Result<Vec<(String, RequirementStatus)>> {
    Ok(
        auto_complete_head_candidates_with_blocked(storage, user_id, role_override)?
            .into_iter()
            .map(|(id, status, _blocked)| (id, status))
            .collect(),
    )
}

/// Resolve the queue head for `aida queue work --auto-complete` invoked with no
/// positional SPEC id (TASK-292) — the natural composition of the no-arg "pick
/// the head" semantics with `--auto-complete`. Walks the active role's queue in
/// pickup order (queue position ascending, same as `aida queue next`) and
/// returns the first item the orchestrator can drive from scratch, skipping —
/// with a note — any item already In Progress / Done / terminal. Errors when
/// nothing drivable remains. trace:TASK-292 | ai:claude
pub(crate) fn resolve_auto_complete_head(
    storage: &Storage,
    user_id: &str,
    role_override: Option<&str>,
) -> Result<String> {
    let role_label = effective_auto_complete_role(role_override);
    let candidates = auto_complete_head_candidates_with_roles(storage, user_id, Some(&role_label))?;

    match pick_auto_complete_head_for_role(&candidates, &role_label)? {
        Some(pick) => {
            for (id, routed) in &pick.role_skipped {
                eprintln!("skipped {id} — routed for {routed}");
            }
            // trace:BUG-1608 | ai:claude
            for (id, reason) in &pick.blocked_skipped {
                eprintln!("skipped {id} — {reason}");
            }
            for id in &pick.deferred_skipped {
                eprintln!("skipped {id} — deferred — skipped");
            }
            for id in &pick.release_skipped {
                eprintln!(
                    "skipped {id} — release task requires operator-guided confirmation; headless auto-complete will not drive it"
                );
            }
            for (id, mode) in &pick.guided_or_operator_skipped {
                eprintln!(
                    "skipped {id} — needs guided/operator session ({mode}); use `aida queue work {id} --guided`, `aida do {id}`, or de-risk it with `aida derisk {id}`"
                );
            }
            // Acceptance criterion: name each item skipped to reach the
            // drivable head so the pickup is never silently surprising.
            for (id, status) in &pick.status_skipped {
                eprintln!(
                    "  {} skipping {} ({}) — the orchestrator drives a full \
                     lifecycle from scratch and cannot resume it",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    id,
                    status
                );
            }
            Ok(pick.spec)
        }
        None if candidates.is_empty() => anyhow::bail!(
            "queue is empty for {role_label}; nothing to drive{}",
            auto_complete_sibling_role_hint(storage, user_id, &role_label).unwrap_or_default()
        ),
        None => {
            let skipped: Vec<(String, RequirementStatus)> = candidates
                .iter()
                .filter(|candidate| {
                    candidate
                        .for_role
                        .as_deref()
                        .map(|r| canonical_role_name(r) == role_label)
                        .unwrap_or(true)
                })
                .filter(|candidate| !candidate.deferred)
                .filter(|candidate| !candidate.is_release_task())
                .filter(|candidate| {
                    !matches!(
                        candidate.execution_mode,
                        Some(
                            aida_core::ExecutionMode::Guided
                                | aida_core::ExecutionMode::Operator
                                | aida_core::ExecutionMode::Decide
                        )
                    )
                })
                .map(|candidate| (candidate.id.clone(), candidate.status.clone()))
                .collect();
            let deferred_skipped: Vec<String> = candidates
                .iter()
                .filter(|candidate| {
                    candidate
                        .for_role
                        .as_deref()
                        .map(|r| canonical_role_name(r) == role_label)
                        .unwrap_or(true)
                })
                .filter(|candidate| candidate.deferred)
                .map(|candidate| candidate.id.clone())
                .collect();
            let role_skipped: Vec<(String, String)> = candidates
                .iter()
                .filter_map(|candidate| {
                    let routed = candidate.for_role.as_deref().map(canonical_role_name)?;
                    (routed != role_label).then(|| (candidate.id.clone(), routed))
                })
                .collect();
            for (id, routed) in &role_skipped {
                eprintln!("skipped {id} — routed for {routed}");
            }
            for id in &deferred_skipped {
                eprintln!("skipped {id} — deferred — skipped");
            }
            let release_skipped: Vec<String> = candidates
                .iter()
                .filter(|candidate| {
                    candidate
                        .for_role
                        .as_deref()
                        .map(|r| canonical_role_name(r) == role_label)
                        .unwrap_or(true)
                })
                .filter(|candidate| candidate.is_release_task())
                .map(|candidate| candidate.id.clone())
                .collect();
            for id in &release_skipped {
                eprintln!(
                    "skipped {id} — release task requires operator-guided confirmation; headless auto-complete will not drive it"
                );
            }
            let guided_or_operator_skipped: Vec<(String, aida_core::ExecutionMode)> = candidates
                .iter()
                .filter(|candidate| {
                    candidate
                        .for_role
                        .as_deref()
                        .map(|r| canonical_role_name(r) == role_label)
                        .unwrap_or(true)
                })
                .filter_map(|candidate| {
                    let mode = candidate.execution_mode?;
                    matches!(
                        mode,
                        aida_core::ExecutionMode::Guided
                            | aida_core::ExecutionMode::Operator
                            | aida_core::ExecutionMode::Decide
                    )
                    .then(|| (candidate.id.clone(), mode))
                })
                .collect();
            for (id, mode) in &guided_or_operator_skipped {
                eprintln!(
                    "skipped {id} — needs guided/operator session ({mode}); use `aida queue work {id} --guided`, `aida do {id}`, or de-risk it with `aida derisk {id}`"
                );
            }
            // BUG-1608: name every drivable-status item the dependency gate
            // held back, with its blocker, so the refusal is never silent.
            // trace:BUG-1608 | ai:claude
            for candidate in candidates.iter().filter(|c| {
                c.for_role
                    .as_deref()
                    .map(|r| canonical_role_name(r) == role_label)
                    .unwrap_or(true)
                    && auto_complete_head_drivable(&c.status)
            }) {
                if let Some(reason) = &candidate.blocked {
                    eprintln!("skipped {} — {reason}", candidate.id);
                }
            }
            // The queue has items, but every one is in-flight or terminal —
            // name the first few so it's clear *why* there's nothing to
            // drive, without dumping a long stale list.
            const SHOWN: usize = 5;
            let detail = skipped
                .iter()
                .take(SHOWN)
                .map(|(id, status)| format!("{id} ({status})"))
                .collect::<Vec<_>>()
                .join(", ");
            let more = skipped.len().saturating_sub(SHOWN);
            let suffix = if more > 0 {
                format!(", +{more} more")
            } else {
                String::new()
            };
            if skipped.is_empty() {
                let empty_reason = if deferred_skipped.is_empty()
                    && release_skipped.is_empty()
                    && guided_or_operator_skipped.is_empty()
                {
                    format!("queue is empty for {role_label}")
                } else {
                    format!("no drivable item in the queue for {role_label}")
                };
                anyhow::bail!(
                    "{empty_reason}; nothing to drive{}",
                    auto_complete_sibling_role_hint(storage, user_id, &role_label)
                        .unwrap_or_default()
                )
            } else {
                anyhow::bail!(
                    "no drivable item in the queue for {role_label}; nothing to drive — \
                     {} queued item{} in-flight, terminal, or blocked: {detail}{suffix}",
                    skipped.len(),
                    if skipped.len() == 1 { "" } else { "s" },
                )
            }
        }
    }
}

// BUG-839: read-only preview for `aida queue work --drain --dry-run`. The
// normal `--drain` alias rewrites into `--auto-complete` and used to enter the
// live orchestrator before the ordinary pickup dry-run path could run.
pub(crate) fn preview_queue_work_drain(
    storage: &Storage,
    user_id: &str,
    effective_id: Option<&str>,
    effective_batch: Option<&str>,
    effective_batches: Option<&[String]>,
    next_kw: NextKeyword,
    role_override: Option<&str>,
    max: Option<usize>,
) -> Result<()> {
    println!("Dry run — would drain, no side effects.");
    println!("  phases: implementer → CI → reviewer → merge → pull → build");

    if let Some(batch_names) = effective_batches {
        println!("  target: batches {}", batch_names.join(", "));
        for batch_name in batch_names {
            let members = resolve_batch_members(storage, user_id, batch_name, role_override)?;
            print_drain_preview_members(
                &format!("batch:{batch_name}"),
                members
                    .into_iter()
                    .map(|(_, id, title, status)| (id, title, status.to_string())),
                max,
            );
        }
        return Ok(());
    }

    if let Some(batch_name) = effective_batch {
        if batch_name.is_empty() {
            anyhow::bail!(
                "batch name is empty — pass `--batch NAME` or `aida queue work batch:NAME`"
            );
        }
        let members = resolve_batch_members(storage, user_id, batch_name, role_override)?;
        println!("  target: batch:{batch_name}");
        print_drain_preview_members(
            &format!("batch:{batch_name}"),
            members
                .into_iter()
                .map(|(_, id, title, status)| (id, title, status.to_string())),
            max,
        );
        return Ok(());
    }

    if let NextKeyword::Count(n) = next_kw {
        let limit = n.max(1);
        let (members, skipped) =
            drain_preview_head_members(storage, user_id, role_override, limit)?;
        println!("  target: next {limit}");
        print_drain_preview_members("queue head", members.into_iter(), Some(limit));
        print_drain_preview_skipped(&skipped);
        return Ok(());
    }

    if let Some(spec) = effective_id {
        let plan = resolve_queue_work_plan(
            storage,
            user_id,
            Some(spec),
            /* type_filter */ None,
            /* strict */ false,
            /* dry_run */ true,
            /* force_needs_attention */ false,
            /* role_hint */ None,
        )?;
        println!("  target: {}", plan.anchor_display);
        let status = plan
            .entries
            .first()
            .map(|entry| entry.status_at_plan.clone())
            .unwrap_or_else(|| "Approved".to_string());
        print_drain_preview_members(
            "explicit spec",
            std::iter::once((
                plan.anchor_display.clone(),
                plan.anchor_title.clone(),
                status,
            )),
            Some(1),
        );
        return Ok(());
    }

    let limit = max.unwrap_or(99).max(1);
    let (members, skipped) = drain_preview_head_members(storage, user_id, role_override, limit)?;
    println!("  target: queue head");
    print_drain_preview_members("queue head", members.into_iter(), Some(limit));
    print_drain_preview_skipped(&skipped);
    Ok(())
}

/// Drivable head-of-queue members for the drain preview, alongside the
/// dependents skipped to reach them. TASK-1490: applies the same
/// `aida_core::pickability::pickability()` verdict the real drain gates
/// dispatch on (BUG-1608), so a `BlockedBy` dependent that the drain would
/// skip is reported as skipped here too, rather than listed as a member the
/// preview implies would run.
// trace:TASK-1490 | ai:claude
pub(crate) fn drain_preview_head_members(
    storage: &Storage,
    user_id: &str,
    role_override: Option<&str>,
    limit: usize,
) -> Result<(Vec<(String, String, String)>, Vec<(String, String)>)> {
    let candidates = auto_complete_head_candidates_with_blocked(storage, user_id, role_override)?;
    let store = storage.load()?;
    let mut members = Vec::new();
    let mut skipped = Vec::new();
    for (id, status, blocked) in candidates {
        if !auto_complete_head_drivable(&status) {
            continue;
        }
        // trace:TASK-1490 | ai:claude
        if let Some(reason) = blocked {
            skipped.push((id, reason));
            continue;
        }
        let title = store
            .get_requirement_by_spec_id(&id)
            .map(|r| r.title.clone())
            .unwrap_or_default();
        members.push((id, title, status.to_string()));
        if members.len() >= limit {
            break;
        }
    }
    Ok((members, skipped))
}

pub(crate) fn print_drain_preview_members<I>(label: &str, members: I, max: Option<usize>)
where
    I: IntoIterator<Item = (String, String, String)>,
{
    let limit = max.unwrap_or(usize::MAX);
    let rows: Vec<(String, String, String)> = members.into_iter().take(limit).collect();
    if rows.is_empty() {
        println!("  plan: no drivable queued items for {label}");
        return;
    }
    println!(
        "  plan: {} item{} in drain order",
        rows.len(),
        if rows.len() == 1 { "" } else { "s" }
    );
    for (i, (id, title, status)) in rows.iter().enumerate() {
        let title_suffix = if title.is_empty() {
            String::new()
        } else {
            format!(" — {title}")
        };
        println!("    {:>2}. {} [{}]{}", i + 1, id, status, title_suffix);
    }
}

/// Report the `BlockedBy` dependents a drain preview skipped to reach its
/// member list — the same `(spec, reason)` shape and wording the real
/// drain's post-run summary uses for its skipped list (see
/// `emit_next_n_drain_summary`), so the preview never implies a blocked
/// dependent would run.
// trace:TASK-1490 | ai:claude
pub(crate) fn print_drain_preview_skipped(skipped: &[(String, String)]) {
    if skipped.is_empty() {
        return;
    }
    let kn = skipped.len();
    let render: Vec<String> = skipped
        .iter()
        .map(|(spec, reason)| format!("{spec} ({reason})"))
        .collect();
    println!(
        "  {kn} dependent spec{} skipped: {}",
        if kn == 1 { "" } else { "s" },
        render.join(", ")
    );
}

/// When an autonomous head pickup finds nothing for the selected role, name
/// sibling role queues that do have queued items so the operator gets a concrete
/// recovery command instead of a dead end.
// trace:BUG-795 | ai:codex
pub(crate) fn auto_complete_sibling_role_hint(
    storage: &Storage,
    user_id: &str,
    selected_role: &str,
) -> Result<String> {
    if selected_role == "any role" {
        return Ok(String::new());
    }
    let entries = storage.queue_list(user_id, /* include_completed */ false)?;
    let store = storage.load()?;
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for entry in entries {
        let Some(role) = entry.for_role.as_deref() else {
            continue;
        };
        let role = canonical_role_name(role);
        if role == selected_role {
            continue;
        }
        let Some(req) = store
            .requirements
            .iter()
            .find(|r| r.id == entry.requirement_id)
        else {
            continue;
        };
        if !matches!(
            queue_fresh_pickup_policy(req, &store, false, storage.path().parent()),
            QueueFreshPickup::Pickable
        ) {
            continue;
        }
        *counts.entry(role).or_default() += 1;
    }
    if counts.is_empty() {
        return Ok(String::new());
    }
    let summary = counts
        .iter()
        .map(|(role, count)| format!("{count} item(s) routed for:{role}"))
        .collect::<Vec<_>>()
        .join(", ");
    let command_role = counts
        .keys()
        .next()
        .cloned()
        .unwrap_or_else(|| selected_role.to_string());
    Ok(format!(
        " — {summary}; re-run with `aida queue work --auto-complete --role {command_role}` or re-route them"
    ))
}

// Keep the queue's reset boundary available to admission-history regressions.
// The watcher retains consumed canonical identities before reaching this point.
// trace:TASK-1993 | ai:codex
pub(crate) fn clear_standalone_reviewer_verdict(path: &std::path::Path) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Clear any stale verdict so the summary cannot read a prior run's.
    let _ = std::fs::remove_file(path);
}

// trace:TASK-3426 | ai:codex
pub(super) fn handle_queue_command(
    cmd: &QueueCommand,
    storage: &Storage,
    _store_path: &std::path::Path,
) -> Result<()> {
    let get_user = |user: &Option<String>| -> String { current_user_id(user.as_deref()) };
    match cmd {
        // STORY-42: one-shot queue pickup → session start → claude launch.
        // trace:STORY-42 | ai:claude
        QueueCommand::Work {
            id,
            count,
            permission_mode,
            sandbox,
            no_launch,
            plan_only,
            plan_phase,
            guided,
            with_plan,
            role,
            no_pull,
            type_filter,
            branch,
            path,
            stack,
            base,
            force_base,
            steal,
            force_claim,
            force,
            batch,
            batches,
            single_branch,
            sequential,
            dry_run,
            resume,
            fresh,
            list_sessions,
            session_id,
            vendor,
            model,
            auto_complete,
            drain,
            json,
            max,
            max_failures,
            max_tokens,
            max_iterations,
            max_runtime,
            no_progress_minutes,
            phase_ceiling_minutes,
            resume_drain,
            drain_id,
            resume_dry_run,
            require_head,
            from_pr,
            no_human,
            escalate_blocks,
            escalate_defaults,
            zen,
            pause_always,
            quiet,
            no_tee_headless,
            user,
            calibrate,
            no_calibrate,
            allow_stale_base,
            allow_intermediate_only,
            no_auto_rebase,
            complexity,
            assist_est,
            effort,
            panes,
            strict,
        } => {
            refuse_retired_steal(*steal)?;
            let user_id = get_user(user);
            // TASK-1120: opt-in pane hosting. Export the requested host so every
            // implementer this drain spawns (single / --batch / nextN) reads it
            // at its run_implementer spawn point. Faithful-launcher: an absent
            // flag leaves the env untouched → byte-identical background spawn.
            if let Some(host) = panes.as_deref() {
                std::env::set_var(pane_host::HOST_ENV, host);
            }
            // TASK-1116: an explicit `--vendor`/`--agent` ALSO routes the
            // headless `--auto-complete` implementer (and any headless
            // orchestrator phase), not just the interactive host — so it is no
            // longer a silent no-op on an autonomous drain. Installed as the
            // top-precedence override BEFORE the orchestrator spawns the
            // `aida queue work <spec> --no-human` implementer child, which
            // inherits the choice via `AIDA_HEADLESS_VENDOR`. A recognized
            // token (claude/codex) installs; an unrecognized value is left to
            // the interactive host path's existing handling below. trace:TASK-1116
            if let Some(raw) = vendor.as_deref() {
                let _ = session::install_headless_vendor_override(raw);
            }
            // STORY-1003: `--model` is a one-shot opaque override. Export it
            // before the auto-complete router so spawned phase children inherit
            // the same model choice; config-only models are read by each child
            // from the normal project config chain.
            if let Some(model) = model.as_deref().filter(|m| !m.trim().is_empty()) {
                std::env::set_var("AIDA_AGENT_MODEL", model);
            }
            // STORY-761: resolve the interactive host vendor — explicit
            // `--vendor` flag > the uniform `[agents] vendor` knob
            // (agents.toml, project over global) > `claude`. Shadowed so
            // everything downstream reads the resolved value.
            let vendor: String = vendor.clone().unwrap_or_else(|| {
                find_project_root()
                    .ok()
                    .and_then(|root| aida_core::agents_config::resolve_default_vendor(&root))
                    .unwrap_or_else(|| "claude".to_string())
            });
            // STORY-717: focus-scope drift guard at the queue-work work-start
            // moment. When an explicit spec `id` is named and the worktree has
            // a focus set, refuse/nudge if the spec is outside the focus
            // subtree per `[focus] out_of_scope`. `--force` always overrides.
            // The `next` / `nextN` / `batch:NAME` keywords don't resolve to a
            // spec, so the guard naturally no-ops for them. trace:STORY-717
            if let Some(spec_arg) = id.as_deref() {
                if let Ok(project_root) = find_project_root() {
                    focus_scope_guard_for_spec(&project_root, spec_arg, *force)?;
                }
            }
            // TASK-578: expand the `--drain` discoverability alias into the
            // underlying `--auto-complete --no-human=both --max <queue-size>`
            // state before anything downstream reads those flags. `--drain` only
            // sets state — every guard, dispatch, and orchestrator path below
            // behaves exactly as if the operator had typed the long form. The
            // drivable queue size feeds the `--max` default (the bare-queue
            // drain N); a best-effort `0` falls back to 99 inside the resolver.
            // Explicit flags always win. trace:TASK-578 | ai:claude
            let drain_queue_size = if *drain {
                drivable_queued_count(storage, &user_id, role.as_deref()).unwrap_or(0)
            } else {
                0
            };
            let drain_resolution = resolve_drain_alias(
                *drain,
                auto_complete.as_deref(),
                no_human.as_deref(),
                *max,
                drain_queue_size,
            );
            // Shadow the raw flags with their drain-resolved values. `max` is
            // only re-applied on routes where it is meaningful (the bare-queue
            // nextN drain and a batch drain); on the single-spec path the
            // existing `--max` rejection still applies, so keep `max` itself the
            // operator-supplied value there. trace:TASK-578 | ai:claude
            let auto_complete = &drain_resolution.auto_complete;
            let no_human = &drain_resolution.no_human;
            // trace:STORY-2642 | ai:codex
            // Check before queue/lease/worktree mutations, including drain routes.
            let preflight_vendor = if crate::lane_host::needs_preflight(
                no_human.is_some(),
                *dry_run,
                *no_launch,
                *list_sessions,
            ) && resume.is_none()
            {
                let root = main_worktree_root_from(&find_project_root()?);
                let lane_vendor = session::resolve_enabled_headless_vendor(&root)?;
                crate::lane_host::preflight(&root, lane_vendor.as_str(), *sandbox)?;
                Some(lane_vendor)
            } else {
                None
            };
            // TASK-966: assemble the hard budget caps once, up front, so a bad
            // `--max-runtime` value bails before any drain work starts. The caps
            // are threaded into the multi-spec drain loops (nextN / batch /
            // batches); they stop the drain cleanly at a spec boundary and
            // compose with `--max-failures` + the goal condition (whichever
            // fires first). trace:TASK-966 | ai:claude
            let drain_caps = {
                let max_runtime = match max_runtime.as_deref() {
                    Some(s) => match drain_caps::parse_duration(s) {
                        Some(d) => Some(d),
                        None => anyhow::bail!(
                            "could not parse --max-runtime value {:?} — use minutes \
                             (e.g. `90`) or a suffixed/compound form like `90s`, `45m`, \
                             `2h`, `1h30m`",
                            s
                        ),
                    },
                    None => None,
                };
                drain_caps::DrainCaps {
                    max_tokens: *max_tokens,
                    max_iterations: *max_iterations,
                    max_runtime,
                }
            };
            // TASK-560: reject --resume + --auto-complete with a message that
            // explains the conflict and names both recovery paths, instead of
            // clap's terse "cannot be used with". trace:TASK-560 | ai:claude
            if let Some(msg) =
                resume_autocomplete_conflict_message(resume.is_some(), auto_complete.is_some())
            {
                anyhow::bail!(msg);
            }
            // TASK-307: propagate the headless-tee flag the same way
            // `--zen` propagates via `AIDA_ZEN` — set the env var once at
            // the top and every downstream child (the direct headless
            // launch, the orchestrator's spawned `aida queue work` phase
            // children) reads it from `TeeOptions::from_env_and_flag`. The
            // env var is the canonical signal; the flag is sugar for setting
            // it. A user-set `AIDA_TEE_HEADLESS=0` in the shell is honored
            // (we never scrub a value we didn't set). trace:TASK-307
            if *no_tee_headless {
                std::env::set_var("AIDA_TEE_HEADLESS", "0");
            }
            // STORY-287: resolve the three-mode autonomy ladder up front,
            // before either the `--auto-complete` orchestrator or the plain
            // `handle_queue_work` path runs. `--zen` works purely through
            // the `AIDA_ZEN` env var — skill templates auto-resolve their
            // `kind:confirmation` prompts when it is set. Setting it here
            // means every downstream child inherits it: the direct
            // `exec_claude`, and the orchestrator's spawned `aida queue
            // work` phase subprocesses (which re-exec `claude`). Same
            // shape as the `AIDA_SESSION_ROLE` propagation below.
            // Precedence: `--no-human` > `--zen` > default — when
            // `--no-human` is effective we actively clear `AIDA_ZEN` so a
            // stale flag (or an inherited env var) never reaches a headless
            // session. trace:STORY-287 | ai:claude
            // TASK-1060 / ADR-10: resolve the autonomy mode ONCE here and carry
            // the typed value; in-process reads (the pre-flight banner below)
            // consult it instead of re-reading the bare `AIDA_ZEN` env var. The
            // env var is still SET below strictly as the cross-process transport
            // to spawned phase children / skill templates. trace:ADR-10 trace:TASK-1060
            let autonomy = resolve_autonomy_mode(*zen, no_human.is_some());
            match autonomy {
                AutonomyMode::Zen => {
                    std::env::set_var(zen::ZEN_ENV, "1");
                    // STORY-564: propagate `--pause-always` to the launched
                    // session so `aida zen finish` pauses at the grab-next/stop
                    // checkpoint instead of auto-exiting on a clean finish. A
                    // leak only ever adds a pause (the safe direction), so —
                    // unlike the zen-intent token — this needs no corroboration.
                    // trace:STORY-564 | ai:claude
                    if *pause_always {
                        std::env::set_var(zen::ZEN_PAUSE_ALWAYS_ENV, "1");
                    }
                    // BUG-237: mint this invocation's zen-intent token. It is
                    // the provenance anchor for `AIDA_ZEN` — the session lease
                    // records it (standalone path), and (under
                    // `--auto-complete`) `run_auto_complete` reads its
                    // presence and records the `zen` flag on the drain-state
                    // file as the orchestrator-path corroboration anchor
                    // (TASK-336). A leaked `AIDA_ZEN=1` carries no token, so
                    // `aida zen status` corroborates it away rather than
                    // auto-resolving a confirmation prompt the user never
                    // authorized. trace:BUG-237 trace:TASK-336 | ai:claude
                    std::env::set_var(zen::ZEN_TOKEN_ENV, uuid::Uuid::now_v7().to_string());
                }
                AutonomyMode::NoHuman => {
                    if *zen {
                        eprintln!(
                            "  {} --zen and --no-human both set; --no-human wins (it is the \
                             stronger autonomy mode — nobody is reachable to consult)",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                        );
                    }
                    std::env::remove_var(zen::ZEN_ENV);
                    std::env::remove_var(zen::ZEN_TOKEN_ENV);
                }
                AutonomyMode::Default => {
                    // BUG-237: scrub any inherited / leaked zen-intent token
                    // so a non-`--zen` `aida queue work` never records zen
                    // provenance — neither in its session lease nor (for an
                    // `--auto-complete` orchestrator) in its run marker.
                    // `AIDA_ZEN` itself is left intact: an orchestrated phase
                    // child legitimately inherits it, and corroboration — not
                    // scrubbing — is the safety net. trace:BUG-237 | ai:claude
                    std::env::remove_var(zen::ZEN_TOKEN_ENV);
                }
            }
            // STORY-564: `--pause-always` only governs the standalone-`--zen`
            // finish; flag it as a no-op when `--zen` isn't effective so a
            // mistyped invocation isn't silently ignored. trace:STORY-564
            if *pause_always && !matches!(autonomy, AutonomyMode::Zen) {
                eprintln!(
                    "  {} --pause-always has no effect without --zen (it governs the \
                     standalone --zen finish checkpoint only)",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                );
            }
            // TASK-270: accept `batch:NAME` as a positional id (equivalent
            // to `--batch NAME`) and strip a redundant `batch:` prefix off
            // the `--batch` flag value. `batch:` is the literal tag printed
            // by `aida queue list`; first-users reflexively copy that whole
            // token back as an identifier. trace:TASK-270 | ai:claude
            let (effective_id, effective_batch) =
                resolve_queue_work_batch(id.as_deref(), batch.as_deref());
            let effective_batches = if let Some(raw) = batches.as_deref() {
                Some(parse_batch_chain(raw)?)
            } else if auto_complete.is_some()
                && effective_batch.map(|b| b.contains(',')).unwrap_or(false)
            {
                Some(parse_batch_chain(effective_batch.unwrap())?)
            } else {
                None
            };
            if auto_complete.is_none() && effective_batch.map(|b| b.contains(',')).unwrap_or(false)
            {
                anyhow::bail!(
                    "comma-separated batches require `--auto-complete` — use \
                     `aida queue work --batch A,B --auto-complete` or `--batches A,B`"
                );
            }
            // TASK-322: a batch drain (--batch NAME / batch:NAME / --batches) and
            // a `nextN` head pickup are two different targets;
            // resolve_queue_work_batch discards the positional when a batch is
            // set, which silently swallowed the `nextN`. Catch the collision
            // explicitly, mirroring the --batch+--type and --max+nextN guards.
            // trace:TASK-322 | ai:claude
            if (effective_batch.is_some() || effective_batches.is_some())
                && id.as_deref().map(is_next_keyword_id).unwrap_or(false)
            {
                anyhow::bail!(
                    "pick one: a batch drain (`--batch NAME`) or a head drain (`nextN`) — not both"
                );
            }
            // TASK-270: clap rejects `--batch` alongside `--type`; the
            // positional `batch:NAME` form bypasses that rule, so enforce
            // the same conflict here rather than silently dropping
            // `--type`. trace:TASK-270 | ai:claude
            if (effective_batch.is_some() || effective_batches.is_some()) && type_filter.is_some() {
                anyhow::bail!(
                    "`--type` does not apply to batch pickup — drop it, or pass an EPIC/STORY id for a typed cluster"
                );
            }
            // TASK-293: resolve the `next` / `nextN` keyword. `next` (N=1) is
            // an explicit alias for no-arg head pickup; `nextN` / `next N`
            // (N>1) is the drain-N-from-head form. A `next*` positional is
            // not a spec-id, so clear `effective_id` — every head-pickup path
            // below keys off `None`. trace:TASK-293 | ai:claude
            let next_kw = parse_next_keyword(effective_id, count.as_deref())?;
            let mut effective_id: Option<&str> = match next_kw {
                NextKeyword::Count(_) => None,
                NextKeyword::NotNext => effective_id,
            };
            // trace:TASK-518 | ai:antigravity
            let resolved_spec_id = if let Some(s) = effective_id {
                if *from_pr {
                    // BUG-897: `--from-pr` is the queue-independent PR
                    // re-entry path. Keep an explicit PR-N scope intact so a
                    // child drain does not resolve it back to a Done/dequeued
                    // spec before phase 3.
                    // trace:BUG-897 | ai:codex
                    None
                } else if let Some(pr) = parse_pr_arg(s) {
                    // STORY-501/BUG-440: when a "Review PR-N" story is queued,
                    // DON'T resolve PR→backing-spec here. The TASK-518 resolution
                    // (resolve to the implemented spec) is right for a human who
                    // wants to keep working the spec, but it runs BEFORE
                    // resolve_queue_work_plan's review-story pickup (TASK-85) and
                    // so defeated it: `queue work PR-N` became `queue work
                    // <spec>` → an implementer pickup that OWNS + re-implements
                    // the spec (the shared root of the resume reviewer
                    // re-implementing, BUG-436, BUG-438, BUG-440). Leaving "PR-N"
                    // when a review story exists lets TASK-85 route to the
                    // reviewer (/aida-review, PR-scoped lease — no spec
                    // ownership). Resolve to the backing spec only when there is
                    // no review story to pick up. trace:STORY-501 | ai:claude
                    // BUG-1195: the lookup is typed — a read FAILURE keeps the PR
                    // scope (the review-story pickup retries its own lookup) and
                    // never feeds the envelope guard; only a genuine no-story
                    // outcome resolves to the backing spec or, under the
                    // envelope, bails with the failing check named.
                    // trace:BUG-1195 | ai:claude
                    let lookup = parse_review_scope(s)
                        .map(|(forge, n)| queued_review_story_for_pr(storage, &user_id, forge, n));
                    let lookup_failed = matches!(&lookup, Some(ReviewStoryLookup::Failed(_)));
                    if let Some(ReviewStoryLookup::Failed(cause)) = &lookup {
                        eprintln!(
                            "  {} review-story lookup for {s} failed ({cause}) — keeping the PR scope so the review pickup can retry",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                        );
                    }
                    if lookup.as_ref().is_some_and(|l| l.is_found()) || lookup_failed {
                        None
                    } else if reviewer_pickup_keeps_pr_scope(role.as_deref())
                        && !orchestrator_review_envelope_for_pr(pr)
                    {
                        // BUG-2150: an explicit `--role reviewer` pickup is a
                        // REVIEW of PR-N, never an implementer pickup of the
                        // backing spec. Keep the PR scope so the plan resolver
                        // synthesizes a PR-scoped review plan — no queue step,
                        // and no conflict with the implementer's spec lease.
                        // trace:BUG-2150 | ai:claude
                        None
                    } else if orchestrator_review_envelope_for_pr(pr) {
                        let why = lookup
                            .as_ref()
                            .map(|l| l.describe())
                            .unwrap_or_else(|| "the scope is not a PR/MR reference".to_string());
                        // BUG-1186 / ADR-40: the orchestrator's review phase set
                        // the review envelope for THIS PR. Falling back to the
                        // backing spec here swaps the seat — an implementer
                        // pickup that executes the spec's rework reason and
                        // pushes to the PR under review. Refuse loudly instead.
                        // trace:BUG-1186 | ai:claude
                        anyhow::bail!(
                            "review envelope is set for PR-{pr} but no pickable review story is \
                             queued ({why}) — refusing to fall back to an implementer pickup of the \
                             backing spec. Queue a fresh review round (`aida pr auto-queue-review \
                             --branch <pr-head-branch>`) and retry"
                        );
                    } else {
                        let store = storage.load()?;
                        let project_root = storage.path().parent().ok_or_else(|| {
                            anyhow::anyhow!("cannot derive project root from store path")
                        })?;
                        let resolved = resolve_pr_to_spec(project_root, pr, &store)?;
                        println!("working on {} (backs {})", resolved, s);
                        Some(resolved)
                    }
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(ref res) = resolved_spec_id {
                effective_id = Some(res.as_str());
            }
            // BUG-839: `--drain` expands to an autonomous drive, but `--dry-run`
            // must still be a read-only preview. Route it after target
            // resolution but before the drain-start RBAC gate, no-human
            // acknowledgement, global drain lock, drain-state write, db pull, or
            // session launch.
            if *drain && *dry_run {
                return preview_queue_work_drain(
                    storage,
                    &user_id,
                    effective_id,
                    effective_batch,
                    effective_batches.as_deref(),
                    next_kw,
                    role.as_deref(),
                    drain_resolution.max,
                );
            }
            // STORY-1353: starting an autonomous drain (`--auto-complete`) is
            // DISPATCH, not disposition, so it is gated on
            // `has_dispatch_authority()` (product, advisor, integrator role,
            // or a live orchestrator) rather than the old advisor-only
            // `[team.permissions]` map. A live-orchestrator re-entry
            // (`--resume-drain`, and the phase children the orchestrator
            // spawns) holds dispatch authority via the orchestrator carve-out,
            // so the gate bypasses those and only the FRESH launch is
            // checked; there is no `--force` bypass on this path, unlike the
            // `aida burndown run` sibling gate. trace:STORY-647 trace:STORY-1353 | ai:claude
            if auto_complete.is_some() && !*resume_drain {
                // trace:STORY-1133 | ai:codex
                if current_role_instance_is_companion() {
                    anyhow::bail!(
                        "companion sessions cannot drive drains. Start the authoritative driver \
                         seat for this role, or run `aida queue work --auto-complete` from the driver."
                    );
                }
                // BUG-1517: `--resume-dry-run` is a read-only preview — it
                // prints the reconciled re-entry plan and never re-enters —
                // so it is disposition, not dispatch, and should not need
                // dispatch authority. Only a FRESH, non-dry-run launch is
                // gated here; a live dispatch without `--resume-dry-run`
                // stays refused. trace:BUG-1517 | ai:claude
                if let Err(msg) = auto_complete_dispatch_authority_ok(
                    *resume_dry_run,
                    crate::gate::role_authority::has_dispatch_authority(),
                ) {
                    anyhow::bail!(msg);
                }
            }
            // STORY-246: `--auto-complete` drives the full
            // implementer→CI→reviewer→merge→pull→build lifecycle. It is a
            // sibling orchestrator, not a decoration of the exec path —
            // `handle_auto_complete` spawns each phase, waits, and never
            // returns (it always terminates the process with a phase-keyed
            // exit code), so the `queue work` logic below is reached only
            // when `--auto-complete` is absent. trace:STORY-246 | ai:claude
            if auto_complete.is_some() {
                // BUG-2145: `--sandbox` used to be read only on the plain
                // queue-work path below, so `--auto-complete --sandbox` ran
                // the whole drain uncontained. Honour it here: AIDA_CONTAINED
                // is what `session::drain_contained` (RealPhaseDriver's
                // contained flag) and every phase child read.
                // trace:BUG-2145 | ai:claude
                honour_auto_complete_sandbox(*sandbox, permission_mode.as_deref())?;
                let mode = auto_complete.as_deref().unwrap_or("full");
                let variant = auto_complete::AutoCompleteVariant::parse(mode)
                    .map_err(|e| anyhow::anyhow!(e))?;
                // STORY-561: operator-presence advisory default (consumers a+b).
                // When the operator is AWAY and passes no explicit
                // --no-human / --escalate-* flag, presence fills the drain-mode
                // default per `[presence] away_drain` — the autonomy ladder
                // keying on a presence STATE instead of per-command flags.
                // Explicit flags ALWAYS win, and the kickoff scope-ack +
                // escalate validation below still apply (advisory only —
                // acceptance #4/#5). Presence is only ever `away` here in a
                // non-TTY context (the interactive-TTY auto-flip preempts to
                // home, main.rs:~922), so this sets the default for unattended
                // drains; an interactive operator is already `home`.
                // trace:STORY-561 | ai:claude
                let presence_cfg = {
                    let cfg_path = storage
                        .path()
                        .parent()
                        .map(config_path_for_project)
                        .unwrap_or_else(|| std::path::PathBuf::from(".aida/config.toml"));
                    presence::read_presence_config(&cfg_path)
                };
                let drain_res = presence::resolve_drain_mode(
                    no_human.as_deref(),
                    *escalate_blocks,
                    *escalate_defaults,
                    presence::current_presence(chrono::Utc::now()),
                    &presence_cfg,
                );
                if drain_res.presence_applied {
                    eprintln!(
                        "  {} operator {} → drain defaulting to {}{} (per [presence] away_drain; override with --no-human / --escalate-*)",
                        crate::glyph(crate::glyphs::Glyph::Away),
                        "away".yellow().bold(),
                        drain_res
                            .no_human
                            .as_deref()
                            .map(|s| format!("--no-human={s}"))
                            .unwrap_or_else(|| "interactive".to_string()),
                        if drain_res.no_human.as_deref() == Some("both") {
                            if drain_res.escalate_defaults {
                                " --escalate-defaults"
                            } else {
                                " --escalate-blocks"
                            }
                        } else {
                            ""
                        },
                    );
                }
                // STORY-263 / STORY-276: `--no-human[=MODE]` runs the
                // orchestrator's phases headless (`claude -p`). `reviewer-only`
                // wires the reviewer (phase 3); `both` additionally runs the
                // implementer (phase 1) headless, with the `/aida-punt` safety
                // net for design-forks (STORY-276). The slug is the explicit
                // flag's value, or the presence-supplied default (STORY-561).
                let no_human_mode = match drain_res.no_human.as_deref() {
                    Some(v) => {
                        Some(auto_complete::NoHumanMode::parse(v).map_err(|e| anyhow::anyhow!(e))?)
                    }
                    None => None,
                };
                let aida_headless_env = std::env::var("AIDA_HEADLESS")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);
                let requested_no_human_mode = no_human_mode;
                let no_human_mode = non_tty_interactive_implementer_preflight(
                    no_human_mode,
                    std::io::IsTerminal::is_terminal(&std::io::stdin()),
                    std::io::IsTerminal::is_terminal(&std::io::stdout()),
                    aida_headless_env,
                )?;
                if no_human_mode == Some(auto_complete::NoHumanMode::Both)
                    && requested_no_human_mode != Some(auto_complete::NoHumanMode::Both)
                    && aida_headless_env
                {
                    std::env::set_var("AIDA_NO_HUMAN_ACKNOWLEDGED", "1");
                    eprintln!(
                        "  {} AIDA_HEADLESS=1 detected — running the implementer headless",
                        crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
                    );
                }
                // TASK-306 / STORY-276: pre-launch gate. Both modes print the
                // loud scope banner (wording per mode) and require a one-time
                // acknowledgement. This dispatch arm runs exactly once per
                // `aida queue work --auto-complete` invocation, so the banner
                // appears once per kickoff even when a batch / `nextN` drain
                // loops `run_auto_complete` internally. trace:TASK-306, STORY-276
                if let Some(mode) = no_human_mode {
                    no_human_kickoff_gate(mode)?;
                }
                // STORY-306: resolve how an advisor escalation is handled.
                // `--escalate-blocks` (the default — pause, don't guess) and
                // `--escalate-defaults` are `--no-human=both`-only: the
                // advisor tier exists only in a fully-headless drain. Reject
                // the flag anywhere else so it never silently no-ops.
                // trace:STORY-306 | ai:claude
                if (*escalate_blocks || *escalate_defaults)
                    && no_human_mode != Some(auto_complete::NoHumanMode::Both)
                {
                    anyhow::bail!(
                        "--escalate-blocks / --escalate-defaults only apply to a \
                         fully-headless drain — pair them with `--no-human=both`"
                    );
                }
                // STORY-561: the escalate default folds in presence — explicit
                // --escalate-* flags win (validated above), else the
                // presence-supplied `away_drain` advice (defaults vs park).
                // `drain_res.escalate_defaults` == `*escalate_defaults` whenever
                // presence supplied nothing, so the non-away path is unchanged.
                let escalate_mode =
                    auto_complete::EscalateMode::from_flags(drain_res.escalate_defaults);
                // STORY-347: per-drain calibration override propagates via an
                // env var, so it composes with `--batch` / `nextN` / single-
                // spec drains without threading through three signatures.
                // `run_advisor` reads `AIDA_CALIBRATE` as `1` (force on) or `0`
                // (force off); absence falls through to `[advisor]
                // calibration_mode` in `.aida/config.toml`. Only meaningful
                // under `--no-human=both` where the advisor tier runs.
                // trace:STORY-347 | ai:claude
                if *calibrate || *no_calibrate {
                    if no_human_mode != Some(auto_complete::NoHumanMode::Both) {
                        anyhow::bail!(
                            "--calibrate / --no-calibrate only apply when the advisor tier runs — \
                             pair with `--no-human=both`"
                        );
                    }
                    std::env::set_var("AIDA_CALIBRATE", if *calibrate { "1" } else { "0" });
                } else {
                    std::env::remove_var("AIDA_CALIBRATE");
                }
                // BUG-420: map the watchdog flags to env vars so the resolved
                // `DrainTuning` (read in `RealPhaseDriver::new`) picks them up
                // across single-spec / batch / nextN drains without threading
                // through every handler signature — the same pattern
                // `AIDA_CALIBRATE` uses just above. trace:BUG-420 | ai:claude
                if let Some(m) = no_progress_minutes {
                    std::env::set_var("AIDA_NO_PROGRESS_MINUTES", m.to_string());
                }
                if let Some(m) = phase_ceiling_minutes {
                    std::env::set_var("AIDA_PHASE_CEILING_MINUTES", m.to_string());
                }
                // TASK-480: propagate the intermediate-only opt-out to the
                // orchestrator phase-3 reviewer pre-flight via an env var
                // (same pattern as the BUG-420 watchdog flags) so we don't
                // thread the bool through every batch/nextN/auto-complete
                // handler signature. The single-spec `handle_queue_work`
                // path below also receives it explicitly.
                // trace:TASK-480 | ai:claude
                if *allow_intermediate_only {
                    std::env::set_var("AIDA_ALLOW_INTERMEDIATE_ONLY", "1");
                }
                // STORY-265 slice 3: `--with-plan` runs a PLAN PRELUDE (plan
                // session → Approved→Planned promote) before the drain's phase
                // 1. Propagate it via an env var — the same pattern AIDA_CALIBRATE
                // / AIDA_ALLOW_INTERMEDIATE_ONLY use — so it composes across the
                // single-spec / batch / nextN drain routes without threading a
                // new bool through every handler signature. `run_auto_complete`
                // reads it once and runs the prelude before driving the phases;
                // the Phase enum is untouched (the prelude is NOT a renumbered
                // phase). trace:STORY-265 | ai:claude
                if *with_plan {
                    std::env::set_var("AIDA_WITH_PLAN", "1");
                }
                // BUG-538: take the global drain lock before dispatching to ANY
                // of the auto-complete routes (single / batch / batches / nextN /
                // --drain / --from-pr / --resume-drain). This drain integrates on
                // main; a second one (here or via `aida burndown run`) would
                // double-drive the tree. The guard is held for the rest of this
                // arm — the route handlers terminate via `std::process::exit`, so
                // Drop won't fire, but a dead pid is stale-reclaimed by the next
                // launch (see drain_lock module docs). The `--resume-dry-run`
                // preview drives nothing, so it skips the lock. A real
                // `--resume-drain` re-entry correctly reclaims the crashed
                // drain's now-dead lock (or refuses if its pid is somehow alive).
                // trace:BUG-538 | ai:claude
                let _drain_guard = if *resume_dry_run {
                    None
                } else {
                    let drain_lock_command = {
                        let mut c = String::from("queue work --auto-complete");
                        if let Some(b) = effective_batches.as_deref() {
                            c.push_str(&format!(" --batches {}", b.join(",")));
                        } else if let Some(b) = effective_batch {
                            c.push_str(&format!(" --batch {b}"));
                        } else if let NextKeyword::Count(n) = next_kw {
                            if n > 1 {
                                c.push_str(&format!(" next{n}"));
                            }
                        } else if let Some(s) = effective_id {
                            c.push(' ');
                            c.push_str(s);
                        }
                        c
                    };
                    let project_root = find_main_worktree_root()?;
                    // STORY-1414: warn about (or, with --require-head, refuse)
                    // a stale dev binary. Before the lock so it records it.
                    // trace:STORY-1414 | ai:claude
                    crate::freshness_gate::enforce_wave_launch_gate(&project_root, *require_head)?;
                    drain_cmd::install_stop_request_env(&project_root);
                    drain_cmd::clear_stop_request(&project_root);
                    Some(drain_lock::acquire_drain_lock(
                        &project_root,
                        &drain_lock_command,
                    )?)
                };
                // TASK-1518: a stopped wave (systemd RuntimeMaxSec / OOMPolicy=stop,
                // `aida drain stop --now`, a manual kill) must not leave the lock
                // and its leases for the next tick to reap. The guard moves into a
                // shared slot so the SIGTERM handler can release it properly
                // (heartbeat, shared claim, pid-checked file) and stamp the
                // in-flight leases before the bounded exit. The handler holds only
                // a Weak handle, so without a signal the slot behaves exactly like
                // the plain guard did: dropped at the end of this arm, or left to
                // the atexit hook on `process::exit`. Advisory install: a failure
                // to register the handler only means today's kill-then-reap
                // recovery. trace:TASK-1518 | ai:claude
                let _drain_guard: crate::drain_signal::GuardSlot =
                    std::sync::Arc::new(std::sync::Mutex::new(_drain_guard));
                if !*resume_dry_run {
                    if let Ok(root) = find_main_worktree_root() {
                        if let Err(e) =
                            crate::drain_signal::install(crate::drain_signal::DrainTermContext {
                                project_root: root,
                                drain_pid: std::process::id(),
                                guard: std::sync::Arc::downgrade(&_drain_guard),
                                grace: crate::drain_signal::grace_from_env(),
                                term_flag: crate::drain_signal::process_term_flag(),
                                borrowed: drain_lock::borrow_requested(),
                            })
                        {
                            eprintln!(
                                "  {} could not install the drain SIGTERM handler ({e:#}); a stopped wave falls back to next-tick reap",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                            );
                        }
                    }
                }
                // BUG-660: prevent the host from sleeping for the duration of an
                // unattended drive — a lidded/idle laptop must not suspend
                // mid-drain. Best-effort (a missing caffeinate / systemd-inhibit
                // degrades to a no-op) and scoped to the drive's lifetime: the
                // held child is pid-tied so it auto-releases on the route
                // handlers' `std::process::exit` (which skips `Drop`). The
                // `--resume-dry-run` preview drives nothing, so it stays
                // unarmed alongside the lock above. trace:BUG-660 | ai:claude
                let _sleep_inhibitor = if *resume_dry_run {
                    None
                } else {
                    let inhibitor =
                        drive_robustness::SleepInhibitor::for_drive("aida unattended drive");
                    if let Some(tool) = inhibitor.tool() {
                        eprintln!(
                            "{} sleep-prevention active for this drive (via {})",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            tool
                        );
                    }
                    Some(inhibitor)
                };
                // STORY-492: `--resume-drain` is its own entry — read the crashed
                // drain-state, gate on PID-liveness, reconcile from reality, and
                // re-enter (or `--dry-run` preview). It never returns. Routed
                // before the batch/single dispatch since it replaces them.
                // trace:STORY-492 | ai:claude
                if *resume_drain {
                    handle_drain_resume(
                        storage,
                        &user_id,
                        drain_id.as_deref(),
                        *resume_dry_run,
                        *json,
                        permission_mode.as_deref(),
                        no_human_mode,
                        escalate_mode,
                        *steal,
                        *force_claim,
                        *allow_stale_base,
                        *no_auto_rebase,
                    );
                }
                // TASK-1003 / SPIKE-70: `--single-branch` drives a coupled batch
                // on ONE shared branch in ONE worktree → ONE cluster PR
                // (commit-per-member, Implementer+CI only between members,
                // halt-on-failure). Routed BEFORE the per-member-PR batch drains
                // below since it replaces them. Requires exactly one `--batch`.
                // trace:TASK-1003 | ai:claude
                if *single_branch {
                    let Some(batch_name) = effective_batch.filter(|b| !b.is_empty()) else {
                        anyhow::bail!(
                            "--single-branch drives ONE coupled batch on one shared branch — \
                             pair it with `--batch NAME` (e.g. `aida queue work --batch NAME \
                             --auto-complete --single-branch`)"
                        );
                    };
                    if effective_batches.is_some() {
                        anyhow::bail!(
                            "--single-branch drives ONE shared branch — use a single \
                             `--batch NAME`, not `--batches`"
                        );
                    }
                    handle_auto_complete_single_branch(
                        storage,
                        &user_id,
                        batch_name,
                        variant,
                        *json,
                        role.as_deref(),
                        *max,
                        // TASK-1137: the per-member checkpoint pauses for a
                        // human at the keyboard and auto-continues when the
                        // drain is fully headless. trace:TASK-1137 | ai:claude
                        no_human_mode,
                    );
                }
                // TASK-1005 / SPIKE-70: `--sequential` NAMES + guards the existing
                // per-member-PR batch drain as a first-class coupled-ordered mode:
                // members run in pickup order, each its own PR off freshly-pulled
                // main, with shelve-and-continue on a member failure. It does NOT
                // change the engine — it requires a batch and then falls through to
                // the same `handle_auto_complete_batch[es]` dispatch below.
                // `conflicts_with = "single_branch"` is enforced by clap.
                // TASK-185: the SINGLE-batch leg of that dispatch
                // (`handle_auto_complete_batch` → `drain_batch_pipelined_with_caps`)
                // honours `[drain] pipeline_depth` since STORY-1091, so this flag
                // governs ORDER + per-member-PR shape, not concurrency — the
                // one-at-a-time property comes from the default depth of 1, not
                // from `--sequential`. The `--batches` leg does NOT:
                // `handle_auto_complete_batches` calls `drain_batch_chain_with_caps`,
                // which is typed on the non-pipelined `BatchDriver` and always calls
                // `drain_batch_with_caps`, so a chain is serial at ANY depth. The
                // notice below is branched on that split rather than asserting the
                // depth claim on a path where it is false.
                // trace:TASK-1005 trace:TASK-185 | ai:claude
                if *sequential {
                    let has_batch = effective_batch.is_some_and(|b| !b.is_empty())
                        || effective_batches.is_some();
                    if !has_batch {
                        anyhow::bail!(
                            "--sequential drives a batch's members in pickup order, each as \
                             its own PR — pair it with `--batch NAME` or `--batches A,B,C` \
                             (e.g. `aida queue work --batch NAME --auto-complete \
                             --sequential`)"
                        );
                    }
                    if !*json {
                        // TASK-185: `--batches` takes precedence in the dispatch
                        // below, and that chain path never reaches the pipelined
                        // scheduler — so scope the concurrency sentence to the leg
                        // the operator actually invoked instead of printing the
                        // depth claim on a path where it is inert.
                        // trace:TASK-185 | ai:claude
                        let concurrency = if effective_batches.is_some() {
                            "Ordering only — a `--batches` chain is always serial: the batches \
                             run in turn and each batch's members one at a time, regardless of \
                             `[drain] pipeline_depth`."
                                .to_string()
                        } else {
                            sequential_single_batch_concurrency_notice(
                                find_main_worktree_root().ok().as_deref(),
                            )
                        };
                        eprintln!(
                            "Sequential drain: members run in pickup order, each its own PR off \
                             freshly-pulled main, and a member failure shelves that member and \
                             continues with the rest. {concurrency}"
                        );
                    }
                    // Fall through to the batch / batches dispatch below — it IS
                    // the sequential one-member-at-a-time engine.
                }
                // TASK-285: `--batch NAME --auto-complete` drains the whole
                // batch — one full lifecycle per member, advancing the head
                // after each — instead of one session per re-invocation.
                // `--max` bounds the drain. trace:TASK-285 | ai:claude
                if let Some(batch_names) = effective_batches.as_deref() {
                    handle_auto_complete_batches(
                        storage,
                        &user_id,
                        batch_names,
                        variant,
                        *json,
                        permission_mode.as_deref(),
                        role.as_deref(),
                        *max,
                        *max_failures,
                        no_human_mode,
                        escalate_mode,
                        *steal,
                        *force_claim,
                        *allow_stale_base,
                        *no_auto_rebase,
                        &drain_caps,
                    );
                }
                if let Some(batch_name) = effective_batch {
                    if batch_name.is_empty() {
                        anyhow::bail!(
                            "batch name is empty — pass `--batch NAME` or `aida queue work batch:NAME`"
                        );
                    }
                    handle_auto_complete_batch(
                        storage,
                        &user_id,
                        batch_name,
                        variant,
                        *json,
                        permission_mode.as_deref(),
                        role.as_deref(),
                        *max,
                        *max_failures,
                        no_human_mode,
                        escalate_mode,
                        *steal,
                        *force_claim,
                        *allow_stale_base,
                        *no_auto_rebase,
                        &drain_caps,
                    );
                }
                // TASK-293: `nextN --auto-complete` drains N specs from the
                // queue head sequentially — one full lifecycle each, advancing
                // the head after every ship. `next` / `next1` carries N=1 and
                // falls through to the single-head path below. trace:TASK-293
                if let NextKeyword::Count(n) = next_kw {
                    if n > 1 {
                        if max.is_some() {
                            anyhow::bail!(
                                "`next{n}` already bounds the drain to {n} specs — drop `--max`"
                            );
                        }
                        handle_auto_complete_next_n(
                            storage,
                            &user_id,
                            n,
                            variant,
                            *json,
                            permission_mode.as_deref(),
                            role.as_deref(),
                            no_human_mode,
                            escalate_mode,
                            *steal,
                            *force_claim,
                            *allow_stale_base,
                            *no_auto_rebase,
                            *max_failures,
                            &drain_caps,
                        );
                    }
                }
                // TASK-578: a bare `--drain` (no batch, no positional spec, no
                // `nextN` keyword) drains the whole drivable queue. It expands to
                // a `nextN` drain where N is the resolved drain `--max` (the
                // queue size, an explicit `--max`, or the 99 fallback). Routing
                // through the existing nextN orchestrator gives us the
                // skip-undrivable + per-member-lifecycle behaviour for free.
                // trace:TASK-578 | ai:claude
                if *drain && effective_id.is_none() {
                    let n = drain_resolution.max.unwrap_or(99).max(1);
                    handle_auto_complete_next_n(
                        storage,
                        &user_id,
                        n,
                        variant,
                        *json,
                        permission_mode.as_deref(),
                        role.as_deref(),
                        no_human_mode,
                        escalate_mode,
                        *steal,
                        *force_claim,
                        *allow_stale_base,
                        *no_auto_rebase,
                        *max_failures,
                        &drain_caps,
                    );
                }
                // `--max` only bounds a batch drain — reject it on the
                // single-spec path so the flag never silently no-ops.
                // trace:TASK-285 | ai:claude
                if max.is_some() {
                    anyhow::bail!(
                        "`--max` bounds a batch drain — pair it with `--batch NAME` \
                         (e.g. `aida queue work --batch NAME --auto-complete --max 3`)"
                    );
                }
                // TASK-292: with no positional SPEC, inherit the no-arg
                // `aida queue work` "pick the queue head" semantics instead of
                // demanding an explicit id — `--auto-complete` composes with
                // head pickup the same way the interactive form already does.
                // trace:TASK-292 | ai:claude
                let spec = match effective_id {
                    Some(s) => s.to_string(),
                    None => resolve_auto_complete_head(storage, &user_id, role.as_deref())?,
                };
                // TASK-405: `--from-pr` — implementation shipped OUTSIDE the
                // orchestrator (a PR is already open). Drive phases 3-6 only,
                // skipping the implementer. Routed here on the single-spec path
                // (batch / nextN drains exit above). The shared `--resume-dry-run`
                // flag previews the plan. trace:TASK-405 | ai:claude
                if *from_pr {
                    handle_from_pr(
                        storage,
                        &user_id,
                        &spec,
                        variant,
                        *resume_dry_run,
                        *json,
                        permission_mode.as_deref(),
                        no_human_mode,
                        escalate_mode,
                        *steal,
                        *force_claim,
                        *allow_stale_base,
                        *no_auto_rebase,
                    );
                }
                handle_auto_complete(
                    storage,
                    &user_id,
                    &spec,
                    variant,
                    *json,
                    permission_mode.as_deref(),
                    no_human_mode,
                    escalate_mode,
                    *steal,
                    *force_claim,
                    *allow_stale_base,
                    *no_auto_rebase,
                );
            }
            // TASK-293: a multi-spec `nextN` has no coherent single
            // interactive session — draining N unrelated head items into one
            // branch/PR is wrong, so it requires the orchestrator. `next`
            // (N=1) already had `effective_id` cleared to None above and
            // falls through as a plain head pickup. trace:TASK-293 | ai:claude
            if let NextKeyword::Count(n) = next_kw {
                if n > 1 {
                    anyhow::bail!(
                        "`next{n}` drains {n} specs sequentially — it needs `--auto-complete`:\n  \
                         aida queue work next{n} --auto-complete\n\
                         For a single interactive pickup use `aida queue work next` (or just `aida queue work`)."
                    );
                }
            }
            // TASK-229: --batch NAME picks the head of the items tagged
            // `batch:NAME`. --dry-run lists the batch instead of acting.
            // The standard handle_queue_work path takes a spec id; we
            // resolve the batch head here and pass it through. Each
            // invocation drains one item (head-pickup loop semantics —
            // re-run after each session exits to advance).
            // trace:TASK-229 | ai:claude
            let resolved_id: Option<String> = if let Some(name) = effective_batch {
                if name.is_empty() {
                    anyhow::bail!(
                        "batch name is empty — pass `--batch NAME` or `aida queue work batch:NAME`"
                    );
                }
                let want = format!("batch:{}", name);
                // trace:TASK-285 | ai:claude — shared with the
                // `--auto-complete` batch drain.
                let members = resolve_batch_members(storage, &user_id, name, role.as_deref())?;
                if *dry_run {
                    if members.is_empty() {
                        println!(
                            "(no queued items tagged `{}` — add the tag via `aida edit <id> --tags {}`)",
                            want.cyan(),
                            want
                        );
                    } else {
                        println!(
                            "{} `{}` ({} item{}, pickup order):",
                            "Batch".bold(),
                            want.cyan(),
                            members.len(),
                            if members.len() == 1 { "" } else { "s" }
                        );
                        for (i, (_, display_id, title, status)) in members.iter().enumerate() {
                            println!(
                                "  {:>2}. {} [{}] {}",
                                i + 1,
                                display_id.bold(),
                                format!("{}", status).dimmed(),
                                title
                            );
                        }
                        println!();
                        println!(
                            "  {}",
                            format!("(run `aida queue work --batch {}` to pick up the head; repeat per item)", name).dimmed()
                        );
                    }
                    return Ok(());
                }
                let Some(head) = members.into_iter().next() else {
                    anyhow::bail!(
                        "no queued items tagged `{}` — tag members via `aida edit <id> --tags {}` first",
                        want,
                        want
                    );
                };
                Some(head.1)
            } else {
                // TASK-1053: a single-spec `--dry-run` is no longer a
                // dead-end — it flows into handle_queue_work, which prints
                // the resolved plan and returns before any side effect.
                // trace:TASK-1053 | ai:claude
                effective_id.map(|s| s.to_string())
            };
            handle_queue_work(
                storage,
                preflight_vendor,
                &user_id,
                resolved_id.as_deref().or(effective_id),
                permission_mode.as_deref(),
                *sandbox,
                *no_launch,
                QueueWorkRoleInput::direct(role.as_deref()),
                *no_pull,
                type_filter.as_deref(),
                branch.as_deref(),
                path.as_deref(),
                // STORY-248: stacked-branch base resolution. Mutually
                // exclusive at the CLI level — `--stack` auto-picks the
                // freshest un-merged implementer lease, `--base BRANCH`
                // takes a name. Threaded through into `session_start` via
                // its existing `base` parameter. trace:STORY-248 | ai:claude
                *stack,
                base.as_deref(),
                *force_base,
                *steal,
                *force,
                *force_claim,
                resume.as_deref(),
                *fresh,
                *list_sessions,
                session_id.as_deref(),
                // TASK-895: which vendor CLI hosts the interactive session
                // (resolved above: flag > [agents] vendor > claude).
                // trace:TASK-895 STORY-761 | ai:claude
                &vendor,
                // STORY-263: presence of `--no-human` (any value) launches
                // this session headless. The orchestrator appends a bare
                // `--no-human` to its reviewer subprocess.
                no_human.is_some(),
                // TASK-1060 / ADR-10: the typed autonomy resolved once above.
                autonomy,
                // TASK-272: the resolved `--batch NAME` (None for a plain
                // or item-mode pickup) — recorded on the session manifest.
                effective_batch,
                // BUG-226: suppress the standalone reviewer summary.
                *quiet,
                // STORY-281: opt out of the reviewer pre-flight stale-base
                // refusal. trace:STORY-281 | ai:claude
                *allow_stale_base,
                // TASK-480: opt out of the reviewer pre-flight
                // intermediate-only refusal. trace:TASK-480 | ai:claude
                *allow_intermediate_only,
                // STORY-439: pickup-time calibration capture. Each value
                // writes to .aida/complexity-calibration/<SPEC>.yaml AND
                // stamps a tag on the spec.
                *complexity,
                *assist_est,
                *effort,
                *strict,
                // STORY-265: plan-only mode — launch /aida-plan in `plan`
                // permission mode instead of /aida-pickup implement. The
                // hidden `--plan-phase` is the drain prelude's headless twin.
                // trace:TASK-1794 | ai:claude
                *plan_only || *plan_phase,
                // STORY-735: guided keystone mode — launch
                // /aida-guided-implement (structured decision dialog) instead
                // of /aida-pickup. trace:STORY-735 | ai:claude
                *guided,
                // TASK-1053: single-spec dry-run — print the resolved plan
                // (session id, worktree, branch, role, skill, lease) and
                // return before any side effect. trace:TASK-1053 | ai:claude
                *dry_run,
                model.as_deref(),
            )?;
        }
        _ => unreachable!("dispatcher selected another queue subcommand"),
    }
    Ok(())
}
