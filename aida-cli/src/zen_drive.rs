//! `aida zen <spec>` — the one-shot AUTONOMOUS implement+ship for a single
//! approved spec (STORY-721). The auto-implement counterpart to `aida ship`'s
//! HUMAN-implement finish, and the single-spec member of the ship/zen/integrate
//! taxonomy (`ship` = human-implement single spec, `zen` = auto-implement single
//! spec, `integrate` = land already-Done PRs).
//!
//! `aida zen <spec>` is a THIN wrapper. It does NOT reimplement the
//! orchestrator. It:
//!
//!   1. resolves + validates the spec — refusing a Draft (not-yet-approved)
//!      spec with clear guidance;
//!   2. relies on the existing `--auto-complete` auto-queue
//!      (`main.rs::ensure_queued_for_implementer`, STORY-246) so the operator
//!      never has to `aida queue add` first;
//!   3. drives the one spec through the EXISTING `--auto-complete`
//!      orchestrator by self-invoking `aida queue work <spec> --auto-complete
//!      --no-human <mode>` (implement → CI → review → merge → pull) — the SAME
//!      per-spec engine `aida burndown` / `aida integrate` use (ADR-7), not a
//!      fork. The default is FULLY HEADLESS (`both`) — fire-and-forget;
//!      `--supervised` keeps the implementer interactive. The review + merge
//!      phases come for free because they are already
//!      in the shared primitive. trace:TASK-1049
//!
//! Because each invocation drives a single spec in its own worktree, the
//! operator runs several `aida zen <spec>` in parallel for INDEPENDENT specs.
//! (Coupled specs that share files must use the SPIKE-70 `--single-branch`
//! sequential mode instead of parallel fire — a FOLLOW-ON, not this slice.)
//!
//! This module owns the **pure pieces** so they're unit-testable without
//! git/gh/the orchestrator: the approval-gate eligibility classification, the
//! `queue work` argv assembly, and the dry-run plan formatting. The handler
//! lives in `main.rs::run_zen_drive`.
//!
//! trace:STORY-721

use std::collections::HashSet;

use aida_core::RequirementStatus;

use crate::autopilot::{
    self, ActionClass, AutopilotEnvelope, Decision, EscalateReason, Grounding, Outcome,
};
use crate::backlog::RiskLevel;
use crate::presence::is_keystone_class;

/// Whether `aida zen <spec>` may drive a spec, given its current status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ZenEligibility {
    /// Approved / Planned / In Progress — drive it through the orchestrator.
    Ready,
    /// Draft — not yet approved. Slice 2 (TASK-1037) routes this through the
    /// autopilot approve-gate ([`classify_draft_gate`]) instead of a blanket
    /// refuse: an `approve = auto` envelope auto-approves and drives; the
    /// conservative `approve = propose` default surfaces to the advisor and
    /// exits cleanly. The old refusal text is kept only as a fallback for the
    /// legacy (centralized) storage path that cannot run the gate.
    NeedsApproval,
    /// Done / Completed — already shipped; nothing to drive.
    AlreadyShipped,
    /// Needs Attention — shelved on a punt; triage before driving.
    Shelved,
    /// Rejected — refuse.
    Rejected,
}

/// Classify a spec's drive-eligibility from its status. Pure: the caller reads
/// the status from the store and passes it in.
pub(crate) fn classify_eligibility(status: &RequirementStatus) -> ZenEligibility {
    match status {
        RequirementStatus::Approved
        | RequirementStatus::Planned
        | RequirementStatus::InProgress => ZenEligibility::Ready,
        RequirementStatus::Draft => ZenEligibility::NeedsApproval,
        RequirementStatus::Done | RequirementStatus::Completed => ZenEligibility::AlreadyShipped,
        RequirementStatus::NeedsAttention => ZenEligibility::Shelved,
        RequirementStatus::Rejected => ZenEligibility::Rejected,
    }
}

impl ZenEligibility {
    /// `None` when the spec may be driven; otherwise the operator-facing
    /// refusal message. (`spec` is the operator's own argument, so echoing the
    /// id is fine — no user-facing SPEC-ID-leak concern here.)
    pub(crate) fn refusal(self, spec: &str) -> Option<String> {
        match self {
            ZenEligibility::Ready => None,
            ZenEligibility::NeedsApproval => Some(format!(
                "aida zen needs an approved spec — {spec} is still a draft. \
                 Approve it first (aida edit {spec} --status approved), then re-run \
                 aida zen {spec}."
            )),
            ZenEligibility::AlreadyShipped => {
                Some(format!("{spec} is already shipped — nothing to zen."))
            }
            ZenEligibility::Shelved => Some(format!(
                "{spec} is shelved in Needs Attention — triage it (aida findings list) \
                 and resolve the punt before driving it."
            )),
            ZenEligibility::Rejected => Some(format!("{spec} was rejected — nothing to zen.")),
        }
    }
}

/// The default autonomy mode for `aida zen <spec>` (no flag): FULLY HEADLESS
/// fire-and-forget — headless implementer + headless reviewer + auto-merge, so
/// several INDEPENDENT specs can be fired in parallel and left to drive
/// themselves to main (STORY-721's intent). Matches `aida burndown`'s
/// per-spec drive.
// trace:TASK-1049 | ai:claude
pub(crate) const DEFAULT_DRIVE_MODE: &str = "both";

/// The autonomy mode `--supervised` maps to: the implementer is interactive
/// (the operator drives it) but the reviewer still runs headless as an
/// independent gate before the auto-merge.
// trace:TASK-1049 | ai:claude
pub(crate) const SUPERVISED_DRIVE_MODE: &str = "reviewer-only";

/// Assemble the `aida queue work <spec> --auto-complete --no-human <mode>`
/// argv that the handler self-invokes. Pure, so the exact set of forwarded
/// flags is pinned by unit tests.
///
/// ONE per-spec orchestration engine (ADR-7): `--auto-complete` is the single
/// home of the implement → CI → review → merge → pull lifecycle — `aida zen`,
/// `aida burndown`, and `aida integrate` all call it and differ only in scope
/// + lifetime, NEVER in the per-spec lifecycle. So zen hands the one spec to
/// the SAME full primitive burndown uses; review + merge come for free because
/// they are already in it.
///
/// The autonomy mode maps onto the orchestrator's EXISTING `--no-human` ladder
/// (no zen-specific pause behavior is invented):
///
///   * **default / `both`** — fully headless fire-and-forget: the implementer
///     AND the reviewer run headless, so several INDEPENDENT specs can be
///     fired in parallel and drive themselves to main. The review still gates
///     and the merge is automatic.
///   * **`--supervised` / `reviewer-only`** — the implementer is interactive
///     (the operator drives it), but the reviewer runs HEADLESS as an
///     independent gate, so the review phase ALWAYS runs and the spec drives
///     all the way to main.
///
/// Mode precedence: an explicit `--no-human=<mode>` wins; else `--supervised`
/// selects `reviewer-only`; else the `both` default. (`--supervised` and
/// `--no-human` are mutually exclusive at the CLI.)
///
/// TASK-1049: the previous drive shelled to `--auto-complete --zen`, whose
/// reviewer phase is an INTERACTIVE Claude session. In zen's autonomous /
/// no-TTY context that interactive reviewer never produced a verdict, so the
/// drive truncated to implement → PR → hand-off with ZERO review
/// (reviewDecision=NONE). Pointing the drive at the headless `--no-human`
/// reviewer makes the independent review run unconditionally.
// trace:TASK-1049 | ai:claude
pub(crate) fn drive_args(
    spec: &str,
    no_human: Option<&str>,
    supervised: bool,
    no_pull: bool,
) -> Vec<String> {
    let mode = match (no_human, supervised) {
        (Some(m), _) => m,
        (None, true) => SUPERVISED_DRIVE_MODE,
        (None, false) => DEFAULT_DRIVE_MODE,
    };
    let mut args = vec![
        "queue".to_string(),
        "work".to_string(),
        spec.to_string(),
        "--auto-complete".to_string(),
        "--no-human".to_string(),
        mode.to_string(),
    ];
    if no_pull {
        args.push("--no-pull".to_string());
    }
    args
}

/// Render the `--dry-run` plan: a one-line summary plus the per-phase steps the
/// `--auto-complete --no-human <mode>` drive will run. Mirrors the actual drive
/// so the preview is faithful. `already_queued` toggles the first step between
/// auto-queue and skip.
pub(crate) fn format_zen_plan(
    spec: &str,
    already_queued: bool,
    no_human: Option<&str>,
    supervised: bool,
) -> String {
    let resolved = match (no_human, supervised) {
        (Some(m), _) => m,
        (None, true) => SUPERVISED_DRIVE_MODE,
        (None, false) => DEFAULT_DRIVE_MODE,
    };
    let mode = match resolved {
        "both" => "headless implementer + reviewer (--no-human both, fire-and-forget)".to_string(),
        // Supervised: interactive implementer, independent headless reviewer.
        // trace:TASK-1049
        _ => "supervised implementer + independent reviewer (--no-human reviewer-only)".to_string(),
    };
    let mut out = String::new();
    out.push_str(&format!(
        "would zen-drive {spec}: queue -> implement -> CI -> review -> merge -> pull\n"
    ));
    out.push_str(&format!("  mode: {mode}\n"));
    let queue_step = if already_queued {
        "already queued for the implementer — skip queue-add"
    } else {
        "queue the spec for the implementer (auto-queue)"
    };
    let steps = [
        queue_step,
        "implement (autonomous orchestrator phase 1)",
        "wait for CI green",
        "review",
        "squash-merge the PR",
        "aida pull (Done -> Completed auto-bump)",
    ];
    for (i, s) in steps.iter().enumerate() {
        out.push_str(&format!("  {}. {}\n", i + 1, s));
    }
    out
}

// =========================================================================
// TASK-1037 (slice 2): the pre-flight suitability gate + autopilot approve-gate
// + scope-routing for `aida zen`. All side-effect-free so the decision logic is
// exhaustively unit-testable; the IO (status write, worktree creation, the
// self-invoked drive) lives in `main.rs::run_zen_drive`. trace:TASK-1037
// =========================================================================

// ── 1. DRAFT → autopilot approve-gate ────────────────────────────────────
//
// `aida zen <draft>` is the FIRST real consumer of the merged autopilot
// `evaluate()` contract (TASK-1007). A Draft is run through the approve-gate
// instead of refused: build a `Decision { action: Approve, … }` and a one-spec
// fence, call `autopilot::evaluate`, then map the `Outcome` to a `DraftGate`.

/// Build the `Approve` decision `aida zen` submits to the autopilot gate for a
/// Draft spec. The grounding is `RecordedB`: the operator's explicit
/// `aida zen <draft>` invocation IS the recorded preference to approve-and-drive
/// this spec, so a cold boot could reconstruct it from the command itself. Risk
/// is `Low` — approving a single named draft is reversible (`aida edit --status
/// draft`). Both feed gates 3/4, which only matter once an `approve = auto`
/// envelope clears gate 2.
// trace:TASK-1037 | ai:claude
pub(crate) fn draft_approve_decision(spec: &str) -> Decision {
    Decision {
        spec_id: spec.to_string(),
        action: ActionClass::Approve,
        grounding: Grounding::RecordedB,
        risk: RiskLevel::Low,
        reason: "operator invoked `aida zen` on this draft — approve and drive".to_string(),
        evidence: vec!["aida zen <draft> invocation".to_string()],
    }
}

/// Build the one-spec fence (gate 1) for the approve-gate. The spec is in the
/// fence — and therefore touchable — UNLESS it is keystone-class
/// ([`is_keystone_class`]): a keystone draft is fenced OUT, so even an
/// `approve = auto` envelope holds it for the advisor (the gate-1 invariant the
/// authority map can never widen).
// trace:TASK-1037 | ai:claude
pub(crate) fn approve_fence(spec: &str, req_type: &str, tags: &[String]) -> HashSet<String> {
    let mut fence = HashSet::new();
    if !is_keystone_class(req_type, tags.iter().map(String::as_str)) {
        fence.insert(spec.to_string());
    }
    fence
}

/// What the Draft approve-gate decided `aida zen` should do. The three arms map
/// one-to-one onto the three [`Outcome`] variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DraftGate {
    /// `Outcome::Execute` — auto-approve the spec (status → Approved) and
    /// proceed to the suitability checks + drive.
    AutoApprove,
    /// `Outcome::Hold` — the conservative `approve = propose` default. Surface
    /// to the advisor (print + record a pending-approval brief) and EXIT
    /// cleanly. The human approves, then re-runs. NOT a block/pause.
    SurfaceToAdvisor,
    /// `Outcome::Escalate` — reject/park and report why (corpus gap, risk, or
    /// a hard `never` authority).
    Escalate(String),
}

/// Map an autopilot [`Outcome`] onto the zen Draft action. Pure — the caller
/// runs `autopilot::evaluate` and feeds the verdict in.
// trace:TASK-1037
pub(crate) fn classify_draft_gate(outcome: Outcome) -> DraftGate {
    match outcome {
        Outcome::Execute => DraftGate::AutoApprove,
        Outcome::Hold => DraftGate::SurfaceToAdvisor,
        Outcome::Escalate(reason) => DraftGate::Escalate(escalate_label(reason).to_string()),
    }
}

/// Human-readable explanation for an autopilot escalation reason — used in the
/// reject/park report.
// trace:TASK-1037 | ai:claude
fn escalate_label(reason: EscalateReason) -> &'static str {
    match reason {
        EscalateReason::NeverAuthority => {
            "the approve action is barred (`[autopilot] approve = \"never\"`)"
        }
        EscalateReason::GroundingGap => {
            "approving this draft needs a human judgment the autopilot can't reconstruct"
        }
        EscalateReason::RiskCeiling => "approving this draft is above the autopilot risk ceiling",
    }
}

/// Run the full Draft approve-gate end-to-end: build the decision + fence,
/// evaluate against `env`, and classify the outcome. The single entry point the
/// handler calls; split from its parts so both the wiring and the parts are
/// testable.
// trace:TASK-1037 | ai:claude
pub(crate) fn run_draft_gate(
    env: &AutopilotEnvelope,
    spec: &str,
    req_type: &str,
    tags: &[String],
) -> DraftGate {
    let decision = draft_approve_decision(spec);
    let fence = approve_fence(spec, req_type, tags);
    classify_draft_gate(autopilot::evaluate(env, &fence, &decision))
}

// ── 2. APPROVED → suitability checks ──────────────────────────────────────
//
// HARD-REFUSE (not overridable): epic (read-only rollup), keystone/supervised,
// blocked (BlockedBy → unshipped). WARN + `--force` to override: under-specified
// (`aida lint`), coupled (file-overlap with in-flight work).

/// Already-probed facts for [`classify_suitability`]. Built in `main.rs` from
/// the store + graph + lint; consumed by the pure verdict.
// trace:TASK-1037
#[derive(Debug, Clone)]
pub(crate) struct SuitabilityInput<'a> {
    /// Lowercased requirement type (`epic`, `task`, `story`, …).
    pub req_type: &'a str,
    /// The spec's tags (keystone classification).
    pub tags: &'a [String],
    /// True when any `BlockedBy` edge points at a not-yet-Completed spec.
    pub has_unsatisfied_blocker: bool,
    /// True when `aida lint` flagged the spec as under-specified (not clean).
    pub under_specified: bool,
    /// True when the spec's likely files overlap an in-flight spec's work.
    pub coupled: bool,
    /// `--force` was passed: soft warnings are overridden, hard refusals are not.
    pub force: bool,
}

/// The suitability verdict for one already-Approved spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Suitability {
    /// Clear to drive.
    Ready,
    /// A HARD refusal — `--force` does NOT override it. The drive must not start.
    HardRefuse(String),
    /// A soft block: refuse UNLESS `--force`. Carries the warning text.
    SoftBlock(String),
    /// A warning the operator overrode with `--force` — proceed, but surface it.
    WarnProceed(String),
}

/// Classify an Approved spec's drive-suitability. Hard refusals are checked
/// first and ordered most-fundamental-first (epic → keystone → blocked); the
/// soft warnings (under-specified, coupled) only matter once the hard gates
/// pass, and `--force` flips a `SoftBlock` into a `WarnProceed`. Pure.
// trace:TASK-1037 | ai:claude
pub(crate) fn classify_suitability(i: &SuitabilityInput) -> Suitability {
    // Hard refusals — NOT overridable by --force.
    if i.req_type.eq_ignore_ascii_case("epic") {
        return Suitability::HardRefuse(
            "an epic is a read-only rollup of its children, not a unit of work — \
             zen one of its bounded children instead."
                .to_string(),
        );
    }
    if is_keystone_class(i.req_type, i.tags.iter().map(String::as_str)) {
        return Suitability::HardRefuse(
            "this is keystone / supervised work — it ships at the keyboard under \
             review, never on an autonomous zen drive."
                .to_string(),
        );
    }
    if i.has_unsatisfied_blocker {
        return Suitability::HardRefuse(
            "blocked by an unshipped dependency (BlockedBy) — ship the blocker first.".to_string(),
        );
    }
    // Soft warnings — overridable with --force.
    let mut warnings = Vec::new();
    if i.under_specified {
        warnings.push(
            "under-specified (aida lint flags vague/missing acceptance) — \
             clarify it, or re-run with --force",
        );
    }
    if i.coupled {
        warnings.push(
            "coupled: its files overlap an in-flight spec — sequence them, \
             or re-run with --force",
        );
    }
    if warnings.is_empty() {
        return Suitability::Ready;
    }
    let msg = warnings.join("; ");
    if i.force {
        Suitability::WarnProceed(msg)
    } else {
        Suitability::SoftBlock(msg)
    }
}

// ── 3. SCOPE-ROUTING (ADR-6) ──────────────────────────────────────────────
//
// A scoped spec (parent epic / active focus) auto-routes into its scope
// worktree (an info line, not a question). `--solo` overrides to own-worktree +
// own-PR; `--into-epic` forces the cluster route.

/// Where `aida zen` should run the drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScopeRoute {
    /// Own worktree + own PR — `--solo`, or the spec has no detectable scope.
    Solo,
    /// Route into the scope (epic / focus) worktree, creating it if absent.
    IntoScope(String),
}

/// Resolve the scope route from the spec's parent epic, the active focus, and
/// the flags. `--solo` always wins (own worktree). Otherwise the parent epic is
/// the scope, falling back to the active focus; `--into-epic` is the explicit
/// cluster route but cannot invent a scope when none exists (falls back to
/// Solo). Pure.
// trace:ADR-6 trace:TASK-1037 | ai:claude
pub(crate) fn resolve_scope_route(
    parent_epic: Option<&str>,
    focus: Option<&str>,
    solo: bool,
    into_epic: bool,
) -> ScopeRoute {
    if solo {
        return ScopeRoute::Solo;
    }
    // Parent epic is the strong default proxy for coupling (ADR-6); the active
    // focus is the fallback scope. `--into-epic` only forces the route when one
    // of those exists to route into.
    let scope = parent_epic
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| focus.map(str::trim).filter(|s| !s.is_empty()));
    let _ = into_epic; // forcing is meaningful only when a scope exists, below.
    match scope {
        Some(s) => ScopeRoute::IntoScope(s.to_string()),
        None => ScopeRoute::Solo,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approved_and_beyond_are_ready() {
        assert_eq!(
            classify_eligibility(&RequirementStatus::Approved),
            ZenEligibility::Ready
        );
        assert_eq!(
            classify_eligibility(&RequirementStatus::Planned),
            ZenEligibility::Ready
        );
        assert_eq!(
            classify_eligibility(&RequirementStatus::InProgress),
            ZenEligibility::Ready
        );
    }

    #[test]
    fn draft_needs_approval_and_refuses_with_guidance() {
        let e = classify_eligibility(&RequirementStatus::Draft);
        assert_eq!(e, ZenEligibility::NeedsApproval);
        let msg = e.refusal("STORY-721").expect("draft must refuse");
        assert!(msg.contains("needs an approved spec"));
        assert!(msg.contains("aida edit STORY-721 --status approved"));
    }

    #[test]
    fn terminal_states_refuse() {
        assert!(classify_eligibility(&RequirementStatus::Done)
            .refusal("X-1")
            .unwrap()
            .contains("already shipped"));
        assert!(classify_eligibility(&RequirementStatus::Completed)
            .refusal("X-1")
            .unwrap()
            .contains("already shipped"));
        assert!(classify_eligibility(&RequirementStatus::Rejected)
            .refusal("X-1")
            .unwrap()
            .contains("rejected"));
        assert!(classify_eligibility(&RequirementStatus::NeedsAttention)
            .refusal("X-1")
            .unwrap()
            .contains("Needs Attention"));
    }

    #[test]
    fn ready_has_no_refusal() {
        assert_eq!(ZenEligibility::Ready.refusal("X-1"), None);
    }

    // TASK-1049: the default drive must invoke the FULL shared per-spec
    // primitive (`--auto-complete`) FULLY HEADLESS (`--no-human both`) — the
    // fire-and-forget default — NOT the old truncated `--auto-complete --zen`
    // whose interactive reviewer no-showed in zen's autonomous context.
    #[test]
    fn drive_args_default_invokes_full_primitive_fully_headless() {
        assert_eq!(
            drive_args("STORY-721", None, false, false),
            vec![
                "queue",
                "work",
                "STORY-721",
                "--auto-complete",
                "--no-human",
                "both"
            ]
        );
    }

    // `--supervised` is the opt-in counterpart: interactive implementer, but
    // the reviewer still runs headless as an independent gate (`reviewer-only`).
    #[test]
    fn drive_args_supervised_maps_to_reviewer_only() {
        assert_eq!(
            drive_args("STORY-721", None, true, false),
            vec![
                "queue",
                "work",
                "STORY-721",
                "--auto-complete",
                "--no-human",
                "reviewer-only"
            ]
        );
    }

    // The drive must reach the full lifecycle (review + merge) in EVERY mode,
    // so it must NOT emit a phase-truncating `--auto-complete=through-ci` /
    // `through-merge` variant, and must NOT fall back to the `--zen` reviewer.
    #[test]
    fn drive_args_is_not_truncated_and_runs_the_reviewer() {
        let cases = [
            (None, false),                  // default — fully headless
            (None, true),                   // --supervised
            (Some("both"), false),          // explicit both
            (Some("reviewer-only"), false), // explicit reviewer-only
        ];
        for (no_human, supervised) in cases {
            let args = drive_args("BUG-9", no_human, supervised, false);
            // Full lifecycle primitive — bare `--auto-complete` is variant Full
            // (implement -> CI -> review -> merge -> pull -> build).
            assert!(args.iter().any(|a| a == "--auto-complete"));
            assert!(
                !args.iter().any(|a| a.starts_with("--auto-complete=")
                    || a == "through-ci"
                    || a == "through-merge"),
                "zen must drive the full lifecycle, not a truncated variant: {args:?}"
            );
            // The reviewer phase runs as an INDEPENDENT headless agent in every
            // mode — never the interactive `--zen` reviewer that skipped review.
            assert!(args.iter().any(|a| a == "--no-human"));
            assert!(!args.iter().any(|a| a == "--zen"));
        }
    }

    // An explicit `--no-human=<mode>` wins over the `--supervised`/default
    // resolution, and `--no-pull` is forwarded.
    #[test]
    fn drive_args_explicit_no_human_wins_and_forwards_no_pull() {
        assert_eq!(
            drive_args("BUG-9", Some("reviewer-only"), false, true),
            vec![
                "queue",
                "work",
                "BUG-9",
                "--auto-complete",
                "--no-human",
                "reviewer-only",
                "--no-pull"
            ]
        );
    }

    #[test]
    fn plan_lists_every_phase_and_auto_queue_step() {
        let plan = format_zen_plan("STORY-721", false, None, false);
        assert!(plan.contains("would zen-drive STORY-721"));
        assert!(plan.contains("queue -> implement -> CI -> review -> merge -> pull"));
        assert!(plan.contains("auto-queue"));
        assert!(plan.contains("squash-merge the PR"));
        assert!(plan.contains("auto-bump"));
        // The default is the fully-headless fire-and-forget drive.
        assert!(plan.contains("fire-and-forget"));
    }

    #[test]
    fn plan_supervised_shows_independent_reviewer() {
        let plan = format_zen_plan("STORY-721", false, None, true);
        assert!(plan.contains("independent reviewer"));
    }

    #[test]
    fn plan_skips_queue_step_when_already_queued_and_shows_headless_mode() {
        let plan = format_zen_plan("BUG-9", true, Some("both"), false);
        assert!(plan.contains("skip queue-add"));
        assert!(plan.contains("headless implementer + reviewer (--no-human both"));
    }

    // --- TASK-1037: DRAFT autopilot approve-gate ----------------------------

    use crate::autopilot::Authority;
    use std::collections::BTreeMap;

    /// The conservative default envelope (`approve = propose`) HOLDS a draft —
    /// `aida zen <draft>` surfaces it to the advisor and exits cleanly. This is
    /// the headline zero-config path: a draft never auto-approves, it routes for
    /// an approve decision.
    // trace:TASK-1037
    #[test]
    fn draft_with_propose_authority_surfaces_to_advisor_and_exits() {
        let env = AutopilotEnvelope::default();
        assert_eq!(env.authority_for(ActionClass::Approve), Authority::Propose);
        let gate = run_draft_gate(&env, "TASK-1", "task", &[]);
        assert_eq!(gate, DraftGate::SurfaceToAdvisor);
    }

    /// With `approve = auto` configured, a grounded, low-risk, in-fence draft
    /// clears all four autopilot gates → auto-approve and proceed to the drive.
    // trace:TASK-1037
    #[test]
    fn draft_with_auto_authority_approves_and_proceeds() {
        let mut overrides = BTreeMap::new();
        overrides.insert(ActionClass::Approve, Authority::Auto);
        let env = AutopilotEnvelope::default().with_overrides(overrides);
        let gate = run_draft_gate(&env, "TASK-1", "task", &[]);
        assert_eq!(gate, DraftGate::AutoApprove);
    }

    /// INVARIANT: even `approve = auto` cannot auto-approve a keystone-class
    /// draft — `approve_fence` fences it out (gate 1), so the gate still HOLDS
    /// it for the advisor.
    // trace:TASK-1037
    #[test]
    fn draft_keystone_holds_even_with_auto_authority() {
        let mut overrides = BTreeMap::new();
        overrides.insert(ActionClass::Approve, Authority::Auto);
        let env = AutopilotEnvelope::default().with_overrides(overrides);
        let gate = run_draft_gate(&env, "EPIC-1", "epic", &["architecture".to_string()]);
        assert_eq!(gate, DraftGate::SurfaceToAdvisor);
    }

    /// `approve = never` escalates (reject/park + report).
    // trace:TASK-1037
    #[test]
    fn draft_with_never_authority_escalates() {
        let mut overrides = BTreeMap::new();
        overrides.insert(ActionClass::Approve, Authority::Never);
        let env = AutopilotEnvelope::default().with_overrides(overrides);
        match run_draft_gate(&env, "TASK-1", "task", &[]) {
            DraftGate::Escalate(reason) => assert!(reason.contains("barred")),
            other => panic!("expected escalate, got {other:?}"),
        }
    }

    // --- TASK-1037: APPROVED suitability checks -----------------------------

    fn suitability(req_type: &str, tags: &[&str], blocked: bool, force: bool) -> Suitability {
        let owned: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
        classify_suitability(&SuitabilityInput {
            req_type,
            tags: &owned,
            has_unsatisfied_blocker: blocked,
            under_specified: false,
            coupled: false,
            force,
        })
    }

    /// An epic is hard-refused — it is a read-only rollup, not a unit of work.
    // trace:TASK-1037
    #[test]
    fn approved_epic_is_refused() {
        match suitability("epic", &[], false, false) {
            Suitability::HardRefuse(msg) => assert!(msg.contains("rollup")),
            other => panic!("expected hard refuse, got {other:?}"),
        }
        // --force does NOT override a hard refusal.
        assert!(matches!(
            suitability("epic", &[], false, true),
            Suitability::HardRefuse(_)
        ));
    }

    /// Keystone / supervised work is hard-refused.
    // trace:TASK-1037
    #[test]
    fn approved_keystone_is_refused() {
        match suitability("task", &["keystone"], false, false) {
            Suitability::HardRefuse(msg) => assert!(msg.contains("keystone")),
            other => panic!("expected hard refuse, got {other:?}"),
        }
        // The supervised-build marker also trips it, and --force cannot override.
        assert!(matches!(
            suitability("story", &["needs-supervised-build"], false, true),
            Suitability::HardRefuse(_)
        ));
    }

    /// A blocked spec (BlockedBy → unshipped) is hard-refused.
    // trace:TASK-1037
    #[test]
    fn approved_blocked_is_refused() {
        match suitability("task", &[], true, false) {
            Suitability::HardRefuse(msg) => assert!(msg.contains("blocked")),
            other => panic!("expected hard refuse, got {other:?}"),
        }
    }

    /// Under-specified is a SOFT block: refused without --force, proceeds (with a
    /// surfaced warning) with it.
    // trace:TASK-1037
    #[test]
    fn approved_under_specified_warns_and_force_overrides() {
        let owned = vec!["cleanup".to_string()];
        let blocked = SuitabilityInput {
            req_type: "task",
            tags: &owned,
            has_unsatisfied_blocker: false,
            under_specified: true,
            coupled: false,
            force: false,
        };
        assert!(matches!(
            classify_suitability(&blocked),
            Suitability::SoftBlock(_)
        ));
        let forced = SuitabilityInput {
            force: true,
            ..blocked
        };
        assert!(matches!(
            classify_suitability(&forced),
            Suitability::WarnProceed(_)
        ));
    }

    /// A clean, unblocked, bounded spec is Ready.
    // trace:TASK-1037
    #[test]
    fn approved_clean_task_is_ready() {
        assert_eq!(
            suitability("task", &["papercut"], false, false),
            Suitability::Ready
        );
    }

    // --- TASK-1037: scope-routing (ADR-6) -----------------------------------

    /// A scoped spec (parent epic) auto-routes into its scope worktree.
    // trace:ADR-6 trace:TASK-1037
    #[test]
    fn scoped_spec_routes_into_epic_worktree() {
        assert_eq!(
            resolve_scope_route(Some("EPIC-55"), None, false, false),
            ScopeRoute::IntoScope("EPIC-55".to_string())
        );
        // Falls back to the active focus when there's no parent epic.
        assert_eq!(
            resolve_scope_route(None, Some("EPIC-42"), false, false),
            ScopeRoute::IntoScope("EPIC-42".to_string())
        );
        // No scope at all → Solo.
        assert_eq!(
            resolve_scope_route(None, None, false, false),
            ScopeRoute::Solo
        );
    }

    /// `--solo` overrides scope-routing to own-worktree + own-PR even when a
    /// parent epic exists.
    // trace:ADR-6 trace:TASK-1037
    #[test]
    fn solo_flag_overrides_scope_routing() {
        assert_eq!(
            resolve_scope_route(Some("EPIC-55"), None, true, false),
            ScopeRoute::Solo
        );
        // --solo wins over --into-epic too.
        assert_eq!(
            resolve_scope_route(Some("EPIC-55"), None, true, true),
            ScopeRoute::Solo
        );
    }
}
