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
// needs-design (unresolved design decisions → route to --guided), blocked
// (BlockedBy → unshipped). WARN + `--force` to override: under-specified
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
/// first and ordered most-fundamental-first (epic → keystone → needs-design →
/// blocked); the soft warnings (under-specified, coupled) only matter once the
/// hard gates pass, and `--force` flips a `SoftBlock` into a `WarnProceed`. Pure.
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
    // TASK-1078: a spec tagged `needs-design` / `needs-operator-design` carries
    // unresolved design decisions the operator must make — it can be lint-CLEAN
    // yet still not be safe for an autonomous drive. Route it to the supervised
    // guided-implement lane instead of driving it headless.
    // trace:TASK-1078 | ai:claude
    if let Some(tag) = i.tags.iter().find(|t| {
        t.eq_ignore_ascii_case("needs-design") || t.eq_ignore_ascii_case("needs-operator-design")
    }) {
        return Suitability::HardRefuse(format!(
            "tagged `{tag}` — it needs operator design input, not an autonomous zen \
             drive. Run `aida queue work <spec> --guided` to decide the design forks \
             at the keyboard."
        ));
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
            "under-specified: the spec body is essentially empty (no describable \
             behavior or acceptance) — add a description + acceptance, or re-run \
             with --force",
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

// ── 2b. STRUCTURED GATE VERDICT (STORY-744) ───────────────────────────────
//
// A machine-readable composition of the eligibility + suitability gates, so a
// shell-out consumer (the TUI drive verb) can read the SAME verdict the drive
// runs instead of parsing the human refusal prose. Emitted by
// `aida zen <spec> --json`.

/// The drive-gate verdict for one spec, as `aida zen <spec> --json` serializes
/// it. Composes [`classify_eligibility`] (status) and [`classify_suitability`]
/// (type / blockers / lint / coupling) into one decision: `verdict` is the
/// headline (`ready` | `hold`), `class` names WHICH gate held, and the two
/// booleans tell a consumer which remedy affordance to offer — `under_specified`
/// → a clarify remedy (author acceptance criteria); `forceable` → `--force`
/// overrides a SOFT hold.
// trace:STORY-744 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct GateVerdict {
    /// The spec's display id (echoed back for the consumer's convenience).
    pub spec: String,
    /// `"ready"` (clear to drive) or `"hold"` (a gate held it).
    pub verdict: &'static str,
    /// Which gate produced the verdict: `"ready"`, `"not-eligible"` (a
    /// terminal / draft status), `"hard-refuse"` (epic / keystone / blocked —
    /// `--force` does NOT override), or `"soft-block"` (under-specified /
    /// coupled — `--force` overrides).
    pub class: &'static str,
    /// The operator-facing hold reason (empty when `ready`).
    pub reason: String,
    /// The hold is (at least partly) because the spec is under-specified — a
    /// clarify remedy applies (author acceptance criteria, then re-offer drive).
    /// Only ever set on a SOFT hold; a hard refusal (e.g. blocked) dominates and
    /// clarifying would not unblock it, so it reports `false`.
    pub under_specified: bool,
    /// The hold is SOFT — re-running with `--force` overrides it.
    pub forceable: bool,
    /// The ADR-6 scope route the DEFAULT drive (no `--solo`) would take:
    /// `"solo"` (own worktree + own PR) or `"into-scope"` (routes into the scope
    /// worktree named by `scope`). Lets a shell-out consumer (the TUI drive verb)
    /// show the resolved routing and offer a `--solo` toggle BEFORE launching,
    /// instead of silently routing an epic-parented spec into the epic worktree.
    /// [`classify_gate`] defaults this to `"solo"` (it is routing-agnostic); the
    /// caller that has the store fills the real route.
    // trace:TASK-1076 | ai:claude
    pub route: &'static str,
    /// The scope (parent epic / active focus) the default drive routes into,
    /// when `route == "into-scope"`; empty otherwise. Named in the TUI routing
    /// affordance so the operator sees WHICH worktree the drive would join.
    // trace:TASK-1076 | ai:claude
    pub scope: String,
}

/// Compose the eligibility + suitability classifications into one structured
/// [`GateVerdict`]. Pure: the caller reads the status + facts from the store and
/// passes them in, so the whole gate is unit-testable without storage. The
/// eligibility gate is checked first — a terminal / draft status is a hold
/// regardless of suitability.
// trace:STORY-744 | ai:claude
pub(crate) fn classify_gate(
    spec: &str,
    status: &RequirementStatus,
    suit_input: &SuitabilityInput,
) -> GateVerdict {
    // `route`/`scope` default to solo here — classify_gate is routing-agnostic;
    // the caller that holds the store overrides them with the resolved ADR-6
    // route. trace:TASK-1076 | ai:claude
    let ready = |class: &'static str| GateVerdict {
        spec: spec.to_string(),
        verdict: "ready",
        class,
        reason: String::new(),
        under_specified: false,
        forceable: false,
        route: "solo",
        scope: String::new(),
    };
    // Eligibility first: a not-Ready status holds regardless of suitability.
    if let Some(reason) = classify_eligibility(status).refusal(spec) {
        return GateVerdict {
            spec: spec.to_string(),
            verdict: "hold",
            class: "not-eligible",
            reason,
            under_specified: false,
            forceable: false,
            route: "solo",
            scope: String::new(),
        };
    }
    match classify_suitability(suit_input) {
        Suitability::Ready => ready("ready"),
        // `--force` was passed and flipped a soft block to proceed: still ready.
        Suitability::WarnProceed(_) => ready("ready"),
        Suitability::HardRefuse(reason) => GateVerdict {
            spec: spec.to_string(),
            verdict: "hold",
            class: "hard-refuse",
            reason,
            under_specified: false,
            forceable: false,
            route: "solo",
            scope: String::new(),
        },
        Suitability::SoftBlock(reason) => GateVerdict {
            spec: spec.to_string(),
            verdict: "hold",
            class: "soft-block",
            reason,
            under_specified: suit_input.under_specified,
            forceable: true,
            route: "solo",
            scope: String::new(),
        },
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

// =========================================================================
// THOUGHT → spec front door (STORY-725). The `aida zen <arg>` positional
// accepts free text as well as a spec id: a non-spec-id arg is drafted into a
// spec and filed as a draft, then driven through the SAME gated path. The pure
// detection + draft-composition live here so they are exhaustively
// unit-testable without storage or the AI transport; the IO (the AI call, the
// draft filing, the drive) lives in `main.rs::run_zen_drive`. trace:STORY-725
// =========================================================================

/// The longest title `aida zen` will file for a drafted thought; mirrors the
/// `aida add` title cap so a paste accident still files something legible.
// trace:STORY-725 | ai:claude
const MAX_DRAFT_TITLE_LEN: usize = 200;

/// Whether `arg` looks like a spec id (`TASK-123`, `STORY-45`, `BUG-9`,
/// `ADR-7`, and multi-segment agreed ids like `FR-1-042`) rather than a
/// free-text thought. The rule: a single whitespace-free token of an
/// all-alphabetic prefix followed by one or more `-<digits>` groups. Anything
/// else — multiple words, a bare word, an empty string — is treated as a
/// thought. A spec id keeps the existing resolve-or-refuse behavior; a thought
/// routes to the draft-and-drive front door. Case-insensitive (`task-7`).
// trace:STORY-725 | ai:claude
pub(crate) fn looks_like_spec_id(arg: &str) -> bool {
    let s = arg.trim();
    if s.is_empty() || s.chars().any(char::is_whitespace) {
        return false;
    }
    let mut segments = s.split('-');
    match segments.next() {
        Some(prefix) if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_alphabetic()) => {}
        _ => return false,
    }
    let mut saw_numeric_segment = false;
    for seg in segments {
        if seg.is_empty() || !seg.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        saw_numeric_segment = true;
    }
    saw_numeric_segment
}

/// Which path produced a drafted spec's body — for the operator-facing report
/// and the `[AI:claude]` provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DraftSource {
    /// A genuine AI draft (title + description + acceptance criteria).
    Ai,
    /// No AI draft was reachable — the thought was captured verbatim.
    Fallback,
}

impl DraftSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            DraftSource::Ai => "AI-drafted acceptance criteria",
            DraftSource::Fallback => "no AI reachable — thought captured verbatim",
        }
    }
}

/// A spec body composed from a free-text thought, ready to file as a draft.
#[derive(Debug, Clone)]
pub(crate) struct DraftedThought {
    pub title: String,
    pub description: String,
    pub source: DraftSource,
}

/// Cap a title at [`MAX_DRAFT_TITLE_LEN`] on a char boundary.
fn truncate_title(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() > MAX_DRAFT_TITLE_LEN {
        s.chars().take(MAX_DRAFT_TITLE_LEN).collect()
    } else {
        s.to_string()
    }
}

/// Compose the draft spec body from the thought and an OPTIONAL AI draft. Pure:
/// the caller runs the AI request (or not) and passes the result in, so the
/// AI-vs-fallback branch is unit-testable without the transport.
///
/// With a usable AI draft: the AI title + description, plus the acceptance
/// criteria rendered under a `## Acceptance` section (the same heading
/// `aida ultraplan` / `aida lint` look for), plus a provenance line naming the
/// originating thought. Without one: the thought becomes the title verbatim and
/// the description captures it with a "refine before it ships" note — which the
/// suitability gate will (correctly) flag as under-specified.
// trace:STORY-725 | ai:claude
pub(crate) fn compose_draft_from_thought(
    thought: &str,
    ai: Option<aida_core::DraftSpecResponse>,
) -> DraftedThought {
    let thought = thought.trim();
    match ai {
        Some(d) if !d.title.trim().is_empty() => {
            let title = truncate_title(&d.title);
            let mut description = d.description.trim().to_string();
            let criteria: Vec<&str> = d
                .acceptance_criteria
                .iter()
                .map(|c| c.trim())
                .filter(|c| !c.is_empty())
                .collect();
            if !criteria.is_empty() {
                description.push_str("\n\n## Acceptance\n");
                for c in &criteria {
                    description.push_str(&format!("- {c}\n"));
                }
            }
            description.push_str(&format!(
                "\n_Drafted by `aida zen` from the thought: \"{thought}\"._"
            ));
            DraftedThought {
                title,
                description,
                source: DraftSource::Ai,
            }
        }
        _ => {
            let description = format!(
                "Drafted by `aida zen` from a free-text thought; no AI draft was reachable, \
                 so this captures the thought verbatim. Refine the description and add \
                 acceptance criteria before it ships.\n\nThought: \"{thought}\""
            );
            DraftedThought {
                title: truncate_title(thought),
                description,
                source: DraftSource::Fallback,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_ids_are_recognized() {
        for id in [
            "TASK-123",
            "STORY-45",
            "BUG-9",
            "ADR-7",
            "task-7",
            "FR-1-042",
            "EPIC-1-001",
        ] {
            assert!(looks_like_spec_id(id), "{id} should look like a spec id");
        }
    }

    #[test]
    fn free_text_is_not_a_spec_id() {
        for thought in [
            "make the tree header show the parent title",
            "refactor",
            "fix the bug in zen",
            "",
            "   ",
            "TASK 123",
            "TASK-",
            "-123",
            "task-12a",
            "123",
        ] {
            assert!(
                !looks_like_spec_id(thought),
                "{thought:?} should be treated as free text"
            );
        }
    }

    #[test]
    fn compose_uses_ai_draft_with_acceptance() {
        let ai = aida_core::DraftSpecResponse {
            title: "Show the parent title in the tree header".to_string(),
            description: "The tree header should display the parent spec's title.".to_string(),
            acceptance_criteria: vec![
                "The header renders the parent's title".to_string(),
                "A root node shows no parent title and does not error".to_string(),
            ],
        };
        let drafted = compose_draft_from_thought("tree header parent title", Some(ai));
        assert_eq!(drafted.source, DraftSource::Ai);
        assert_eq!(drafted.title, "Show the parent title in the tree header");
        assert!(drafted.description.contains("## Acceptance"));
        assert!(drafted
            .description
            .contains("- The header renders the parent's title"));
        assert!(drafted.description.contains("tree header parent title"));
    }

    #[test]
    fn compose_falls_back_when_no_ai() {
        let drafted =
            compose_draft_from_thought("make the tree header show the parent title", None);
        assert_eq!(drafted.source, DraftSource::Fallback);
        assert_eq!(drafted.title, "make the tree header show the parent title");
        assert!(drafted.description.contains("no AI draft was reachable"));
        assert!(drafted
            .description
            .contains("make the tree header show the parent title"));
    }

    #[test]
    fn compose_treats_empty_ai_title_as_fallback() {
        let ai = aida_core::DraftSpecResponse {
            title: "   ".to_string(),
            description: "x".to_string(),
            acceptance_criteria: vec![],
        };
        let drafted = compose_draft_from_thought("do the thing", Some(ai));
        assert_eq!(drafted.source, DraftSource::Fallback);
        assert_eq!(drafted.title, "do the thing");
    }

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

    /// A spec tagged `needs-design` / `needs-operator-design` is hard-refused —
    /// it needs operator design input, and `--force` cannot override it. The
    /// refusal routes to the supervised `--guided` lane.
    // trace:TASK-1078
    #[test]
    fn approved_needs_design_is_refused() {
        for tag in ["needs-design", "needs-operator-design"] {
            match suitability("task", &[tag], false, false) {
                Suitability::HardRefuse(msg) => {
                    assert!(msg.contains(tag), "reason names the tag: {msg}");
                    assert!(msg.contains("--guided"), "reason routes to --guided: {msg}");
                }
                other => panic!("expected hard refuse for `{tag}`, got {other:?}"),
            }
            // --force does NOT override a hard refusal.
            assert!(matches!(
                suitability("task", &[tag], false, true),
                Suitability::HardRefuse(_)
            ));
        }
        // Case-insensitive tag match.
        assert!(matches!(
            suitability("story", &["Needs-Design"], false, false),
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

    // --- STORY-744: structured gate verdict ---------------------------------

    fn gate(
        status: RequirementStatus,
        req_type: &str,
        tags: &[&str],
        blocked: bool,
        under: bool,
    ) -> GateVerdict {
        let owned: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
        classify_gate(
            "TASK-1",
            &status,
            &SuitabilityInput {
                req_type,
                tags: &owned,
                has_unsatisfied_blocker: blocked,
                under_specified: under,
                coupled: false,
                force: false,
            },
        )
    }

    /// A clean, approved, bounded spec → ready to drive.
    // trace:STORY-744
    #[test]
    fn gate_clean_approved_is_ready() {
        let v = gate(RequirementStatus::Approved, "task", &[], false, false);
        assert_eq!(v.verdict, "ready");
        assert_eq!(v.class, "ready");
        assert!(v.reason.is_empty());
        assert!(!v.under_specified);
        assert!(!v.forceable);
    }

    /// A not-yet-approved (Draft) spec → a not-eligible HOLD, never forceable.
    // trace:STORY-744
    #[test]
    fn gate_draft_is_not_eligible_hold() {
        let v = gate(RequirementStatus::Draft, "task", &[], false, false);
        assert_eq!(v.verdict, "hold");
        assert_eq!(v.class, "not-eligible");
        assert!(!v.reason.is_empty());
        assert!(!v.forceable);
        assert!(!v.under_specified);
    }

    /// An under-specified approved spec → a SOFT hold: forceable AND flagged
    /// under_specified so a consumer can offer the clarify remedy.
    // trace:STORY-744
    #[test]
    fn gate_under_specified_is_soft_and_clarifiable() {
        let v = gate(RequirementStatus::Approved, "task", &[], false, true);
        assert_eq!(v.verdict, "hold");
        assert_eq!(v.class, "soft-block");
        assert!(v.forceable, "a soft block is forceable");
        assert!(
            v.under_specified,
            "under-specified drives the clarify remedy"
        );
        assert!(v.reason.contains("under-specified"));
    }

    /// A blocked spec (BlockedBy → unshipped) → a HARD refusal: NOT forceable,
    /// and under_specified is suppressed (clarifying would not unblock it).
    // trace:STORY-744
    #[test]
    fn gate_blocked_is_hard_refuse_not_forceable() {
        // Even when the spec ALSO happens to be under-specified, the hard
        // refusal dominates and no soft remedy is offered.
        let v = gate(RequirementStatus::Approved, "task", &[], true, true);
        assert_eq!(v.verdict, "hold");
        assert_eq!(v.class, "hard-refuse");
        assert!(!v.forceable, "a hard refusal is not overridable");
        assert!(
            !v.under_specified,
            "a hard refusal suppresses the clarify remedy"
        );
    }

    /// A keystone spec → a hard refusal (it ships human-supervised).
    // trace:STORY-744
    #[test]
    fn gate_keystone_is_hard_refuse() {
        let v = gate(
            RequirementStatus::Approved,
            "task",
            &["keystone"],
            false,
            false,
        );
        assert_eq!(v.class, "hard-refuse");
        assert!(!v.forceable);
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
