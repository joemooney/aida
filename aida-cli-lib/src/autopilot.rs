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
//! What is wired: the read-only inspect/audit/challenge surface (TASK-1147),
//! the `aida zen` draft approve-gate (TASK-1037), the mode-composition
//! precedence contract [`effective_envelope`] (TASK-1020, the TASK-0432
//! ratification), and the durable execution audit + one-command reversal in the
//! sibling [`crate::autopilot_audit`] (TASK-1018, the TASK-0430 design). What is
//! deliberately NOT wired: a `groom --autopilot`
//! execution path — granting real approve/reject/queue authority is
//! keystone-autonomy the EPIC-0428 TASK-1147 decision defers.
//!
//! Design + the conservative default authority table:
//! `docs/plans/2026-06-29-epic-0428-policy-envelope.md`.
//! trace:TASK-1007 trace:TASK-0429 | ai:claude
#![allow(dead_code)]

use std::collections::{BTreeMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::backlog::RiskLevel;
use crate::intake::IntakeConfig;
use crate::presence::SoloPosture;

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

    /// Whether this action class is REVERSIBLE — i.e. a later command can put
    /// the spec back the way it was from a recorded prior state.
    ///
    /// This is the structural half of the gate-4 risk ceiling: gate 4 bounds a
    /// decision by its *risk read*, and this bounds it by its *action shape*.
    /// [`crate::autopilot_audit::execution_record`] refuses to mint a durable
    /// execution record for an irreversible class, so "every `Execute` outcome
    /// has a one-command reversal" is enforced at the type level rather than by
    /// convention.
    ///
    /// Every class in the taxonomy is reversible today (that is *why* the
    /// default authority table can grant `auto` at all): status flips restore,
    /// tags remove, queue entries pop, and the two append-only actions
    /// (`Comment` / `Ask`) reverse by retraction note. A future irreversible
    /// class (delete, force-push, external-side-effect) returns `false` here and
    /// is thereby barred from auto-execution.
    // trace:TASK-1018 | ai:claude
    pub(crate) fn is_reversible(self) -> bool {
        match self {
            ActionClass::Approve
            | ActionClass::Reject
            | ActionClass::Dedupe
            | ActionClass::Tag
            | ActionClass::Queue
            | ActionClass::Park
            | ActionClass::Route => true,
            // Append-only: the substrate keeps the comment/finding, so the
            // reversal is a recorded RETRACTION rather than a deletion. Still
            // reversible in the sense that matters — the operator can undo the
            // effect with one command and the trail stays honest.
            ActionClass::Comment | ActionClass::Ask => true,
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

impl Grounding {
    /// Stable lowercase token for display + the durable audit record.
    // trace:TASK-1018 | ai:claude
    pub(crate) fn token(self) -> &'static str {
        match self {
            Grounding::TypeA => "type-a",
            Grounding::RecordedB => "recorded-b",
            Grounding::UnrecordedB => "unrecorded-b",
            Grounding::TypeC => "type-c",
        }
    }
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

// ---------------------------------------------------------------------------
// TASK-1020 — mode-composition precedence (the TASK-0432 contract, ratified in
// code).
//
// Autopilot is a GROOMING-stage posture; the three-mode ladder (`--zen` /
// `--no-human`) is a DRAINING-stage axis. They never decide the same prompt,
// so there is no "which wins" between them — the only real composition is
// CONTEXT TIGHTENING of the envelope, and it is DEMOTE-ONLY: a headless
// context or an active solo posture can only make the envelope stricter,
// never wider. The worst-case composition bug is therefore over-conservatism
// (a held action), never an un-gated execute.
// (`docs/architecture/autonomy-and-escalation.md` §8, the composition matrix.)
// trace:TASK-1020 trace:TASK-0432 | ai:claude
// ---------------------------------------------------------------------------

impl Authority {
    /// Strictness rank for the demote-only composition:
    /// `Auto` (widest) < `Propose` < `Never` (strictest).
    fn strictness(self) -> u8 {
        match self {
            Authority::Auto => 0,
            Authority::Propose => 1,
            Authority::Never => 2,
        }
    }
}

/// PURE: compose the base envelope (defaults + `[autopilot]` overrides) with
/// the runtime context — headlessness and the solo posture — into the
/// EFFECTIVE envelope the four-gate [`evaluate`] runs under. The precedence
/// contract (autonomy doc §8):
///
/// - **default context** (interactive, solo off): the base envelope, untouched.
/// - **headless** (`AIDA_HEADLESS`): a headless run cannot pause-and-ask, so
///   every `propose` (pause-and-ask) authority demotes to `never` — the
///   would-be hold becomes a RECORDED escalation that enters the §2 cascade
///   instead of a report line nobody is watching. `auto` actions are untouched
///   (in-fence, grounded, under-ceiling autos are exactly what a headless
///   groom is for), and `grounding_required` is forced on — headless can never
///   relax gate 3.
/// - **solo [`SoloPosture::ParkForHuman`]** (solo active + keystone context):
///   every action demotes to `never` — the same "park keystone for the human"
///   verdict the drain-side posture ships. Belt-and-braces: gate 1 already
///   fences keystone specs via the SAME `is_keystone_class` detector, so this
///   branch firing means the fence and the posture AGREE, not that one rescued
///   the other.
/// - **solo [`SoloPosture::ProceedOnDefault`] / [`SoloPosture::Inactive`]**:
///   the base envelope — solo never WIDENS autopilot authority (solo is
///   drain-side discretion, not a grooming-stage grant).
///
/// Demote-only invariant: for every action class, the effective authority is
/// at least as strict as the base. Unit-tested over the full
/// (headless × posture × base-authority) cross-product.
pub(crate) fn effective_envelope(
    base: AutopilotEnvelope,
    headless: bool,
    solo: SoloPosture,
) -> AutopilotEnvelope {
    let mut env = base;
    if headless {
        env.grounding_required = true;
        for authority in env.authorities.values_mut() {
            if *authority == Authority::Propose {
                *authority = Authority::Never;
            }
        }
    }
    if matches!(solo, SoloPosture::ParkForHuman) {
        for authority in env.authorities.values_mut() {
            *authority = Authority::Never;
        }
    }
    env
}

/// Is this process running with nobody in the loop (`AIDA_HEADLESS=1`)?
///
/// The one reader of the flag for the autopilot surfaces, so the envelope
/// tightening and the audit trail's supervision context can never disagree
/// about whether a run was attended.
// trace:TASK-1022 | ai:claude
pub(crate) fn current_headless() -> bool {
    std::env::var("AIDA_HEADLESS").as_deref() == Ok("1")
}

// ---------------------------------------------------------------------------
// TASK-1019 — product-role recommendations as EVIDENCE feeding gate 3, never as
// authority (the TASK-0431 producer half; the reader half is TASK-1013).
//
// The product seat is non-privileged BY CONSTRUCTION: the TASK-647 advisor gate
// downgrades a product `--status approved` to Draft and refuses its queue
// writes. So a product recommendation can only ever reach autopilot as durable,
// inert metadata on a draft — `from-product:<who>` provenance, a
// `recommend:<disposition>` opinion, a `risk:<level>` flag, and `cites:<ref>`
// substrate citations backing its rationale.
//
// This section is the rule by which the envelope CONSUMES that metadata. The
// safety property is an ASYMMETRY, and it is structural rather than conventional:
//
//   - Product input feeds gate 3 ONLY, and only by supplying substrate the
//     cold-boot advisor independently VERIFIED is recorded. A product *claim* of
//     grounding is not self-certifying: an unverified citation changes nothing.
//   - Gate 1 (fence) and gate 2 (authority) never see product input at all —
//     [`decision_with_product_evidence`] copies `spec_id` and `action` verbatim,
//     so `recommend:approve` is context for a reader, not a lever on the gates.
//   - Gate 4 (risk) is RAISE-ONLY: a product `risk:high` tightens the ceiling
//     check, a product `risk:low` is discarded. Product input is monotonic
//     toward caution.
//
// Design: `docs/plans/2026-06-29-epic-0428-product-role-integration.md`.
// trace:TASK-1019 trace:TASK-0431 | ai:claude
// ---------------------------------------------------------------------------

/// Durable provenance tag on a product-filed draft: `from-product:<who>`. Its
/// presence is what makes a set of tags PRODUCT input at all — an anonymous
/// `recommend:` with no provenance is not attributable and is ignored.
pub(crate) const FROM_PRODUCT_TAG_PREFIX: &str = "from-product:";

/// The product seat's recommended disposition: `recommend:<action-class>`.
/// Deliberately NOT a gate input — see [`decision_with_product_evidence`].
pub(crate) const RECOMMEND_TAG_PREFIX: &str = "recommend:";

/// The product seat's risk read: `risk:<low|medium|high|unknown>`. Composes
/// raise-only with the advisor's own read.
pub(crate) const PRODUCT_RISK_TAG_PREFIX: &str = "risk:";

/// The durable tag form of a rationale's substrate citation: `cites:<ref>` — a
/// memory name, doc path, or prior spec/decision id the product seat says backs
/// its recommendation. A citation is a POINTER, never a proof; it only moves
/// gate 3 once the advisor has verified the ref genuinely exists.
pub(crate) const PRODUCT_CITES_TAG_PREFIX: &str = "cites:";

/// The structured recommendation a product seat left on a draft, parsed from its
/// durable tags. Inert on its own: nothing here changes a spec's state, and only
/// [`apply_product_evidence`] gives any of it a bounded effect.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProductInput {
    /// The named seat from `from-product:<who>` (empty when the marker is bare).
    pub(crate) who: String,
    /// The recommended disposition, when it parses to a known action class.
    /// Recorded for the audit trail and the human report; NOT a gate input.
    pub(crate) recommend: Option<ActionClass>,
    /// The product seat's risk read, composed raise-only with the advisor's.
    pub(crate) risk: Option<RiskLevel>,
    /// Substrate refs cited as backing the recommendation, in tag order.
    pub(crate) cites: Vec<String>,
}

/// The `<value>` half of one `<prefix><value>` tag, case-insensitively matched
/// and whitespace-trimmed, or `None` when the tag is not that marker.
fn tag_value<'a>(tag: &'a str, prefix: &str) -> Option<&'a str> {
    let tag = tag.trim();
    let head = tag.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| tag[prefix.len()..].trim())
}

/// PURE: parse a spec's tags into a [`ProductInput`].
///
/// `None` unless a `from-product:` provenance tag is present — an unattributed
/// `recommend:` is not product input and must not be consumed as any, which is
/// also the anti-laundering property: consumed product evidence always names
/// (or at minimum marks) the seat it came from.
// trace:TASK-1019 | ai:claude
pub(crate) fn parse_product_input(tags: &[String]) -> Option<ProductInput> {
    let who = tags
        .iter()
        .find_map(|t| tag_value(t, FROM_PRODUCT_TAG_PREFIX))?
        .to_string();
    Some(ProductInput {
        who,
        recommend: tags
            .iter()
            .find_map(|t| tag_value(t, RECOMMEND_TAG_PREFIX))
            .and_then(ActionClass::parse),
        risk: tags
            .iter()
            .find_map(|t| tag_value(t, PRODUCT_RISK_TAG_PREFIX))
            .and_then(|v| RiskLevel::parse(v).ok()),
        cites: tags
            .iter()
            .filter_map(|t| tag_value(t, PRODUCT_CITES_TAG_PREFIX))
            .filter(|c| !c.is_empty())
            .map(|c| c.to_string())
            .collect(),
    })
}

/// True when at least one of the product seat's citations is in `recorded` —
/// the set of refs the cold-boot advisor INDEPENDENTLY VERIFIED exists in
/// substrate. Case-insensitive so a hand-written citation is not lost on
/// capitalization.
///
/// This is the whole trust boundary of the feature: the product seat supplies
/// the pointer, the advisor supplies the verification. A rationale that merely
/// *asserts* a preference exists is unverified and moves nothing.
// trace:TASK-1019 | ai:claude
pub(crate) fn product_citation_verified(input: &ProductInput, recorded: &HashSet<String>) -> bool {
    let recorded: Vec<String> = recorded.iter().map(|r| r.trim().to_lowercase()).collect();
    input
        .cites
        .iter()
        .any(|c| recorded.iter().any(|r| *r == c.trim().to_lowercase()))
}

/// PURE: compose product evidence into the two gate inputs it is allowed to
/// touch, with the raise-only asymmetry.
///
/// - **Grounding** rises only from a NON-resolvable classification
///   (unrecorded-B / Type-C) to `RecordedB`, and only on a VERIFIED citation.
///   It never reaches `TypeA` — a recorded PRINCIPLE is not a product seat's to
///   assert — and an already-resolvable grounding passes through untouched
///   (product input is not authority over the advisor's own classification in
///   either direction).
/// - **Risk** takes the stricter of the two reads. A product `risk:high` raises;
///   a product `risk:low` under an advisor `medium` is discarded.
///
/// Everything else about the decision is out of reach. Gate 3 is the only gate
/// this can move, and only in the one direction the substrate itself justifies.
// trace:TASK-1019 | ai:claude
pub(crate) fn apply_product_evidence(
    grounding: Grounding,
    risk: RiskLevel,
    input: &ProductInput,
    recorded: &HashSet<String>,
) -> (Grounding, RiskLevel) {
    let grounded = if !is_resolvable(grounding) && product_citation_verified(input, recorded) {
        Grounding::RecordedB
    } else {
        grounding
    };
    // Raise-only: keep the advisor's read unless the product's is STRICTER.
    // `within_ceiling` is the crate's risk ordering (Unknown ranks above Medium),
    // so "product risk is admitted under the advisor's ceiling" == "not stricter".
    let risked = match input.risk {
        Some(p) if !p.within_ceiling(risk) => p,
        _ => risk,
    };
    (grounded, risked)
}

/// The `product:<who>` evidence marker for the durable audit record — the
/// spelling the TASK-1013 `--from-product` filter reads
/// ([`crate::autopilot_audit::PRODUCT_EVIDENCE_PREFIX`]). Emitting it is what
/// makes "did this action consume product input?" answerable after the fact.
// trace:TASK-1019 trace:TASK-1013 | ai:claude
pub(crate) fn product_evidence_marker(input: &ProductInput) -> String {
    format!(
        "{}{}",
        crate::autopilot_audit::PRODUCT_EVIDENCE_PREFIX,
        input.who.trim()
    )
}

/// PURE: the decision as it enters [`evaluate`] once product evidence is
/// consumed.
///
/// `spec_id` and `action` are copied VERBATIM — that is the structural
/// non-authority guarantee, not a convention: gates 1 and 2 are computed from
/// fields this function cannot reach, so a `recommend:approve` can never widen
/// the fence, raise a `propose` authority, or change what is being proposed.
/// Only `grounding` and `risk` move, under [`apply_product_evidence`]'s
/// raise-only rule, and the `product:<who>` marker is appended so the audit
/// records the consumption.
// trace:TASK-1019 | ai:claude
pub(crate) fn decision_with_product_evidence(
    d: &Decision,
    input: &ProductInput,
    recorded: &HashSet<String>,
) -> Decision {
    let (grounding, risk) = apply_product_evidence(d.grounding, d.risk, input, recorded);
    let mut evidence = d.evidence.clone();
    let marker = product_evidence_marker(input);
    if !evidence.iter().any(|e| e.trim() == marker) {
        evidence.push(marker);
    }
    Decision {
        // Untouchable by product input — gate 1 and gate 2 inputs.
        spec_id: d.spec_id.clone(),
        action: d.action,
        grounding,
        risk,
        reason: d.reason.clone(),
        evidence,
    }
}

/// The four-gate [`evaluate`], run over a decision that consumed a product
/// recommendation. Thin by design: all the bounding lives in
/// [`decision_with_product_evidence`], so the gates themselves stay the single
/// implementation and cannot drift into a product-specific relaxation.
// trace:TASK-1019 | ai:claude
pub(crate) fn evaluate_with_product_evidence(
    env: &AutopilotEnvelope,
    fenced_ids: &HashSet<String>,
    d: &Decision,
    input: &ProductInput,
    recorded: &HashSet<String>,
) -> Outcome {
    evaluate(
        env,
        fenced_ids,
        &decision_with_product_evidence(d, input, recorded),
    )
}

// ---------------------------------------------------------------------------
// TASK-1147 — the auditability + reversal SURFACE (read-only half of EPIC-0428).
//
// This is deliberately the SAFE substrate: it INSPECTS what the envelope WOULD
// decide (a side-effect-free dry-run over the live groom/intake candidates) and
// records those projected verdicts to a lightweight local append log so they
// can be reviewed and CHALLENGED (reversed) after the fact. It does NOT grant
// real approve/reject/queue authority — the envelope is never wired to execute
// a disposition here. That keystone-autonomy slice is deferred.
// trace:TASK-1147 | ai:claude
// ---------------------------------------------------------------------------

impl Outcome {
    /// Short, stable verdict token for display + the audit log.
    pub(crate) fn verdict_token(self) -> &'static str {
        match self {
            Outcome::Execute => "execute",
            Outcome::Hold => "hold",
            Outcome::Escalate(_) => "escalate",
        }
    }

    /// Which gate produced this verdict — the "why" column of the inspect view
    /// and the audit trail. `inspect` only ever runs [`evaluate`] over in-fence
    /// (eligible) specs, so a `Hold` here is always the gate-2 propose authority
    /// (the gate-1 fence drop is surfaced separately with its richer reason).
    pub(crate) fn gate_label(self) -> &'static str {
        match self {
            Outcome::Execute => "all-gates-pass",
            Outcome::Hold => "gate2:authority(propose)",
            Outcome::Escalate(EscalateReason::NeverAuthority) => "gate2:authority(never)",
            Outcome::Escalate(EscalateReason::GroundingGap) => "gate3:grounding",
            Outcome::Escalate(EscalateReason::RiskCeiling) => "gate4:risk-ceiling",
        }
    }
}

/// One row of the `aida autopilot inspect` dry-run: the envelope's verdict for
/// one proposed action on one candidate spec. Pure projection — carries no
/// side effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InspectRow {
    pub spec_id: String,
    pub action: ActionClass,
    pub outcome: Outcome,
    /// The one-line rationale carried on the source [`Decision`].
    pub reason: String,
}

/// The PURE inspect projection: run the envelope over each proposed decision
/// and collect the per-spec verdict WITHOUT mutating anything. This is the
/// read-only heart of `aida autopilot inspect`; exhaustively unit-testable like
/// [`evaluate`] itself.
// trace:TASK-1147 | ai:claude
pub(crate) fn project_decisions(
    env: &AutopilotEnvelope,
    fenced_ids: &HashSet<String>,
    decisions: &[Decision],
) -> Vec<InspectRow> {
    decisions
        .iter()
        .map(|d| InspectRow {
            spec_id: d.spec_id.clone(),
            action: d.action,
            outcome: evaluate(env, fenced_ids, d),
            reason: d.reason.clone(),
        })
        .collect()
}

/// One line of the local autopilot audit log (`.aida/autopilot-audit.jsonl`).
///
/// Two PROJECTION entry kinds share one shape here:
///   - `decision`  — a projected envelope verdict for one spec + action.
///   - `challenge` — a reversal marker targeting a prior `decision` entry's id.
///
/// The same log also carries the TASK-0430 durable EXECUTION rows (`execution`
/// / `reversal`, defined in [`crate::autopilot_audit`]). One file, one format,
/// four discriminated kinds: readers filter on `type`, so the projection
/// readers below and the execution readers there never see each other's rows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct AuditEntry {
    /// Stable short id (`d########` for decisions, `c########` for challenges).
    pub id: String,
    /// RFC-3339 UTC timestamp.
    pub ts: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// (challenge only) the `decision` entry id this challenge reverses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// (challenge only) the operator's reversal note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// FNV-1a → 8 hex chars, prefixed to keep decision / challenge / execution /
/// reversal ids distinct. Shared with [`crate::autopilot_audit`] so every row in
/// the one log mints ids the same way.
// trace:TASK-1018 | ai:claude
pub(crate) fn short_id(prefix: char, seed: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in seed.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{prefix}{:08x}", (hash & 0xffff_ffff) as u32)
}

/// The local audit-log path. Lives under `.aida/` — runtime per-clone state by
/// the deny-by-default `.gitignore` convention, never committed.
pub(crate) fn audit_log_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("autopilot-audit.jsonl")
}

/// Build a `decision` audit entry from an inspected verdict. `seq` disambiguates
/// entries written in the same millisecond so ids stay unique within a batch.
pub(crate) fn decision_entry(
    ts: &str,
    seq: usize,
    spec_id: &str,
    action: ActionClass,
    outcome: Outcome,
    reason: &str,
    source: &str,
) -> AuditEntry {
    let seed = format!(
        "{ts}|{seq}|{spec_id}|{}|{}",
        action.token(),
        outcome.verdict_token()
    );
    AuditEntry {
        id: short_id('d', &seed),
        ts: ts.to_string(),
        kind: "decision".to_string(),
        spec_id: Some(spec_id.to_string()),
        action: Some(action.token().to_string()),
        verdict: Some(outcome.verdict_token().to_string()),
        gate: Some(outcome.gate_label().to_string()),
        reason: (!reason.trim().is_empty()).then(|| reason.to_string()),
        source: Some(source.to_string()),
        target: None,
        note: None,
    }
}

/// Build a `challenge` audit entry reversing the decision entry `target_id`.
pub(crate) fn challenge_entry(ts: &str, target_id: &str, note: Option<&str>) -> AuditEntry {
    AuditEntry {
        id: short_id('c', &format!("{ts}|{target_id}|challenge")),
        ts: ts.to_string(),
        kind: "challenge".to_string(),
        spec_id: None,
        action: None,
        verdict: None,
        gate: None,
        reason: None,
        source: None,
        target: Some(target_id.to_string()),
        note: note.map(|s| s.to_string()),
    }
}

/// Append entries to the local audit log, creating `.aida/` if needed.
pub(crate) fn append_audit_entries(
    project_root: &Path,
    entries: &[AuditEntry],
) -> std::io::Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let path = audit_log_path(project_root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    for e in entries {
        let line = serde_json::to_string(e)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        writeln!(file, "{line}")?;
    }
    Ok(())
}

/// Read the audit log. A missing file is an empty log (not an error); malformed
/// lines are skipped so one bad row never blinds the whole trail.
pub(crate) fn read_audit_entries(project_root: &Path) -> std::io::Result<Vec<AuditEntry>> {
    let path = audit_log_path(project_root);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<AuditEntry>(l).ok())
        // The log is shared with the TASK-1018 durable `execution` / `reversal`
        // rows; keep this reader to the PROJECTION kinds so the two surfaces
        // can never cross-talk. trace:TASK-1018
        .filter(|e: &AuditEntry| e.kind == "decision" || e.kind == "challenge")
        .collect())
}

/// True iff a `challenge` entry already targets this decision id.
pub(crate) fn is_challenged(entries: &[AuditEntry], decision_id: &str) -> bool {
    entries
        .iter()
        .any(|e| e.kind == "challenge" && e.target.as_deref() == Some(decision_id))
}

/// Resolve which decision entry a `challenge <target>` refers to. `target`
/// matches either a decision entry id EXACTLY, or (fallback) the most recent
/// still-UNCHALLENGED decision entry for that spec id (case-insensitive).
/// Returns the resolved decision id, or `None` if nothing matches. Pure +
/// exhaustively testable.
// trace:TASK-1147 | ai:claude
pub(crate) fn resolve_challenge_target(entries: &[AuditEntry], target: &str) -> Option<String> {
    // Exact id match against a decision entry wins.
    if let Some(e) = entries
        .iter()
        .find(|e| e.kind == "decision" && e.id == target)
    {
        return Some(e.id.clone());
    }
    // Else the latest un-challenged decision for that spec id.
    let challenged: HashSet<&str> = entries
        .iter()
        .filter(|e| e.kind == "challenge")
        .filter_map(|e| e.target.as_deref())
        .collect();
    entries
        .iter()
        .rev()
        .find(|e| {
            e.kind == "decision"
                && e.spec_id
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(target))
                && !challenged.contains(e.id.as_str())
        })
        .map(|e| e.id.clone())
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

    // ---- TASK-1147: inspect projection + audit/reversal ----

    #[test]
    fn project_decisions_yields_envelope_verdict_per_candidate() {
        // The dry-run projection is exactly `evaluate` per candidate, with NO
        // mutation. Default envelope: queue=auto (executes when grounded/low),
        // approve=propose (holds), and a high-risk queue escalates on gate 4.
        let env = AutopilotEnvelope::default();
        let fence = fence_of(["TASK-1", "TASK-2", "TASK-3"]);
        let decisions = vec![
            decision(
                "TASK-1",
                ActionClass::Queue,
                Grounding::TypeA,
                RiskLevel::Low,
            ),
            decision(
                "TASK-2",
                ActionClass::Approve,
                Grounding::TypeA,
                RiskLevel::Low,
            ),
            decision(
                "TASK-3",
                ActionClass::Queue,
                Grounding::TypeA,
                RiskLevel::High,
            ),
        ];
        let rows = project_decisions(&env, &fence, &decisions);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].outcome, Outcome::Execute);
        assert_eq!(rows[1].outcome, Outcome::Hold);
        assert_eq!(
            rows[2].outcome,
            Outcome::Escalate(EscalateReason::RiskCeiling)
        );
        // Verdict/gate labels are stable tokens for the audit trail.
        assert_eq!(rows[0].outcome.verdict_token(), "execute");
        assert_eq!(rows[1].outcome.gate_label(), "gate2:authority(propose)");
        assert_eq!(rows[2].outcome.gate_label(), "gate4:risk-ceiling");
    }

    #[test]
    fn audit_append_and_read_roundtrip() {
        let dir = std::env::temp_dir().join(format!("aida-ap-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Empty log reads as no entries.
        assert!(read_audit_entries(&dir).unwrap().is_empty());

        let e1 = decision_entry(
            "2026-07-15T00:00:00Z",
            0,
            "TASK-1",
            ActionClass::Queue,
            Outcome::Execute,
            "grounded + low risk",
            "inspect",
        );
        let e2 = decision_entry(
            "2026-07-15T00:00:00Z",
            1,
            "TASK-2",
            ActionClass::Approve,
            Outcome::Hold,
            "approve is propose-only",
            "inspect",
        );
        append_audit_entries(&dir, &[e1.clone(), e2.clone()]).unwrap();

        let read = read_audit_entries(&dir).unwrap();
        assert_eq!(read, vec![e1.clone(), e2.clone()]);
        // Distinct ids even in the same millisecond (seq disambiguates).
        assert_ne!(e1.id, e2.id);
        assert!(e1.id.starts_with('d'));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn challenge_resolves_and_records_reversal() {
        let dir = std::env::temp_dir().join(format!("aida-ap-chal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let d = decision_entry(
            "2026-07-15T00:00:00Z",
            0,
            "TASK-9",
            ActionClass::Queue,
            Outcome::Execute,
            "r",
            "inspect",
        );
        append_audit_entries(&dir, &[d.clone()]).unwrap();
        let entries = read_audit_entries(&dir).unwrap();

        // Not yet challenged.
        assert!(!is_challenged(&entries, &d.id));
        // Resolve by exact id AND by spec id (fallback to latest).
        assert_eq!(
            resolve_challenge_target(&entries, &d.id),
            Some(d.id.clone())
        );
        assert_eq!(
            resolve_challenge_target(&entries, "task-9"),
            Some(d.id.clone())
        );
        // Unknown target resolves to nothing.
        assert_eq!(resolve_challenge_target(&entries, "TASK-404"), None);

        // Record a challenge, then confirm it reads back as challenged and the
        // same decision no longer resolves via the spec-id fallback.
        let c = challenge_entry("2026-07-15T00:01:00Z", &d.id, Some("wrong call"));
        append_audit_entries(&dir, &[c]).unwrap();
        let entries = read_audit_entries(&dir).unwrap();
        assert!(is_challenged(&entries, &d.id));
        assert_eq!(resolve_challenge_target(&entries, "TASK-9"), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- TASK-1018: reversibility is a precondition for auto-execution ----

    #[test]
    fn every_auto_authority_action_is_reversible() {
        // INVARIANT: an action the default envelope may auto-execute MUST be
        // reversible — otherwise "every Execute outcome has a one-command
        // reversal" is unenforceable. Widening `[autopilot]` cannot break this
        // either: the durable record refuses to mint for an irreversible class.
        let env = AutopilotEnvelope::default();
        for action in ALL_ACTIONS {
            if env.authority_for(action) == Authority::Auto {
                assert!(
                    action.is_reversible(),
                    "{action:?} is auto-executable but not reversible"
                );
            }
        }
    }

    #[test]
    fn grounding_tokens_are_stable_and_distinct() {
        // The durable audit record stores the grounding as a token; drift there
        // would silently rewrite history's meaning.
        let tokens = [
            Grounding::TypeA.token(),
            Grounding::RecordedB.token(),
            Grounding::UnrecordedB.token(),
            Grounding::TypeC.token(),
        ];
        assert_eq!(tokens, ["type-a", "recorded-b", "unrecorded-b", "type-c"]);
        let unique: HashSet<&str> = tokens.iter().copied().collect();
        assert_eq!(unique.len(), tokens.len());
    }

    // ---- TASK-1020: mode-composition precedence (effective_envelope) ----

    /// Every action class, for the cross-product tests.
    const ALL_ACTIONS: [ActionClass; 9] = [
        ActionClass::Approve,
        ActionClass::Reject,
        ActionClass::Dedupe,
        ActionClass::Tag,
        ActionClass::Queue,
        ActionClass::Park,
        ActionClass::Route,
        ActionClass::Comment,
        ActionClass::Ask,
    ];

    #[test]
    fn effective_envelope_default_context_is_base_envelope() {
        // Interactive + solo off: composition is the identity.
        let base = AutopilotEnvelope::default();
        let eff = effective_envelope(base.clone(), false, SoloPosture::Inactive);
        for action in ALL_ACTIONS {
            assert_eq!(eff.authority_for(action), base.authority_for(action));
        }
        assert_eq!(eff.grounding_required, base.grounding_required);
    }

    #[test]
    fn effective_envelope_headless_demotes_propose_to_escalate() {
        // approve/reject default to propose (pause-and-ask); a headless run
        // cannot pause-and-ask, so they demote to never — and evaluate turns
        // the would-be Hold into a RECORDED escalation.
        let eff = effective_envelope(AutopilotEnvelope::default(), true, SoloPosture::Inactive);
        assert_eq!(eff.authority_for(ActionClass::Approve), Authority::Never);
        assert_eq!(eff.authority_for(ActionClass::Reject), Authority::Never);

        let fence = fence_of(["TASK-1"]);
        let d = decision(
            "TASK-1",
            ActionClass::Approve,
            Grounding::TypeA,
            RiskLevel::Low,
        );
        assert_eq!(
            evaluate(&eff, &fence, &d),
            Outcome::Escalate(EscalateReason::NeverAuthority)
        );
    }

    #[test]
    fn effective_envelope_headless_keeps_grounded_autos() {
        // Headless tightening never touches the auto tier — in-fence, grounded,
        // under-ceiling reversible actions are exactly what a headless groom is
        // for (still strictly more conservative than binary `--apply`).
        let eff = effective_envelope(AutopilotEnvelope::default(), true, SoloPosture::Inactive);
        assert_eq!(eff.authority_for(ActionClass::Tag), Authority::Auto);

        let fence = fence_of(["TASK-1"]);
        let d = decision("TASK-1", ActionClass::Tag, Grounding::TypeA, RiskLevel::Low);
        assert_eq!(evaluate(&eff, &fence, &d), Outcome::Execute);
    }

    #[test]
    fn effective_envelope_headless_forces_grounding_required() {
        // Headless can never relax gate 3, even if a base envelope somehow
        // arrived with grounding disabled.
        let base = AutopilotEnvelope {
            grounding_required: false,
            ..AutopilotEnvelope::default()
        };
        let eff = effective_envelope(base, true, SoloPosture::Inactive);
        assert!(eff.grounding_required);
    }

    #[test]
    fn effective_envelope_solo_keystone_parks() {
        // Solo active + keystone context: everything parks for the human —
        // the drain-side ParkForHuman verdict, mirrored at the grooming stage.
        let eff = effective_envelope(
            AutopilotEnvelope::default(),
            false,
            SoloPosture::ParkForHuman,
        );
        for action in ALL_ACTIONS {
            assert_eq!(eff.authority_for(action), Authority::Never);
        }
    }

    #[test]
    fn effective_envelope_solo_safe_partition_is_base() {
        // Solo active + safe work: solo never WIDENS the grooming envelope —
        // ProceedOnDefault is drain-side discretion, not a grooming grant.
        let base = AutopilotEnvelope::default();
        let eff = effective_envelope(base.clone(), false, SoloPosture::ProceedOnDefault);
        for action in ALL_ACTIONS {
            assert_eq!(eff.authority_for(action), base.authority_for(action));
        }
    }

    #[test]
    fn effective_envelope_never_widens() {
        // The demote-only invariant over the FULL (headless × posture ×
        // base-authority × action) cross-product: composition may hold or
        // raise strictness, never lower it.
        for headless in [false, true] {
            for solo in [
                SoloPosture::Inactive,
                SoloPosture::ProceedOnDefault,
                SoloPosture::ParkForHuman,
            ] {
                for base_authority in [Authority::Auto, Authority::Propose, Authority::Never] {
                    for action in ALL_ACTIONS {
                        let mut base = AutopilotEnvelope::default();
                        base.authorities.insert(action, base_authority);
                        let eff = effective_envelope(base, headless, solo);
                        assert!(
                            eff.authority_for(action).strictness() >= base_authority.strictness(),
                            "widened: {action:?} {base_authority:?} \
                             headless={headless} solo={solo:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn keystone_fence_and_solo_posture_agree() {
        // The one-classifier invariant (§8): a keystone fixture must be
        // (a) fenced out of the groom candidate set at gate 1 and (b) parked
        // by the drain-side solo posture — both via presence::is_keystone_class,
        // so no stage can disagree on "keystone".
        let tags = vec!["architecture".to_string()];
        let keystone = is_keystone_class("task", tags.iter().map(|s| s.as_str()));
        assert!(keystone);
        assert_eq!(
            crate::presence::resolve_solo_posture(true, keystone),
            SoloPosture::ParkForHuman
        );

        let spec = crate::intake::IntakeSpec {
            id: "TASK-KEY".to_string(),
            req_type: "task".to_string(),
            tags,
            deferred: false,
            risk: RiskLevel::Low,
            risk_reason: String::new(),
        };
        let (eligible, fenced) = crate::intake::select_intake_candidates(
            &[spec],
            &IntakeConfig::default(),
            &crate::intake::IntakeFilters::default(),
        );
        assert!(eligible.is_empty());
        assert!(matches!(
            fenced.as_slice(),
            [(_, crate::intake::FenceReason::Keystone(_))]
        ));
    }

    // ---- TASK-1019: product evidence feeds gate 3, never authority ----------
    //
    // The NEGATIVE tests are the point of this block. A product recommendation
    // is the one input that arrives from a seat with no privileges at all, so
    // every way it could become authority is asserted CLOSED here.

    fn tags_of<const N: usize>(tags: [&str; N]) -> Vec<String> {
        tags.iter().map(|s| s.to_string()).collect()
    }

    fn recorded_of<const N: usize>(refs: [&str; N]) -> HashSet<String> {
        refs.iter().map(|s| s.to_string()).collect()
    }

    /// A product seat that recommends approving, cites `PRIN-3`, and calls it
    /// low risk — the maximally-pushy input.
    fn pushy_product() -> ProductInput {
        parse_product_input(&tags_of([
            "from-product:pat",
            "recommend:approve",
            "risk:low",
            "cites:PRIN-3",
        ]))
        .expect("provenance present")
    }

    /// Every (grounding, risk) pair, for the cross-product invariants.
    const ALL_GROUNDINGS: [Grounding; 4] = [
        Grounding::TypeA,
        Grounding::RecordedB,
        Grounding::UnrecordedB,
        Grounding::TypeC,
    ];
    const ALL_RISKS: [RiskLevel; 4] = [
        RiskLevel::Low,
        RiskLevel::Medium,
        RiskLevel::Unknown,
        RiskLevel::High,
    ];

    #[test]
    fn parse_product_input_reads_the_four_tag_forms() {
        let input = parse_product_input(&tags_of([
            "batch:demo",
            "From-Product: Pat ",
            "recommend:queue",
            "risk:high",
            "cites:PRIN-3",
            "cites:docs/lifecycle.md",
        ]))
        .expect("provenance present");
        assert_eq!(input.who, "Pat");
        assert_eq!(input.recommend, Some(ActionClass::Queue));
        assert_eq!(input.risk, Some(RiskLevel::High));
        assert_eq!(input.cites, vec!["PRIN-3", "docs/lifecycle.md"]);
    }

    #[test]
    fn unattributed_recommendation_is_not_product_input() {
        // No `from-product:` provenance -> not attributable, not consumed. The
        // anti-laundering property: consumed product evidence always carries the
        // marker that makes it visible to the audit filter.
        assert_eq!(
            parse_product_input(&tags_of(["recommend:approve", "cites:PRIN-3"])),
            None
        );
    }

    #[test]
    fn product_evidence_raises_grounding_only_when_the_citation_is_verified() {
        // The trust boundary: the product seat supplies the POINTER, the
        // cold-boot advisor supplies the VERIFICATION.
        let input = pushy_product();

        let (verified, _) = apply_product_evidence(
            Grounding::TypeC,
            RiskLevel::Low,
            &input,
            &recorded_of(["PRIN-3"]),
        );
        assert_eq!(verified, Grounding::RecordedB);

        // A citation the advisor could NOT find in substrate moves nothing —
        // a product claim of grounding is not self-certifying.
        let (unverified, _) = apply_product_evidence(
            Grounding::TypeC,
            RiskLevel::Low,
            &input,
            &recorded_of(["PRIN-9"]),
        );
        assert_eq!(unverified, Grounding::TypeC);
    }

    #[test]
    fn product_evidence_never_reaches_type_a_and_never_demotes() {
        // `TypeA` is a recorded PRINCIPLE — not a product seat's to assert. And
        // an already-resolvable grounding passes through untouched: product
        // input is not authority over the advisor's classification in EITHER
        // direction.
        let recorded = recorded_of(["PRIN-3"]);
        let input = pushy_product();
        for g in [Grounding::UnrecordedB, Grounding::TypeC] {
            let (out, _) = apply_product_evidence(g, RiskLevel::Low, &input, &recorded);
            assert_eq!(out, Grounding::RecordedB, "{g:?} may only reach recorded-B");
        }
        for g in [Grounding::TypeA, Grounding::RecordedB] {
            let (out, _) = apply_product_evidence(g, RiskLevel::Low, &input, &recorded);
            assert_eq!(out, g, "{g:?} must pass through untouched");
        }
    }

    #[test]
    fn product_risk_flag_raises_only_and_never_lowers() {
        // Monotonic toward caution: a product `risk:high` tightens the gate-4
        // check; a product `risk:low` under a stricter advisor read is discarded.
        let recorded = recorded_of(["PRIN-3"]);
        let low = pushy_product();
        let high = parse_product_input(&tags_of(["from-product:pat", "risk:high"])).unwrap();
        for advisor_read in ALL_RISKS {
            let (_, lowered) =
                apply_product_evidence(Grounding::TypeA, advisor_read, &low, &recorded);
            assert_eq!(
                lowered, advisor_read,
                "product risk:low must not lower {advisor_read:?}"
            );

            let (_, raised) =
                apply_product_evidence(Grounding::TypeA, advisor_read, &high, &recorded);
            assert!(
                !raised.within_ceiling(advisor_read) || raised == advisor_read,
                "product risk:high must be at least as strict as {advisor_read:?}"
            );
        }
        // Concretely: low advisor read + product high == high.
        let (_, raised) =
            apply_product_evidence(Grounding::TypeA, RiskLevel::Low, &high, &recorded);
        assert_eq!(raised, RiskLevel::High);
    }

    #[test]
    fn product_recommendation_never_changes_the_proposed_action_or_spec() {
        // The structural non-authority guarantee: gate 1 and gate 2 read fields
        // this path copies verbatim, so `recommend:approve` cannot become an
        // approve, and cannot re-target another spec.
        let d = decision("TASK-1", ActionClass::Tag, Grounding::TypeA, RiskLevel::Low);
        let adjusted =
            decision_with_product_evidence(&d, &pushy_product(), &recorded_of(["PRIN-3"]));
        assert_eq!(adjusted.spec_id, "TASK-1");
        assert_eq!(adjusted.action, ActionClass::Tag);
    }

    #[test]
    fn product_recommend_approve_does_not_relax_propose_authority() {
        // Gate 2 dominates: the conservative default holds `approve`, and a
        // product recommendation — verified citation and all — cannot widen it.
        let env = AutopilotEnvelope::default();
        let fence = fence_of(["TASK-1"]);
        let d = decision(
            "TASK-1",
            ActionClass::Approve,
            Grounding::TypeA,
            RiskLevel::Low,
        );
        assert_eq!(
            evaluate_with_product_evidence(
                &env,
                &fence,
                &d,
                &pushy_product(),
                &recorded_of(["PRIN-3"])
            ),
            Outcome::Hold
        );
    }

    #[test]
    fn product_recommend_on_a_never_action_still_escalates() {
        // Gate 2's hard exclusion is likewise untouchable.
        let env = AutopilotEnvelope::default()
            .with_overrides([(ActionClass::Tag, Authority::Never)].into_iter().collect());
        let fence = fence_of(["TASK-1"]);
        let d = decision("TASK-1", ActionClass::Tag, Grounding::TypeA, RiskLevel::Low);
        assert_eq!(
            evaluate_with_product_evidence(
                &env,
                &fence,
                &d,
                &pushy_product(),
                &recorded_of(["PRIN-3"])
            ),
            Outcome::Escalate(EscalateReason::NeverAuthority)
        );
    }

    #[test]
    fn product_recommend_on_a_fenced_spec_is_still_dropped() {
        // Gate 1 dominates: an out-of-fence spec (keystone, deferred, whatever
        // the fence excluded) stays dropped. Product input has no special lane.
        let env = AutopilotEnvelope::default();
        let d = decision(
            "TASK-KEY",
            ActionClass::Tag,
            Grounding::TypeA,
            RiskLevel::Low,
        );
        assert_eq!(
            evaluate_with_product_evidence(
                &env,
                &fence_of(["TASK-OTHER"]),
                &d,
                &pushy_product(),
                &recorded_of(["PRIN-3"])
            ),
            Outcome::Hold
        );
    }

    #[test]
    fn product_risk_low_cannot_beat_the_gate_4_ceiling() {
        // Gate 4: a product `risk:low` on a high-risk decision does not buy an
        // execution.
        let env = AutopilotEnvelope::default();
        let fence = fence_of(["TASK-1"]);
        let d = decision(
            "TASK-1",
            ActionClass::Tag,
            Grounding::TypeA,
            RiskLevel::High,
        );
        assert_eq!(
            evaluate_with_product_evidence(
                &env,
                &fence,
                &d,
                &pushy_product(),
                &recorded_of(["PRIN-3"])
            ),
            Outcome::Escalate(EscalateReason::RiskCeiling)
        );
    }

    #[test]
    fn unverified_product_evidence_changes_no_verdict_anywhere() {
        // The strongest negative: with nothing verified, the product-aware path
        // is INDISTINGUISHABLE from the plain gates over the whole
        // (action × grounding × risk) cross-product — no escalation flips into
        // an execution, no hold flips into anything.
        let env = AutopilotEnvelope::default();
        let fence = fence_of(["TASK-1"]);
        let input = pushy_product();
        let nothing_recorded: HashSet<String> = HashSet::new();
        for action in ALL_ACTIONS {
            for g in ALL_GROUNDINGS {
                for risk in ALL_RISKS {
                    let d = decision("TASK-1", action, g, risk);
                    assert_eq!(
                        evaluate_with_product_evidence(&env, &fence, &d, &input, &nothing_recorded),
                        evaluate(&env, &fence, &d),
                        "{action:?}/{g:?}/{risk:?} must be unaffected by unverified product input"
                    );
                }
            }
        }
    }

    #[test]
    fn verified_product_evidence_only_ever_moves_a_grounding_gap() {
        // The bounded positive: even with a VERIFIED citation, the only verdict
        // product evidence can change is a gate-3 escalation. Every other cell
        // of the cross-product is byte-identical — so product input can never
        // rescue a fenced spec, a propose/never authority, or a risk ceiling.
        let env = AutopilotEnvelope::default();
        let fence = fence_of(["TASK-1"]);
        let input = pushy_product();
        let recorded = recorded_of(["PRIN-3"]);
        for action in ALL_ACTIONS {
            for g in ALL_GROUNDINGS {
                for risk in ALL_RISKS {
                    let d = decision("TASK-1", action, g, risk);
                    let base = evaluate(&env, &fence, &d);
                    let with = evaluate_with_product_evidence(&env, &fence, &d, &input, &recorded);
                    if base == Outcome::Escalate(EscalateReason::GroundingGap) {
                        continue;
                    }
                    assert_eq!(
                        with, base,
                        "{action:?}/{g:?}/{risk:?}: product evidence moved a non-gate-3 verdict"
                    );
                }
            }
        }
    }

    #[test]
    fn product_evidence_cannot_relax_gate_3_under_headless() {
        // Headless forces `grounding_required` on and demotes every `propose` to
        // `never`; product input composes with that rather than around it. An
        // unverified recommendation still escalates on the grounding gap, and a
        // `recommend:approve` still hits the demoted authority first.
        let env = effective_envelope(AutopilotEnvelope::default(), true, SoloPosture::Inactive);
        let fence = fence_of(["TASK-1"]);
        let input = pushy_product();

        let tag = decision("TASK-1", ActionClass::Tag, Grounding::TypeC, RiskLevel::Low);
        assert_eq!(
            evaluate_with_product_evidence(&env, &fence, &tag, &input, &HashSet::new()),
            Outcome::Escalate(EscalateReason::GroundingGap)
        );

        let approve = decision(
            "TASK-1",
            ActionClass::Approve,
            Grounding::TypeA,
            RiskLevel::Low,
        );
        assert_eq!(
            evaluate_with_product_evidence(
                &env,
                &fence,
                &approve,
                &input,
                &recorded_of(["PRIN-3"])
            ),
            Outcome::Escalate(EscalateReason::NeverAuthority)
        );
    }

    #[test]
    fn solo_park_for_human_outranks_a_verified_product_recommendation() {
        // The keystone posture is absolute: every action demotes to `never`, so
        // a perfectly-grounded product recommendation still parks for the human.
        let env = effective_envelope(
            AutopilotEnvelope::default(),
            false,
            SoloPosture::ParkForHuman,
        );
        let fence = fence_of(["TASK-1"]);
        let d = decision("TASK-1", ActionClass::Tag, Grounding::TypeA, RiskLevel::Low);
        assert_eq!(
            evaluate_with_product_evidence(
                &env,
                &fence,
                &d,
                &pushy_product(),
                &recorded_of(["PRIN-3"])
            ),
            Outcome::Escalate(EscalateReason::NeverAuthority)
        );
    }

    #[test]
    fn consumed_product_evidence_is_marked_for_the_from_product_filter() {
        // Provenance is first-class: the consumed decision carries the
        // `product:<who>` marker the audit filter reads, so "is a product seat
        // steering the queue?" stays answerable. Idempotent — a second pass
        // does not duplicate the marker.
        let d = decision("TASK-1", ActionClass::Tag, Grounding::TypeC, RiskLevel::Low);
        let once = decision_with_product_evidence(&d, &pushy_product(), &HashSet::new());
        assert!(once.evidence.contains(&"product:pat".to_string()));
        assert!(crate::autopilot_audit::evidence_has_product_handoff(
            &once.evidence
        ));
        assert_eq!(
            crate::autopilot_audit::product_provenance(&once.evidence).as_deref(),
            Some("pat")
        );

        let twice = decision_with_product_evidence(&once, &pushy_product(), &HashSet::new());
        assert_eq!(twice.evidence, once.evidence);
    }

    #[test]
    fn an_unnamed_product_seat_is_still_marked() {
        // A bare `from-product:` marker means the handoff happened without the
        // seat naming itself — it must still be visible to the audit rather
        // than silently laundered into an unattributed decision.
        let input = parse_product_input(&tags_of(["from-product:"])).expect("bare marker parses");
        assert_eq!(input.who, "");
        let d = decision("TASK-1", ActionClass::Tag, Grounding::TypeA, RiskLevel::Low);
        let marked = decision_with_product_evidence(&d, &input, &HashSet::new());
        assert!(crate::autopilot_audit::evidence_has_product_handoff(
            &marked.evidence
        ));
    }

    // ---- TASK-1022: a product seat under `--no-human` / solo -----------------
    //
    // The composition question: does a product seat change anything when nobody
    // is in the loop? The answer this block PROVES is no — the product-evidence
    // rule (TASK-1019) and the context tightening (TASK-1020) compose without a
    // seam, so an unattended run grants a product recommendation strictly LESS
    // than an attended one, never more. A product-sourced recommendation during
    // a `--no-human` drain cannot escalate its own privileges.
    //
    // The visibility half — recording that composition in the audit `mode` —
    // lives in `autopilot_audit.rs`.

    /// The envelope a headless (`--no-human`) drain actually runs under.
    fn headless_env() -> AutopilotEnvelope {
        effective_envelope(AutopilotEnvelope::default(), true, SoloPosture::Inactive)
    }

    #[test]
    fn under_headless_verified_product_evidence_still_only_moves_a_grounding_gap() {
        // The TASK-1019 bound, re-proven under the tightened envelope: even with
        // a citation the advisor verified, the ONLY verdict product evidence can
        // change is a gate-3 escalation. Every other cell of
        // (action × grounding × risk) is identical to the plain gates, so
        // product input can no more rescue a fenced spec, a demoted authority or
        // a risk ceiling headless than it can attended.
        let env = headless_env();
        let fence = fence_of(["TASK-1"]);
        let input = pushy_product();
        let recorded = recorded_of(["PRIN-3"]);
        for action in ALL_ACTIONS {
            for g in ALL_GROUNDINGS {
                for risk in ALL_RISKS {
                    let d = decision("TASK-1", action, g, risk);
                    let base = evaluate(&env, &fence, &d);
                    if base == Outcome::Escalate(EscalateReason::GroundingGap) {
                        continue;
                    }
                    assert_eq!(
                        evaluate_with_product_evidence(&env, &fence, &d, &input, &recorded),
                        base,
                        "{action:?}/{g:?}/{risk:?}: product evidence moved a non-gate-3 verdict \
                         under headless"
                    );
                }
            }
        }
    }

    #[test]
    fn headless_never_unlocks_an_execution_product_evidence_could_not_get_attended() {
        // The no-new-authority invariant stated as a SUBSET property: anything a
        // product-sourced decision may auto-execute with nobody in the loop, it
        // could already have auto-executed with a human present. Removing the
        // human is never what buys the execution.
        let attended = AutopilotEnvelope::default();
        let unattended = headless_env();
        let fence = fence_of(["TASK-1"]);
        let input = pushy_product();
        for recorded in [recorded_of(["PRIN-3"]), HashSet::new()] {
            for action in ALL_ACTIONS {
                for g in ALL_GROUNDINGS {
                    for risk in ALL_RISKS {
                        let d = decision("TASK-1", action, g, risk);
                        if evaluate_with_product_evidence(
                            &unattended,
                            &fence,
                            &d,
                            &input,
                            &recorded,
                        ) == Outcome::Execute
                        {
                            assert_eq!(
                                evaluate_with_product_evidence(
                                    &attended, &fence, &d, &input, &recorded
                                ),
                                Outcome::Execute,
                                "{action:?}/{g:?}/{risk:?}: headless unlocked an execution the \
                                 attended envelope refused"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_product_seat_cannot_restore_a_gate_3_relaxation_headless_took_away() {
        // A project that turned gate 3 off in config gets it forced back on the
        // moment the run goes unattended — and the product seat, whose ONLY
        // reach is gate 3, cannot undo that. An unverified citation leaves the
        // gap open; the seat's own claim of grounding is not self-certifying.
        let relaxed = AutopilotEnvelope {
            grounding_required: false,
            ..AutopilotEnvelope::default()
        };
        let fence = fence_of(["TASK-1"]);
        let d = decision("TASK-1", ActionClass::Tag, Grounding::TypeC, RiskLevel::Low);
        let input = pushy_product();

        // Attended, with gate 3 relaxed: the ungrounded tag executes.
        let attended = effective_envelope(relaxed.clone(), false, SoloPosture::Inactive);
        assert_eq!(evaluate(&attended, &fence, &d), Outcome::Execute);

        // Unattended: gate 3 is forced back on, and product input cannot reopen
        // it without substrate the advisor independently verified.
        let unattended = effective_envelope(relaxed, true, SoloPosture::Inactive);
        assert_eq!(
            evaluate_with_product_evidence(&unattended, &fence, &d, &input, &HashSet::new()),
            Outcome::Escalate(EscalateReason::GroundingGap)
        );
    }

    #[test]
    fn product_evidence_never_executes_under_the_solo_keystone_posture() {
        // Solo's keystone partition is absolute and product input does not dent
        // it: across the whole cross-product, with a verified citation and
        // headless on top, nothing auto-executes. The human still decides.
        let fence = fence_of(["TASK-1"]);
        let input = pushy_product();
        let recorded = recorded_of(["PRIN-3"]);
        for headless in [false, true] {
            let env = effective_envelope(
                AutopilotEnvelope::default(),
                headless,
                SoloPosture::ParkForHuman,
            );
            for action in ALL_ACTIONS {
                for g in ALL_GROUNDINGS {
                    for risk in ALL_RISKS {
                        let d = decision("TASK-1", action, g, risk);
                        assert_ne!(
                            evaluate_with_product_evidence(&env, &fence, &d, &input, &recorded),
                            Outcome::Execute,
                            "{action:?}/{g:?}/{risk:?}: product evidence executed under the solo \
                             keystone posture (headless={headless})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn an_unattended_product_decision_is_recorded_as_such() {
        // The seam between the two halves: a decision that consumed product
        // input carries the marker the audit reads, so once the mint path adds
        // the headless layer the composition is nameable —
        // `headless+product+autopilot`. Asserted here so a change to the marker
        // spelling breaks the producer's test, not just the reader's.
        let d = decision("TASK-1", ActionClass::Tag, Grounding::TypeA, RiskLevel::Low);
        let consumed = decision_with_product_evidence(&d, &pushy_product(), &HashSet::new());
        assert_eq!(
            crate::autopilot_audit::composition_mode(
                "groom",
                false,
                true,
                crate::autopilot_audit::evidence_has_product_handoff(&consumed.evidence),
            ),
            "headless+product+autopilot"
        );
    }
}
