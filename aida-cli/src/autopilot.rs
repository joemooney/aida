//! TASK-1007 (EPIC-0428 / TASK-0429 smallest-first slice): the advisor
//! autopilot POLICY ENVELOPE — the side-effect-free heart only.
//!
//! Autopilot is NOT a new disposition engine. It is a bounded-authority
//! envelope wrapped around the existing `aida groom` pass (STORY-560 /
//! STORY-708) and its `IntakeConfig` policy (`intake.rs`). `groom` already
//! owns the disposition judgment, the cold-boot caveat, the candidate FENCE
//! (`select_intake_candidates`), and the `--apply` execution path. Autopilot
//! adds the missing axis: a per-action-class AUTHORITY map plus a pure
//! four-gate `evaluate` that decides, for one proposed disposition, whether it
//! may auto-execute, must only be proposed (held), or must be escalated.
//!
//! This module is WIRED TO NOTHING in this slice — there is no `--autopilot`
//! flag, no config plumbing, and no `groom` integration yet (those are
//! TASK-0430/0431/0432). It lands the governing contract and its exhaustive
//! unit-test suite as a reviewable, low-risk artifact the later slices build
//! against. Everything is `#![allow(dead_code)]` because nothing in the rest
//! of the crate calls it yet.
//!
//! Design + the conservative default authority table:
//! `docs/plans/2026-06-29-epic-0428-policy-envelope.md`.
//! trace:TASK-1007 trace:TASK-0429 | ai:claude
#![allow(dead_code)]

use std::collections::{BTreeMap, HashSet};

use crate::backlog::RiskLevel;
use crate::intake::IntakeConfig;

/// The canonical taxonomy of what advisor autopilot can do to a spec. Each
/// action class has a wildly different blast radius (tagging is reversible and
/// cheap; rejecting or approving onto the buildable queue is not), so authority
/// is granted per class rather than via a single autonomy on/off switch.
///
/// `Ord`/`Hash` are derived so this is a `BTreeMap` key in the authority map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ActionClass {
    /// Move a draft to Approved (draft -> Approved).
    Approve,
    /// Reject a draft / spec.
    Reject,
    /// Declare a spec a duplicate — autopilot's `auto` half is *propose the
    /// link* only (add a `duplicate-of:<ID>` tag + comment). The destructive
    /// *reject the duplicate* half routes through [`ActionClass::Reject`].
    Dedupe,
    /// Add a tag (reversible, informational).
    Tag,
    /// Move an already-Approved spec onto a role queue.
    Queue,
    /// Park a spec (NeedsAttention / deferred shelf).
    Park,
    /// Route to an existing role queue (`queue move`/add) — never creates a new
    /// routing target, never routes to a human's keystone queue.
    Route,
    /// Add an informational comment.
    Comment,
    /// Escalate-when-uncertain: a first-class, recorded action (a `needs-human`
    /// finding + spec comment), NOT a silent no-op.
    Ask,
}

impl ActionClass {
    /// The lowercase `[autopilot]` config token for this action.
    pub(crate) fn token(self) -> &'static str {
        match self {
            ActionClass::Approve => "approve",
            ActionClass::Reject => "reject",
            ActionClass::Dedupe => "dedupe",
            ActionClass::Tag => "tag",
            ActionClass::Queue => "queue",
            ActionClass::Park => "park",
            ActionClass::Route => "route",
            ActionClass::Comment => "comment",
            ActionClass::Ask => "ask",
        }
    }

    /// Parse a `[autopilot]` config key into an action class.
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "approve" => Some(ActionClass::Approve),
            "reject" => Some(ActionClass::Reject),
            "dedupe" => Some(ActionClass::Dedupe),
            "tag" => Some(ActionClass::Tag),
            "queue" => Some(ActionClass::Queue),
            "park" => Some(ActionClass::Park),
            "route" => Some(ActionClass::Route),
            "comment" => Some(ActionClass::Comment),
            "ask" => Some(ActionClass::Ask),
            _ => None,
        }
    }
}

/// Per-action-class authority. Maps cleanly onto the existing three-mode
/// autonomy ladder: `Auto` = auto-resolve, `Propose` = pause-and-ask,
/// `Never` = escalate (refuse).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Authority {
    /// Autopilot may execute this action autonomously (if the other gates pass).
    Auto,
    /// Autopilot may only propose this action; a human reviews before it runs.
    Propose,
    /// Autopilot may never execute this action.
    Never,
}

impl Authority {
    pub(crate) fn token(self) -> &'static str {
        match self {
            Authority::Auto => "auto",
            Authority::Propose => "propose",
            Authority::Never => "never",
        }
    }

    /// Parse a `[autopilot]` authority value. Unknown tokens return `None` so
    /// the parser leaves that action at its default rather than guessing.
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Authority::Auto),
            "propose" => Some(Authority::Propose),
            "never" => Some(Authority::Never),
            _ => None,
        }
    }
}

/// The grounding classification for one decision — the Type A/B/C calibration
/// from `docs/architecture/autonomy-and-escalation.md` §3. The *classification*
/// is the agent's recorded judgment (stored on the [`Decision`]); the *gate*
/// over it ([`is_resolvable`]) is pure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Grounding {
    /// Type-A: grounded in a recorded PRINCIPLE — autopilot may resolve.
    TypeA,
    /// Recorded-B: grounded in a recorded PREFERENCE — autopilot may resolve.
    RecordedB,
    /// Unrecorded-B: a preference that exists only in the operator's head, not
    /// in substrate — a cold boot can't reconstruct it, so escalate.
    UnrecordedB,
    /// Type-C: synthesized in-flight context a cold boot can't reconstruct —
    /// escalate.
    TypeC,
}

/// True iff the grounding is resolvable on a cold boot — Type-A (recorded
/// principle) or recorded-B (recorded preference). Unrecorded-B and Type-C
/// always escalate. This is gate 3 and is NOT overridable by the authority map.
pub(crate) fn is_resolvable(g: Grounding) -> bool {
    matches!(g, Grounding::TypeA | Grounding::RecordedB)
}

/// One proposed disposition emitted by the cold-boot advisor groom pass. The
/// agent records its own grounding classification and risk read here; the
/// pure [`evaluate`] gate then accepts or holds/escalates it.
#[derive(Debug, Clone)]
pub(crate) struct Decision {
    /// Display SPEC-ID (e.g. `STORY-560`) the action targets.
    pub(crate) spec_id: String,
    /// What the advisor proposes to do.
    pub(crate) action: ActionClass,
    /// The advisor's recorded grounding classification (gate 3 input).
    pub(crate) grounding: Grounding,
    /// The advisor's risk read (gate 4 input) — reuses `backlog::RiskLevel`.
    pub(crate) risk: RiskLevel,
    /// One-line rationale, for the audit/review surface.
    pub(crate) reason: String,
    /// Cited substrate (principle/preference/comment refs) backing the grounding.
    pub(crate) evidence: Vec<String>,
}

/// Why an action was escalated (parked for a human) rather than executed or
/// merely held. Mirrors the advisor tier's reason categories (strategy /
/// irreversibility / corpus-gap).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EscalateReason {
    /// Gate 2: the action class is `never` in the envelope (hard exclusion /
    /// strategy-class).
    NeverAuthority,
    /// Gate 3: the decision is unrecorded-B or Type-C — a corpus gap a cold
    /// boot can't close.
    GroundingGap,
    /// Gate 4: the decision's risk is above the autopilot ceiling
    /// (irreversibility / blast radius).
    RiskCeiling,
}

/// The verdict of the four-gate evaluation for one decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// All four gates passed — auto-execute (and durably audit; TASK-0430).
    Execute,
    /// Held for human review (proposed, or dropped because the fence already
    /// excluded the spec) — NOT executed, NOT escalated as a new finding.
    Hold,
    /// Parked / escalated to a human with a reason.
    Escalate(EscalateReason),
}

/// The risk ceiling for gate 4: autopilot auto-executes only decisions whose
/// risk is within this ceiling. `Medium` admits Low + Medium; `High` and
/// `Unknown` (unknown blast radius) park — consistent with the existing
/// `--risk` ceiling semantics where Unknown ranks above Medium
/// (`backlog::RiskLevel::within_ceiling`). The configurable form is a later
/// slice; the contract here pins the conservative default.
const AUTOPILOT_RISK_CEILING: RiskLevel = RiskLevel::Medium;

/// The bounded-authority envelope: composes the existing `IntakeConfig`
/// (P1/P2/P3 fence policy) with the new per-action authority map. It EMBEDS
/// `IntakeConfig`, never forks it — the fence (which SPECS are touchable) stays
/// owned by `intake`; the envelope owns only the action-authority map (which
/// ACTIONS auto-execute) and the `grounding_required` toggle.
#[derive(Debug, Clone)]
pub(crate) struct AutopilotEnvelope {
    /// Per-action-class authority. Default-filled with all nine classes.
    pub(crate) authorities: BTreeMap<ActionClass, Authority>,
    /// The embedded fence policy — never forked.
    pub(crate) intake: IntakeConfig,
    /// Whether gate 3 (grounding) is active. Defaults true.
    pub(crate) grounding_required: bool,
}

impl Default for AutopilotEnvelope {
    /// The conservative default authority table from the plan. Zero-config
    /// autopilot can only ever auto-execute REVERSIBLE, low-blast actions.
    ///
    /// - `auto`    : tag, comment, dedupe (link-only), route, park, queue
    ///               (of an already-Approved spec), ask (escalating is always
    ///               allowed).
    /// - `propose` : approve (draft -> Approved) and reject.
    /// - `never`   : the fallback for any unmapped action ([`authority_for`]);
    ///               anything touching a fenced spec is barred at gate 1, which
    ///               the authority map cannot widen.
    fn default() -> Self {
        let mut authorities = BTreeMap::new();
        authorities.insert(ActionClass::Tag, Authority::Auto);
        authorities.insert(ActionClass::Comment, Authority::Auto);
        authorities.insert(ActionClass::Dedupe, Authority::Auto);
        authorities.insert(ActionClass::Route, Authority::Auto);
        authorities.insert(ActionClass::Park, Authority::Auto);
        authorities.insert(ActionClass::Queue, Authority::Auto);
        authorities.insert(ActionClass::Ask, Authority::Auto);
        authorities.insert(ActionClass::Approve, Authority::Propose);
        authorities.insert(ActionClass::Reject, Authority::Propose);
        Self {
            authorities,
            intake: IntakeConfig::default(),
            grounding_required: true,
        }
    }
}

impl AutopilotEnvelope {
    /// The authority for one action class. Falls back to the most conservative
    /// `Never` for any action not present in the map (so a partially-built
    /// envelope never silently auto-executes an unconfigured action).
    pub(crate) fn authority_for(&self, action: ActionClass) -> Authority {
        self.authorities
            .get(&action)
            .copied()
            .unwrap_or(Authority::Never)
    }

    /// Overlay a sparse override map (e.g. from [`parse_authority_overrides`])
    /// onto this envelope's authority table. Only the listed actions change;
    /// the rest keep their existing (default) authority. NOTE: this widens the
    /// authority map only — it cannot widen the fence (gate 1) or the grounding
    /// bound (gate 3), which are not part of the authority map.
    pub(crate) fn with_overrides(mut self, overrides: BTreeMap<ActionClass, Authority>) -> Self {
        self.authorities.extend(overrides);
        self
    }
}

/// The PURE four-gate evaluation. Given the envelope, the set of in-fence
/// (eligible / touchable) spec ids, and one proposed decision, decide whether
/// to execute, hold, or escalate. Side-effect-free and exhaustively unit
/// testable — exactly like `select_intake_candidates`.
///
/// `fenced_ids` is the set of specs that ARE inside the fence (the `eligible`
/// partition from `select_intake_candidates`): membership = touchable. A spec
/// absent from this set was already excluded by the fence and is dropped.
///
/// The gates, AND-composed for `Execute`:
/// 1. **fence membership** — spec must be in `fenced_ids`. NOT overridable by
///    the authority map. Fail -> [`Outcome::Hold`] (drop; the fence handles it).
/// 2. **authority == auto** — `Never` -> escalate, `Propose` -> hold,
///    `Auto` -> continue.
/// 3. **grounding resolvable** — Type-A or recorded-B (when
///    `grounding_required`). NOT overridable by the authority map. Fail ->
///    escalate (corpus gap).
/// 4. **under risk ceiling** — `risk` within [`AUTOPILOT_RISK_CEILING`].
///    Fail -> escalate (risk).
///
/// Invariant (Risks #1/#3 in the plan): gates 1 and 3 are HARD bounds the
/// authority map can never relax — even `approve = "auto"` cannot touch a
/// fenced spec or resolve an unrecorded-B/Type-C call.
pub(crate) fn evaluate(
    env: &AutopilotEnvelope,
    fenced_ids: &HashSet<String>,
    d: &Decision,
) -> Outcome {
    // Gate 1 — fence membership (outermost, not overridable by authority).
    if !fenced_ids.contains(&d.spec_id) {
        return Outcome::Hold;
    }

    // Gate 2 — action authority.
    match env.authority_for(d.action) {
        Authority::Never => return Outcome::Escalate(EscalateReason::NeverAuthority),
        Authority::Propose => return Outcome::Hold,
        Authority::Auto => {}
    }

    // Gate 3 — grounding (not overridable by authority).
    if env.grounding_required && !is_resolvable(d.grounding) {
        return Outcome::Escalate(EscalateReason::GroundingGap);
    }

    // Gate 4 — risk ceiling.
    if !d.risk.within_ceiling(AUTOPILOT_RISK_CEILING) {
        return Outcome::Escalate(EscalateReason::RiskCeiling);
    }

    Outcome::Execute
}

/// Parse the `[autopilot]` section of a config string into a SPARSE override
/// map (only the action classes explicitly set). Section-aware; hand-rolled to
/// stay dependency-light, mirroring `intake.rs`'s `[intake]` scanner. Unknown
/// keys / values are skipped so a typo leaves that action at its default.
pub(crate) fn parse_authority_overrides(toml_section: &str) -> BTreeMap<ActionClass, Authority> {
    let mut overrides = BTreeMap::new();
    let mut in_section = false;
    for raw in toml_section.lines() {
        let line = strip_inline_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(stripped) = line.strip_prefix('[') {
            in_section = stripped.trim_end_matches(']').trim() == "autopilot";
            continue;
        }
        if in_section {
            if let Some((k, v)) = line.split_once('=') {
                let key = k.trim();
                let val = v.trim().trim_matches('"').trim_matches('\'').trim();
                if let (Some(action), Some(authority)) =
                    (ActionClass::parse(key), Authority::parse(val))
                {
                    overrides.insert(action, authority);
                }
            }
        }
    }
    overrides
}

/// Strip a `#` inline comment that is not inside quotes. Same shape as
/// `intake::strip_inline_comment`, duplicated locally so the module is
/// self-contained (the intake one is private).
fn strip_inline_comment(s: &str) -> &str {
    let (mut dq, mut sq) = (false, false);
    for (i, c) in s.char_indices() {
        match c {
            '"' if !sq => dq = !dq,
            '\'' if !dq => sq = !sq,
            '#' if !dq && !sq => return &s[..i],
            _ => {}
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presence::is_keystone_class;

    /// Build an in-fence (eligible) id set from string literals.
    fn fence_of<const N: usize>(ids: [&str; N]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    /// Build a `Decision` with the load-bearing fields set; reason/evidence are
    /// not gate inputs.
    fn decision(
        spec: &str,
        action: ActionClass,
        grounding: Grounding,
        risk: RiskLevel,
    ) -> Decision {
        Decision {
            spec_id: spec.to_string(),
            action,
            grounding,
            risk,
            reason: "test".to_string(),
            evidence: vec![],
        }
    }

    #[test]
    fn evaluate_auto_action_in_fence_grounded_under_ceiling_executes() {
        // Happy path: all four gates pass.
        let env = AutopilotEnvelope::default();
        let fence = fence_of(["TASK-1"]);
        let d = decision("TASK-1", ActionClass::Tag, Grounding::TypeA, RiskLevel::Low);
        assert_eq!(evaluate(&env, &fence, &d), Outcome::Execute);
    }

    #[test]
    fn evaluate_propose_action_holds_even_when_grounded() {
        // Authority gate dominates: a `propose` action holds even with perfect
        // grounding and acceptable risk.
        let env = AutopilotEnvelope::default();
        let fence = fence_of(["TASK-1"]);
        let d = decision(
            "TASK-1",
            ActionClass::Approve,
            Grounding::TypeA,
            RiskLevel::Low,
        );
        assert_eq!(evaluate(&env, &fence, &d), Outcome::Hold);
    }

    #[test]
    fn evaluate_never_action_escalates() {
        // A `never` action escalates (hard exclusion).
        let env = AutopilotEnvelope {
            authorities: {
                let mut m = AutopilotEnvelope::default().authorities;
                m.insert(ActionClass::Tag, Authority::Never);
                m
            },
            ..AutopilotEnvelope::default()
        };
        let fence = fence_of(["TASK-1"]);
        let d = decision("TASK-1", ActionClass::Tag, Grounding::TypeA, RiskLevel::Low);
        assert_eq!(
            evaluate(&env, &fence, &d),
            Outcome::Escalate(EscalateReason::NeverAuthority)
        );
    }

    #[test]
    fn evaluate_unrecorded_b_escalates_even_if_authority_auto() {
        // Grounding gate dominates authority: an `auto` action with an
        // unrecorded-B grounding still escalates (gate 3 is not overridable).
        let env = AutopilotEnvelope::default();
        let fence = fence_of(["TASK-1"]);
        let d = decision(
            "TASK-1",
            ActionClass::Tag,
            Grounding::UnrecordedB,
            RiskLevel::Low,
        );
        assert_eq!(
            evaluate(&env, &fence, &d),
            Outcome::Escalate(EscalateReason::GroundingGap)
        );
    }

    #[test]
    fn evaluate_type_c_escalates() {
        // Type-C (synthesized in-flight context) escalates regardless of `auto`.
        let env = AutopilotEnvelope::default();
        let fence = fence_of(["TASK-1"]);
        let d = decision("TASK-1", ActionClass::Tag, Grounding::TypeC, RiskLevel::Low);
        assert_eq!(
            evaluate(&env, &fence, &d),
            Outcome::Escalate(EscalateReason::GroundingGap)
        );
    }

    #[test]
    fn evaluate_risk_above_ceiling_parks() {
        // Risk gate: a High-risk decision parks even when grounded and `auto`.
        let env = AutopilotEnvelope::default();
        let fence = fence_of(["TASK-1"]);
        let d = decision(
            "TASK-1",
            ActionClass::Tag,
            Grounding::TypeA,
            RiskLevel::High,
        );
        assert_eq!(
            evaluate(&env, &fence, &d),
            Outcome::Escalate(EscalateReason::RiskCeiling)
        );
        // Unknown blast radius is also above a Medium ceiling.
        let d2 = decision(
            "TASK-1",
            ActionClass::Tag,
            Grounding::TypeA,
            RiskLevel::Unknown,
        );
        assert_eq!(
            evaluate(&env, &fence, &d2),
            Outcome::Escalate(EscalateReason::RiskCeiling)
        );
    }

    #[test]
    fn evaluate_spec_not_in_fence_drops_regardless_of_authority() {
        // Gate 1 is outermost: a spec absent from the fence drops (Hold) even
        // with an `auto` authority, perfect grounding, and low risk.
        let env = AutopilotEnvelope::default();
        let fence = fence_of(["TASK-1"]);
        let d = decision("TASK-2", ActionClass::Tag, Grounding::TypeA, RiskLevel::Low);
        assert_eq!(evaluate(&env, &fence, &d), Outcome::Hold);
    }

    #[test]
    fn default_envelope_holds_approve_and_reject() {
        // Safe zero-config: approve + reject are `propose` -> held.
        let env = AutopilotEnvelope::default();
        assert_eq!(env.authority_for(ActionClass::Approve), Authority::Propose);
        assert_eq!(env.authority_for(ActionClass::Reject), Authority::Propose);
        let fence = fence_of(["TASK-1"]);
        for action in [ActionClass::Approve, ActionClass::Reject] {
            let d = decision("TASK-1", action, Grounding::TypeA, RiskLevel::Low);
            assert_eq!(evaluate(&env, &fence, &d), Outcome::Hold);
        }
    }

    #[test]
    fn default_envelope_autos_tag_comment_park() {
        // The reversible actions are `auto` and flow through to Execute.
        let env = AutopilotEnvelope::default();
        let fence = fence_of(["TASK-1"]);
        for action in [ActionClass::Tag, ActionClass::Comment, ActionClass::Park] {
            assert_eq!(env.authority_for(action), Authority::Auto);
            let d = decision("TASK-1", action, Grounding::TypeA, RiskLevel::Low);
            assert_eq!(evaluate(&env, &fence, &d), Outcome::Execute);
        }
    }

    #[test]
    fn dedupe_auto_adds_link_but_reject_half_routes_through_reject_authority() {
        // Split-verb invariant: dedupe's `auto` half (propose-the-link) executes,
        // but the destructive reject half is a SEPARATE action governed by
        // reject's (default `propose`) authority.
        let env = AutopilotEnvelope::default();
        let fence = fence_of(["TASK-1"]);
        let dedupe = decision(
            "TASK-1",
            ActionClass::Dedupe,
            Grounding::TypeA,
            RiskLevel::Low,
        );
        assert_eq!(evaluate(&env, &fence, &dedupe), Outcome::Execute);
        let reject = decision(
            "TASK-1",
            ActionClass::Reject,
            Grounding::TypeA,
            RiskLevel::Low,
        );
        assert_eq!(evaluate(&env, &fence, &reject), Outcome::Hold);
    }

    #[test]
    fn parse_authority_overrides_widens_single_action() {
        // Config widens a single action; the returned map is sparse (only the
        // explicitly-set action) so the envelope keeps every other default.
        let overrides = parse_authority_overrides("[autopilot]\napprove = \"auto\"\n");
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides.get(&ActionClass::Approve), Some(&Authority::Auto));

        let env = AutopilotEnvelope::default().with_overrides(overrides);
        // Approve is now auto; reject stays at its default propose.
        assert_eq!(env.authority_for(ActionClass::Approve), Authority::Auto);
        assert_eq!(env.authority_for(ActionClass::Reject), Authority::Propose);
        let fence = fence_of(["TASK-1"]);
        let d = decision(
            "TASK-1",
            ActionClass::Approve,
            Grounding::TypeA,
            RiskLevel::Low,
        );
        assert_eq!(evaluate(&env, &fence, &d), Outcome::Execute);
    }

    #[test]
    fn parse_authority_overrides_cannot_override_keystone_fence() {
        // INVARIANT: config can widen the authority map but can NEVER widen the
        // fence (gate 1). A keystone-class spec is excluded from the eligible
        // set by `is_keystone_class`; even `approve = "auto"` cannot touch it.
        let overrides = parse_authority_overrides("[autopilot]\napprove = \"auto\"\n");
        let env = AutopilotEnvelope::default().with_overrides(overrides);

        // Build the eligible (in-fence) set the way the launcher would: keystone
        // classes are fenced OUT via the canonical detector.
        let specs: [(&str, &str, Vec<&str>); 2] = [
            ("TASK-safe", "task", vec![]),
            ("EPIC-key", "epic", vec!["architecture"]),
        ];
        let fence: HashSet<String> = specs
            .iter()
            .filter(|(_, ty, tags)| !is_keystone_class(ty, tags.iter().copied()))
            .map(|(id, _, _)| id.to_string())
            .collect();

        // The keystone epic is NOT in the fence -> gate 1 drops it regardless of
        // the widened authority.
        let keystone = decision(
            "EPIC-key",
            ActionClass::Approve,
            Grounding::TypeA,
            RiskLevel::Low,
        );
        assert_eq!(evaluate(&env, &fence, &keystone), Outcome::Hold);

        // Sanity: the safe spec, with the same widened authority, executes.
        let safe = decision(
            "TASK-safe",
            ActionClass::Approve,
            Grounding::TypeA,
            RiskLevel::Low,
        );
        assert_eq!(evaluate(&env, &fence, &safe), Outcome::Execute);
    }
}
