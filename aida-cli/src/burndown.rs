//! STORY-527 slice 1: the pure pickability gate for `aida burndown plan`.
//!
//! `/aida-burndown` fans out worktree-isolated implementer subagents over a
//! ready set, with the main session integrating (see
//! `docs/aida/discipline/autonomous-burndown.md`). The non-negotiable safety
//! property is that only **bounded, unblocked, decision-free** specs are fanned
//! out — anything needing a human decision is parked, never dragged in. That is
//! what makes "never stop to ask" safe.
//!
//! This module is the side-effect-free heart of that gate: given a candidate
//! spec's already-probed facts, decide READY vs PARKED(reason). The selector
//! resolution and the graph/blocker probing live in `main.rs`; keeping the
//! verdict pure makes it exhaustively unit-testable. trace:STORY-527 | ai:claude

/// The gate's verdict for one candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Pickability {
    /// Bounded + unblocked + decision-free — safe to fan out autonomously.
    Ready,
    /// Held back, with a human-readable reason.
    Parked(String),
}

/// Already-probed facts about one candidate spec. Built in `main.rs` from the
/// store + graph; consumed by the pure [`classify`]. trace:STORY-527
#[derive(Debug, Clone)]
pub(crate) struct BurndownCandidate {
    /// Display SPEC-ID (e.g. `TASK-702`).
    pub id: String,
    /// Lowercased requirement type (`epic`, `task`, `story`, …).
    pub req_type: String,
    /// The spec's tags (used for the parking-tag check).
    pub tags: Vec<String>,
    /// True when any `BlockedBy` edge points at a not-yet-Completed spec.
    pub has_unsatisfied_blocker: bool,
    /// True when the spec carries a pending `DecisionRequest` (an open
    /// human-decision question via `aida questions`).
    pub has_pending_decision: bool,
}

/// A tag that marks a spec as not-autonomously-pickable — a human decision, a
/// deferral, or a draft-review gate. Matched case-insensitively; `deferred:` is
/// a prefix. Returns the matched tag (for the parked reason). trace:STORY-527
fn parking_tag(tags: &[String]) -> Option<String> {
    for t in tags {
        let lo = t.trim().to_ascii_lowercase();
        let parks = lo == "blocked"
            || lo == "needs-human-input"
            || lo == "needs-human"
            // A spec awaiting a human design/architecture decision or an
            // explicit operator action is NOT autonomously pickable, even
            // though it's bounded + unblocked. (Found dogfooding /aida-burndown:
            // STORY-493 needs-design-signoff + STORY-497 operator-action slipped
            // the gate.) trace:STORY-527 | ai:claude
            || lo == "needs-design-signoff"
            || lo == "needs-design"
            || lo == "operator-action"
            // TASK-744: the two halves of the split `needs-human` umbrella — a
            // decision to answer, or clear-to-build keystone work for the
            // at-keyboard `--zen` lane. Both still park from the unsupervised
            // drain; they differ only in the resolution a human applies.
            || lo == "needs-decision"
            || lo == "needs-supervised-build"
            || lo == "review:draft-only"
            || lo.starts_with("deferred:");
        if parks {
            return Some(t.trim().to_string());
        }
    }
    None
}

/// The pickability gate. READY iff the spec is bounded (not an epic),
/// decision-free, unblocked, and not parking-tagged. Exclusions are ordered
/// cheapest/broadest first so the parked reason names the most fundamental
/// blocker. trace:STORY-527
pub(crate) fn classify(c: &BurndownCandidate) -> Pickability {
    if c.req_type.eq_ignore_ascii_case("epic") {
        return Pickability::Parked("epic — decompose into bounded specs first".to_string());
    }
    if c.has_pending_decision {
        return Pickability::Parked(
            "pending decision request — answer via `aida questions`".to_string(),
        );
    }
    if c.has_unsatisfied_blocker {
        return Pickability::Parked("blocked by an unsatisfied dependency (BlockedBy)".to_string());
    }
    if let Some(tag) = parking_tag(&c.tags) {
        return Pickability::Parked(format!("tagged `{tag}`"));
    }
    Pickability::Ready
}

/// Partition candidates into `(ready_ids, parked)` preserving input order —
/// the fan-out set and the skipped set with reasons. trace:STORY-527
pub(crate) fn partition(candidates: &[BurndownCandidate]) -> (Vec<String>, Vec<(String, String)>) {
    let mut ready = Vec::new();
    let mut parked = Vec::new();
    for c in candidates {
        match classify(c) {
            Pickability::Ready => ready.push(c.id.clone()),
            Pickability::Parked(reason) => parked.push((c.id.clone(), reason)),
        }
    }
    (ready, parked)
}

/// STORY-546: split the pickable set into `(ready, awaiting_signoff)` by advisor
/// sign-off. Queue membership IS the sign-off (`queue add` is advisor-authority-
/// gated, ADR-3 / TASK-647), so `ready` = blessed + drainable and
/// `awaiting_signoff` = pickable but not yet queued. Order-preserving + pure so
/// the queue-gate is unit-testable independent of the filesystem queue read.
/// trace:STORY-546 | ai:claude
pub(crate) fn split_by_signoff(
    pickable: Vec<String>,
    queued: &std::collections::HashSet<String>,
) -> (Vec<String>, Vec<String>) {
    pickable.into_iter().partition(|id| queued.contains(id))
}

/// STORY-547: the broader "why is this open spec *still open*?" classifier.
/// Where [`classify`] answers the narrow pickability question for the candidate
/// set (the approved+queued specs a burndown would fan out), `explain_open`
/// answers it for **every** open spec, deriving the reason purely from store
/// signals (type, status, tags, BlockedBy edges, pending decisions, live
/// leases). No new stored field, no hand-written status, no findings — the
/// reason a spec stays open is already latent in the substrate; this just reads
/// it back. trace:STORY-547 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenBucket {
    /// Built; a draft PR is held for human review (`review:draft-only`).
    HeldForReview,
    /// A live session lease is working it right now.
    InFlight,
    /// Parked on a human decision (pending DecisionRequest, design-signoff,
    /// operator-action, needs-human, or NeedsAttention triage).
    AwaitingDecision,
    /// Clear to build, but keystone / blast-radius work that ships at the
    /// keyboard (`aida queue work <id> --zen`), not the unsupervised drain
    /// (tagged `needs-supervised-build`). trace:TASK-744 | ai:claude
    BuildSupervised,
    /// Deliberately postponed (`deferred:<why>`).
    Deferred,
    /// Blocked by an unsatisfied dependency.
    Blocked,
    /// An umbrella epic — driven by its children, not directly pickable.
    Umbrella,
    /// A vision/principle — no terminal state by design.
    LongLived,
    /// Draft, not yet advisor-approved.
    Ungroomed,
    /// Done on a branch — awaiting merge to the default branch.
    AwaitingMerge,
    /// Work in progress.
    InProgress,
    /// Approved & unblocked — ready to pick up (the burndown ready set).
    Actionable,
}

impl OpenBucket {
    /// Stable kebab-case key for JSON / grouping.
    pub(crate) fn key(self) -> &'static str {
        match self {
            OpenBucket::HeldForReview => "held-for-review",
            OpenBucket::InFlight => "in-flight",
            OpenBucket::AwaitingDecision => "awaiting-decision",
            OpenBucket::BuildSupervised => "build-supervised",
            OpenBucket::Deferred => "deferred",
            OpenBucket::Blocked => "blocked",
            OpenBucket::Umbrella => "umbrella",
            OpenBucket::LongLived => "long-lived",
            OpenBucket::Ungroomed => "ungroomed",
            OpenBucket::AwaitingMerge => "awaiting-merge",
            OpenBucket::InProgress => "in-progress",
            OpenBucket::Actionable => "actionable",
        }
    }

    /// True for the buckets that genuinely need a human nudge (vs. those that
    /// will resolve themselves through normal flow). Drives the explainer's
    /// "needs you" grouping. trace:STORY-547
    pub(crate) fn needs_human(self) -> bool {
        matches!(
            self,
            OpenBucket::HeldForReview
                | OpenBucket::AwaitingDecision
                | OpenBucket::BuildSupervised
                | OpenBucket::Ungroomed
                | OpenBucket::Umbrella
        )
    }
}

/// Already-probed facts about one OPEN spec. `status` is normalized to
/// alphanumeric-lowercase (`inprogress`, `needsattention`, …) by the caller so
/// this stays pure + exhaustively testable. trace:STORY-547
#[derive(Debug, Clone)]
pub(crate) struct OpenFacts {
    /// Display SPEC-ID.
    pub id: String,
    /// Lowercased requirement type (`epic`, `vision`, `task`, …).
    pub req_type: String,
    /// Normalized status key (`draft`, `approved`, `inprogress`, `done`, …).
    pub status: String,
    /// The spec's tags.
    pub tags: Vec<String>,
    /// A `BlockedBy` edge points at a not-yet-Completed spec.
    pub has_unsatisfied_blocker: bool,
    /// Carries a pending `DecisionRequest`.
    pub has_pending_decision: bool,
    /// A live session lease's scope matches this spec.
    pub in_flight: bool,
    /// TASK-723 (source #2): display-ids of FINDINGS filed against this spec
    /// (attempt outcomes — CI red / RequestChanges / build fail). Linked, not
    /// recomputed: the finding already exists; the view just folds it in.
    pub findings: Vec<String>,
    /// TASK-723 (source #3): non-derivable residual reasons a human recorded as
    /// `why-open:<reason>` prefixed comments. Folded in verbatim; derivable
    /// state is NEVER written here (staleness trap).
    pub residual_notes: Vec<String>,
}

/// TASK-723: prefix marking a comment as a non-derivable residual openness
/// reason — the chosen vehicle (resolving STORY-548's open fork). A comment
/// whose content (trimmed, case-insensitive) starts with `why-open:` records a
/// reason the graph can't derive ("waiting on upstream X", "deferred pending
/// budget"). trace:TASK-723 | ai:claude
pub(crate) const WHY_OPEN_PREFIX: &str = "why-open:";

/// TASK-723: extract the residual reason from a `why-open:<reason>` comment
/// body, or `None` if the comment isn't a residual note. Case-insensitive on
/// the prefix; returns the trimmed reason text. trace:TASK-723 | ai:claude
pub(crate) fn parse_why_open_comment(content: &str) -> Option<String> {
    let trimmed = content.trim_start();
    if trimmed.len() < WHY_OPEN_PREFIX.len() {
        return None;
    }
    let (head, rest) = trimmed.split_at(WHY_OPEN_PREFIX.len());
    if head.eq_ignore_ascii_case(WHY_OPEN_PREFIX) {
        let reason = rest.trim();
        if reason.is_empty() {
            None
        } else {
            Some(reason.to_string())
        }
    } else {
        None
    }
}

/// TASK-723: where a single openness reason came from — drives grouping,
/// ordering (most-fundamental-first), and the JSON `source` field. trace:TASK-723
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReasonSource {
    /// Derived purely from the graph (status / tags / BlockedBy / lease / type).
    Derived,
    /// An attempt outcome — links an existing FINDING (source #2).
    Finding,
    /// A human-recorded non-derivable residual note (`why-open:`, source #3).
    Residual,
}

impl ReasonSource {
    /// Stable key for JSON.
    pub(crate) fn key(self) -> &'static str {
        match self {
            ReasonSource::Derived => "derived",
            ReasonSource::Finding => "finding",
            ReasonSource::Residual => "residual",
        }
    }
}

/// TASK-723: one openness reason — its text plus where it came from. The
/// derived reason carries the [`OpenBucket`]; finding/residual reasons don't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reason {
    pub source: ReasonSource,
    pub text: String,
}

/// Classify one open spec into its `(bucket, human-readable reason)`. Precedence
/// runs live/human signals first (most actionable + most current), then
/// structural facts (epic / vision), then a status fallback — so the reason
/// names the *most specific* thing keeping the spec open. trace:STORY-547
pub(crate) fn explain_open(f: &OpenFacts) -> (OpenBucket, String) {
    let has_tag =
        |name: &str| -> bool { f.tags.iter().any(|t| t.trim().eq_ignore_ascii_case(name)) };

    if f.in_flight {
        return (
            OpenBucket::InFlight,
            "in flight — a live session lease is working this now".to_string(),
        );
    }
    if has_tag("review:draft-only") {
        return (
            OpenBucket::HeldForReview,
            "built — held as a draft PR for human review (`review:draft-only`)".to_string(),
        );
    }
    if f.has_pending_decision {
        return (
            OpenBucket::AwaitingDecision,
            "awaiting a human decision — answer via `aida questions`".to_string(),
        );
    }
    // TASK-744: build-supervised — clear to build, but keystone / blast-radius
    // work for the at-keyboard `--zen` lane, not the unsupervised drain.
    if f.tags
        .iter()
        .any(|t| t.trim().eq_ignore_ascii_case("needs-supervised-build"))
    {
        return (
            OpenBucket::BuildSupervised,
            "build-supervised — clear to build, but at the keyboard \
             (`aida queue work <id> --zen`), not the unsupervised drain"
                .to_string(),
        );
    }
    for t in &f.tags {
        let lo = t.trim().to_ascii_lowercase();
        if lo == "needs-design-signoff"
            || lo == "needs-design"
            || lo == "operator-action"
            || lo == "needs-decision"
            || lo == "needs-human"
            || lo == "needs-human-input"
        {
            return (
                OpenBucket::AwaitingDecision,
                format!("awaiting a human decision (tagged `{}`)", t.trim()),
            );
        }
    }
    for t in &f.tags {
        if t.trim().to_ascii_lowercase().starts_with("deferred:") {
            return (
                OpenBucket::Deferred,
                format!("deliberately deferred (tagged `{}`)", t.trim()),
            );
        }
    }
    if f.has_unsatisfied_blocker || has_tag("blocked") {
        return (
            OpenBucket::Blocked,
            "blocked by an unsatisfied dependency (BlockedBy → a not-yet-Completed spec)"
                .to_string(),
        );
    }
    if f.req_type.eq_ignore_ascii_case("epic") {
        return (
            OpenBucket::Umbrella,
            "umbrella epic — driven by its children; decompose or complete them".to_string(),
        );
    }
    if f.req_type.eq_ignore_ascii_case("vision") || f.req_type.eq_ignore_ascii_case("principle") {
        return (
            OpenBucket::LongLived,
            format!("long-lived {} — no terminal state by design", f.req_type),
        );
    }
    match f.status.as_str() {
        "needsattention" => (
            OpenBucket::AwaitingDecision,
            "parked for triage (NeedsAttention) — see `aida findings list`".to_string(),
        ),
        "draft" => (
            OpenBucket::Ungroomed,
            "draft — awaiting advisor grooming/approval before it can be picked up".to_string(),
        ),
        "done" => (
            OpenBucket::AwaitingMerge,
            "done on a branch — awaiting merge to the default branch (auto-completes on merge)"
                .to_string(),
        ),
        "inprogress" => (OpenBucket::InProgress, "work in progress".to_string()),
        _ => (
            OpenBucket::Actionable,
            "ready to pick up — approved & unblocked (appears in the `burndown plan` ready set)"
                .to_string(),
        ),
    }
}

/// The canonical "a human is required" classification predicate (SPIKE-57).
///
/// This is the single, intention-revealing name for the bottleneck signal the
/// codebase previously expressed five different ways (the `human_only` marker,
/// the `needs-human` / `needs-design-signoff` / `operator-action` tags, the
/// `review:draft-only` gate, `--escalate-blocks` parking, and the burndown
/// "needs a human nudge" bucket). It re-derives NOTHING: four of those signals
/// already converge on [`OpenBucket::needs_human`] via [`explain_open`], so the
/// predicate delegates to it; the fifth, the permanent `human_only` marker, is
/// orthogonal to status (a pickability flag, not an open-bucket) and folds in as
/// an explicit OR clause.
///
/// `human_only` is passed separately because it lives on the [`crate`]-level
/// `Requirement`, not on the pure [`OpenFacts`] this module classifies — keeping
/// `OpenFacts` free of the marker preserves its exhaustive testability.
/// trace:TASK-746 | ai:claude
pub(crate) fn human_required(f: &OpenFacts, human_only: bool) -> bool {
    let (bucket, _) = explain_open(f);
    bucket.needs_human() || human_only
}

/// TASK-723: the FULL reason set for one open spec, most-fundamental-first.
/// Unions the three sources from STORY-548's design:
///   1. DERIVED (the [`explain_open`] graph reason) — always first; it's the
///      most fundamental thing keeping the spec open.
///   2. FINDING links (attempt outcomes already filed by shelved drains) —
///      reused, never recomputed.
///   3. RESIDUAL notes (human `why-open:` comments) — the non-derivable tail.
/// Returns the primary [`OpenBucket`] (from the derived reason — it still drives
/// grouping + the needs-human signal) alongside the ordered reasons.
/// trace:TASK-723 | ai:claude
pub(crate) fn explain_reasons(f: &OpenFacts) -> (OpenBucket, Vec<Reason>) {
    let (bucket, derived) = explain_open(f);
    let mut reasons = Vec::with_capacity(1 + f.findings.len() + f.residual_notes.len());
    reasons.push(Reason {
        source: ReasonSource::Derived,
        text: derived,
    });
    for finding in &f.findings {
        reasons.push(Reason {
            source: ReasonSource::Finding,
            text: format!(
                "attempt outcome filed as finding {finding} — triage via `aida findings list`"
            ),
        });
    }
    for note in &f.residual_notes {
        reasons.push(Reason {
            source: ReasonSource::Residual,
            text: note.clone(),
        });
    }
    (bucket, reasons)
}

/// STORY-563: the classification of an open spec by WHAT keeps it out of the
/// burndown ready set — the lens `aida human unblock` groups by. Where
/// [`explain_open`] answers "why is this still open?" for every open spec,
/// `classify_unblock` answers the narrower operator question "what do I, the
/// human, have to DO to move this into the burndown?". The buckets map onto
/// three actions: queue it, clarify it first, or leave it parked. trace:STORY-563
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnblockClass {
    /// Draft — needs advisor approval before it can be queued.
    NeedsApproval,
    /// Approved + bounded + unblocked + decision-free, but not yet queued —
    /// the advisor can queue it straight into the burndown.
    ApprovedUnqueued,
    /// Implementable but missing acceptance criteria — clarify FIRST.
    UnderSpecified,
    /// Built/in-progress work that wants the at-keyboard `--zen` lane, not an
    /// unattended drain — leave parked.
    BuildSupervised,
    /// Parked on a human decision (pending DecisionRequest / design-signoff /
    /// operator-action / NeedsAttention triage) — leave parked.
    DecisionPending,
    /// Deliberately deferred (`deferred:<why>`) — leave parked.
    Deferred,
    /// Blocked by an unsatisfied dependency — leave parked until it clears.
    BlockedBy,
}

impl UnblockClass {
    /// Stable kebab-case key for JSON / grouping.
    pub(crate) fn key(self) -> &'static str {
        match self {
            UnblockClass::NeedsApproval => "needs-approval",
            UnblockClass::ApprovedUnqueued => "approved-unqueued",
            UnblockClass::UnderSpecified => "under-specified",
            UnblockClass::BuildSupervised => "build-supervised",
            UnblockClass::DecisionPending => "decision-pending",
            UnblockClass::Deferred => "deferred",
            UnblockClass::BlockedBy => "blocked-by",
        }
    }

    /// The grooming action the paste-ready prompt asks the advisor to take.
    pub(crate) fn action(self) -> UnblockAction {
        match self {
            // Both of these can move into the burndown now.
            UnblockClass::NeedsApproval | UnblockClass::ApprovedUnqueued => UnblockAction::Queue,
            // Author acceptance criteria first, THEN it becomes queueable.
            UnblockClass::UnderSpecified => UnblockAction::Clarify,
            // Everything else stays parked — a human keyboard, a decision, a
            // dependency, or a deliberate deferral has to happen first.
            UnblockClass::BuildSupervised
            | UnblockClass::DecisionPending
            | UnblockClass::Deferred
            | UnblockClass::BlockedBy => UnblockAction::Leave,
        }
    }
}

/// STORY-563: the three grooming actions the prompt routes each spec to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnblockAction {
    /// Queue it straight into the burndown.
    Queue,
    /// Clarify (author acceptance criteria) first.
    Clarify,
    /// Leave parked.
    Leave,
}

/// STORY-563: already-probed facts about one open spec, for the unblock lens.
/// Pure inputs so [`classify_unblock`] is exhaustively unit-testable. The caller
/// (`main.rs`) probes the store/queue/graph to build these. trace:STORY-563
#[derive(Debug, Clone)]
pub(crate) struct UnblockFacts {
    /// Display SPEC-ID.
    pub id: String,
    /// Lowercased requirement type (`task`, `story`, `epic`, …).
    pub req_type: String,
    /// Normalized status key (`draft`, `approved`, `inprogress`, `done`, …).
    pub status: String,
    /// The spec's tags.
    pub tags: Vec<String>,
    /// A `BlockedBy` edge points at a not-yet-Completed spec.
    pub has_unsatisfied_blocker: bool,
    /// Carries a pending `DecisionRequest`.
    pub has_pending_decision: bool,
    /// A live session lease's scope matches this spec.
    pub in_flight: bool,
    /// The spec is already in some user's queue (advisor sign-off, ADR-3).
    pub queued: bool,
    /// The spec text carries acceptance criteria (only meaningful for
    /// implementable, not-yet-built types).
    pub has_acceptance: bool,
    /// Implementable type (not vision/folder/meta/principle/term) for which a
    /// missing-acceptance gap is real signal, not noise (BUG-495).
    pub implementable: bool,
}

impl UnblockFacts {
    /// True for the items already inside the burndown ready set — queued AND
    /// actionable (approved, bounded, unblocked, decision-free, has acceptance).
    /// These are EXCLUDED from `aida human unblock`: the human has nothing left
    /// to do for them. trace:STORY-563
    pub(crate) fn in_burndown_ready_set(&self) -> bool {
        self.queued
            && self.status == "approved"
            && !self.has_unsatisfied_blocker
            && !self.has_pending_decision
            && !self.in_flight
            && !self.req_type.eq_ignore_ascii_case("epic")
            && self.has_acceptance
            && parking_tag(&self.tags).is_none()
    }
}

/// STORY-563: classify one open spec by what keeps it out of the burndown
/// ready set, or `None` if it's already in the ready set (nothing for the human
/// to do). Precedence runs the "leave parked" hard blockers first (decision /
/// blocker / deferral / in-flight) so the most fundamental reason wins, then the
/// human-actionable buckets (clarify, approve, queue). Pure + testable.
/// trace:STORY-563 | ai:claude
pub(crate) fn classify_unblock(f: &UnblockFacts) -> Option<UnblockClass> {
    if f.in_burndown_ready_set() {
        return None;
    }
    let has_tag =
        |name: &str| -> bool { f.tags.iter().any(|t| t.trim().eq_ignore_ascii_case(name)) };

    // --- LEAVE-PARKED blockers, most-fundamental first. ---
    // A pending human decision (request, design-signoff, operator-action,
    // needs-human, or NeedsAttention triage) parks the spec.
    if f.has_pending_decision || f.status == "needsattention" {
        return Some(UnblockClass::DecisionPending);
    }
    // TASK-744: an explicit `needs-supervised-build` tag is keyboard-build
    // (`--zen`), not a decision — route it before the `needs-human` umbrella.
    if has_tag("needs-supervised-build") {
        return Some(UnblockClass::BuildSupervised);
    }
    for t in &f.tags {
        let lo = t.trim().to_ascii_lowercase();
        if lo == "needs-design-signoff"
            || lo == "needs-design"
            || lo == "operator-action"
            || lo == "needs-decision"
            || lo == "needs-human"
            || lo == "needs-human-input"
        {
            return Some(UnblockClass::DecisionPending);
        }
    }
    // Deliberately deferred.
    if f.tags
        .iter()
        .any(|t| t.trim().to_ascii_lowercase().starts_with("deferred:"))
    {
        return Some(UnblockClass::Deferred);
    }
    // Blocked by an unsatisfied dependency.
    if f.has_unsatisfied_blocker || has_tag("blocked") {
        return Some(UnblockClass::BlockedBy);
    }
    // In-flight or built-and-in-progress work belongs to the at-keyboard
    // `--zen` lane, not an unattended drain — leave it for the human.
    if f.in_flight || f.status == "inprogress" || f.status == "done" {
        return Some(UnblockClass::BuildSupervised);
    }
    // Epics are umbrellas — they get decomposed, not queued; treat that as a
    // decision the human owns.
    if f.req_type.eq_ignore_ascii_case("epic") {
        return Some(UnblockClass::DecisionPending);
    }

    // --- HUMAN-ACTIONABLE buckets. ---
    // Implementable but under-specified — clarify BEFORE it can be queued.
    if f.implementable && !f.has_acceptance {
        return Some(UnblockClass::UnderSpecified);
    }
    // Draft — needs advisor approval before queuing.
    if f.status == "draft" {
        return Some(UnblockClass::NeedsApproval);
    }
    // Approved, bounded, unblocked, decision-free, has acceptance — but not yet
    // queued. The advisor can queue it straight into the burndown.
    Some(UnblockClass::ApprovedUnqueued)
}

/// STORY-563: one classified line for the prompt — the spec id, its class, and
/// a one-line reason. Built by the caller from [`classify_unblock`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnblockLine {
    pub id: String,
    pub class: UnblockClass,
    pub reason: String,
}

/// STORY-563: a one-line human-readable reason for each unblock class — the
/// `WHAT keeps it out` text the prompt and the human view both print. Pure.
/// trace:STORY-563 | ai:claude
pub(crate) fn unblock_reason(class: UnblockClass) -> &'static str {
    match class {
        UnblockClass::NeedsApproval => "draft — needs advisor approval before it can be queued",
        UnblockClass::ApprovedUnqueued => {
            "approved, bounded & unblocked — queueable straight into the burndown"
        }
        UnblockClass::UnderSpecified => "missing acceptance criteria — clarify before queuing",
        UnblockClass::BuildSupervised => {
            "in progress / in flight — wants the at-keyboard `--zen` lane, not an unattended drain"
        }
        UnblockClass::DecisionPending => {
            "awaiting a human decision — answer via `aida questions` / decompose / triage"
        }
        UnblockClass::Deferred => "deliberately deferred",
        UnblockClass::BlockedBy => "blocked by an unsatisfied dependency (BlockedBy)",
    }
}

/// STORY-563: assemble the PASTE-READY advisor prompt from the classified set.
/// DETERMINISTIC + side-effect-free — this is the SPIKE-55 prompt-assembler
/// pattern (like `aida ultraplan` / `aida goal`): no LLM in the CLI, just turn
/// store state into an instruction the advisor (the grooming skill / live
/// session) executes. The prompt tells the advisor to QUEUE the autonomous-able,
/// CLARIFY the under-specified first, and LEAVE parked the rest, with one line +
/// spec-id + reason each. trace:STORY-563 | ai:claude
pub(crate) fn assemble_unblock_prompt(lines: &[UnblockLine]) -> String {
    let by_action = |want: UnblockAction| -> Vec<&UnblockLine> {
        lines.iter().filter(|l| l.class.action() == want).collect()
    };
    let queue = by_action(UnblockAction::Queue);
    let clarify = by_action(UnblockAction::Clarify);
    let leave = by_action(UnblockAction::Leave);

    let mut out = String::new();
    out.push_str(
        "Groom these open specs into the burndown. For each, take exactly the action below — \
         queue the autonomous-able, clarify the under-specified FIRST (author acceptance criteria \
         before queuing), and LEAVE the rest parked (they need a human keyboard, a decision, a \
         dependency, or a deliberate deferral first).\n",
    );

    let section = |out: &mut String, title: &str, verb: &str, rows: &[&UnblockLine]| {
        out.push('\n');
        if rows.is_empty() {
            out.push_str(&format!("{title}: (none)\n"));
            return;
        }
        out.push_str(&format!("{title} ({verb}):\n"));
        for l in rows {
            out.push_str(&format!("  - {} — {}\n", l.id, l.reason));
        }
    };

    section(
        &mut out,
        "QUEUE",
        "aida queue add <id> — moves it into the burndown ready set",
        &queue,
    );
    section(
        &mut out,
        "CLARIFY FIRST",
        "add a ## Acceptance section (or run /aida-clarify), THEN queue",
        &clarify,
    );
    section(
        &mut out,
        "LEAVE PARKED",
        "do not queue — resolve the blocker / decision / deferral first",
        &leave,
    );
    out
}

/// BUG-493: the observed forge state of a `review:draft-only` spec's PR, as
/// probed by the caller (cheap for a single-spec `aida why`, too expensive for
/// the bulk `burndown explain`). Lets [`reconcile_held_for_review`] tell the
/// real story instead of asserting a draft PR exists purely from the tag.
// trace:BUG-493 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DraftPrObservation {
    /// An open PR was found referencing the spec — the tag's claim holds.
    Open(u64),
    /// The forge ran cleanly and reported NO open PR — the draft PR is closed
    /// or was never opened. The tag over-claims; report the true state.
    NoOpenPr,
    /// The forge could not be reached (gh missing/failed/unreachable). Cannot
    /// confirm or deny the PR's existence — soften the claim, don't assert it.
    Unverifiable,
}

/// BUG-493: reconcile the `HeldForReview` reason against the spec's real forge
/// state. `explain_open` derives "held as a draft PR for human review" purely
/// from the `review:draft-only` tag; that over-claims when the draft PR was
/// closed/never-opened (origin: `aida why TASK-715` asserted draft PR #709 was
/// held for review after #709 was closed in the Session-63 reset, while
/// `aida show TASK-715` reported "no PR opened yet"). Given the observed PR
/// state, return the honest reason. Pure + testable.
// trace:BUG-493 | ai:claude
// TASK-741: the held-for-review hold IS the `review:draft-only ⇒ an open draft
// PR exists` cross-axis invariant, declared as the `BUG-493` row of
// `aida_core::lifecycle::INVARIANTS`. The forge-specific three-way probe
// (`DraftPrObservation`) lives here, but whether an observed state SATISFIES the
// invariant is decided by the model — the `debug_assert`s below pin each
// determinable arm to `lifecycle::held_for_review_claim_holds`, so the reason
// wording can never drift away from the rule it claims to enforce.
// trace:TASK-741 | ai:claude
pub(crate) fn reconcile_held_for_review(obs: &DraftPrObservation) -> String {
    use aida_core::lifecycle::held_for_review_claim_holds;
    match obs {
        DraftPrObservation::Open(num) => {
            debug_assert!(
                held_for_review_claim_holds(true),
                "an open draft PR must satisfy the held-for-review invariant"
            );
            format!("built — held as draft PR #{num} for human review (`review:draft-only`)")
        }
        DraftPrObservation::NoOpenPr => {
            debug_assert!(
                !held_for_review_claim_holds(false),
                "no open PR must violate the held-for-review invariant"
            );
            "built & tagged for draft review (`review:draft-only`), \
             but no open PR exists — its draft PR was closed or never opened. \
             Reopen/re-push the PR to review, or drop the tag if it's superseded \
             (`aida show <ID>` shows the branch / PR state)."
                .to_string()
        }
        DraftPrObservation::Unverifiable => {
            "tagged for draft review (`review:draft-only`) — could not reach the forge to \
             confirm the draft PR is open; verify with `aida show <ID>`."
                .to_string()
        }
    }
}

/// Plain-language description of the active selector for the human-facing
/// header — glosses the bare word "selector" so a new user understands what is
/// being shown and how to narrow it. Pure (no color), so it's unit-testable;
/// the caller colorizes. trace:STORY-544 | ai:claude
pub(crate) fn selector_summary(status: &str, tag: Option<&str>, batch: Option<&str>) -> String {
    let mut filters: Vec<String> = Vec::new();
    if let Some(t) = tag {
        filters.push(format!("tag {t}"));
    }
    if let Some(b) = batch {
        filters.push(format!("batch {b}"));
    }
    let scope = if filters.is_empty() {
        format!("Showing {status} specs (default).")
    } else {
        format!(
            "Showing {status} specs filtered to {}.",
            filters.join(" + ")
        )
    };
    format!("{scope} Narrow with --batch NAME, --tag X, or --status <s>.")
}

/// The next-step footer printed after a non-empty ready set. Points the user at
/// `aida burndown run` (the kick-off-and-walk-away headless drain) as the primary
/// command, and notes `/aida-burndown` as the in-Claude alternative.
/// Pure text (no color); the caller colorizes. trace:STORY-544 | ai:claude
// trace:BUG-494 | ai:claude
pub(crate) fn next_step_footer() -> String {
    "Next step: run `aida burndown run` to drain the ready set above (kick off and walk away).\n\
     Or invoke /aida-burndown in Claude Code to fan it out from your session."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(
        id: &str,
        req_type: &str,
        tags: &[&str],
        blocked: bool,
        decision: bool,
    ) -> BurndownCandidate {
        BurndownCandidate {
            id: id.to_string(),
            req_type: req_type.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            has_unsatisfied_blocker: blocked,
            has_pending_decision: decision,
        }
    }

    #[test]
    fn bounded_unblocked_decision_free_spec_is_ready() {
        assert_eq!(
            classify(&cand("TASK-1", "task", &["papercut"], false, false)),
            Pickability::Ready
        );
    }

    #[test]
    fn epic_is_parked_for_decomposition() {
        match classify(&cand("EPIC-1", "epic", &[], false, false)) {
            Pickability::Parked(r) => assert!(r.contains("decompose")),
            other => panic!("expected Parked, got {other:?}"),
        }
    }

    #[test]
    fn pending_decision_parks() {
        assert!(matches!(
            classify(&cand("STORY-1", "story", &[], false, true)),
            Pickability::Parked(_)
        ));
    }

    #[test]
    fn unsatisfied_blocker_parks() {
        assert!(matches!(
            classify(&cand("TASK-2", "task", &[], true, false)),
            Pickability::Parked(_)
        ));
    }

    #[test]
    fn parking_tags_park_case_insensitively_with_deferred_prefix() {
        for tag in [
            "blocked",
            "needs-human-input",
            "Needs-Human",
            "needs-design-signoff",
            "operator-action",
            "needs-decision",
            "needs-supervised-build",
            "review:draft-only",
            "deferred:post-stability",
        ] {
            match classify(&cand("X-1", "task", &[tag], false, false)) {
                Pickability::Parked(r) => assert!(r.to_lowercase().contains(&tag.to_lowercase())),
                other => panic!("tag {tag} should park, got {other:?}"),
            }
        }
        // A benign tag does not park.
        assert_eq!(
            classify(&cand(
                "X-2",
                "task",
                &["batch:foo", "papercut"],
                false,
                false
            )),
            Pickability::Ready
        );
    }

    #[test]
    fn partition_preserves_order_and_separates() {
        let cands = vec![
            cand("A", "task", &[], false, false),             // ready
            cand("B", "epic", &[], false, false),             // parked (epic)
            cand("C", "task", &["deferred:x"], false, false), // parked (tag)
            cand("D", "story", &[], false, false),            // ready
        ];
        let (ready, parked) = partition(&cands);
        assert_eq!(ready, vec!["A".to_string(), "D".to_string()]);
        assert_eq!(
            parked.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
            vec!["B".to_string(), "C".to_string()]
        );
    }

    // STORY-544: the human-facing presentation helpers — plain-language
    // selector gloss + next-step footer pointing at the /aida-burndown skill.
    #[test]
    fn selector_summary_default_is_plain_language() {
        let s = selector_summary("approved", None, None);
        assert!(s.contains("Showing approved specs (default)."));
        // The narrowing hint names all three knobs and drops bare "selector".
        assert!(s.contains("--batch NAME"));
        assert!(s.contains("--tag X"));
        assert!(s.contains("--status <s>"));
        assert!(!s.to_lowercase().contains("selector:"));
    }

    #[test]
    fn selector_summary_reflects_filters() {
        let s = selector_summary("draft", Some("papercut"), Some("scaffolding"));
        assert!(s.contains("Showing draft specs filtered to tag papercut + batch scaffolding."));
    }

    // STORY-546: the queue-gate split.
    #[test]
    fn split_by_signoff_blesses_only_queued_pickable_specs() {
        let pickable = vec![
            "TASK-1".to_string(),
            "TASK-2".to_string(),
            "STORY-3".to_string(),
        ];
        let queued: std::collections::HashSet<String> =
            ["TASK-2".to_string()].into_iter().collect();
        let (ready, awaiting) = split_by_signoff(pickable, &queued);
        // Only the queued pickable spec is drainable.
        assert_eq!(ready, vec!["TASK-2".to_string()]);
        // The rest are pickable but await advisor sign-off (queueing).
        assert_eq!(awaiting, vec!["TASK-1".to_string(), "STORY-3".to_string()]);
    }

    #[test]
    fn split_by_signoff_empty_queue_blesses_nothing() {
        let pickable = vec!["TASK-1".to_string(), "TASK-2".to_string()];
        let (ready, awaiting) = split_by_signoff(pickable, &std::collections::HashSet::new());
        assert!(ready.is_empty());
        assert_eq!(awaiting.len(), 2);
    }

    // STORY-547: the broader "why still open" explainer.
    fn open(
        req_type: &str,
        status: &str,
        tags: &[&str],
        blocked: bool,
        decision: bool,
        in_flight: bool,
    ) -> OpenFacts {
        OpenFacts {
            id: "X-1".to_string(),
            req_type: req_type.to_string(),
            status: status.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            has_unsatisfied_blocker: blocked,
            has_pending_decision: decision,
            in_flight,
            findings: Vec::new(),
            residual_notes: Vec::new(),
        }
    }

    #[test]
    fn explain_open_buckets_every_status_and_signal() {
        // Live + held + decision signals win over everything else.
        assert_eq!(
            explain_open(&open("task", "approved", &[], false, false, true)).0,
            OpenBucket::InFlight
        );
        assert_eq!(
            explain_open(&open(
                "task",
                "approved",
                &["review:draft-only"],
                false,
                false,
                false
            ))
            .0,
            OpenBucket::HeldForReview
        );
        assert_eq!(
            explain_open(&open("task", "approved", &[], false, true, false)).0,
            OpenBucket::AwaitingDecision
        );
        assert_eq!(
            explain_open(&open(
                "bug",
                "approved",
                &["needs-design-signoff"],
                false,
                false,
                false
            ))
            .0,
            OpenBucket::AwaitingDecision
        );
        assert_eq!(
            explain_open(&open(
                "task",
                "draft",
                &["deferred:post-stability"],
                false,
                false,
                false
            ))
            .0,
            OpenBucket::Deferred
        );
        assert_eq!(
            explain_open(&open("story", "approved", &[], true, false, false)).0,
            OpenBucket::Blocked
        );
        // Structural facts.
        assert_eq!(
            explain_open(&open("epic", "planned", &[], false, false, false)).0,
            OpenBucket::Umbrella
        );
        assert_eq!(
            explain_open(&open("vision", "inprogress", &[], false, false, false)).0,
            OpenBucket::LongLived
        );
        // Status fallbacks.
        assert_eq!(
            explain_open(&open("task", "needsattention", &[], false, false, false)).0,
            OpenBucket::AwaitingDecision
        );
        assert_eq!(
            explain_open(&open("task", "draft", &[], false, false, false)).0,
            OpenBucket::Ungroomed
        );
        assert_eq!(
            explain_open(&open("task", "done", &[], false, false, false)).0,
            OpenBucket::AwaitingMerge
        );
        assert_eq!(
            explain_open(&open("task", "inprogress", &[], false, false, false)).0,
            OpenBucket::InProgress
        );
        // Approved + unblocked + ungated = the burndown-ready case.
        assert_eq!(
            explain_open(&open(
                "task",
                "approved",
                &["papercut"],
                false,
                false,
                false
            ))
            .0,
            OpenBucket::Actionable
        );
    }

    #[test]
    fn explain_open_precedence_decision_beats_structural() {
        // An epic that is ALSO tagged for a human decision reports the decision
        // (the more actionable, human-facing reason) rather than "umbrella".
        let (bucket, _) = explain_open(&open(
            "epic",
            "draft",
            &["needs-design-signoff"],
            false,
            false,
            false,
        ));
        assert_eq!(bucket, OpenBucket::AwaitingDecision);
    }

    /// TASK-744: the split `needs-human` umbrella routes to distinct buckets —
    /// `needs-supervised-build` is keyboard-build (`--zen`), `needs-decision`
    /// is a decision to answer; bare `needs-human` stays a decision (the
    /// umbrella) until specs are migrated.
    #[test]
    fn split_needs_human_routes_build_supervised_vs_decision() {
        assert_eq!(
            explain_open(&open(
                "task",
                "approved",
                &["needs-supervised-build"],
                false,
                false,
                false
            ))
            .0,
            OpenBucket::BuildSupervised
        );
        assert_eq!(
            explain_open(&open(
                "task",
                "approved",
                &["needs-decision"],
                false,
                false,
                false
            ))
            .0,
            OpenBucket::AwaitingDecision
        );
        assert_eq!(
            explain_open(&open(
                "task",
                "approved",
                &["needs-human"],
                false,
                false,
                false
            ))
            .0,
            OpenBucket::AwaitingDecision
        );
        // The new bucket has a stable key and still demands a human (at the keyboard).
        assert_eq!(OpenBucket::BuildSupervised.key(), "build-supervised");
        assert!(OpenBucket::BuildSupervised.needs_human());
    }

    #[test]
    fn open_bucket_keys_and_needs_human_are_stable() {
        assert_eq!(OpenBucket::HeldForReview.key(), "held-for-review");
        assert_eq!(OpenBucket::Deferred.key(), "deferred");
        assert!(OpenBucket::HeldForReview.needs_human());
        assert!(OpenBucket::Ungroomed.needs_human());
        assert!(OpenBucket::Umbrella.needs_human());
        // Self-resolving / flow buckets don't demand a human nudge.
        assert!(!OpenBucket::InProgress.needs_human());
        assert!(!OpenBucket::AwaitingMerge.needs_human());
        assert!(!OpenBucket::Deferred.needs_human());
    }

    // SPIKE-57 / TASK-746: the canonical predicate must agree with
    // `OpenBucket::needs_human()` for every open-bucket signal, and must OR in
    // the orthogonal `human_only` marker.
    #[test]
    fn human_required_matches_needs_human_for_open_buckets() {
        // Buckets that need a human → predicate true (human_only = false).
        let held = open(
            "task",
            "approved",
            &["review:draft-only"],
            false,
            false,
            false,
        );
        assert!(human_required(&held, false));
        let decision = open("task", "approved", &[], false, true, false);
        assert!(human_required(&decision, false));
        let ungroomed = open("task", "draft", &[], false, false, false);
        assert!(human_required(&ungroomed, false));
        let umbrella = open("epic", "planned", &[], false, false, false);
        assert!(human_required(&umbrella, false));

        // Self-resolving / flow buckets → predicate false (no marker).
        let in_flight = open("task", "approved", &[], false, false, true);
        assert!(!human_required(&in_flight, false));
        let in_progress = open("task", "inprogress", &[], false, false, false);
        assert!(!human_required(&in_progress, false));
        let awaiting_merge = open("task", "done", &[], false, false, false);
        assert!(!human_required(&awaiting_merge, false));
    }

    #[test]
    fn human_required_ors_in_human_only_marker() {
        // A spec whose bucket would self-resolve is STILL human-required when
        // the orthogonal `human_only` marker is set.
        let in_progress = open("task", "inprogress", &[], false, false, false);
        assert!(!human_required(&in_progress, false));
        assert!(human_required(&in_progress, true));
    }

    // BUG-493: the HeldForReview reason must reconcile against real forge state
    // rather than asserting a draft PR exists purely from the `review:draft-only`
    // tag. Origin: `aida why TASK-715` claimed "held as a draft PR for human
    // review" after draft PR #709 was CLOSED in the Session-63 reset, while
    // `aida show TASK-715` correctly reported "no PR opened yet".
    #[test]
    fn reconcile_held_for_review_honest_about_pr_state() {
        // Open PR found — the tag's claim holds; name the PR number.
        let open_reason = reconcile_held_for_review(&DraftPrObservation::Open(709));
        assert!(open_reason.contains("#709"));
        assert!(open_reason.to_lowercase().contains("held"));

        // No open PR — the BUG-493 case. The reason must NOT over-claim that a
        // draft PR is held for review; it must say no open PR exists and point
        // the user at the recovery (reopen / `aida show`).
        let closed_reason = reconcile_held_for_review(&DraftPrObservation::NoOpenPr);
        let lc = closed_reason.to_lowercase();
        assert!(
            lc.contains("no open pr"),
            "closed/absent PR must be reported as no open PR, got: {closed_reason}"
        );
        // The exact over-claim BUG-493 is about must be gone: never assert the
        // draft PR IS held for review when none is open.
        assert!(
            !lc.contains("held as a draft pr for human review") && !lc.contains("held as draft pr"),
            "must not assert a held draft PR when none is open, got: {closed_reason}"
        );
        assert!(lc.contains("reopen") || lc.contains("aida show"));

        // Forge unreachable — soften, don't assert.
        let unknown_reason = reconcile_held_for_review(&DraftPrObservation::Unverifiable);
        let ulc = unknown_reason.to_lowercase();
        assert!(!ulc.contains("held as a draft pr for human review"));
        assert!(ulc.contains("could not reach") || ulc.contains("verify"));
    }

    /// TASK-741: the held-for-review reconcile enforces a NAMED cross-axis
    /// invariant declared in the lifecycle model. Pin that the row exists (so a
    /// rename/removal in `lifecycle.rs` breaks here, at the consumer) and that
    /// the model's predicate agrees with each determinable reconcile arm.
    #[test]
    fn reconcile_held_for_review_enforces_declared_invariant() {
        use aida_core::lifecycle::{held_for_review_claim_holds, invariant};
        let row = invariant("held-for-review-implies-open-draft-pr")
            .expect("held-for-review invariant is declared in lifecycle::INVARIANTS");
        assert_eq!(row.origin, "BUG-493");
        // The Open arm asserts the claim holds; the NoOpenPr arm asserts it is
        // violated — exactly what the model predicate says.
        assert!(held_for_review_claim_holds(true));
        assert!(!held_for_review_claim_holds(false));
    }

    #[test]
    fn next_step_footer_points_at_run_command_and_skill() {
        let f = next_step_footer();
        // (a) points at the real `aida burndown run` CLI command (BUG-494:
        // the footer used to falsely claim this command didn't exist).
        assert!(f.contains("aida burndown run"));
        // (b) still notes /aida-burndown as the in-Claude alternative.
        assert!(f.contains("/aida-burndown"));
        // (c) no longer denies the run command exists.
        assert!(!f.to_lowercase().contains("there is no"));
        assert!(!f.to_lowercase().contains("not a cli subcommand"));
        // No internal trace SPEC-IDs leak into user-facing text.
        assert!(!f.contains("STORY-"));
        assert!(!f.contains("BUG-"));
    }

    // TASK-723: multi-reason — derived + finding-link + residual note.
    #[test]
    fn parse_why_open_comment_extracts_reason_case_insensitively() {
        assert_eq!(
            parse_why_open_comment("why-open: waiting on upstream X"),
            Some("waiting on upstream X".to_string())
        );
        // Case-insensitive prefix, leading whitespace tolerated, reason trimmed.
        assert_eq!(
            parse_why_open_comment("  WHY-OPEN:   deferred pending budget  "),
            Some("deferred pending budget".to_string())
        );
        // Not a residual note.
        assert_eq!(parse_why_open_comment("a normal comment"), None);
        // Empty reason after the prefix is not a note.
        assert_eq!(parse_why_open_comment("why-open:   "), None);
    }

    #[test]
    fn explain_reasons_orders_derived_then_findings_then_residual() {
        let mut f = open("task", "approved", &[], false, false, false);
        f.findings = vec!["TASK-900".to_string()];
        f.residual_notes = vec!["waiting on upstream X".to_string()];
        let (bucket, reasons) = explain_reasons(&f);
        // The derived reason still drives the bucket.
        assert_eq!(bucket, OpenBucket::Actionable);
        assert_eq!(reasons.len(), 3);
        // Most-fundamental-first: derived → finding → residual.
        assert_eq!(reasons[0].source, ReasonSource::Derived);
        assert_eq!(reasons[1].source, ReasonSource::Finding);
        assert!(reasons[1].text.contains("TASK-900"));
        assert_eq!(reasons[2].source, ReasonSource::Residual);
        assert_eq!(reasons[2].text, "waiting on upstream X");
    }

    #[test]
    fn explain_reasons_derived_only_when_no_extras() {
        let f = open("task", "draft", &[], false, false, false);
        let (_, reasons) = explain_reasons(&f);
        assert_eq!(reasons.len(), 1);
        assert_eq!(reasons[0].source, ReasonSource::Derived);
    }

    #[test]
    fn reason_source_keys_are_stable() {
        assert_eq!(ReasonSource::Derived.key(), "derived");
        assert_eq!(ReasonSource::Finding.key(), "finding");
        assert_eq!(ReasonSource::Residual.key(), "residual");
    }

    // ---- STORY-563: `aida human unblock` classifier + prompt assembler. ----

    #[allow(clippy::too_many_arguments)]
    fn ufacts(
        id: &str,
        req_type: &str,
        status: &str,
        tags: &[&str],
        blocked: bool,
        decision: bool,
        in_flight: bool,
        queued: bool,
        has_acceptance: bool,
        implementable: bool,
    ) -> UnblockFacts {
        UnblockFacts {
            id: id.to_string(),
            req_type: req_type.to_string(),
            status: status.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            has_unsatisfied_blocker: blocked,
            has_pending_decision: decision,
            in_flight,
            queued,
            has_acceptance,
            implementable,
        }
    }

    #[test]
    fn unblock_ready_set_item_is_excluded() {
        // Queued, approved, unblocked, decision-free, has acceptance, bounded.
        let f = ufacts(
            "TASK-1",
            "task",
            "approved",
            &[],
            false,
            false,
            false,
            true,
            true,
            true,
        );
        assert!(f.in_burndown_ready_set());
        assert_eq!(classify_unblock(&f), None);
    }

    #[test]
    fn unblock_draft_needs_approval() {
        // Draft with acceptance — needs approval, not under-specified.
        let f = ufacts(
            "STORY-1",
            "story",
            "draft",
            &[],
            false,
            false,
            false,
            false,
            true,
            true,
        );
        assert_eq!(classify_unblock(&f), Some(UnblockClass::NeedsApproval));
    }

    #[test]
    fn unblock_approved_unqueued_is_queueable() {
        let f = ufacts(
            "TASK-2",
            "task",
            "approved",
            &[],
            false,
            false,
            false,
            false,
            true,
            true,
        );
        assert_eq!(classify_unblock(&f), Some(UnblockClass::ApprovedUnqueued));
        assert_eq!(
            UnblockClass::ApprovedUnqueued.action(),
            UnblockAction::Queue
        );
    }

    #[test]
    fn unblock_missing_acceptance_is_under_specified() {
        // Approved but no acceptance + implementable → clarify first.
        let f = ufacts(
            "TASK-3",
            "task",
            "approved",
            &[],
            false,
            false,
            false,
            false,
            false,
            true,
        );
        assert_eq!(classify_unblock(&f), Some(UnblockClass::UnderSpecified));
        assert_eq!(
            UnblockClass::UnderSpecified.action(),
            UnblockAction::Clarify
        );
    }

    #[test]
    fn unblock_non_implementable_missing_acceptance_is_not_under_specified() {
        // A vision has no acceptance and is non-implementable — not flagged
        // under-specified; falls through to approved-unqueued.
        let f = ufacts(
            "VIS-1",
            "vision",
            "approved",
            &[],
            false,
            false,
            false,
            false,
            false,
            false,
        );
        assert_eq!(classify_unblock(&f), Some(UnblockClass::ApprovedUnqueued));
    }

    #[test]
    fn unblock_decision_blocker_deferral_leave_parked() {
        let decision = ufacts(
            "S-1",
            "story",
            "approved",
            &["needs-design-signoff"],
            false,
            false,
            false,
            false,
            true,
            true,
        );
        assert_eq!(
            classify_unblock(&decision),
            Some(UnblockClass::DecisionPending)
        );

        let pending = ufacts(
            "S-2",
            "story",
            "approved",
            &[],
            false,
            true,
            false,
            false,
            true,
            true,
        );
        assert_eq!(
            classify_unblock(&pending),
            Some(UnblockClass::DecisionPending)
        );

        let blocked = ufacts(
            "T-1",
            "task",
            "approved",
            &[],
            true,
            false,
            false,
            false,
            true,
            true,
        );
        assert_eq!(classify_unblock(&blocked), Some(UnblockClass::BlockedBy));

        let deferred = ufacts(
            "T-2",
            "task",
            "approved",
            &["deferred:post-stability"],
            false,
            false,
            false,
            false,
            true,
            true,
        );
        assert_eq!(classify_unblock(&deferred), Some(UnblockClass::Deferred));

        for cls in [
            UnblockClass::DecisionPending,
            UnblockClass::BlockedBy,
            UnblockClass::Deferred,
            UnblockClass::BuildSupervised,
        ] {
            assert_eq!(cls.action(), UnblockAction::Leave);
        }
    }

    #[test]
    fn unblock_in_flight_and_in_progress_are_build_supervised() {
        let in_flight = ufacts(
            "T-3",
            "task",
            "approved",
            &[],
            false,
            false,
            true,
            true,
            true,
            true,
        );
        assert_eq!(
            classify_unblock(&in_flight),
            Some(UnblockClass::BuildSupervised)
        );

        let in_progress = ufacts(
            "T-4",
            "task",
            "inprogress",
            &[],
            false,
            false,
            false,
            false,
            true,
            true,
        );
        assert_eq!(
            classify_unblock(&in_progress),
            Some(UnblockClass::BuildSupervised)
        );
    }

    #[test]
    fn unblock_epic_is_decision_pending() {
        let f = ufacts(
            "EPIC-1",
            "epic",
            "approved",
            &[],
            false,
            false,
            false,
            false,
            true,
            false,
        );
        assert_eq!(classify_unblock(&f), Some(UnblockClass::DecisionPending));
    }

    #[test]
    fn unblock_class_keys_are_stable() {
        assert_eq!(UnblockClass::NeedsApproval.key(), "needs-approval");
        assert_eq!(UnblockClass::ApprovedUnqueued.key(), "approved-unqueued");
        assert_eq!(UnblockClass::UnderSpecified.key(), "under-specified");
        assert_eq!(UnblockClass::BuildSupervised.key(), "build-supervised");
        assert_eq!(UnblockClass::DecisionPending.key(), "decision-pending");
        assert_eq!(UnblockClass::Deferred.key(), "deferred");
        assert_eq!(UnblockClass::BlockedBy.key(), "blocked-by");
    }

    #[test]
    fn assemble_unblock_prompt_groups_by_action() {
        let lines = vec![
            UnblockLine {
                id: "TASK-2".to_string(),
                class: UnblockClass::ApprovedUnqueued,
                reason: unblock_reason(UnblockClass::ApprovedUnqueued).to_string(),
            },
            UnblockLine {
                id: "TASK-3".to_string(),
                class: UnblockClass::UnderSpecified,
                reason: unblock_reason(UnblockClass::UnderSpecified).to_string(),
            },
            UnblockLine {
                id: "T-1".to_string(),
                class: UnblockClass::BlockedBy,
                reason: unblock_reason(UnblockClass::BlockedBy).to_string(),
            },
        ];
        let prompt = assemble_unblock_prompt(&lines);
        assert!(prompt.contains("QUEUE"));
        assert!(prompt.contains("CLARIFY FIRST"));
        assert!(prompt.contains("LEAVE PARKED"));
        // Each spec lands under its action heading, with its reason.
        assert!(prompt.contains("TASK-2 — approved"));
        assert!(prompt.contains("TASK-3 — missing acceptance"));
        assert!(prompt.contains("T-1 — blocked"));
        // The leading instruction names the three actions.
        assert!(prompt.contains("queue the autonomous-able"));
    }

    #[test]
    fn assemble_unblock_prompt_renders_empty_sections() {
        let prompt = assemble_unblock_prompt(&[]);
        assert!(prompt.contains("QUEUE: (none)"));
        assert!(prompt.contains("CLARIFY FIRST: (none)"));
        assert!(prompt.contains("LEAVE PARKED: (none)"));
    }
}
