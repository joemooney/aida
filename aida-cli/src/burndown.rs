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
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// The per-spec verdict of the burndown selector's first pass: should this spec
/// be discarded, collected into the keyboard-only `supervised` section, or
/// carried forward as a drain candidate? Extracted from the inline loop in
/// `resolve_burndown_sets` so the filter/bucket ordering — the load-bearing
/// invariant behind BUG-537 (deferred), BUG-551 (supervised AFTER the status
/// filter), and the selector semantics — is unit-testable without a live store.
/// trace:TASK-803 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SpecDisposition {
    /// Out of scope for this selector — drop it entirely.
    Skip,
    /// `supervised`-tagged + in scope: keyboard pickup, excluded from the drain.
    Supervised,
    /// A drain candidate (still subject to the pickability gate downstream).
    Candidate(BurndownCandidate),
}

/// Already-probed inputs for [`classify_spec`]. All status strings are expected
/// pre-normalized (alphanumeric-lowercase) by the caller so comparison is a
/// plain `==`. trace:TASK-803
pub(crate) struct SpecClassifyInput<'a> {
    pub archived: bool,
    pub deferred: bool,
    /// Normalized status of the spec.
    pub status_norm: &'a str,
    /// Normalized selector status (what `--status` asked for).
    pub want_status: &'a str,
    pub tags: &'a [String],
    /// Optional `--tag` filter (matched case-insensitively).
    pub tag_filter: Option<&'a str>,
    /// Optional `batch:<name>` tag filter (matched case-insensitively).
    pub batch_tag: Option<&'a str>,
    pub disp: &'a str,
    pub req_type: &'a str,
    pub has_unsatisfied_blocker: bool,
    pub has_pending_decision: bool,
}

/// Classify ONE spec for the burndown selector. The filter order is load-bearing
/// and mirrors the historical inline loop EXACTLY:
///   archived → deferred → status → tag → batch → supervised → candidate.
/// The `supervised` check sits AFTER the status/tag/batch filters (BUG-551), so a
/// Done/Completed supervised spec fails the status guard and is `Skip`ped before
/// it can reach the supervised bucket. trace:TASK-803 trace:BUG-551 trace:BUG-537
pub(crate) fn classify_spec(input: &SpecClassifyInput) -> SpecDisposition {
    if input.archived {
        return SpecDisposition::Skip;
    }
    if input.deferred {
        return SpecDisposition::Skip;
    }
    if input.status_norm != input.want_status {
        return SpecDisposition::Skip;
    }
    if let Some(t) = input.tag_filter {
        if !input.tags.iter().any(|x| x.eq_ignore_ascii_case(t)) {
            return SpecDisposition::Skip;
        }
    }
    if let Some(bt) = input.batch_tag {
        if !input.tags.iter().any(|x| x.eq_ignore_ascii_case(bt)) {
            return SpecDisposition::Skip;
        }
    }
    if input
        .tags
        .iter()
        .any(|t| t.eq_ignore_ascii_case("supervised"))
    {
        return SpecDisposition::Supervised;
    }
    SpecDisposition::Candidate(BurndownCandidate {
        id: input.disp.to_string(),
        req_type: input.req_type.to_string(),
        tags: input.tags.to_vec(),
        has_unsatisfied_blocker: input.has_unsatisfied_blocker,
        has_pending_decision: input.has_pending_decision,
    })
}

/// A tag that marks a spec as not-autonomously-pickable — a human decision, a
/// deferral, or a draft-review gate. Matched case-insensitively; `deferred:` is
/// a prefix. Returns the matched tag (for the parked reason). trace:STORY-527
///
/// `pub(crate)` so the `aida questions answer` unpark path (STORY-555) can ask
/// "does the tag I just added/cleared still park this spec?" against the SAME
/// predicate the burndown gate uses — the answer path and the gate can never
/// disagree on what a parking tag is. trace:STORY-555 | ai:claude
pub(crate) fn parking_tag(tags: &[String]) -> Option<String> {
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

/// STORY-568: which lane a SPIKE belongs in. A spike's deliverable is an
/// analysis + decision, NOT a mergeable PR, so it never enters the implementer
/// fan-out — but "not for the code drain" is three distinct things the old
/// gate flattened into a single "human-only" label:
///   - `Research`     — agent-able; dispatch to the research lane (`aida research`).
///   - `NeedsDecision`— the research is done / not needed; a human must pick.
///   - `HumanOnly`    — genuinely requires a human to do the analysis (rare).
/// Tags drive the split; the default for an untagged spike is `Research`
/// (most spikes are research the operator wrongly believed needed a human).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpikeLane {
    Research,
    NeedsDecision,
    HumanOnly,
}

/// Classify a spike into its lane from its tags. `human-only` wins over
/// `needs-decision` (most specific human gate first); everything else is the
/// agent-able `Research` default. Matched case-insensitively.
// trace:STORY-568 | ai:claude
pub(crate) fn classify_spike_lane(tags: &[String]) -> SpikeLane {
    let has = |name: &str| tags.iter().any(|t| t.trim().eq_ignore_ascii_case(name));
    if has("human-only") {
        SpikeLane::HumanOnly
    } else if has("needs-decision") {
        SpikeLane::NeedsDecision
    } else {
        SpikeLane::Research
    }
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
    // trace:BUG-514 | ai:codex
    if let Some(tag) = parking_tag(&c.tags) {
        return Pickability::Parked(format!("tagged `{tag}`"));
    }
    // STORY-568: a spike is never an implementer-fan-out candidate (no PR
    // lifecycle), but name the precise lane instead of the old flat "human-
    // only" — agent-able research is dispatched, not human-gated.
    if c.req_type.eq_ignore_ascii_case("spike") {
        return Pickability::Parked(match classify_spike_lane(&c.tags) {
            SpikeLane::Research => {
                "spike (research-lane) — dispatch to a research agent via `aida research <ID>`"
                    .to_string()
            }
            SpikeLane::NeedsDecision => {
                "spike (needs-decision) — escalate the call via `aida questions`".to_string()
            }
            SpikeLane::HumanOnly => "spike (human-only) — human analysis required".to_string(),
        });
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

/// Extract the `serialize:<group>` group names from a spec's tags (case-
/// insensitive prefix, lowercased group key). A spec may belong to several
/// serialize groups; each is returned. trace:STORY-614
pub(crate) fn serialize_groups(tags: &[String]) -> Vec<String> {
    tags.iter()
        .filter_map(|t| {
            t.trim()
                .strip_prefix("serialize:")
                .or_else(|| t.trim().strip_prefix("Serialize:"))
                .map(|g| g.trim().to_ascii_lowercase())
        })
        .filter(|g| !g.is_empty())
        .collect()
}

/// STORY-614: substrate-enforce the `serialize:<group>` convention. Given the
/// READY fan-out set (display ids, in deterministic order) and a lookup from
/// display id → its serialize groups, collapse the set so that AT MOST ONE spec
/// per distinct group survives in this wave. The held-back members are returned
/// separately (NOT dropped) so the caller can fold them into the "awaiting" set
/// — they drain in successive waves.
///
/// Pick is deterministic: the FIRST id (in the supplied order) claims each
/// group; later members sharing any already-claimed group are held. The caller
/// supplies `ready` sorted (lowest id first) so the pick is stable across runs.
/// Specs with no serialize tag are never held — they all stay parallel. Pure +
/// order-preserving so the gate is unit-testable without a live store.
/// trace:STORY-614 | ai:claude
pub(crate) fn collapse_serialize_groups(
    ready: Vec<String>,
    groups_by_id: &std::collections::HashMap<String, Vec<String>>,
) -> (Vec<String>, Vec<String>) {
    let mut kept = Vec::new();
    let mut held = Vec::new();
    let mut claimed: std::collections::HashSet<String> = std::collections::HashSet::new();
    for id in ready {
        let groups = groups_by_id.get(&id).cloned().unwrap_or_default();
        // A spec is held if ANY of its groups is already claimed this wave.
        if groups.iter().any(|g| claimed.contains(g)) {
            held.push(id);
        } else {
            for g in groups {
                claimed.insert(g);
            }
            kept.push(id);
        }
    }
    (kept, held)
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
    /// BUG-543: an umbrella epic whose child rollup is fully delivered (every
    /// child Completed) but the epic itself is still open — a fully-delivered
    /// epic reading as Draft/open. Surfaced for a one-click close (detection
    /// only; the operator confirms — a rollup miscount must never auto-close).
    ReadyToClose,
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
            OpenBucket::Umbrella => "decompose",
            OpenBucket::ReadyToClose => "ready-to-close",
            OpenBucket::LongLived => "long-lived",
            OpenBucket::Ungroomed => "to-groom",
            OpenBucket::AwaitingMerge => "awaiting-merge",
            OpenBucket::InProgress => "in-progress",
            OpenBucket::Actionable => "actionable",
        }
    }

    /// True for the buckets that genuinely need a person's nudge (vs. those
    /// that will resolve themselves through normal flow). This is the UNION
    /// across both seats — drives the explainer's "needs you" grouping. The
    /// per-seat split (operator vs advisor) is layered on top by
    /// [`OpenBucket::advisor_seat`] / [`OpenBucket::operator_seat`], so the two
    /// worklist views (`aida human` / `aida advisor`) partition this union
    /// without changing what "needs a person" means. trace:STORY-547
    pub(crate) fn needs_human(self) -> bool {
        matches!(
            self,
            OpenBucket::HeldForReview
                | OpenBucket::AwaitingDecision
                | OpenBucket::BuildSupervised
                | OpenBucket::Ungroomed
                | OpenBucket::Umbrella
                | OpenBucket::ReadyToClose
        )
    }

    /// STORY-618: the "needs a person" buckets that are ADVISOR work — routine
    /// dispositions that require cross-spec awareness + product judgment but NOT
    /// the operator's strategic sign-off. These belong on `aida advisor`'s
    /// worklist (GROOM ungroomed drafts, DECOMPOSE childless umbrellas, CLOSE
    /// fully-delivered epics), NOT on `aida human` — leaking them onto the
    /// operator's list is the seat-confusion EPIC-42 fixes. trace:STORY-618
    pub(crate) fn advisor_seat(self) -> bool {
        matches!(
            self,
            OpenBucket::Ungroomed | OpenBucket::Umbrella | OpenBucket::ReadyToClose
        )
    }

    /// STORY-618: the "needs a person" buckets that are genuinely OPERATOR work
    /// — they require the human's strategic/keystone judgment the advisor can't
    /// make unilaterally (review a PR, decide a posed fork, drive a keystone
    /// build). The complement of [`advisor_seat`] within [`needs_human`].
    /// (`AwaitingDecision` is operator work too, but the human view renders it
    /// as its own first-class "decisions-awaiting" bucket, so it is excluded
    /// from the derived-bucket grouping here.) trace:STORY-618
    pub(crate) fn operator_seat(self) -> bool {
        matches!(
            self,
            OpenBucket::HeldForReview | OpenBucket::BuildSupervised
        )
    }
}

/// BUG-579: the fixed, most-actionable-first render order for the DERIVED buckets
/// in `aida human`. The single source of truth — `handle_list_human` iterates
/// this, and [`classified_row_is_rendered`] checks against it so the count and
/// the render can never silently disagree. trace:BUG-579 | ai:claude
pub(crate) const HUMAN_DERIVED_BUCKET_ORDER: [OpenBucket; 5] = [
    OpenBucket::ReadyToClose,
    OpenBucket::HeldForReview,
    OpenBucket::BuildSupervised,
    OpenBucket::Ungroomed,
    OpenBucket::Umbrella,
];

/// BUG-579: does a `classified` row get rendered somewhere in `aida human`?
///
/// A classified row is *counted* toward the "N items need a human" total
/// unconditionally; it is only *rendered* if it is covered by one of:
///   1. the ordered derived-bucket loop ([`HUMAN_DERIVED_BUCKET_ORDER`]), or
///   2. the first-class "decisions-awaiting" bucket — but that bucket only lists
///      `AwaitingDecision` specs that carry a real `DecisionRequest`
///      (`has_pending_decision`).
///
/// The bug: an `AwaitingDecision` spec with NO posed `DecisionRequest`
/// (`has_pending_decision == false`) satisfies neither — it is counted but
/// invisible, the phantom "1 item needs a human" with no bucket to act on. This
/// predicate returns `false` for exactly those rows, so the catch-all in
/// `handle_list_human` can surface them and restore the count==render invariant.
/// trace:BUG-579 | ai:claude
pub(crate) fn classified_row_is_rendered(bucket: OpenBucket, has_pending_decision: bool) -> bool {
    HUMAN_DERIVED_BUCKET_ORDER.contains(&bucket)
        || (bucket == OpenBucket::AwaitingDecision && has_pending_decision)
}

/// BUG-579: a short, actionable hint for a counted-but-unrendered classified row
/// in the `aida human` catch-all. `AwaitingDecision` with no posed question is
/// the common case (a Spike that needs a decision but has no formal
/// `DecisionRequest`); other buckets fall back to their derived reason text.
/// trace:BUG-579 | ai:claude
pub(crate) fn needs_attention_hint(bucket: OpenBucket) -> &'static str {
    match bucket {
        OpenBucket::AwaitingDecision => {
            "needs a decision — no question posed yet; pose one with `aida clarify` \
             / `aida questions sweep`, or groom it"
        }
        _ => "needs a human — no bucket-specific action derived yet",
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
    /// BUG-511: the role recorded on the live lease (`reviewer`,
    /// `implementer`, …), when known — lets the in-flight reason say WHAT
    /// kind of work holds the spec ("being reviewed"). `None` when not in
    /// flight or the lease carries no role.
    pub in_flight_role: Option<String>,
    /// TASK-723 (source #2): display-ids of FINDINGS filed against this spec
    /// (attempt outcomes — CI red / RequestChanges / build fail). Linked, not
    /// recomputed: the finding already exists; the view just folds it in.
    pub findings: Vec<String>,
    /// TASK-723 (source #3): non-derivable residual reasons a human recorded as
    /// `why-open:<reason>` prefixed comments. Folded in verbatim; derivable
    /// state is NEVER written here (staleness trap).
    pub residual_notes: Vec<String>,
    /// BUG-543: for an EPIC, its child rollup as `(completed, total)` over the
    /// hierarchy subtree (the same numbers `aida graph --tree` prints). `None`
    /// for non-epics and for epics with no children (`total == 0`). When
    /// `total > 0 && completed == total` the epic is fully delivered and
    /// surfaces as `ReadyToClose` rather than the generic umbrella reason. The
    /// caller computes it (it needs the store); `OpenFacts` stays pure.
    pub epic_rollup: Option<(usize, usize)>,
    /// BUG-564: the orthogonal `human_only` pickability marker (`req.human_only`
    /// on the crate-level `Requirement`), carried here so the human worklist can
    /// fold it into the [`human_required`] predicate. It is an INPUT to
    /// classification, never derived from the other facts, so `OpenFacts` stays
    /// exhaustively testable — a `human_only` spec is just one more bool the
    /// predicate ORs in (mirroring [`human_required`]'s separate-arg contract).
    /// Before this field, the human view fed `human_required` a hardcoded
    /// `false`, so a spec that was human-required PURELY via this marker (a
    /// non-human bucket) was invisible in `aida list human` while the queue's
    /// human-only bucket — which reads the same `req.human_only` — showed it.
    /// trace:BUG-564 | ai:claude
    pub human_only: bool,
    /// TASK-884: for an EPIC, whether ANY child is actually in motion
    /// (Completed / Done / InProgress). An epic with children that are all still
    /// Draft/Approved/Planned (or shelved) is NOT self-driving "in progress" —
    /// nothing is being worked, so it needs decomposition/grooming attention and
    /// must surface in the advisor's `decompose` lane rather than vanishing into
    /// the (invisible-to-both-worklists) `InProgress` bucket. `false` for
    /// non-epics and childless epics. trace:TASK-884
    pub epic_children_in_motion: bool,
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
        // BUG-511: name the work when the lease says what it is — a live
        // review (the `aida review` verb or a reviewer session) reads
        // "being reviewed", not the generic working line.
        let reason = if f
            .in_flight_role
            .as_deref()
            .is_some_and(|r| r.eq_ignore_ascii_case("reviewer"))
        {
            "in flight — being reviewed by a live review session".to_string()
        } else {
            "in flight — a live session lease is working this now".to_string()
        };
        return (OpenBucket::InFlight, reason);
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
            "clear to build, but at the keyboard (`aida queue work <id> --zen`), \
             not the unsupervised drain"
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
        // BUG-543: a fully-delivered epic (every child Completed) should NOT
        // read as a generic umbrella — surface it for a one-click close. Strict
        // `completed == total` (a Rejected/remaining child keeps it umbrella):
        // detection-only, so under-flagging is the safe direction.
        if let Some((completed, total)) = f.epic_rollup {
            if total > 0 && completed == total {
                return (
                    OpenBucket::ReadyToClose,
                    format!(
                        "ready to close — all {total} children Completed; \
                         close with `aida edit {} --status completed`",
                        f.id
                    ),
                );
            }
        }
        // TASK-808: only an epic that NEEDS a human belongs in `aida human`.
        // A childless epic needs decomposition (actionable). An epic with
        // in-flight children is SELF-DRIVING — it resolves as its children
        // land, so it's not a human action and drops out of the human view
        // (InProgress is not a needs_human bucket). Rollup-complete is the
        // ReadyToClose case handled above. trace:TASK-808 | ai:claude
        return match f.epic_rollup {
            // TASK-884: an epic with children IN MOTION (a child Completed / Done
            // / InProgress) is genuinely self-driving — nothing for a worklist
            // until the children finish. But an epic whose children are ALL still
            // un-started (Draft/Approved/Planned) or shelved is NOT in progress;
            // it needs decomposition/grooming attention and must land in the
            // advisor's `decompose` lane rather than the invisible InProgress
            // bucket (the dogfood gap: a draft epic with draft children appeared
            // in NO section). trace:TASK-884
            Some((_, total)) if total > 0 && f.epic_children_in_motion => (
                OpenBucket::InProgress,
                "epic in progress — driven by its children; nothing for you until they all finish (then it shows as ready to close)".to_string(),
            ),
            Some((_, total)) if total > 0 => (
                OpenBucket::Umbrella,
                "an epic whose children are all still un-started — drive them forward or refine the breakdown (`aida graph --tree <id>`, then groom/queue the children)".to_string(),
            ),
            _ => (
                OpenBucket::Umbrella,
                "an epic with no children yet — break it into child specs (`aida add --parent <id> ...`) or close it".to_string(),
            ),
        };
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
            "an ungroomed draft — groom it: weigh it against the graph (overlap / staleness / scope), \
             then dispose — `aida edit <id> --status approved|rejected`, `aida defer <id> --until <trigger>`, or refine the spec"
                .to_string(),
        ),
        "done" => (
            OpenBucket::AwaitingMerge,
            "done on a branch — awaiting merge to the default branch (auto-completes on merge)"
                .to_string(),
        ),
        // STORY-727 (FIX 6): say what's actually true and name the next command.
        // The old "work in progress" was a tautology that named nothing.
        "inprogress" => (
            OpenBucket::InProgress,
            "in progress — being implemented now; run `aida status <id>` to see who's on it and whether it's live or stale"
                .to_string(),
        ),
        // STORY-727 (FIX 6 + FIX 11): Approved and Planned must read DIFFERENTLY
        // so a human learns what Planned means. Both are buildable now; the
        // difference is design state. Each names the next command in plain
        // language (no "burndown ready set" jargon).
        "approved" => (
            OpenBucket::Actionable,
            "ready — run `aida zen <id>` to build + ship it autonomously, or `aida ship <id>` if you'll implement it yourself"
                .to_string(),
        ),
        "planned" => (
            OpenBucket::Actionable,
            "planned & ready — the design is settled; run `aida zen <id>` to build + ship it, or `aida ship <id>` to implement it yourself"
                .to_string(),
        ),
        _ => (
            OpenBucket::Actionable,
            "open & actionable — run `aida zen <id>` to build + ship it autonomously".to_string(),
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
///
/// STORY-618 split the `aida human` view onto the per-seat
/// [`OpenBucket::operator_seat`] predicate, so this union predicate no longer
/// has a production caller — it is retained (and still tested) because it
/// folds in the orthogonal `human_only` marker that the seat predicates do not,
/// which the deferred SPIKE-57 human_only view phase will need.
/// trace:TASK-746 trace:STORY-618 | ai:claude
#[allow(dead_code)]
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
    /// BUG-502: built work awaiting human REVIEW — a `review:draft-only` spec
    /// (or one at/past Done) is DONE work, not re-implementation. It belongs in
    /// a REVIEW bucket ("review it / reopen the draft PR"), never QUEUE or
    /// CLARIFY — those would re-build done work. trace:BUG-502 | ai:claude
    HeldForReview,
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
            UnblockClass::HeldForReview => "held-for-review",
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
            // BUG-502: built — the human reviews it, never queues/clarifies it.
            UnblockClass::HeldForReview => UnblockAction::Review,
            // Everything else stays parked — a human keyboard, a decision, a
            // dependency, or a deliberate deferral has to happen first.
            UnblockClass::BuildSupervised
            | UnblockClass::DecisionPending
            | UnblockClass::Deferred
            | UnblockClass::BlockedBy => UnblockAction::Leave,
        }
    }
}

/// STORY-563: the grooming actions the prompt routes each spec to. BUG-502 added
/// `Review` for built-but-held work that wants a human review pass, not a queue
/// or clarify. trace:BUG-502 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnblockAction {
    /// Queue it straight into the burndown.
    Queue,
    /// Clarify (author acceptance criteria) first.
    Clarify,
    /// BUG-502: built — review it (`aida review` / reopen the draft PR).
    Review,
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
    /// BUG-502: built work whose acceptance gap is MOOT — at/past Done, or tagged
    /// `review:draft-only` (work done, awaiting human review). Mirrors the
    /// questions-sweep `is_built_or_held` predicate (BUG-495) so the two surfaces
    /// agree on "this is built, don't treat it as re-implementation." Excludes the
    /// in-flight case (that's its own BuildSupervised signal). trace:BUG-502
    pub(crate) fn is_built_or_held(&self) -> bool {
        let at_or_past_done = self.status == "done" || self.status == "completed";
        let draft_only = self
            .tags
            .iter()
            .any(|t| t.trim().eq_ignore_ascii_case("review:draft-only"));
        at_or_past_done || draft_only
    }

    /// BUG-502: does this spec pass the same pickability gate `aida burndown plan`
    /// applies (see [`classify`])? Bounded (not epic), decision-free, unblocked,
    /// not parking-tagged. Crucially the gate does NOT require a `## Acceptance`
    /// section — so a queued spec that passes the gate is GROOMED and must not be
    /// flagged "missing acceptance → clarify" by unblock. Reuses [`parking_tag`]
    /// so the buckets agree with the burndown plan. trace:BUG-502 | ai:claude
    pub(crate) fn passes_pickability_gate(&self) -> bool {
        !self.req_type.eq_ignore_ascii_case("epic")
            && !self.has_pending_decision
            && !self.has_unsatisfied_blocker
            && !self.in_flight
            && parking_tag(&self.tags).is_none()
    }

    /// True for the items the human has nothing left to do for — the GROOMED set
    /// that `aida human unblock` excludes. BUG-502 reconciles this with the
    /// pickability gate: a spec already QUEUED that passes the gate is groomed
    /// regardless of an `## Acceptance` section (the gate doesn't require one), so
    /// a queued-ready spec is no longer mis-flagged "missing acceptance". The
    /// status must be approved+ (queued draft still needs sign-off). trace:BUG-502
    pub(crate) fn in_burndown_ready_set(&self) -> bool {
        self.queued && self.status == "approved" && self.passes_pickability_gate()
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

    // BUG-502: built-and-held work is awaiting a human REVIEW pass, not a queue
    // or clarify — those would re-build done work. A `review:draft-only` spec (or
    // one at/past Done) is the most specific "this is built" signal, so it wins
    // over the clarify/queue buckets below. Mirrors the questions-sweep
    // `is_built_or_held` exclusion (BUG-495) — but routes to its OWN bucket here
    // rather than dropping it, so the human still sees "review it". An in-flight
    // lease (live work now) stays BuildSupervised; a pending decision on a
    // draft-only spec is a genuine decision — both are handled below, so only
    // route to HeldForReview when neither of those more-current signals fires.
    // trace:BUG-502 | ai:claude
    if f.is_built_or_held() && !f.in_flight && !f.has_pending_decision {
        return Some(UnblockClass::HeldForReview);
    }

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
        UnblockClass::HeldForReview => {
            "built — review it (`aida review` / reopen the draft PR), not re-queue or clarify"
        }
        UnblockClass::UnderSpecified => "missing acceptance criteria — clarify before queuing",
        UnblockClass::BuildSupervised => {
            "build-supervised — clear to build, but the at-keyboard `--zen` lane \
             (in-flight, or tagged `needs-supervised-build`), not an unattended drain"
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
    let review = by_action(UnblockAction::Review);
    let leave = by_action(UnblockAction::Leave);

    let mut out = String::new();
    out.push_str(
        "Groom these open specs into the burndown. For each, take exactly the action below — \
         queue the autonomous-able, clarify the under-specified FIRST (author acceptance criteria \
         before queuing), REVIEW the built-and-held (it's done work — review it, don't re-queue or \
         re-clarify), and LEAVE the rest parked (they need a human keyboard, a decision, a \
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
    // BUG-502: built-and-held work — review it, never re-queue or re-clarify.
    section(
        &mut out,
        "REVIEW",
        "built — review it via `aida review` / reopen the draft PR, do NOT re-queue or clarify",
        &review,
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

/// STORY-566: the primary next ACTION `aida queue advance` offers for a queued
/// spec in each bucket — a ROUTER over the existing flows, not a new flow. The
/// kind drives the interactive menu's first option and the `--yes` auto-take
/// gate. Pure + exhaustively testable; the handler maps the chosen kind back to
/// the existing `aida review` / `aida queue work [--zen]` / approve / reject
/// dispatch. trace:STORY-566 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdvanceAction {
    /// Run the human review pass (`aida review <id>`); on approval, offer to
    /// drop the `review:draft-only` tag so it drains.
    Review,
    /// Drain it now at the keyboard — supervised build (`aida queue work <id>
    /// --zen`).
    SupervisedBuild,
    /// Answer the open decision (`aida questions`).
    Decision,
    /// Drain it now (`aida queue work <id>`, no --zen) — it's in the ready set.
    Drain,
    /// Promote draft/planned → Approved so it becomes drainable.
    Approve,
    /// Resolve it out — set status → Rejected (or leave parked).
    Reject,
    /// BUG-543: close a fully-delivered umbrella epic — set status → Completed
    /// (or leave open). Confirmed by the operator, never auto-taken under
    /// `--yes` (a rollup miscount must not silently close an epic).
    Close,
    /// Nothing to do here — the bucket resolves itself through normal flow
    /// (in-flight, awaiting-merge, in-progress).
    None,
}

impl AdvanceAction {
    /// True for the ONE unambiguous autonomous step `--yes` may auto-take
    /// without a human in the loop (drain a ready spec / approve a groomed
    /// draft). Everything else (review, supervised build, decision, reject) is
    /// a human call and is SKIPPED under `--yes`. trace:STORY-566 | ai:claude
    pub(crate) fn is_autonomous(self) -> bool {
        matches!(self, AdvanceAction::Drain | AdvanceAction::Approve)
    }
}

/// STORY-566: the primary action `aida queue advance` routes each open-bucket
/// to. PURE map (bucket → action) — the side-effecting dispatch lives in
/// `main.rs`; keeping the routing here makes it exhaustively unit-testable and
/// keeps the menu + the `--yes` gate agreeing on what each bucket needs.
/// trace:STORY-566 | ai:claude
pub(crate) fn advance_action(bucket: OpenBucket) -> AdvanceAction {
    match bucket {
        OpenBucket::HeldForReview => AdvanceAction::Review,
        OpenBucket::BuildSupervised => AdvanceAction::SupervisedBuild,
        OpenBucket::AwaitingDecision => AdvanceAction::Decision,
        OpenBucket::Actionable => AdvanceAction::Drain,
        OpenBucket::Ungroomed => AdvanceAction::Approve,
        OpenBucket::Deferred | OpenBucket::Blocked => AdvanceAction::Reject,
        // BUG-543: a fully-delivered epic — close it (operator-confirmed).
        OpenBucket::ReadyToClose => AdvanceAction::Close,
        // Vision/principle have no terminal state by design, and a not-yet-
        // delivered epic is driven by its children — neither is
        // rejectable/processable directly here. In-flight / awaiting-merge /
        // in-progress resolve through normal flow. Nothing for the operator to
        // do on these directly.
        OpenBucket::LongLived
        | OpenBucket::Umbrella
        | OpenBucket::InFlight
        | OpenBucket::AwaitingMerge
        | OpenBucket::InProgress => AdvanceAction::None,
    }
}

/// STORY-566: the menu/label text for each bucket's primary action — the first
/// `inquire::Select` option `aida queue advance` shows. Pure. trace:STORY-566
pub(crate) fn advance_action_label(bucket: OpenBucket) -> &'static str {
    match advance_action(bucket) {
        AdvanceAction::Review => "Review it (aida review)",
        AdvanceAction::SupervisedBuild => "Build it now at the keyboard (queue work --zen)",
        AdvanceAction::Decision => "Answer the open decision (aida questions)",
        AdvanceAction::Drain => "Drain it now (queue work)",
        AdvanceAction::Approve => "Approve it (makes it drainable)",
        AdvanceAction::Reject => "Reject (resolve it out)",
        AdvanceAction::Close => "Close it (all children completed → status completed)",
        AdvanceAction::None => "Nothing to do — resolves through normal flow",
    }
}

/// STORY-565: the SINGLE next action for ONE non-ready queued item, phrased with
/// the operator's own SPEC-ID inlined so the footer reads as a copy-pasteable
/// instruction. A ROUTER over `advance_action` — same bucket→action map the
/// interactive `aida queue advance` uses, just rendered as one imperative line.
/// Pure + testable; the `id` is the operator's own queued spec (fine to print —
/// it's their item, not a breadcrumb). trace:STORY-565 | ai:claude
fn advance_action_sentence(bucket: OpenBucket, id: &str) -> String {
    match advance_action(bucket) {
        AdvanceAction::Review => format!("review it (`aida review {id}`)"),
        AdvanceAction::SupervisedBuild => {
            format!("build at the keyboard (`aida queue work {id} --zen`)")
        }
        AdvanceAction::Decision => "answer the decision (`aida questions`)".to_string(),
        AdvanceAction::Drain => format!("drain it (`aida queue work {id}`)"),
        AdvanceAction::Approve => format!("approve it (`aida edit {id} --status approved`)"),
        AdvanceAction::Reject => format!("resolve it out (`aida edit {id} --status rejected`)"),
        AdvanceAction::Close => {
            format!("close it — all children completed (`aida edit {id} --status completed`)")
        }
        AdvanceAction::None => "resolves through normal flow — nothing to do".to_string(),
    }
}

/// STORY-565: one queued item, already classified, for the path-to-empty footer.
/// `bucket == Actionable` ⇒ it's part of the "ready" count; everything else gets
/// its own per-item "needs you" line. trace:STORY-565 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct QueuedItem {
    /// The operator's own display SPEC-ID.
    pub id: String,
    /// Its open-bucket classification (from `explain_open`).
    pub bucket: OpenBucket,
}

/// STORY-565: render the "how do I get to zero?" footer for a non-empty queue —
/// SIGNPOSTING over the SAME classifier `aida queue advance` uses, not new
/// state. Disambiguates the two meanings of "empty": DRAIN (`aida burndown run`)
/// does the work, CLEAR (`aida queue clear`) just drops queue membership — and
/// names the single next action for each non-ready (parked/blocked/held) item.
/// Pure (no color, no store) so it's unit-testable without a store; the caller
/// colorizes. Returns `None` for an empty slice (caller prints the empty-queue
/// line instead). trace:STORY-565 | ai:claude
pub(crate) fn render_path_to_empty(items: &[QueuedItem]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let ready = items
        .iter()
        .filter(|i| i.bucket == OpenBucket::Actionable)
        .count();

    // trace:TASK-840 | ai:claude
    let bullet = crate::glyphs::get(
        crate::glyphs::Glyph::Bullet,
        crate::find_project_root().ok().as_deref(),
    );
    let mut out = String::from("To empty this queue:");
    if ready > 0 {
        out.push_str(&format!(
            "\n  {bullet} {ready} ready    → `aida burndown run` (does the work)  ·  \
             `aida queue clear` (just drops them)"
        ));
    }
    // BUG-621: split the non-Actionable items by whether they ACTUALLY need the
    // operator. Three groups, not one:
    //  1. genuinely-parked/blocked/held → keep the "needs you:" prefix + a single
    //     next action.
    //  2. self-resolving in flight (lease-backed work, awaiting merge, long-lived
    //     by design) → NO operator action; reads "nothing for you to do, clears
    //     through normal flow". Pairing "needs you" with the AdvanceAction::None
    //     "nothing to do" sentence was the self-contradiction this fixes.
    //  3. InProgress WITHOUT a live lease (drift) → NOT self-resolving: a spec
    //     marked in-progress with no session lease working it will not clear on
    //     its own, so it does NOT get the reassuring "clears when shipped" line —
    //     it needs a nudge. The classifier already keeps the two apart: a live
    //     lease lands in OpenBucket::InFlight (line ~590), a status-only
    //     in-progress spec lands in OpenBucket::InProgress (line ~728).
    // trace:BUG-621 | ai:claude
    for item in items.iter().filter(|i| {
        i.bucket != OpenBucket::Actionable && !self_resolving(i.bucket) && !is_drift(i.bucket)
    }) {
        out.push_str(&format!(
            "\n  {bullet} {:<10} needs you: {}",
            item.id,
            advance_action_sentence(item.bucket, &item.id)
        ));
    }
    // Group 3: in-progress-without-lease drift — flag, but never as "needs you:
    // resolves through normal flow". These will NOT self-resolve.
    for item in items.iter().filter(|i| is_drift(i.bucket)) {
        out.push_str(&format!(
            "\n  {bullet} {:<10} in progress, but no live session lease — stalled; \
             pick it back up (`aida queue work {}`) or park it",
            item.id, item.id
        ));
    }
    // Group 2: genuinely in flight / self-resolving — no operator action.
    let self_resolving: Vec<&str> = items
        .iter()
        .filter(|i| self_resolving(i.bucket))
        .map(|i| i.id.as_str())
        .collect();
    if !self_resolving.is_empty() {
        out.push_str(&format!(
            "\n  {bullet} {} in flight — nothing for you to do; clears when shipped: {}",
            self_resolving.len(),
            self_resolving.join(", ")
        ));
    }
    Some(out)
}

/// BUG-621: a bucket whose items clear through the normal lifecycle with NO
/// operator action — lease-backed live work, work awaiting merge, and
/// long-lived specs with no terminal state by design. These must NOT be labeled
/// "needs you". Deliberately EXCLUDES [`OpenBucket::InProgress`] (status-only,
/// no lease = drift; see [`is_drift`]). trace:BUG-621 | ai:claude
fn self_resolving(bucket: OpenBucket) -> bool {
    matches!(
        bucket,
        OpenBucket::InFlight | OpenBucket::AwaitingMerge | OpenBucket::LongLived
    )
}

/// BUG-621: an item marked in-progress with NO live session lease — drift, not
/// genuine motion. It will not self-resolve, so it gets neither the reassuring
/// "clears when shipped" line nor the contradictory "needs you: nothing to do"
/// line; it gets a "stalled — pick it back up" nudge. trace:BUG-621 | ai:claude
fn is_drift(bucket: OpenBucket) -> bool {
    matches!(bucket, OpenBucket::InProgress)
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

    // TASK-803: the per-spec classifier — the filter/bucket ordering that was
    // previously inline (and untested) in `resolve_burndown_sets`.
    fn classify_case(
        archived: bool,
        deferred: bool,
        status: &str,
        want: &str,
        tags: &[&str],
        tag_filter: Option<&str>,
        batch_tag: Option<&str>,
    ) -> SpecDisposition {
        let tags: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
        classify_spec(&SpecClassifyInput {
            archived,
            deferred,
            status_norm: status,
            want_status: want,
            tags: &tags,
            tag_filter,
            batch_tag,
            disp: "TASK-1",
            req_type: "task",
            has_unsatisfied_blocker: false,
            has_pending_decision: false,
        })
    }

    /// STORY-618: the operator/advisor seat split must be a clean partition of
    /// the `needs_human` union — every needs-human bucket belongs to exactly
    /// one seat (or is `AwaitingDecision`, which the human view renders as its
    /// own first-class bucket), and the two seats never overlap.
    #[test]
    fn seat_split_partitions_needs_human() {
        use OpenBucket::*;
        let all = [
            HeldForReview,
            InFlight,
            AwaitingDecision,
            BuildSupervised,
            Deferred,
            Blocked,
            Umbrella,
            ReadyToClose,
            LongLived,
            Ungroomed,
            AwaitingMerge,
            InProgress,
            Actionable,
        ];
        for b in all {
            // Seats never overlap.
            assert!(
                !(b.advisor_seat() && b.operator_seat()),
                "{b:?} is in both seats"
            );
            // A seated bucket always needs a human.
            if b.advisor_seat() || b.operator_seat() {
                assert!(b.needs_human(), "{b:?} is seated but not needs_human");
            }
            // Every needs-human bucket is seated, except AwaitingDecision (its
            // own first-class human-view bucket).
            if b.needs_human() && b != AwaitingDecision {
                assert!(
                    b.advisor_seat() ^ b.operator_seat(),
                    "{b:?} needs a human but isn't in exactly one seat"
                );
            }
        }
        // Spot-check the seat assignments the user complaint hinges on.
        assert!(Ungroomed.advisor_seat(), "drafts groom on the advisor seat");
        assert!(ReadyToClose.advisor_seat());
        assert!(Umbrella.advisor_seat());
        assert!(HeldForReview.operator_seat());
        assert!(BuildSupervised.operator_seat());
    }

    // BUG-572: the `aida human` operator-view filter ORs in the `human_only`
    // marker (BUG-564) so a human-only spec surfaces even when its derived
    // bucket isn't a needs-human one. But the marker must NOT override the
    // to-groom→advisor routing: an ungroomed Draft (the `Ungroomed` bucket) with
    // `human_only=true` is grooming work, not a decision — it belongs on the
    // advisor, never on the operator. A GROOMED human_only spec (any non-
    // `Ungroomed` bucket — e.g. BUG-564's High/Approved scaffolding task) STILL
    // reaches the operator. This encodes the exact `human_only` clause from
    // `handle_list_human`'s `classified` filter. trace:BUG-572 | ai:claude
    #[test]
    fn human_only_does_not_pull_ungroomed_draft_onto_operator_seat() {
        // The operator-view `human_only` clause, lifted from
        // `handle_list_human`: human_only-marked specs surface here EXCEPT when
        // their bucket is the to-groom (`Ungroomed`) bucket.
        let human_clause =
            |human_only: bool, bucket: OpenBucket| human_only && bucket != OpenBucket::Ungroomed;

        // An ungroomed to-groom draft marked human_only stays OFF the operator
        // seat — grooming is advisor work.
        assert!(
            !human_clause(true, OpenBucket::Ungroomed),
            "an ungroomed (to-groom) human_only draft must not reach the operator"
        );
        // ...and it IS routed to the advisor: the advisor-worklist filter gates
        // on advisor_seat(), which Ungroomed satisfies — so it is not dropped
        // from both views.
        assert!(
            OpenBucket::Ungroomed.advisor_seat(),
            "the ungroomed draft must surface on the advisor seat instead"
        );

        // BUG-564 regression guard: a GROOMED human_only spec (non-Ungroomed
        // bucket) STILL reaches the operator via the human_only clause.
        assert!(
            human_clause(true, OpenBucket::HeldForReview),
            "a groomed human_only spec must still reach the operator (BUG-564)"
        );
        assert!(
            human_clause(true, OpenBucket::Actionable),
            "a groomed (non-Ungroomed) human_only spec must still reach the operator"
        );
    }

    // BUG-579: the phantom "N items need a human" bug — a `classified`
    // AwaitingDecision row with NO posed DecisionRequest is COUNTED toward the
    // total but, before the fix, was rendered by neither the ordered derived-
    // bucket loop nor the decisions-awaiting bucket (which only lists rows with a
    // real DecisionRequest). `aida human`'s catch-all renders exactly the rows
    // for which `classified_row_is_rendered` returns false, so this asserts that
    // such a row IS surfaced (count==render). The test FAILS against the old code
    // (AwaitingDecision was excluded from `order` AND from decisions-awaiting
    // when has_pending_decision==false → invisible). trace:BUG-579 | ai:claude
    #[test]
    fn awaiting_decision_without_posed_question_is_rendered_not_just_counted() {
        // The exact case from BUG-579: a Spike classified AwaitingDecision with
        // no formal DecisionRequest. It is NOT covered by the ordered buckets and
        // NOT covered by the decisions-awaiting bucket → must be caught by the
        // catch-all (predicate returns false → catch-all renders it).
        assert!(
            !classified_row_is_rendered(OpenBucket::AwaitingDecision, false),
            "AwaitingDecision with no DecisionRequest must NOT be considered \
             already-rendered — the catch-all must surface it (BUG-579)"
        );
        // It gets the actionable, decision-specific hint, not the generic one.
        assert!(
            needs_attention_hint(OpenBucket::AwaitingDecision).contains("decision"),
            "the needs-attention hint must point the operator at posing a decision"
        );

        // The complement: an AwaitingDecision row WITH a posed DecisionRequest is
        // already rendered by the first-class decisions-awaiting bucket — the
        // catch-all must NOT double-render it.
        assert!(
            classified_row_is_rendered(OpenBucket::AwaitingDecision, true),
            "AwaitingDecision WITH a DecisionRequest is shown by the decisions \
             bucket — the catch-all must not re-render it"
        );

        // The predicate must stay the exact mirror of the render order — if a
        // bucket is in `HUMAN_DERIVED_BUCKET_ORDER` it is rendered by the ordered
        // loop; otherwise it is rendered only via decisions-awaiting (and only
        // with a posed question) or, failing that, the catch-all. This guards the
        // predicate from drifting away from the loop and silently re-introducing
        // the count!=render gap. trace:BUG-579
        use OpenBucket::*;
        for b in HUMAN_DERIVED_BUCKET_ORDER {
            assert!(
                classified_row_is_rendered(b, false),
                "{b:?} is in the render order but the predicate calls it unrendered"
            );
        }
        // Buckets NOT in the order array (other than AwaitingDecision-with-a-
        // question) are reported unrendered → the catch-all is responsible for
        // them. Spot-check the variants that can reach `classified` outside the
        // ordered set.
        for b in [
            InFlight,
            Deferred,
            Blocked,
            LongLived,
            AwaitingMerge,
            InProgress,
            Actionable,
        ] {
            assert!(
                !classified_row_is_rendered(b, false),
                "{b:?} is outside the ordered loop — the catch-all must own it"
            );
        }
    }

    #[test]
    fn classify_skips_archived_deferred_and_status_mismatch() {
        assert_eq!(
            classify_case(true, false, "approved", "approved", &[], None, None),
            SpecDisposition::Skip
        );
        assert_eq!(
            classify_case(false, true, "approved", "approved", &[], None, None),
            SpecDisposition::Skip
        );
        assert_eq!(
            classify_case(false, false, "done", "approved", &[], None, None),
            SpecDisposition::Skip
        );
    }

    #[test]
    fn classify_supervised_only_when_status_matches() {
        // BUG-551: the supervised check runs AFTER the status filter, so a
        // supervised spec only buckets as Supervised when it matches want_status.
        assert_eq!(
            classify_case(
                false,
                false,
                "approved",
                "approved",
                &["supervised"],
                None,
                None
            ),
            SpecDisposition::Supervised
        );
        // Done / Completed supervised specs fail the status guard first → Skip
        // (they belong in the reviews bucket / are shipped, not "work this").
        assert_eq!(
            classify_case(
                false,
                false,
                "done",
                "approved",
                &["supervised"],
                None,
                None
            ),
            SpecDisposition::Skip
        );
        assert_eq!(
            classify_case(
                false,
                false,
                "completed",
                "approved",
                &["supervised"],
                None,
                None
            ),
            SpecDisposition::Skip
        );
        // Tag match is case-insensitive.
        assert_eq!(
            classify_case(
                false,
                false,
                "approved",
                "approved",
                &["Supervised"],
                None,
                None
            ),
            SpecDisposition::Supervised
        );
    }

    #[test]
    fn classify_honors_tag_and_batch_filters_before_supervised() {
        // --tag mismatch → Skip even if otherwise in scope.
        assert_eq!(
            classify_case(
                false,
                false,
                "approved",
                "approved",
                &["foo"],
                Some("bar"),
                None
            ),
            SpecDisposition::Skip
        );
        assert!(matches!(
            classify_case(
                false,
                false,
                "approved",
                "approved",
                &["bar"],
                Some("bar"),
                None
            ),
            SpecDisposition::Candidate(_)
        ));
        // --batch mismatch → Skip; match → Candidate.
        assert_eq!(
            classify_case(
                false,
                false,
                "approved",
                "approved",
                &["batch:other"],
                None,
                Some("batch:x")
            ),
            SpecDisposition::Skip
        );
        assert!(matches!(
            classify_case(
                false,
                false,
                "approved",
                "approved",
                &["batch:x"],
                None,
                Some("batch:x")
            ),
            SpecDisposition::Candidate(_)
        ));
        // A supervised spec that fails the --tag filter is Skipped, not
        // Supervised — the filter precedes the supervised bucket.
        assert_eq!(
            classify_case(
                false,
                false,
                "approved",
                "approved",
                &["supervised"],
                Some("bar"),
                None
            ),
            SpecDisposition::Skip
        );
    }

    #[test]
    fn classify_plain_matching_spec_is_candidate_carrying_probed_fields() {
        let tags = vec!["papercut".to_string()];
        let d = classify_spec(&SpecClassifyInput {
            archived: false,
            deferred: false,
            status_norm: "approved",
            want_status: "approved",
            tags: &tags,
            tag_filter: None,
            batch_tag: None,
            disp: "TASK-9",
            req_type: "task",
            has_unsatisfied_blocker: true,
            has_pending_decision: true,
        });
        match d {
            SpecDisposition::Candidate(c) => {
                assert_eq!(c.id, "TASK-9");
                assert_eq!(c.req_type, "task");
                assert!(c.has_unsatisfied_blocker);
                assert!(c.has_pending_decision);
                assert_eq!(c.tags, vec!["papercut".to_string()]);
            }
            other => panic!("expected Candidate, got {other:?}"),
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

    // STORY-568: spikes are parked from the implementer fan-out, but with a
    // precise lane reason — never the old flat "human-only" for agent-able
    // research.
    #[test]
    fn untagged_spike_is_research_lane_not_human_only() {
        match classify(&cand("SPIKE-1", "spike", &[], false, false)) {
            Pickability::Parked(r) => {
                assert!(r.contains("research-lane"), "got {r:?}");
                assert!(
                    r.contains("aida research"),
                    "names the dispatch path: {r:?}"
                );
                assert!(!r.to_lowercase().contains("human-only"), "got {r:?}");
            }
            other => panic!("a spike should park, got {other:?}"),
        }
    }

    #[test]
    fn spike_lane_split_by_tag() {
        assert_eq!(classify_spike_lane(&[]), SpikeLane::Research);
        assert_eq!(
            classify_spike_lane(&["needs-supervised-build".into()]),
            SpikeLane::Research
        );
        assert_eq!(
            classify_spike_lane(&["needs-decision".into()]),
            SpikeLane::NeedsDecision
        );
        assert_eq!(
            classify_spike_lane(&["Human-Only".into()]),
            SpikeLane::HumanOnly
        );
        // human-only is the most specific human gate and wins the tie.
        assert_eq!(
            classify_spike_lane(&["needs-decision".into(), "human-only".into()]),
            SpikeLane::HumanOnly
        );
    }

    #[test]
    fn needs_decision_spike_parks_as_decision_lane() {
        match classify(&cand("SPIKE-2", "spike", &["needs-decision"], false, false)) {
            Pickability::Parked(r) => assert!(r.contains("needs-decision"), "got {r:?}"),
            other => panic!("expected Parked, got {other:?}"),
        }
    }

    // trace:BUG-514 | ai:codex
    #[test]
    fn deferred_spike_reports_parking_tag_before_research_lane() {
        match classify(&cand(
            "SPIKE-4",
            "spike",
            &["deferred:stabilization-first"],
            false,
            false,
        )) {
            Pickability::Parked(r) => {
                assert!(r.contains("deferred:stabilization-first"), "got {r:?}");
                assert!(!r.contains("research-lane"), "got {r:?}");
                assert!(!r.contains("aida research"), "got {r:?}");
            }
            other => panic!("expected Parked, got {other:?}"),
        }
    }

    #[test]
    fn spike_with_pending_decision_reports_the_pending_question_first() {
        // Research already escalated -> the pending-decision reason (more
        // fundamental) wins over the lane reason.
        match classify(&cand("SPIKE-3", "spike", &[], false, true)) {
            Pickability::Parked(r) => assert!(r.contains("pending decision"), "got {r:?}"),
            other => panic!("expected Parked, got {other:?}"),
        }
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
            in_flight_role: None,
            findings: Vec::new(),
            residual_notes: Vec::new(),
            epic_rollup: None,
            human_only: false,
            epic_children_in_motion: false,
        }
    }

    /// BUG-564: a human-only spec whose DERIVED bucket is NOT a needs-human
    /// bucket must still be classified human-required (so it surfaces in
    /// `aida list human`), and must NOT be human-required when the marker is
    /// off. This is the predicate the human-view filter now folds `f.human_only`
    /// into — before the fix the view fed `human_required` a hardcoded `false`,
    /// so the init scaffolding-commit task (High/Approved → Actionable, a
    /// non-human bucket) was invisible while the queue's human-only bucket showed
    /// it. trace:BUG-564 | ai:claude
    #[test]
    fn human_required_folds_in_human_only_marker_over_non_human_bucket() {
        // An approved, unblocked, decision-free, in-flight-free task classifies
        // to the Actionable bucket — explicitly NOT a needs-human bucket.
        let f = open("task", "approved", &[], false, false, false);
        let (bucket, _) = explain_open(&f);
        assert!(
            !bucket.needs_human(),
            "precondition: {bucket:?} must be a non-human bucket for this test"
        );

        // human_only OFF → not human-required (the bucket alone doesn't qualify).
        assert!(
            !human_required(&f, false),
            "non-human bucket without the human_only marker must NOT be human-required"
        );

        // human_only ON → human-required PURELY via the orthogonal marker, even
        // though the derived bucket self-resolves. This is the BUG-564 case.
        assert!(
            human_required(&f, true),
            "a human-only spec must be human-required even with a non-human bucket"
        );
    }

    /// BUG-511: a live reviewer lease reads "being reviewed", any other
    /// (or unknown) role keeps the generic in-flight line — and both stay
    /// in the InFlight bucket so the footer says "nothing to do".
    #[test]
    fn explain_open_in_flight_names_review_when_role_is_reviewer() {
        let mut f = open("task", "approved", &[], false, false, true);
        f.in_flight_role = Some("reviewer".to_string());
        let (bucket, reason) = explain_open(&f);
        assert_eq!(bucket, OpenBucket::InFlight);
        assert!(reason.contains("being reviewed"), "{reason}");

        f.in_flight_role = Some("implementer".to_string());
        let (bucket, reason) = explain_open(&f);
        assert_eq!(bucket, OpenBucket::InFlight);
        assert!(!reason.contains("being reviewed"), "{reason}");

        f.in_flight_role = None;
        let (bucket, reason) = explain_open(&f);
        assert_eq!(bucket, OpenBucket::InFlight);
        assert!(reason.contains("in flight"), "{reason}");
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

    // STORY-727 (FIX 6 + FIX 11): the plain-language `why` reasons NAME the next
    // command and read DIFFERENTLY for Approved vs Planned, so a human learns
    // what Planned means and never sees the old "burndown ready set" jargon.
    #[test]
    fn explain_open_reasons_name_command_and_differentiate_approved_planned() {
        let approved = explain_open(&open("task", "approved", &[], false, false, false)).1;
        let planned = explain_open(&open("task", "planned", &[], false, false, false)).1;
        let inprogress = explain_open(&open("task", "inprogress", &[], false, false, false)).1;

        // Approved names the autonomous drive and the manual alternative.
        assert!(approved.contains("aida zen"), "approved reason: {approved}");
        assert!(
            approved.contains("aida ship"),
            "approved reason: {approved}"
        );

        // Planned names the same commands but reads differently (design settled).
        assert!(planned.contains("aida zen"), "planned reason: {planned}");
        assert!(
            planned.contains("design is settled"),
            "planned reason: {planned}"
        );
        assert_ne!(
            approved, planned,
            "Approved and Planned must read differently (FIX 11)"
        );

        // In-progress says what's true and points at the liveness view, not the
        // old "work in progress" tautology.
        assert!(
            inprogress.contains("aida status"),
            "inprogress reason: {inprogress}"
        );
        assert_ne!(inprogress, "work in progress");

        // No jargon leaks: the old "burndown ready set" phrasing is gone.
        assert!(!approved.contains("burndown plan"));
        assert!(!planned.contains("burndown plan"));
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

    // BUG-543: a fully-delivered epic (rollup completed == total) surfaces as
    // ReadyToClose with the one-click close command naming its own id; a
    // partial rollup (or no rollup) stays the generic Umbrella.
    #[test]
    fn explain_open_epic_ready_to_close_when_rollup_complete() {
        let mut f = open("epic", "draft", &[], false, false, false);
        f.id = "EPIC-41".to_string();

        // All children Completed → ready to close.
        f.epic_rollup = Some((4, 4));
        let (bucket, reason) = explain_open(&f);
        assert_eq!(bucket, OpenBucket::ReadyToClose);
        assert!(reason.contains("ready to close"), "{reason}");
        assert!(
            reason.contains("aida edit EPIC-41 --status completed"),
            "names the close command with its own id: {reason}"
        );
        assert!(
            bucket.needs_human(),
            "ready-to-close needs operator confirm"
        );

        // TASK-808: a child still open AND something IN MOTION → epic is
        // SELF-DRIVING → InProgress, and it must NOT need a human (it drops out
        // of `aida human`). TASK-884: "in motion" is now an explicit signal.
        f.epic_rollup = Some((3, 4));
        f.epic_children_in_motion = true;
        assert_eq!(explain_open(&f).0, OpenBucket::InProgress);
        assert!(
            !explain_open(&f).0.needs_human(),
            "an in-flight epic is driven by its children — not a human action"
        );

        // No rollup info (a childless epic) → needs decomposition → Umbrella
        // (still a human action: break it into children).
        f.epic_rollup = None;
        assert_eq!(explain_open(&f).0, OpenBucket::Umbrella);
        assert!(
            explain_open(&f).0.needs_human(),
            "a childless epic needs decomposition"
        );

        // A zero-total rollup (no children) is the same childless case → Umbrella.
        f.epic_rollup = Some((0, 0));
        assert_eq!(explain_open(&f).0, OpenBucket::Umbrella);
    }

    // TASK-884: a draft epic WITH children that are all still un-started (nothing
    // in motion) must surface in the advisor's `decompose` lane (Umbrella), not
    // vanish into the invisible-to-both-worklists InProgress bucket. This was the
    // dogfood gap: EPIC-6 appeared in NO advisor section.
    #[test]
    fn explain_open_draft_epic_with_unstarted_children_is_umbrella() {
        let mut f = open("epic", "draft", &[], false, false, false);
        f.id = "EPIC-6".to_string();
        // Children exist (total > 0) but none are Completed/Done/InProgress.
        f.epic_rollup = Some((0, 3));
        f.epic_children_in_motion = false;

        let (bucket, reason) = explain_open(&f);
        assert_eq!(
            bucket,
            OpenBucket::Umbrella,
            "an epic whose children are all un-started belongs in the decompose lane"
        );
        assert!(
            bucket.advisor_seat(),
            "the decompose lane is an advisor-seat bucket — it must be visible on `aida advisor`"
        );
        assert!(reason.contains("un-started"), "{reason}");

        // The moment a single child starts moving, it flips to self-driving.
        f.epic_children_in_motion = true;
        assert_eq!(explain_open(&f).0, OpenBucket::InProgress);
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

    // STORY-565: the path-to-empty footer builder is pure — testable without a
    // store. trace:STORY-565 | ai:claude
    #[test]
    fn render_path_to_empty_is_none_for_empty_queue() {
        assert!(render_path_to_empty(&[]).is_none());
    }

    #[test]
    fn render_path_to_empty_disambiguates_drain_from_clear() {
        let items = vec![QueuedItem {
            id: "TASK-101".to_string(),
            bucket: OpenBucket::Actionable,
        }];
        let f = render_path_to_empty(&items).expect("non-empty");
        assert!(f.starts_with("To empty this queue:"));
        // Ready count + BOTH the drain (does the work) and clear (drops them)
        // commands on the one line — AC2.
        assert!(f.contains("1 ready"));
        assert!(f.contains("aida burndown run"));
        assert!(f.contains("does the work"));
        assert!(f.contains("aida queue clear"));
        // No per-item "needs you" line when everything is ready.
        assert!(!f.contains("needs you"));
    }

    #[test]
    fn render_path_to_empty_names_single_action_per_nonready_item() {
        let items = vec![
            QueuedItem {
                id: "TASK-1".to_string(),
                bucket: OpenBucket::Actionable,
            },
            QueuedItem {
                id: "STORY-2".to_string(),
                bucket: OpenBucket::HeldForReview,
            },
            QueuedItem {
                id: "TASK-3".to_string(),
                bucket: OpenBucket::BuildSupervised,
            },
            QueuedItem {
                id: "BUG-4".to_string(),
                bucket: OpenBucket::AwaitingDecision,
            },
        ];
        let f = render_path_to_empty(&items).expect("non-empty");
        // One ready item still drives the drain/clear line.
        assert!(f.contains("1 ready"));
        // Each non-ready item gets its OWN line with the operator's id + the
        // single next action, inlining the id where the command takes one.
        assert!(f.contains("STORY-2") && f.contains("aida review STORY-2"));
        assert!(f.contains("TASK-3") && f.contains("aida queue work TASK-3 --zen"));
        assert!(f.contains("BUG-4") && f.contains("aida questions"));
        // Exactly three "needs you" lines (the three non-ready items).
        assert_eq!(f.matches("needs you").count(), 3);
        // No internal trace SPEC-IDs leak into the STATIC framing. The
        // operator's OWN queued ids (TASK-1 etc.) are fine — those aren't
        // breadcrumbs. We only assert the framing words carry no leak.
        for line in f.lines() {
            // Strip the operator's own ids by checking the static prose only.
            assert!(!line.contains("trace:"));
        }
    }

    // BUG-621: the self-resolving (in-flight / awaiting-merge / long-lived)
    // bucket must NEVER pair "needs you" with "nothing to do" — it reads as
    // requiring no operator action. trace:BUG-621 | ai:claude
    #[test]
    fn render_path_to_empty_self_resolving_is_not_needs_you() {
        for bucket in [
            OpenBucket::InFlight,
            OpenBucket::AwaitingMerge,
            OpenBucket::LongLived,
        ] {
            let items = vec![QueuedItem {
                id: "TASK-894".to_string(),
                bucket,
            }];
            let f = render_path_to_empty(&items).expect("non-empty");
            // The self-contradiction is gone: no "needs you" on a self-resolving
            // item, and no "needs you: ... nothing to do" pairing anywhere.
            assert!(
                !f.contains("needs you"),
                "self-resolving bucket {bucket:?} must not say 'needs you': {f}"
            );
            assert!(
                !f.contains("nothing to do"),
                "self-resolving bucket {bucket:?} must not say 'nothing to do': {f}"
            );
            // It reads as no-action: "nothing for you to do; clears when shipped".
            assert!(
                f.contains("nothing for you to do") && f.contains("TASK-894"),
                "self-resolving item should read as no-action: {f}"
            );
        }
    }

    // BUG-621: an in-progress spec WITHOUT a live lease (drift) is NOT
    // self-resolving — it must not get the reassuring "clears when shipped" line,
    // and it must not say "needs you: ... nothing to do" either. It reads as
    // stalled. trace:BUG-621 | ai:claude
    #[test]
    fn render_path_to_empty_in_progress_without_lease_reads_as_stalled() {
        let items = vec![QueuedItem {
            id: "TASK-895".to_string(),
            bucket: OpenBucket::InProgress,
        }];
        let f = render_path_to_empty(&items).expect("non-empty");
        assert!(
            !f.contains("needs you"),
            "drift must not say 'needs you': {f}"
        );
        assert!(
            !f.contains("nothing to do"),
            "drift must not say 'nothing to do': {f}"
        );
        assert!(
            !f.contains("clears when shipped"),
            "drift must NOT promise resolution: {f}"
        );
        assert!(
            f.contains("stalled") && f.contains("TASK-895"),
            "drift should read as stalled with a nudge: {f}"
        );
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

    // BUG-502: a `review:draft-only` spec is BUILT work awaiting human review —
    // it must land in the REVIEW bucket, never QUEUE or CLARIFY (which would
    // re-build done work). Covers the STORY-543/TASK-715 dogfood regression.
    #[test]
    fn unblock_review_draft_only_is_held_for_review() {
        // Queued draft-only (like STORY-543, which was mis-routed to QUEUE).
        let queued_draft_only = ufacts(
            "STORY-543",
            "story",
            "approved",
            &["review:draft-only"],
            false,
            false,
            false,
            true,
            true,
            true,
        );
        assert_eq!(
            classify_unblock(&queued_draft_only),
            Some(UnblockClass::HeldForReview)
        );
        assert_eq!(
            UnblockClass::HeldForReview.action(),
            UnblockAction::Review,
            "built work is reviewed, never re-queued or clarified"
        );

        // Unqueued draft-only with NO acceptance section (like TASK-715, which
        // was mis-routed to CLARIFY-FIRST "missing acceptance"). Must NOT be
        // flagged under-specified — it's built.
        let unqueued_draft_only_no_acceptance = ufacts(
            "TASK-715",
            "task",
            "approved",
            &["review:draft-only"],
            false,
            false,
            false,
            false,
            false, // no acceptance
            true,
        );
        assert_eq!(
            classify_unblock(&unqueued_draft_only_no_acceptance),
            Some(UnblockClass::HeldForReview)
        );

        // A spec at Done is likewise built — review it, don't re-queue.
        let done = ufacts(
            "TASK-9",
            "task",
            "done",
            &[],
            false,
            false,
            false,
            false,
            true,
            true,
        );
        assert_eq!(classify_unblock(&done), Some(UnblockClass::HeldForReview));
    }

    // BUG-502: a queued spec that passes the pickability gate is GROOMED — even
    // without a `## Acceptance` section, because the gate (`aida burndown plan`)
    // does NOT require one. It must be excluded from `aida human unblock`, NOT
    // flagged "missing acceptance → clarify". Reconciles the under-specified
    // check with the pickability gate.
    #[test]
    fn unblock_queued_ready_without_acceptance_is_not_clarify() {
        let queued_ready_no_acceptance = ufacts(
            "BUG-499",
            "bug",
            "approved",
            &[],
            false,
            false,
            false,
            true,  // queued
            false, // no acceptance section
            true,  // implementable
        );
        assert!(
            queued_ready_no_acceptance.passes_pickability_gate(),
            "a bounded, unblocked, decision-free, untagged spec passes the gate"
        );
        assert!(
            queued_ready_no_acceptance.in_burndown_ready_set(),
            "queued + passes-gate = groomed, regardless of acceptance"
        );
        assert_eq!(
            classify_unblock(&queued_ready_no_acceptance),
            None,
            "groomed/ready specs are excluded — not flagged 'missing acceptance'"
        );
    }

    // BUG-502: the reconciliation must NOT swallow a genuinely under-specified,
    // NOT-yet-queued spec — that still needs clarifying before it can be queued.
    #[test]
    fn unblock_unqueued_missing_acceptance_still_clarifies() {
        let unqueued_no_acceptance = ufacts(
            "TASK-50",
            "task",
            "approved",
            &[],
            false,
            false,
            false,
            false, // NOT queued
            false, // no acceptance
            true,
        );
        assert!(!unqueued_no_acceptance.in_burndown_ready_set());
        assert_eq!(
            classify_unblock(&unqueued_no_acceptance),
            Some(UnblockClass::UnderSpecified)
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
            // BUG-502: a built-and-held spec must render under REVIEW.
            UnblockLine {
                id: "STORY-543".to_string(),
                class: UnblockClass::HeldForReview,
                reason: unblock_reason(UnblockClass::HeldForReview).to_string(),
            },
        ];
        let prompt = assemble_unblock_prompt(&lines);
        assert!(prompt.contains("QUEUE"));
        assert!(prompt.contains("CLARIFY FIRST"));
        assert!(prompt.contains("REVIEW"));
        assert!(prompt.contains("LEAVE PARKED"));
        // Each spec lands under its action heading, with its reason.
        assert!(prompt.contains("TASK-2 — approved"));
        assert!(prompt.contains("TASK-3 — missing acceptance"));
        assert!(prompt.contains("T-1 — blocked"));
        // BUG-502: the draft-only spec lands under REVIEW, not QUEUE/CLARIFY.
        assert!(prompt.contains("STORY-543 — built"));
        // The leading instruction names the actions, including review.
        assert!(prompt.contains("queue the autonomous-able"));
        assert!(prompt.contains("REVIEW the built-and-held"));
    }

    #[test]
    fn assemble_unblock_prompt_renders_empty_sections() {
        let prompt = assemble_unblock_prompt(&[]);
        assert!(prompt.contains("QUEUE: (none)"));
        assert!(prompt.contains("CLARIFY FIRST: (none)"));
        assert!(prompt.contains("REVIEW: (none)"));
        assert!(prompt.contains("LEAVE PARKED: (none)"));
    }

    // STORY-566: the pure bucket → advance-action routing the
    // `aida queue advance` router dispatches on. Every variant must map.
    #[test]
    fn advance_action_maps_every_bucket() {
        use OpenBucket::*;
        let cases = [
            (HeldForReview, AdvanceAction::Review),
            (BuildSupervised, AdvanceAction::SupervisedBuild),
            (AwaitingDecision, AdvanceAction::Decision),
            (Actionable, AdvanceAction::Drain),
            (Ungroomed, AdvanceAction::Approve),
            (Deferred, AdvanceAction::Reject),
            (Blocked, AdvanceAction::Reject),
            (ReadyToClose, AdvanceAction::Close),
            (LongLived, AdvanceAction::None),
            (Umbrella, AdvanceAction::None),
            (InFlight, AdvanceAction::None),
            (AwaitingMerge, AdvanceAction::None),
            (InProgress, AdvanceAction::None),
        ];
        for (bucket, want) in cases {
            assert_eq!(advance_action(bucket), want, "bucket {bucket:?}");
            // The label is non-empty for every bucket (menu safety).
            assert!(
                !advance_action_label(bucket).is_empty(),
                "bucket {bucket:?}"
            );
        }
    }

    // STORY-566: only the two unambiguous autonomous steps may be auto-taken
    // under `--yes`; everything human-required is skipped.
    #[test]
    fn advance_only_drain_and_approve_are_autonomous() {
        assert!(AdvanceAction::Drain.is_autonomous());
        assert!(AdvanceAction::Approve.is_autonomous());
        for a in [
            AdvanceAction::Review,
            AdvanceAction::SupervisedBuild,
            AdvanceAction::Decision,
            AdvanceAction::Reject,
            AdvanceAction::Close,
            AdvanceAction::None,
        ] {
            assert!(
                !a.is_autonomous(),
                "{a:?} must NOT be auto-taken under --yes"
            );
        }
    }

    /// STORY-614: the wave-builder must never fan more than one spec per
    /// `serialize:<group>` into one wave. Given 3 ready specs where 2 share
    /// `serialize:docs`, the kept set has exactly one of the two + the
    /// independent one (2 total); the held one is preserved (not lost) so it
    /// drains in a later wave.
    #[test]
    fn collapse_serialize_groups_holds_all_but_one_per_group() {
        let mut groups: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        groups.insert("TASK-1".to_string(), vec!["docs".to_string()]);
        groups.insert("TASK-2".to_string(), vec!["docs".to_string()]);
        groups.insert("TASK-3".to_string(), vec![]); // independent — no serialize tag

        let ready = vec![
            "TASK-1".to_string(),
            "TASK-2".to_string(),
            "TASK-3".to_string(),
        ];
        let (kept, held) = collapse_serialize_groups(ready, &groups);

        // Exactly one of the docs-group pair + the independent spec survive.
        assert_eq!(kept, vec!["TASK-1".to_string(), "TASK-3".to_string()]);
        // The held member is preserved, not dropped.
        assert_eq!(held, vec!["TASK-2".to_string()]);
        // Nothing is lost across the split.
        assert_eq!(kept.len() + held.len(), 3);
    }

    /// STORY-614: specs with NO serialize tag all stay parallel — the collapse
    /// is a no-op for the untagged set.
    #[test]
    fn collapse_serialize_groups_keeps_all_untagged() {
        let groups: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        let ready = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let (kept, held) = collapse_serialize_groups(ready.clone(), &groups);
        assert_eq!(kept, ready);
        assert!(held.is_empty());
    }

    /// STORY-614: distinct groups don't collide — one spec per DISTINCT group
    /// survives, not one spec overall.
    #[test]
    fn collapse_serialize_groups_independent_groups_coexist() {
        let mut groups: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        groups.insert("X".to_string(), vec!["docs".to_string()]);
        groups.insert("Y".to_string(), vec!["core".to_string()]);
        let ready = vec!["X".to_string(), "Y".to_string()];
        let (kept, held) = collapse_serialize_groups(ready, &groups);
        assert_eq!(kept, vec!["X".to_string(), "Y".to_string()]);
        assert!(held.is_empty());
    }

    /// STORY-614: the tag parser lowercases the group and strips the prefix.
    #[test]
    fn serialize_groups_parses_prefix_case_insensitive() {
        let tags = vec![
            "serialize:Docs".to_string(),
            "batch:foo".to_string(),
            "Serialize:CORE".to_string(),
            "serialize:".to_string(), // empty group — ignored
        ];
        let mut g = serialize_groups(&tags);
        g.sort();
        assert_eq!(g, vec!["core".to_string(), "docs".to_string()]);
    }
}
