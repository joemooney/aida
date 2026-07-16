//! Pure decision logic for the thin integrator watch-loop (STORY-520, bounded
//! slice).
//!
//! The producer/consumer split (STORY-520) decouples PR *production* (parallel
//! implementers, each leased to its own scope) from PR *integration* (a single
//! serial loop that drives the back-end merge phases). The operator's decision
//! (2026-06-06) was to PROTOTYPE the watch-loop as a thin command first —
//! reusing the TASK-405 `--from-pr` primitive — and promote it to a first-class
//! `integrator` role only if the loop proves out. This module is the loop's
//! pure core: **given already-probed facts about each candidate spec, decide
//! which are ready for integration**, as side-effect-free functions.
//!
//! The probing (loading the store, looking up open PRs / merged state via the
//! forge) and the acting (shelling out to the TASK-405 `--from-pr` path) are
//! SEPARATE and live in `main.rs::handle_queue_integrate`. Keeping the decision
//! pure means it is exhaustively unit-testable with zero risk to the live
//! integration control flow — the same discipline `drain_resume` follows.
//!
//! The handoff protocol is the SUBSTRATE, not a message bus: an implementer
//! finishing work flips the spec to Done and leaves an open PR. The integrator
//! polls for exactly that pair — Done + open PR (and not already merged) — so
//! the substrate state machine *is* the protocol.
//!
//! trace:STORY-520 | ai:claude

/// Already-probed facts about one candidate spec the integrator considers.
/// Built in `main.rs` from the store status + a forge PR lookup; consumed by
/// the pure [`classify_candidate`] below. trace:STORY-520 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IntegrationCandidate {
    /// The display SPEC-ID (e.g. `STORY-520`) — for messaging only.
    pub id: String,
    /// True when the spec's status is `Done` (work finished on a branch).
    pub is_done: bool,
    /// True when an OPEN PR was found for this spec via the forge.
    pub has_open_pr: bool,
    /// True when the spec's PR (open or by-branch) is already merged. A merged
    /// PR means integration already happened — only the auto-bump pull is left,
    /// which the integrator does NOT own (TASK-405 refuses an already-merged
    /// drive). trace:STORY-520 | ai:claude
    pub pr_merged: bool,
    /// True when the PR lookup was INCONCLUSIVE (gh missing, auth failure,
    /// transient network error) rather than a clean "no PR". The integrator
    /// must not treat "couldn't tell" as "no PR" — it skips and reports, so a
    /// flaky probe never silently strands a mergeable spec. trace:STORY-520
    pub pr_lookup_inconclusive: bool,
    /// TASK-813: the spec is keystone work the human must review — tagged
    /// `supervised` (excluded from the drain) or `review:draft-only`. The
    /// integrator PARKS it (leaves the PR for the operator) instead of
    /// auto-merging, so solo mode works the SAFE backlog and never ships
    /// keystone/security unattended. trace:TASK-813 | ai:claude
    pub held_for_human: bool,
}

/// What the integrator should do with one candidate. trace:STORY-520 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CandidateVerdict {
    /// Done + open, unmerged PR — drive the TASK-405 `--from-pr` phases.
    Integrate,
    /// Done but no open PR yet — the implementer hasn't opened one (or it was
    /// closed without merge). Nothing to integrate; skip quietly.
    SkipNoPr,
    /// Done with an already-merged PR — integration already happened; the
    /// pull/auto-bump (which the integrator doesn't own) will promote it.
    SkipAlreadyMerged,
    /// Status is not Done — not in the ready-for-integration set at all.
    SkipNotDone,
    /// The PR probe was inconclusive (gh missing / auth / network). Skip and
    /// surface, never guess. trace:STORY-520 | ai:claude
    SkipProbeInconclusive,
    /// TASK-813: keystone work (`supervised` / `review:draft-only`) — PARK it
    /// for the operator's review; the integrator never auto-merges keystone.
    /// trace:TASK-813 | ai:claude
    SkipHeldForHuman,
}

/// The pure heart of the integrator: classify ONE candidate from probed facts.
///
/// The ready-for-integration set is exactly "Done + open PR + not merged". An
/// inconclusive probe is surfaced rather than guessed (a flaky `gh` must not
/// strand a mergeable spec OR — worse — drive a merged one). Order matters:
/// non-Done is filtered first (the cheapest, broadest exclusion), then the
/// inconclusive guard (we can't trust the PR facts), then merged (irreversible
/// already happened), then the open-PR gate. trace:STORY-520 | ai:claude
pub(crate) fn classify_candidate(c: &IntegrationCandidate) -> CandidateVerdict {
    if !c.is_done {
        return CandidateVerdict::SkipNotDone;
    }
    if c.pr_lookup_inconclusive {
        return CandidateVerdict::SkipProbeInconclusive;
    }
    if c.pr_merged {
        return CandidateVerdict::SkipAlreadyMerged;
    }
    // TASK-813: keystone work is parked for the human BEFORE the integrate gate
    // — a supervised / review:draft-only spec is never auto-merged, even with a
    // clean open PR. Checked after `pr_merged` so an already-landed one still
    // reports SkipAlreadyMerged. trace:TASK-813 | ai:claude
    if c.held_for_human {
        return CandidateVerdict::SkipHeldForHuman;
    }
    if c.has_open_pr {
        return CandidateVerdict::Integrate;
    }
    CandidateVerdict::SkipNoPr
}

/// The integration-ready subset of a candidate batch, in input order — the
/// specs the watch-loop will drive this pass. Pure projection over
/// [`classify_candidate`] so the "which specs merge this pass, in what order"
/// decision is testable without any forge/store I/O. The serial-merge invariant
/// (one merge at a time over the shared `main`) is enforced by the CALLER
/// driving these in turn; this only decides the membership + order.
/// trace:STORY-520 | ai:claude
pub(crate) fn ready_for_integration(
    candidates: &[IntegrationCandidate],
) -> Vec<&IntegrationCandidate> {
    candidates
        .iter()
        .filter(|c| classify_candidate(c) == CandidateVerdict::Integrate)
        .collect()
}

// ── TASK-1036: focus-scope membership for the candidate scan ─────────────────
//
// The event-driven integrator `--watch` loop (TASK-1036) optionally scopes its
// candidate scan to a focus subtree (the `aida focus` epic/spec + its transitive
// descendants, STORY-706). This is the pure membership predicate: given a
// candidate's display id and the subtree's display-id set, is the candidate
// in-scope? The caller builds the subtree set from the cache's `descendant_ids`
// closure (TASK-955) mapped to display ids; keeping the test pure means the
// scope filter is unit-testable with synthetic id sets, the same discipline
// `classify_candidate` follows. trace:TASK-1036

/// Is `candidate_id` (a display SPEC-ID, e.g. `STORY-520`) inside the focus
/// subtree? A plain set-membership test over the display-id set the caller built
/// from the focus root's transitive descendants (which INCLUDES the root). PURE.
// trace:TASK-1036 | ai:claude
pub(crate) fn in_focus_scope(
    candidate_id: &str,
    subtree: &std::collections::HashSet<String>,
) -> bool {
    subtree.contains(candidate_id)
}

// ── TASK-836: pre-merge scenario gate ────────────────────────────────────────
//
// `classify_candidate` above answers the *membership* question — "is this a
// Done + open + unmerged + not-keystone PR the integrator should TRY to drive?"
// It is deliberately coarse: it knows nothing about the PR's CI state, review
// verdict, or mergeability. That coarseness was fine for the common case (one
// Done spec, one clean open PR), but running the integrator inside the solo loop
// surfaced the gaps — a PR with CI still running, a pending RequestChanges, or a
// branch behind base that needs a rebase would all reach the `Integrate`
// verdict and be driven *blind* into the `--from-pr` merge.
//
// This pre-merge gate runs ONLY on the members `classify_candidate` already
// admitted, with the *richer* probed facts ([`PrIntegrationState`]). It decides,
// per member, whether to actually drive the merge now or to PARK + report and
// continue (reusing the resilient-drain park-and-continue contract: a parked
// member doesn't stop the loop, and the run exits non-zero when anything was
// parked). It is PURE — given a normalized PR state, it returns the action — so
// the handle-vs-park policy is exhaustively unit-testable with zero forge/store
// I/O, the same discipline `classify_candidate` follows. trace:TASK-836

/// The richer, normalized PR facts the pre-merge gate decides on. Built in
/// `main.rs` from the forge probe (`gh pr list` rollup + per-spec lookup) and
/// the local review-verdict file. Every field is a normalized
/// already-interpreted signal so the decision stays pure + trivially testable —
/// the messy string-parsing of `gh` output lives at the probe boundary, not
/// here. trace:TASK-836 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct PrIntegrationState {
    /// CI rollup for the PR head, as interpreted from the forge.
    pub ci: CiState,
    /// Whether a review verdict / forge review-decision is RequestChanges.
    /// Sourced from BOTH the local `.aida/review-verdicts/` file AND the forge's
    /// `reviewDecision` (`CHANGES_REQUESTED`) — either one is a hard stop.
    pub request_changes_pending: bool,
    /// Whether the PR is mergeable per the forge. `Mergeable` = clean,
    /// `Conflicting` = real merge conflict (never auto-resolve), `Unknown` = the
    /// forge hasn't computed it (or we couldn't tell). The behind-base scenario
    /// (branch behind base, no conflict) is NOT a gate input — the forge merges
    /// behind-base branches via a merge commit, and the caller's `--rebase` step
    /// owns rebasing the branch onto current main before the merge — so it never
    /// reaches this gate as a distinct state. trace:TASK-836
    pub mergeable: MergeableState,
}

/// CI rollup state for a PR head, normalized from the forge. trace:TASK-836
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum CiState {
    /// All required checks passed.
    Passing,
    /// At least one required check failed.
    Failing,
    /// CI is queued / in-progress — no terminal verdict yet.
    Running,
    /// No CI is configured / no checks ran, or we couldn't tell.
    #[default]
    None,
}

/// Mergeability of a PR per the forge. trace:TASK-836
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum MergeableState {
    /// Forge reports the PR merges cleanly.
    Mergeable,
    /// A real merge conflict — never auto-resolved; the spec is parked.
    Conflicting,
    /// The forge hasn't computed mergeability yet (or the probe was
    /// inconclusive). Treated optimistically — the `--from-pr` drive still
    /// re-gates on CI + merge, so "unknown" never ships something unsafe.
    #[default]
    Unknown,
}

/// What the pre-merge gate decides for one already-admitted candidate.
/// trace:TASK-836 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IntegrationAction {
    /// Facts are clean (CI passing/none/unknown, no RequestChanges, mergeable or
    /// unknown) — drive the `--from-pr` merge now.
    Merge,
    /// CI is still running — skip this pass + report; a `--watch` re-scan will
    /// re-decide once CI reaches a terminal state. Bounded by NOT blocking the
    /// serial loop on a single PR. trace:TASK-836
    WaitCi,
    /// A shelvable scenario: park the spec + report ONE legible line, then
    /// continue the loop (resilient-drain park-and-continue). The string is the
    /// human-facing reason. trace:TASK-836
    Park(ParkReason),
}

/// Why the pre-merge gate parked a member — kept structured so the message + the
/// exit-code accounting stay legible and testable. trace:TASK-836
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParkReason {
    /// CI failed — never merge a red PR.
    CiRed,
    /// A RequestChanges review verdict is pending — never merge over it
    /// (matches the known auto-merge-over-RequestChanges hazard).
    RequestChanges,
    /// A real merge conflict — never auto-resolved.
    MergeConflict,
}

impl ParkReason {
    /// One-line, SPEC-ID-free human reason (the caller prefixes the spec id).
    pub fn message(self) -> &'static str {
        match self {
            ParkReason::CiRed => "CI is red — not merging a failing PR (parked for triage)",
            ParkReason::RequestChanges => {
                "a RequestChanges review is pending — not merging over it (parked for triage)"
            }
            ParkReason::MergeConflict => {
                "the PR has a merge conflict — never auto-resolved (parked; rebase/resolve, then re-run)"
            }
        }
    }
}

/// The pure pre-merge gate: given the richer probed PR facts for one
/// already-admitted candidate, decide whether to merge now, wait for CI, or
/// park. Order matters and encodes the safety priority:
///   1. RequestChanges — a human asked for changes; never merge over it, even
///      with green CI (the strongest, most explicit human signal).
///   2. CI red — never merge a failing PR.
///   3. Merge conflict — never auto-resolve.
///   4. CI running — wait (re-decide next pass), don't merge blind.
///   5. otherwise (mergeable / unknown, CI passing / none, no RequestChanges) —
///      Merge. A behind-base branch is rebased by the caller's `--rebase` step
///      before this gate, and the `--from-pr` drive re-gates the merge, so an
///      Unknown-mergeable case is safe to let through (merge refuses, never
///      corrupts).
/// trace:TASK-836 | ai:claude
pub(crate) fn classify_integration_action(s: &PrIntegrationState) -> IntegrationAction {
    if s.request_changes_pending {
        return IntegrationAction::Park(ParkReason::RequestChanges);
    }
    if s.ci == CiState::Failing {
        return IntegrationAction::Park(ParkReason::CiRed);
    }
    if s.mergeable == MergeableState::Conflicting {
        return IntegrationAction::Park(ParkReason::MergeConflict);
    }
    if s.ci == CiState::Running {
        return IntegrationAction::WaitCi;
    }
    IntegrationAction::Merge
}

// ── STORY-335: rebase-conflict forecast (read-only first slice) ──────────────
//
// A deferred batch (`--auto-complete=through-ci`) cuts every branch from the
// same stale main; integration must rebase each onto the advancing main. Before
// landing anything, forecast which members WILL conflict — turning "hope the
// rebase isn't bad" into a checkable preview. This slice is read-only: it uses
// `git merge-tree` (no worktree mutation) and never touches the merge path.
//
// Scope note: each branch is forecast against *current* main independently, so
// it catches conflicts with already-landed code. It does NOT yet model the
// sequential accumulation (a member that only conflicts with an earlier,
// not-yet-landed batch member won't show here) — that sequence-aware forecast
// is a follow-up. trace:STORY-335 | ai:claude

/// Read-only forecast of whether a PR branch rebases cleanly onto current main.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RebaseForecast {
    /// No conflict — the branch integrates onto current main cleanly.
    Clean,
    /// Conflicts in these files (best-effort list; may be empty if unparsed).
    Conflict(Vec<String>),
    /// Couldn't tell (git too old, branch missing, probe error) — never guessed.
    Unknown(String),
}

/// One member's forecast row, in batch order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForecastRow {
    pub id: String,
    pub forecast: RebaseForecast,
}

/// Aggregate counts over a batch forecast. Pure projection for testability.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ForecastSummary {
    pub clean: usize,
    pub conflict: usize,
    pub unknown: usize,
    /// IDs that will conflict, in batch order — the "resolve these first" list.
    pub conflicting_ids: Vec<String>,
}

/// Parse the conflicted file paths from `git merge-tree --write-tree
/// --name-only` output. On conflict (git exits 1) the first line is the written
/// tree OID and the conflicted paths follow until the first blank line; the
/// informational "Auto-merging/CONFLICT" messages come after that blank.
/// trace:STORY-335 | ai:claude
pub(crate) fn parse_merge_tree_conflict_files(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .skip(1) // first line is the written tree OID
        .take_while(|l| !l.trim().is_empty())
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Summarize a batch's per-member forecast rows.
pub(crate) fn summarize_forecast(rows: &[ForecastRow]) -> ForecastSummary {
    let mut s = ForecastSummary::default();
    for r in rows {
        match &r.forecast {
            RebaseForecast::Clean => s.clean += 1,
            RebaseForecast::Conflict(_) => {
                s.conflict += 1;
                s.conflicting_ids.push(r.id.clone());
            }
            RebaseForecast::Unknown(_) => s.unknown += 1,
        }
    }
    s
}

// ── STORY-335: accumulation-strategy selector ───────────────────────────────
//
// The accumulation shape — how a deferred batch's items are grouped before
// integration — is orthogonal to the phase-range. Only `per-item` is built; the
// other two are accepted on the CLI from day one and error cleanly until
// implemented (the established `--no-human=both` "accept-the-value,
// error-until-implemented" pattern). trace:STORY-335 | ai:claude

/// How a deferred batch's items are accumulated before integration.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum IntegrateStrategy {
    /// Branch + PR per item; integrate rebases + merges them in order. The MVP
    /// and the only implemented strategy today.
    PerItem,
    /// All items as commits on one branch; one rebase, one PR. Not built yet.
    OneBranch,
    /// Each item builds on the previous; integrate rebases the stack as a unit.
    /// Not built yet (gated on stacked-branch awareness).
    Stacked,
}

/// A clean "not built yet" message for strategies accepted on the CLI but not
/// yet implemented, or `None` for the supported `per-item` strategy. Lets the
/// flag accept all three values from day one while refusing the unbuilt ones
/// with a pointer rather than a silent no-op. trace:STORY-335 | ai:claude
pub(crate) fn strategy_unsupported_message(strategy: IntegrateStrategy) -> Option<String> {
    match strategy {
        // `per-item` is the MVP; `stacked` is built as a stack-aware ordering +
        // gating layer over the same per-item drive (TASK-841). Only `one-branch`
        // remains unbuilt.
        IntegrateStrategy::PerItem | IntegrateStrategy::Stacked => None,
        IntegrateStrategy::OneBranch => Some(
            "the `one-branch` accumulation strategy isn't built yet — all batch items on one \
             branch with a single rebase + PR is a follow-up. Use `--strategy per-item` (default)."
                .to_string(),
        ),
    }
}

// ── TASK-841: stacked-branch integration planning ───────────────────────────
//
// COMPLETION MODEL (the design decision this task gated on): a stacked batch
// integrates with PER-COMMIT completion — each member is Completed when its OWN
// commit lands on the default branch, via the existing done→completed auto-bump
// (STORY-86), NOT held until the whole stack lands. `completed` already MEANS
// "merged to the default branch", so once a member's PR merges it genuinely is
// on main; an atomic whole-stack model would need a new "pending" barrier and
// would misreport a landed member as still-Done for zero benefit. The auto-bump
// fires per-spec per-merged-commit, so no new completion machinery is needed —
// the integrator's only extra job over `per-item` is to ORDER and GATE.
//
// This planner is the pure core of that ordering/gating: given the ready set +
// each member's PR branch + the recorded stack graph, decide which members are
// mergeable THIS pass (bottom-of-stack first) and which must be DEFERRED because
// their stack-parent hasn't landed yet. Side-effect-free → exhaustively
// unit-tested, exactly like `classify_candidate`.
//
// Scope of THIS slice: merge the mergeable bottom layer safely and defer
// still-stacked members with a legible, actionable line (the drive's `aida pull`
// then cascade-rebases the next layer's worktree; STORY-248). Force-pushing a
// promoted child's PR branch with the stack-aware `git rebase --onto
// origin/main <parent_sha>` — so the second layer becomes mergeable
// automatically instead of waiting on a manual `/aida-rebase` — is the
// TASK-1080 promotion path below (`classify_stacked_promotion`).
// trace:TASK-841 | ai:claude

/// A ready stacked member held back this pass because its stack-parent hasn't
/// landed on the default branch yet.
// trace:TASK-841 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeferredMember {
    /// The deferred member's spec id.
    pub id: String,
    /// The stack-parent branch that must land (merge) first.
    pub blocked_on_branch: String,
    /// The ready member that owns `blocked_on_branch`, when the parent is itself
    /// in this pass's ready set — for a legible "waiting on SPEC-X" line. `None`
    /// when the parent isn't ready (not Done / no PR yet) or already merged but
    /// the child's PR branch still needs a stack-aware rebase.
    pub blocked_on_spec: Option<String>,
}

/// The bottom-up plan for one integrate pass over a stacked ready set.
// trace:TASK-841 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct StackedPlan {
    /// Members mergeable this pass — each a bottom-of-stack (parent is the
    /// default branch) or an independent branch, so a plain rebase-onto-main is
    /// correct. Preserves the caller's input order (stable).
    pub mergeable: Vec<String>,
    /// Members held back this pass — their stack-parent must land first.
    pub deferred: Vec<DeferredMember>,
}

/// Partition a stacked ready set into mergeable-this-pass vs deferred, using the
/// recorded stack graph. A member is mergeable iff it is NOT stacked behind
/// another still-open branch: either it has no stack entry (an independent
/// branch forked from main), or its recorded `parent_branch` IS the default
/// branch (the bottom of a chain). Anything stacked behind another branch is
/// deferred — merging it now would drag the parent's un-squashed commits in
/// under the wrong PR.
// trace:TASK-841 | ai:claude
pub(crate) fn plan_stacked_integration(
    ready_ids: &[String],
    branch_of: &std::collections::HashMap<String, Option<String>>,
    graph: &crate::stacks::StackGraph,
    default_branch: &str,
) -> StackedPlan {
    // Reverse map: PR branch -> ready spec id, so a deferred child can name the
    // ready sibling that owns its parent branch.
    let mut spec_of_branch: std::collections::HashMap<&str, &str> =
        std::collections::HashMap::new();
    for id in ready_ids {
        if let Some(Some(b)) = branch_of.get(id) {
            spec_of_branch.insert(b.as_str(), id.as_str());
        }
    }

    let mut plan = StackedPlan::default();
    for id in ready_ids {
        let branch = match branch_of.get(id).and_then(|b| b.as_deref()) {
            Some(b) => b,
            // No resolved PR branch → treat as independent; keep it mergeable so
            // we never silently drop a ready spec (the drive re-gates it).
            None => {
                plan.mergeable.push(id.clone());
                continue;
            }
        };
        match graph.get(branch) {
            // Not in the stack graph → an independent branch (or a child the
            // cascade already un-stacked). Mergeable; the drive re-gates.
            None => plan.mergeable.push(id.clone()),
            // Bottom of a chain — nothing un-landed sits beneath it.
            Some(entry) if entry.parent_branch == default_branch => plan.mergeable.push(id.clone()),
            // Stacked behind another branch → defer until the parent lands.
            Some(entry) => plan.deferred.push(DeferredMember {
                id: id.clone(),
                blocked_on_branch: entry.parent_branch.clone(),
                blocked_on_spec: spec_of_branch
                    .get(entry.parent_branch.as_str())
                    .map(|s| s.to_string()),
            }),
        }
    }
    plan
}

// ── TASK-1080: stack-aware promotion of a deferred child ─────────────────────
//
// TASK-841's planner defers a member stacked behind another still-open branch,
// and only merges the bottom layer per pass. Once the parent lands (squash-
// merged + branch deleted on origin), the child's ORIGIN PR branch still
// carries the parent's pre-squash commits — the STORY-248 cascade rebases the
// child's WORKTREE locally on `aida pull` but never force-pushes the PR branch,
// so the child stayed deferred behind a manual `/aida-rebase`. This is the
// promotion decision that closes the loop: when the deferred child's parent
// branch is GONE on origin (the merged+deleted signature of `gh pr merge
// --squash --delete-branch`), the integrator rebases the PR branch with the
// stack-aware `git rebase --onto <default> <recorded parent fork SHA>` +
// force-push-with-lease (composing `aida pr rebase --onto-parent`), then lets
// the normal drive merge it — no manual step.
//
// PURE, like every other integrate decision: the caller probes (stack entry,
// live ls-remote, PR number) and acts (subprocess); this only decides. The
// churn guard is two-layered: an entry the cascade already removed never
// reaches here as deferred (the planner classifies it mergeable), and a stale
// recorded SHA (branch already rebased by someone else) is refused fail-closed
// by the ancestor guard inside `aida pr rebase --onto-parent` itself.
// trace:TASK-1080

/// What to do with ONE deferred stacked member whose parent is not in this
/// pass's ready set.
// trace:TASK-1080 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StackedPromotion {
    /// Parent branch is gone on origin (merged + deleted) and both the stack
    /// record and the PR number are in hand — run the stack-aware rebase and
    /// force-push, then merge this pass. `parent_sha` is the recorded
    /// fork-point SHA (the second argument of the 3-arg `git rebase --onto`
    /// form).
    Promote { parent_sha: String },
    /// The parent branch still exists on origin — it hasn't merged yet; keep
    /// deferring (a later pass promotes once it lands).
    ParentStillOpen,
    /// The ls-remote probe was inconclusive (offline / auth) — never rebase
    /// blind on "couldn't tell"; keep deferring and re-probe next pass.
    ProbeInconclusive,
    /// Churn guard (defensive): no stack entry recorded for the child branch —
    /// the cascade already un-stacked it. The planner classifies such a member
    /// mergeable, so a deferred member without an entry means the graph changed
    /// under us; keep deferring this pass and let the next re-plan decide.
    ChurnedNoEntry,
    /// Parent merged, but no PR number resolved for the child — nothing to
    /// force-push against; keep deferring with a manual pointer.
    NoPrNumber,
}

/// The pure promotion decision for one deferred stacked member. Inputs are
/// already-probed facts:
///   * `recorded_parent_sha` — the stack entry's `parent_branch_sha` for the
///     child's PR branch, `None` when the entry is gone (churn);
///   * `parent_gone_on_origin` — live ls-remote verdict for the parent branch:
///     `Some(true)` = ref absent (merged+deleted), `Some(false)` = still there,
///     `None` = probe failed (offline/auth) — surfaced, never guessed;
///   * `has_pr_number` — whether the child's open-PR number resolved.
///
/// Order encodes the safety priority: trust the graph first (no entry → the
/// plan is stale), then the probe quality, then the parent's state, then the
/// push preconditions.
// trace:TASK-1080 | ai:claude
pub(crate) fn classify_stacked_promotion(
    recorded_parent_sha: Option<&str>,
    parent_gone_on_origin: Option<bool>,
    has_pr_number: bool,
) -> StackedPromotion {
    let Some(parent_sha) = recorded_parent_sha else {
        return StackedPromotion::ChurnedNoEntry;
    };
    match parent_gone_on_origin {
        None => StackedPromotion::ProbeInconclusive,
        Some(false) => StackedPromotion::ParentStillOpen,
        Some(true) => {
            if !has_pr_number {
                return StackedPromotion::NoPrNumber;
            }
            StackedPromotion::Promote {
                parent_sha: parent_sha.to_string(),
            }
        }
    }
}

/// Assemble the `aida pr rebase <N> --no-smoke --onto-parent <SHA>` argv the
/// integrator self-invokes to promote a deferred stacked child. Pure, mirroring
/// `drive_args`: the stack-aware rebase + force-push-with-lease live in the ONE
/// `pr rebase` machinery (temp worktree, BUG-640 patch-id guard, lease-anchored
/// push), never inlined here. `--no-smoke` because the `--from-pr` drive that
/// follows runs CI.
// trace:TASK-1080 | ai:claude
pub(crate) fn promotion_rebase_args(pr_number: u32, parent_sha: &str) -> Vec<String> {
    vec![
        "pr".to_string(),
        "rebase".to_string(),
        pr_number.to_string(),
        "--no-smoke".to_string(),
        "--onto-parent".to_string(),
        parent_sha.to_string(),
    ]
}

/// Parse a strategy from its CLI/config string form (`per-item`, `one-branch`,
/// `stacked`; underscores tolerated). trace:TASK-691 | ai:claude
pub(crate) fn parse_strategy(s: &str) -> Option<IntegrateStrategy> {
    match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "per-item" => Some(IntegrateStrategy::PerItem),
        "one-branch" => Some(IntegrateStrategy::OneBranch),
        "stacked" => Some(IntegrateStrategy::Stacked),
        _ => None,
    }
}

/// TASK-691: read the project-default accumulation strategy from a
/// `.aida/config.toml` body — the `strategy` key under `[integrate]`. Pure
/// (takes the file content) + section-aware, mirroring the hand-rolled scanner
/// the `[advisor]` config uses so we don't pull a serde-TOML dep for one
/// scalar. Returns None when the section/key is absent or the value is
/// unrecognized; the caller falls back to the `per-item` default.
/// trace:TASK-691 | ai:claude
pub(crate) fn integrate_strategy_from_config(content: &str) -> Option<IntegrateStrategy> {
    let mut in_integrate = false;
    for raw in content.lines() {
        // Strip inline comments + trim.
        let line = raw.split('#').next().unwrap_or(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(stripped) = line.strip_prefix('[') {
            in_integrate = stripped.trim_end_matches(']').trim() == "integrate";
            continue;
        }
        if in_integrate {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == "strategy" {
                    let v = v.trim().trim_matches('"').trim_matches('\'').trim();
                    return parse_strategy(v);
                }
            }
        }
    }
    None
}

// ── TASK-843: multiple open PRs for one spec — newest-canonical policy ────────
//
// `detect_open_pr_for_spec_via_forge` returns at most one PR per spec, so a spec
// with >1 open PR (a reopened/duplicate, or two branches both naming the spec in
// their body) was resolved by whichever the forge happened to return first — a
// silent, non-deterministic pick. This decides that case explicitly:
//
//   * exactly one mergeable PR → integrate it (the common case);
//   * several mergeable PRs   → the NEWEST (highest PR number) is canonical;
//     integrate it and report the rest as ignored-this-pass (legible, not
//     silent — the operator can close the dupes);
//   * none mergeable          → PARK + report the candidate numbers (reuses the
//     park-and-continue contract; the exit code already mirrors a park).
//
// Newest-wins is the safe default: a reopened/rebased duplicate is almost always
// the higher number, and the `--from-pr` drive re-gates the merge, so an ignored
// stale PR is never silently merged. The decision is PURE over the candidate
// list so it is exhaustively unit-testable with synthetic PR numbers, the same
// discipline `classify_candidate` / `classify_integration_action` follow.
// trace:TASK-843

/// One open PR found for a spec, reduced to the two facts the canonical-pick
/// needs: its number (the newest-wins key) and whether it is mergeable (clean /
/// admissible this pass). The richer per-PR gate ([`classify_integration_action`])
/// still runs on the chosen PR afterwards. trace:TASK-843 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PrCandidate {
    /// The forge PR number — also the newest-canonical tiebreak (higher = newer).
    pub number: u64,
    /// True when this PR is clean/mergeable enough to be a merge candidate this
    /// pass (no merge conflict, not already merged). Non-mergeable PRs can't be
    /// the canonical pick — if NONE are mergeable the spec is parked.
    pub mergeable: bool,
}

/// What to do when a spec has one-or-more open PRs. trace:TASK-843 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CanonicalPrDecision {
    /// No open PRs at all — nothing to integrate (the caller skips quietly,
    /// matching `CandidateVerdict::SkipNoPr`).
    NoPr,
    /// Integrate `chosen` (the newest mergeable PR). `ignored` lists the OTHER
    /// open PR numbers this pass skipped (empty in the common single-PR case),
    /// in descending order, so the caller can emit one legible "ignored #N, #M"
    /// line. trace:TASK-843
    Integrate { chosen: u64, ignored: Vec<u64> },
    /// One-or-more open PRs but NONE are mergeable — park + report the
    /// candidates (descending order). Reuses the existing Park path.
    Park { candidates: Vec<u64> },
}

/// Pure newest-canonical policy for a spec's open PRs. Given the candidate PR
/// list (any order), pick the newest mergeable PR as canonical, report the rest
/// as ignored, or park when none are mergeable. Order of the returned
/// `ignored` / `candidates` lists is descending by PR number (newest first) for
/// stable, legible output. trace:TASK-843 | ai:claude
pub(crate) fn select_canonical_pr(prs: &[PrCandidate]) -> CanonicalPrDecision {
    if prs.is_empty() {
        return CanonicalPrDecision::NoPr;
    }
    // Newest-first by number, stable + deterministic regardless of input order.
    let mut sorted: Vec<&PrCandidate> = prs.iter().collect();
    sorted.sort_by(|a, b| b.number.cmp(&a.number));

    let mergeable: Vec<&PrCandidate> = sorted.iter().copied().filter(|p| p.mergeable).collect();
    if mergeable.is_empty() {
        // None clean — park, reporting every candidate (newest first).
        return CanonicalPrDecision::Park {
            candidates: sorted.iter().map(|p| p.number).collect(),
        };
    }
    // Newest mergeable is canonical; every OTHER open PR (mergeable or not) is
    // ignored this pass and reported so the operator can close the dupes.
    let chosen = mergeable[0].number;
    let ignored: Vec<u64> = sorted
        .iter()
        .map(|p| p.number)
        .filter(|n| *n != chosen)
        .collect();
    CanonicalPrDecision::Integrate { chosen, ignored }
}

// ── TASK-842: multi-spec PR recognition + dedupe ─────────────────────────────
//
// A single PR whose commit trailers reference MULTIPLE specs (a cluster PR, e.g.
// `(BUG-566) (BUG-567)`) was previously keyed on one spec id by the integrator;
// the SIBLING specs on the PR were unrecognized, and — because the integrator
// scans Done specs and looks up a PR per spec — the SAME PR could be selected
// once per member spec and driven repeatedly.
//
// The actual completion is unchanged: the merge-trailer → auto-bump path already
// promotes every trailered spec to Completed on merge. This is RECOGNITION +
// DEDUPE + REPORTING only:
//
//   * detect the full set of spec-IDs a PR's trailers reference (the caller
//     extracts them via `extract_spec_ids_from_commit`; this layer dedupes +
//     reports so it stays pure over the trailer set);
//   * collapse a multi-spec PR to ONE integration unit (dedupe by PR number) so
//     it is driven once, not N times;
//   * emit a legible line naming ALL specs the merge will complete.
//
// Pure over the (pr_number, spec_ids) recognition rows so it is unit-testable
// with synthetic trailer sets. trace:TASK-842

/// One PR recognized as completing one-or-more specs on merge. `spec_ids` is the
/// full trailer set the PR references (deduped, in first-seen order).
/// trace:TASK-842 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrCompletion {
    pub number: u64,
    pub spec_ids: Vec<String>,
}

/// Collapse per-spec recognition rows into one [`PrCompletion`] per PR number,
/// so a multi-spec cluster PR — which the Done-spec scan surfaces once per member
/// spec — is integrated ONCE, not N times. Input is `(pr_number, spec_ids)` rows
/// (one per Done spec the scan considered); output is one row per distinct PR
/// number, with the UNION of every spec the PR's trailers reference (first-seen
/// order across rows preserved, case-insensitive de-dup). PR rows are returned
/// in first-seen order so the report is stable. trace:TASK-842 | ai:claude
pub(crate) fn dedupe_pr_completions(rows: &[(u64, Vec<String>)]) -> Vec<PrCompletion> {
    let mut order: Vec<u64> = Vec::new();
    let mut by_number: std::collections::HashMap<u64, Vec<String>> =
        std::collections::HashMap::new();
    for (number, spec_ids) in rows {
        let entry = by_number.entry(*number).or_default();
        if !order.contains(number) {
            order.push(*number);
        }
        for id in spec_ids {
            let id = id.trim();
            if id.is_empty() {
                continue;
            }
            if !entry.iter().any(|x| x.eq_ignore_ascii_case(id)) {
                entry.push(id.to_string());
            }
        }
    }
    order
        .into_iter()
        .map(|number| PrCompletion {
            number,
            spec_ids: by_number.remove(&number).unwrap_or_default(),
        })
        .collect()
}

/// One legible, SPEC-ID-bearing line for a multi-spec PR completion — the
/// "integrating PR #N → completes BUG-566, BUG-567" report. The spec ids ARE the
/// payload here (developer-facing integrator output that names what the merge
/// completes), so they stay in the line. trace:TASK-842 | ai:claude
pub(crate) fn describe_pr_completion(c: &PrCompletion) -> String {
    if c.spec_ids.is_empty() {
        format!(
            "integrating PR #{} → completes (no trailered spec)",
            c.number
        )
    } else {
        format!(
            "integrating PR #{} → completes {}",
            c.number,
            c.spec_ids.join(", ")
        )
    }
}

/// Assemble the `aida queue work <id> --auto-complete --from-pr` argv the
/// integrator self-invokes per ready PR. Pure, so the routing guarantee is
/// pinned by a unit test (and the `orchestration_routing` guardrail).
///
/// ONE per-spec orchestration engine (ADR-7): `integrate` does NOT run its own
/// merge lifecycle — it hands each Done+PR spec to the SAME
/// `--auto-complete` engine `aida zen` and `aida queue work` use, entering at
/// the reviewer phase via `--from-pr` (phases 3-6: review → merge → pull →
/// build). It differs from zen only in SCOPE (an already-Done PR rather than a
/// from-scratch spec) and START PHASE, never in the per-spec lifecycle. The
/// caller adds the `AIDA_DRAIN_FORCE` env scoped to the child (TASK-1050) and
/// runs it in the integrator's own checkout — those are delivery concerns, not
/// part of the routing argv asserted here.
// trace:ADR-7 trace:ADR-9 | ai:claude
pub(crate) fn drive_args(id: &str) -> Vec<String> {
    vec![
        "queue".to_string(),
        "work".to_string(),
        id.to_string(),
        "--auto-complete".to_string(),
        "--from-pr".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(
        id: &str,
        is_done: bool,
        has_open_pr: bool,
        pr_merged: bool,
        inconclusive: bool,
    ) -> IntegrationCandidate {
        IntegrationCandidate {
            id: id.to_string(),
            is_done,
            has_open_pr,
            pr_merged,
            pr_lookup_inconclusive: inconclusive,
            held_for_human: false,
        }
    }

    // TASK-813: a keystone spec (supervised / review:draft-only) with a clean
    // open PR is PARKED, not integrated — even though it's Done + open +
    // unmerged. An already-merged keystone spec still reports SkipAlreadyMerged
    // (the held check is after the merged check).
    #[test]
    fn held_for_human_keystone_is_parked_not_integrated() {
        let mut c = candidate("STORY-1", true, true, false, false);
        // baseline: without the flag it would integrate.
        assert_eq!(classify_candidate(&c), CandidateVerdict::Integrate);
        c.held_for_human = true;
        assert_eq!(classify_candidate(&c), CandidateVerdict::SkipHeldForHuman);
        // held + already-merged → still SkipAlreadyMerged (merged check first).
        c.pr_merged = true;
        assert_eq!(classify_candidate(&c), CandidateVerdict::SkipAlreadyMerged);
        // held is not in the ready set.
        let held = candidate("STORY-2", true, true, false, false);
        let mut held = held;
        held.held_for_human = true;
        assert!(ready_for_integration(std::slice::from_ref(&held)).is_empty());
    }

    #[test]
    fn done_with_open_unmerged_pr_integrates() {
        let c = candidate("STORY-1", true, true, false, false);
        assert_eq!(classify_candidate(&c), CandidateVerdict::Integrate);
    }

    #[test]
    fn not_done_is_excluded_even_with_open_pr() {
        // An InProgress spec with an open PR is the producer still working —
        // never the integrator's to drive.
        let c = candidate("STORY-2", false, true, false, false);
        assert_eq!(classify_candidate(&c), CandidateVerdict::SkipNotDone);
    }

    #[test]
    fn done_without_pr_is_skipped() {
        let c = candidate("STORY-3", true, false, false, false);
        assert_eq!(classify_candidate(&c), CandidateVerdict::SkipNoPr);
    }

    #[test]
    fn done_with_merged_pr_is_skipped_not_redriven() {
        // The irreversible merge already happened; driving --from-pr would be
        // refused (TASK-405 RefuseAlreadyMerged). We must not even attempt it.
        let c = candidate("STORY-4", true, true, true, false);
        assert_eq!(classify_candidate(&c), CandidateVerdict::SkipAlreadyMerged);
    }

    #[test]
    fn merged_takes_precedence_over_open_flag() {
        // Defensive: even if both flags are set, merged wins — never re-drive.
        let c = candidate("STORY-5", true, true, true, false);
        assert_ne!(classify_candidate(&c), CandidateVerdict::Integrate);
    }

    #[test]
    fn inconclusive_probe_is_surfaced_not_guessed() {
        // gh missing / auth / network: we cannot tell whether a PR exists —
        // skip and report, never treat as no-PR and never drive blind.
        let c = candidate("STORY-6", true, false, false, true);
        assert_eq!(
            classify_candidate(&c),
            CandidateVerdict::SkipProbeInconclusive
        );
    }

    #[test]
    fn inconclusive_guard_runs_before_merged_and_open() {
        // Even with has_open_pr set, an inconclusive probe means the facts are
        // untrustworthy — surface rather than integrate.
        let c = candidate("STORY-7", true, true, false, true);
        assert_eq!(
            classify_candidate(&c),
            CandidateVerdict::SkipProbeInconclusive
        );
    }

    #[test]
    fn not_done_short_circuits_before_inconclusive() {
        // A non-Done spec is excluded regardless of probe quality (we wouldn't
        // even have probed it in practice).
        let c = candidate("STORY-8", false, false, false, true);
        assert_eq!(classify_candidate(&c), CandidateVerdict::SkipNotDone);
    }

    #[test]
    fn ready_set_filters_and_preserves_order() {
        let candidates = vec![
            candidate("A", true, true, false, false),  // integrate
            candidate("B", false, true, false, false), // not done
            candidate("C", true, false, false, false), // no pr
            candidate("D", true, true, true, false),   // merged
            candidate("E", true, true, false, false),  // integrate
            candidate("F", true, false, false, true),  // inconclusive
        ];
        let ready = ready_for_integration(&candidates);
        let ids: Vec<&str> = ready.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["A", "E"]);
    }

    #[test]
    fn empty_batch_yields_empty_ready_set() {
        assert!(ready_for_integration(&[]).is_empty());
    }

    // ── TASK-1036 focus-scope membership ────────────────────────────────────

    #[test]
    fn in_focus_scope_filters_out_of_subtree_candidates() {
        // The subtree set is the focus root + its transitive descendants, as
        // display ids. A candidate in the set is in-scope; anything else is out.
        let subtree: std::collections::HashSet<String> = [
            "EPIC-54".to_string(),
            "STORY-1".to_string(),
            "TASK-9".to_string(),
        ]
        .into_iter()
        .collect();
        assert!(in_focus_scope("EPIC-54", &subtree), "the focus root itself");
        assert!(in_focus_scope("STORY-1", &subtree), "a descendant");
        assert!(in_focus_scope("TASK-9", &subtree), "a deeper descendant");
        assert!(
            !in_focus_scope("STORY-99", &subtree),
            "a spec under a DIFFERENT epic is out of scope"
        );
        // An empty subtree admits nothing.
        assert!(!in_focus_scope(
            "STORY-1",
            &std::collections::HashSet::new()
        ));
    }

    #[test]
    fn all_skipped_yields_empty_ready_set() {
        let candidates = vec![
            candidate("A", false, true, false, false),
            candidate("B", true, false, false, false),
            candidate("C", true, true, true, false),
        ];
        assert!(ready_for_integration(&candidates).is_empty());
    }

    // ── TASK-836 pre-merge scenario gate ────────────────────────────────────

    fn state(ci: CiState, rc: bool, m: MergeableState) -> PrIntegrationState {
        PrIntegrationState {
            ci,
            request_changes_pending: rc,
            mergeable: m,
        }
    }

    #[test]
    fn clean_pr_merges() {
        // CI passing, no RequestChanges, mergeable → drive the merge.
        let s = state(CiState::Passing, false, MergeableState::Mergeable);
        assert_eq!(classify_integration_action(&s), IntegrationAction::Merge);
    }

    #[test]
    fn no_ci_and_unknown_mergeable_still_merges() {
        // A repo with no CI and a forge that hasn't computed mergeability is the
        // common small-project case — let it through; --from-pr re-gates merge.
        let s = state(CiState::None, false, MergeableState::Unknown);
        assert_eq!(classify_integration_action(&s), IntegrationAction::Merge);
    }

    #[test]
    fn ci_running_waits_not_merges() {
        // CI in-progress: skip this pass + wait, never merge blind.
        let s = state(CiState::Running, false, MergeableState::Mergeable);
        assert_eq!(classify_integration_action(&s), IntegrationAction::WaitCi);
    }

    #[test]
    fn ci_red_parks() {
        let s = state(CiState::Failing, false, MergeableState::Mergeable);
        assert_eq!(
            classify_integration_action(&s),
            IntegrationAction::Park(ParkReason::CiRed)
        );
    }

    #[test]
    fn request_changes_parks_even_with_green_ci() {
        // The strongest human signal: a pending RequestChanges is never merged
        // over, even with passing CI and a mergeable branch.
        let s = state(CiState::Passing, true, MergeableState::Mergeable);
        assert_eq!(
            classify_integration_action(&s),
            IntegrationAction::Park(ParkReason::RequestChanges)
        );
    }

    #[test]
    fn request_changes_takes_precedence_over_ci_red() {
        // Both bad: RequestChanges wins the message (most explicit signal),
        // though either alone parks. Ordering is the contract.
        let s = state(CiState::Failing, true, MergeableState::Conflicting);
        assert_eq!(
            classify_integration_action(&s),
            IntegrationAction::Park(ParkReason::RequestChanges)
        );
    }

    #[test]
    fn merge_conflict_parks_never_auto_resolved() {
        let s = state(CiState::Passing, false, MergeableState::Conflicting);
        assert_eq!(
            classify_integration_action(&s),
            IntegrationAction::Park(ParkReason::MergeConflict)
        );
    }

    #[test]
    fn conflict_parks_before_ci_running_is_considered() {
        // A conflicting PR with CI still running is parked (conflict is
        // terminal), not WaitCi — no point waiting on CI for an unmergeable PR.
        let s = state(CiState::Running, false, MergeableState::Conflicting);
        assert_eq!(
            classify_integration_action(&s),
            IntegrationAction::Park(ParkReason::MergeConflict)
        );
    }

    #[test]
    fn ci_red_parks_before_conflict_message() {
        // CI red is reported when there's no RequestChanges, even if also
        // conflicting — the CI signal is checked before the conflict signal.
        let s = state(CiState::Failing, false, MergeableState::Conflicting);
        assert_eq!(
            classify_integration_action(&s),
            IntegrationAction::Park(ParkReason::CiRed)
        );
    }

    #[test]
    fn park_reason_messages_are_distinct_and_nonempty() {
        // Legibility: each park reason emits its own one-line, SPEC-ID-free why.
        let msgs = [
            ParkReason::CiRed.message(),
            ParkReason::RequestChanges.message(),
            ParkReason::MergeConflict.message(),
        ];
        for m in msgs {
            assert!(!m.is_empty());
            // SPEC-IDs must not leak into user-facing integrator output.
            assert!(!m.contains("TASK-"));
        }
        assert_ne!(msgs[0], msgs[1]);
        assert_ne!(msgs[1], msgs[2]);
    }

    // ── STORY-335 forecast helpers ──────────────────────────────────────────

    #[test]
    fn parse_merge_tree_conflict_files_extracts_paths_before_blank() {
        // Real `git merge-tree --write-tree --name-only` conflict output:
        // OID line, conflicted paths, blank line, then info messages.
        let out = "663d4da1f330\nsrc/main.rs\nsrc/lib.rs\n\nAuto-merging src/main.rs\nCONFLICT (content): Merge conflict in src/main.rs\n";
        assert_eq!(
            parse_merge_tree_conflict_files(out),
            vec!["src/main.rs".to_string(), "src/lib.rs".to_string()]
        );
    }

    #[test]
    fn parse_merge_tree_conflict_files_handles_single_file_no_trailing() {
        let out = "abc123\nf.txt\n\nCONFLICT (content): Merge conflict in f.txt";
        assert_eq!(parse_merge_tree_conflict_files(out), vec!["f.txt"]);
    }

    #[test]
    fn parse_merge_tree_conflict_files_empty_when_only_oid() {
        // Defensive: malformed/short output yields no files rather than panicking.
        assert!(parse_merge_tree_conflict_files("justoid\n").is_empty());
        assert!(parse_merge_tree_conflict_files("").is_empty());
    }

    fn row(id: &str, f: RebaseForecast) -> ForecastRow {
        ForecastRow {
            id: id.to_string(),
            forecast: f,
        }
    }

    #[test]
    fn summarize_forecast_counts_and_orders_conflicts() {
        let rows = vec![
            row("A", RebaseForecast::Clean),
            row(
                "B",
                RebaseForecast::Conflict(vec!["src/main.rs".to_string()]),
            ),
            row("C", RebaseForecast::Clean),
            row("D", RebaseForecast::Conflict(vec![])),
            row("E", RebaseForecast::Unknown("gh".to_string())),
        ];
        let s = summarize_forecast(&rows);
        assert_eq!(s.clean, 2);
        assert_eq!(s.conflict, 2);
        assert_eq!(s.unknown, 1);
        // Conflicting IDs preserved in batch order — the "resolve first" list.
        assert_eq!(s.conflicting_ids, vec!["B".to_string(), "D".to_string()]);
    }

    #[test]
    fn summarize_forecast_empty_is_all_zero() {
        let s = summarize_forecast(&[]);
        assert_eq!(s, ForecastSummary::default());
        assert!(s.conflicting_ids.is_empty());
    }

    // ── STORY-335 strategy selector ─────────────────────────────────────────

    #[test]
    fn per_item_and_stacked_strategies_are_supported() {
        // TASK-841: stacked is now built (a stack-aware layer over per-item).
        assert!(strategy_unsupported_message(IntegrateStrategy::PerItem).is_none());
        assert!(strategy_unsupported_message(IntegrateStrategy::Stacked).is_none());
    }

    #[test]
    fn one_branch_errors_cleanly_pointing_at_followup() {
        // Accepted on the CLI but not built — must refuse with a pointer to the
        // supported strategy, never silently no-op.
        let one = strategy_unsupported_message(IntegrateStrategy::OneBranch)
            .expect("one-branch is unsupported");
        assert!(one.contains("one-branch"));
        assert!(one.contains("per-item"));
    }

    #[test]
    fn parse_strategy_accepts_all_three_and_rejects_garbage() {
        assert_eq!(parse_strategy("per-item"), Some(IntegrateStrategy::PerItem));
        assert_eq!(
            parse_strategy("one-branch"),
            Some(IntegrateStrategy::OneBranch)
        );
        assert_eq!(
            parse_strategy(" Stacked "),
            Some(IntegrateStrategy::Stacked)
        );
        // underscores tolerated (toml-ish), case-insensitive.
        assert_eq!(
            parse_strategy("ONE_BRANCH"),
            Some(IntegrateStrategy::OneBranch)
        );
        assert_eq!(parse_strategy("bogus"), None);
        assert_eq!(parse_strategy(""), None);
    }

    // ── TASK-841 stacked integration planner ────────────────────────────────

    fn branch_map(
        pairs: &[(&str, Option<&str>)],
    ) -> std::collections::HashMap<String, Option<String>> {
        pairs
            .iter()
            .map(|(id, b)| (id.to_string(), b.map(|s| s.to_string())))
            .collect()
    }

    fn stack_entry(branch: &str, parent: &str) -> crate::stacks::StackEntry {
        crate::stacks::StackEntry {
            branch: branch.to_string(),
            parent_branch: parent.to_string(),
            parent_branch_sha: "sha".to_string(),
            spec_id: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn plan_stacked_no_graph_is_all_mergeable() {
        // No stack entries → every ready member is an independent branch.
        let ready = vec!["A".to_string(), "B".to_string()];
        let branches = branch_map(&[("A", Some("a")), ("B", Some("b"))]);
        let graph = crate::stacks::StackGraph::default();
        let plan = plan_stacked_integration(&ready, &branches, &graph, "main");
        assert_eq!(plan.mergeable, vec!["A".to_string(), "B".to_string()]);
        assert!(plan.deferred.is_empty());
    }

    #[test]
    fn plan_stacked_defers_child_behind_ready_parent() {
        // A (branch a, forked from main) ← B (branch b, forked from a). Both
        // ready. A is the bottom → mergeable; B is stacked behind a → deferred,
        // and it names A as the ready sibling it waits on.
        let ready = vec!["A".to_string(), "B".to_string()];
        let branches = branch_map(&[("A", Some("a")), ("B", Some("b"))]);
        let mut graph = crate::stacks::StackGraph::default();
        crate::stacks::add(&mut graph, stack_entry("a", "main"));
        crate::stacks::add(&mut graph, stack_entry("b", "a"));
        let plan = plan_stacked_integration(&ready, &branches, &graph, "main");
        assert_eq!(plan.mergeable, vec!["A".to_string()]);
        assert_eq!(plan.deferred.len(), 1);
        assert_eq!(plan.deferred[0].id, "B");
        assert_eq!(plan.deferred[0].blocked_on_branch, "a");
        assert_eq!(plan.deferred[0].blocked_on_spec.as_deref(), Some("A"));
    }

    #[test]
    fn plan_stacked_defers_child_whose_parent_is_not_ready() {
        // The child's parent branch merged already (or isn't in the ready set),
        // so blocked_on_spec is None — but the child is still deferred until the
        // TASK-1080 stack-aware promotion rebases + force-pushes its PR branch.
        let ready = vec!["B".to_string()];
        let branches = branch_map(&[("B", Some("b"))]);
        let mut graph = crate::stacks::StackGraph::default();
        crate::stacks::add(&mut graph, stack_entry("b", "a"));
        let plan = plan_stacked_integration(&ready, &branches, &graph, "main");
        assert!(plan.mergeable.is_empty());
        assert_eq!(plan.deferred.len(), 1);
        assert_eq!(plan.deferred[0].blocked_on_spec, None);
    }

    #[test]
    fn plan_stacked_member_with_no_branch_stays_mergeable() {
        // A ready spec whose PR branch couldn't be resolved must not be silently
        // dropped — keep it mergeable and let the drive re-gate it.
        let ready = vec!["A".to_string()];
        let branches = branch_map(&[("A", None)]);
        let graph = crate::stacks::StackGraph::default();
        let plan = plan_stacked_integration(&ready, &branches, &graph, "main");
        assert_eq!(plan.mergeable, vec!["A".to_string()]);
        assert!(plan.deferred.is_empty());
    }

    // ── TASK-1080 stack-aware promotion of a deferred child ─────────────────

    #[test]
    fn promotion_fires_when_parent_gone_with_record_and_pr() {
        // The whole point: parent merged+deleted on origin, stack record + PR
        // number in hand → promote via the stack-aware rebase.
        assert_eq!(
            classify_stacked_promotion(Some("abc123"), Some(true), true),
            StackedPromotion::Promote {
                parent_sha: "abc123".to_string()
            }
        );
    }

    #[test]
    fn promotion_waits_while_parent_branch_still_on_origin() {
        // Parent hasn't merged yet — promoting now would DROP the parent's
        // commits from the child branch. Keep deferring.
        assert_eq!(
            classify_stacked_promotion(Some("abc123"), Some(false), true),
            StackedPromotion::ParentStillOpen
        );
    }

    #[test]
    fn promotion_never_rebases_blind_on_inconclusive_probe() {
        // Offline / auth failure: "couldn't tell" is surfaced, never treated
        // as "gone" — a false promotion would rewrite a PR branch wrongly.
        assert_eq!(
            classify_stacked_promotion(Some("abc123"), None, true),
            StackedPromotion::ProbeInconclusive
        );
    }

    #[test]
    fn promotion_churn_guard_when_cascade_removed_the_entry() {
        // The cascade already un-stacked the child (entry gone) — the plan is
        // stale; defer and re-plan rather than rebase off a missing record.
        // The entry check runs FIRST, regardless of the other facts.
        assert_eq!(
            classify_stacked_promotion(None, Some(true), true),
            StackedPromotion::ChurnedNoEntry
        );
        assert_eq!(
            classify_stacked_promotion(None, None, false),
            StackedPromotion::ChurnedNoEntry
        );
    }

    #[test]
    fn promotion_requires_a_pr_number_to_force_push() {
        // Parent gone but no open-PR number resolved — nothing to
        // force-push-with-lease against; defer with a manual pointer.
        assert_eq!(
            classify_stacked_promotion(Some("abc123"), Some(true), false),
            StackedPromotion::NoPrNumber
        );
    }

    #[test]
    fn promotion_rebase_args_compose_pr_rebase_onto_parent() {
        // The promotion routes through the ONE pr-rebase machinery (temp
        // worktree + BUG-640 guard + force-with-lease), with the 3-arg --onto
        // form and no local smoke (the --from-pr drive runs CI).
        assert_eq!(
            promotion_rebase_args(57, "deadbeef"),
            vec![
                "pr".to_string(),
                "rebase".to_string(),
                "57".to_string(),
                "--no-smoke".to_string(),
                "--onto-parent".to_string(),
                "deadbeef".to_string(),
            ]
        );
    }

    #[test]
    fn config_reads_integrate_strategy_from_its_section() {
        let toml = "\
[advisor]
strategy = \"stacked\"

[integrate]
# the default accumulation strategy
strategy = \"one-branch\"

[telemetry]
enabled = true
";
        // Reads the [integrate] section's strategy, not [advisor]'s.
        assert_eq!(
            integrate_strategy_from_config(toml),
            Some(IntegrateStrategy::OneBranch)
        );
    }

    #[test]
    fn config_strategy_absent_or_unknown_is_none() {
        assert_eq!(integrate_strategy_from_config(""), None);
        assert_eq!(
            integrate_strategy_from_config("[integrate]\nother = \"x\"\n"),
            None
        );
        assert_eq!(
            integrate_strategy_from_config("[integrate]\nstrategy = \"bogus\"\n"),
            None
        );
        // strategy outside the [integrate] section is ignored.
        assert_eq!(
            integrate_strategy_from_config("[other]\nstrategy = \"stacked\"\n"),
            None
        );
    }

    // ── TASK-843 newest-canonical policy for multiple PRs per spec ───────────

    fn pr(number: u64, mergeable: bool) -> PrCandidate {
        PrCandidate { number, mergeable }
    }

    #[test]
    fn no_open_prs_yields_nopr() {
        assert_eq!(select_canonical_pr(&[]), CanonicalPrDecision::NoPr);
    }

    #[test]
    fn single_mergeable_pr_integrates_with_no_ignored() {
        // The common case: exactly one clean open PR → integrate it, nothing
        // ignored.
        let d = select_canonical_pr(&[pr(42, true)]);
        assert_eq!(
            d,
            CanonicalPrDecision::Integrate {
                chosen: 42,
                ignored: vec![],
            }
        );
    }

    #[test]
    fn multiple_mergeable_prs_newest_wins_rest_ignored() {
        // Several clean PRs: the highest number is canonical, the rest are
        // reported as ignored this pass (descending order).
        let d = select_canonical_pr(&[pr(10, true), pr(57, true), pr(31, true)]);
        assert_eq!(
            d,
            CanonicalPrDecision::Integrate {
                chosen: 57,
                ignored: vec![31, 10],
            }
        );
    }

    #[test]
    fn newest_wins_independent_of_input_order() {
        // Determinism: any input order yields the same canonical pick + ignored
        // list (descending).
        let a = select_canonical_pr(&[pr(57, true), pr(10, true), pr(31, true)]);
        let b = select_canonical_pr(&[pr(31, true), pr(57, true), pr(10, true)]);
        assert_eq!(a, b);
        assert_eq!(
            a,
            CanonicalPrDecision::Integrate {
                chosen: 57,
                ignored: vec![31, 10],
            }
        );
    }

    #[test]
    fn newest_mergeable_wins_even_when_a_higher_pr_is_unmergeable() {
        // PR #99 is the newest but conflicting; the newest MERGEABLE (#57) is
        // canonical, and #99 is still reported as ignored (operator can close).
        let d = select_canonical_pr(&[pr(99, false), pr(57, true), pr(10, true)]);
        assert_eq!(
            d,
            CanonicalPrDecision::Integrate {
                chosen: 57,
                ignored: vec![99, 10],
            }
        );
    }

    #[test]
    fn none_mergeable_parks_reporting_all_candidates() {
        // No clean PR among several → park, reporting every candidate number
        // (descending) so the operator sees the ambiguity.
        let d = select_canonical_pr(&[pr(10, false), pr(57, false), pr(31, false)]);
        assert_eq!(
            d,
            CanonicalPrDecision::Park {
                candidates: vec![57, 31, 10],
            }
        );
    }

    #[test]
    fn single_unmergeable_pr_parks() {
        // Even one PR, if not mergeable, parks rather than driving blind.
        let d = select_canonical_pr(&[pr(42, false)]);
        assert_eq!(
            d,
            CanonicalPrDecision::Park {
                candidates: vec![42],
            }
        );
    }

    // ── TASK-842 multi-spec PR recognition + dedupe ─────────────────────────

    #[test]
    fn single_spec_pr_recognized_as_one_completion() {
        let out = dedupe_pr_completions(&[(7, vec!["BUG-566".to_string()])]);
        assert_eq!(
            out,
            vec![PrCompletion {
                number: 7,
                spec_ids: vec!["BUG-566".to_string()],
            }]
        );
    }

    #[test]
    fn multi_spec_pr_detected_and_deduped_to_one_unit() {
        // The cluster PR #7 carries BUG-566 + BUG-567. The Done-spec scan
        // surfaces it once per member spec (two rows, same PR number); dedupe
        // collapses to ONE completion so the PR is driven once, not twice, with
        // the UNION of specs.
        let rows = vec![
            (7, vec!["BUG-566".to_string(), "BUG-567".to_string()]),
            (7, vec!["BUG-567".to_string(), "BUG-566".to_string()]),
        ];
        let out = dedupe_pr_completions(&rows);
        assert_eq!(out.len(), 1, "multi-spec PR collapses to a single unit");
        assert_eq!(out[0].number, 7);
        // First-seen order across rows preserved, case-insensitive de-dup.
        assert_eq!(
            out[0].spec_ids,
            vec!["BUG-566".to_string(), "BUG-567".to_string()]
        );
    }

    #[test]
    fn distinct_prs_stay_separate_in_first_seen_order() {
        let rows = vec![
            (12, vec!["STORY-1".to_string()]),
            (7, vec!["BUG-566".to_string(), "BUG-567".to_string()]),
            (12, vec!["STORY-1".to_string()]), // dupe row for PR 12
        ];
        let out = dedupe_pr_completions(&rows);
        assert_eq!(out.len(), 2);
        // PR 12 first-seen, then PR 7.
        assert_eq!(out[0].number, 12);
        assert_eq!(out[0].spec_ids, vec!["STORY-1".to_string()]);
        assert_eq!(out[1].number, 7);
        assert_eq!(
            out[1].spec_ids,
            vec!["BUG-566".to_string(), "BUG-567".to_string()]
        );
    }

    #[test]
    fn dedupe_unions_specs_seen_across_separate_rows() {
        // A PR whose trailer set was only partially recognized per row: the
        // union across rows is the full completion set.
        let rows = vec![
            (9, vec!["BUG-566".to_string()]),
            (9, vec!["BUG-567".to_string()]),
            (9, vec!["bug-566".to_string()]), // case-insensitive dup of BUG-566
        ];
        let out = dedupe_pr_completions(&rows);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].spec_ids,
            vec!["BUG-566".to_string(), "BUG-567".to_string()]
        );
    }

    #[test]
    fn dedupe_empty_rows_yields_empty() {
        assert!(dedupe_pr_completions(&[]).is_empty());
    }

    #[test]
    fn describe_pr_completion_lists_all_specs() {
        let c = PrCompletion {
            number: 7,
            spec_ids: vec!["BUG-566".to_string(), "BUG-567".to_string()],
        };
        assert_eq!(
            describe_pr_completion(&c),
            "integrating PR #7 → completes BUG-566, BUG-567"
        );
    }

    #[test]
    fn describe_pr_completion_handles_no_trailer() {
        let c = PrCompletion {
            number: 8,
            spec_ids: vec![],
        };
        assert_eq!(
            describe_pr_completion(&c),
            "integrating PR #8 → completes (no trailered spec)"
        );
    }

    // ── ADR-7 / ADR-9: integrate routes through the one engine ───────────────

    #[test]
    fn drive_args_routes_through_the_auto_complete_engine() {
        let args = drive_args("STORY-520");
        // The routing invariant the orchestration_routing guardrail relies on:
        // integrate hands the spec to `queue work --auto-complete`, never an
        // inlined merge lifecycle.
        assert_eq!(
            args,
            vec![
                "queue".to_string(),
                "work".to_string(),
                "STORY-520".to_string(),
                "--auto-complete".to_string(),
                "--from-pr".to_string(),
            ]
        );
        assert!(args.contains(&"--auto-complete".to_string()));
        // `--from-pr` is the re-entry seam — the engine starts at the reviewer
        // phase (phases 3-6) rather than from-scratch implement.
        assert!(args.contains(&"--from-pr".to_string()));
    }
}
