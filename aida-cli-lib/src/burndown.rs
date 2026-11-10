//! STORY-527 slice 1: the pure pickability gate for `aida burndown plan`.
//!
//! `/aida-burndown` fans out worktree-isolated implementer subagents over a
//! ready set, with the main session integrating (see
//! `.aida/discipline/autonomous-burndown.md`). The non-negotiable safety
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
    /// Optional explicit requirement-type filter, pre-normalized by the caller
    /// to the same lowercased `Debug` form as `req_type` (e.g. `changerequest`,
    /// `nonfunctional`). When set it REPLACES the default work-item type-class
    /// filter, so asking for a knowledge-class type by name opts it back in.
    // trace:BUG-784 | ai:claude
    pub type_filter: Option<&'a str>,
    pub disp: &'a str,
    pub req_type: &'a str,
    pub has_unsatisfied_blocker: bool,
    pub has_pending_decision: bool,
    /// The spec's groomed execution mode, or `None` when ungroomed. A mode that
    /// needs a human in the loop (`guided`, `operator`, `decide`) buckets as
    /// `Supervised` instead of becoming a fan-out candidate.
    // trace:BUG-1717 | ai:claude
    pub execution_mode: Option<aida_core::ExecutionMode>,
}

/// Classify ONE spec for the burndown selector. The filter order is load-bearing
/// and mirrors the historical inline loop EXACTLY:
///   archived → deferred → type → status → tag → batch → supervised → candidate.
/// The `supervised` check sits AFTER the status/tag/batch filters (BUG-551), so a
/// Done/Completed supervised spec fails the status guard and is `Skip`ped before
/// it can reach the supervised bucket. The type-class filter (BUG-784) sits at
/// the FRONT, beside the other "out of scope entirely" guards, so a
/// knowledge-class record can't reach the supervised bucket either.
// trace:TASK-803 trace:BUG-551 trace:BUG-537 trace:BUG-784
pub(crate) fn classify_spec(input: &SpecClassifyInput) -> SpecDisposition {
    if input.archived {
        return SpecDisposition::Skip;
    }
    if input.deferred {
        return SpecDisposition::Skip;
    }
    // BUG-784: an accepted ADR / a vision / a term / a principle sits at
    // `Approved` just like blessed work does, so without a type-class filter the
    // selector offers knowledge-class records as implementable candidates. An
    // explicit `--type` REPLACES this default (that is how you opt one back in).
    // trace:BUG-784 | ai:claude
    match input.type_filter {
        Some(want) => {
            if !input.req_type.trim().eq_ignore_ascii_case(want.trim()) {
                return SpecDisposition::Skip;
            }
        }
        None => {
            if !aida_core::lifecycle::is_work_item_type_str(input.req_type) {
                return SpecDisposition::Skip;
            }
        }
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
    // BUG-1717: the `supervised` TAG was the only thing that could reach this
    // bucket, so a spec whose groomed execution_mode is `guided` / `operator` /
    // `decide` was handed to the drain as a fan-out candidate — the exact
    // opposite of what those modes mean. A guided keystone dispatched
    // unattended is the failure this gate exists to prevent, and it is not
    // hypothetical: STORY-1484 (heft 17, cache-refresh architecture, mode
    // guided) was listed as "Ready to fan out" while the supervised bucket
    // reported 0. Mode is authoritative here and the tag remains an
    // independent opt-in, so either one is sufficient to supervise a spec.
    // An ungroomed spec (None) keeps its previous disposition: it is not
    // supervised, it is simply not yet groomed.
    // trace:BUG-1717 | ai:claude
    if input.execution_mode.is_some_and(|m| !m.is_autonomous()) {
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

/// How the drain orders the unblocked ready set before a wave is fanned.
///
/// [`ReadyOrder::Priority`] (the default) fans high-priority work first;
/// [`ReadyOrder::Queue`] is the opt-out for operators who want the set drained
/// in the order they queued it. Both are anchored on the same substrate fact —
/// each spec's queue-entry `added_at` timestamp — so ordering is a real,
/// observable property of the queue rather than an artifact of spec-id spelling.
// trace:TASK-1172 | ai:claude
// trace:TASK-1175 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ReadyOrder {
    /// High before medium before low, queue-insertion order as the tiebreak.
    #[default]
    Priority,
    /// Queue-insertion order — oldest `added_at` first, no priority reordering.
    Queue,
}

/// Parse a `[burndown] order` config value. Unknown values return `None` so the
/// caller falls through to the default rather than failing a drain over a typo.
///
/// `input` is a silently-accepted back-compat alias for `queue`: a machine that
/// set it before TASK-1175 keeps working, and it is now truthful — the set IS
/// fanned in the order the specs went in.
// trace:TASK-1172 | ai:claude
// trace:TASK-1175 | ai:claude
pub(crate) fn parse_ready_order(raw: &str) -> Option<ReadyOrder> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "priority" => Some(ReadyOrder::Priority),
        "queue" | "input" => Some(ReadyOrder::Queue),
        _ => None,
    }
}

/// Sort key for one priority label — lower sorts (drains) first. An absent or
/// unrecognized label ranks with `medium`, so an odd value neither jumps the
/// wave nor sinks below explicitly low-priority work.
// trace:TASK-1172 | ai:claude
fn priority_rank(priority: &str) -> u8 {
    match priority.trim().to_ascii_lowercase().as_str() {
        "high" => 0,
        "low" => 2,
        _ => 1,
    }
}

/// Insertion-order sort key for one ready spec: its queue-entry `added_at`.
///
/// A ready spec is by construction a queued spec (queue membership IS the
/// advisor sign-off, STORY-546), so the timestamp is normally present. A spec
/// whose entry we could not read sorts LAST rather than jumping the queue with
/// an invented "epoch" position, and the display id is the final tiebreak so
/// two entries stamped in the same millisecond still order deterministically.
// trace:TASK-1175 | ai:claude
fn insertion_key<'a>(
    id: &'a str,
    added_at_by_id: &std::collections::HashMap<String, chrono::DateTime<chrono::Utc>>,
) -> (bool, Option<chrono::DateTime<chrono::Utc>>, &'a str) {
    let at = added_at_by_id.get(id).copied();
    (at.is_none(), at, id)
}

/// Order the ALREADY-COMPUTED ready set. Deliberately a pure, total sort over
/// display ids so it composes with the machinery on either side of it rather
/// than duplicating any of it:
///   - dependencies still gate — a blocked spec never reached this set (it was
///     parked by [`classify`] upstream), so no priority can promote it;
///   - the `serialize:<group>` collapse still sequences same-file members —
///     it runs downstream over this reordered set, so the group claim simply
///     goes to the winning member under whichever order is in force;
///   - the sort is total and deterministic on its own inputs, so the wave is
///     reproducible across runs without the caller pre-sorting anything.
///
/// [`ReadyOrder::Queue`] sorts purely by queue-insertion time (`added_at`
/// ascending). [`ReadyOrder::Priority`] (the default) sorts by priority band
/// first and breaks ties by that same insertion time — so equal-priority specs
/// drain in the order they were queued, NOT in lexical spec-id order (which is
/// an accident of spelling, not a fact about the work).
// trace:TASK-1172 | ai:claude
// trace:TASK-1175 | ai:claude
pub(crate) fn sort_ready(
    ready: Vec<String>,
    priority_by_id: &std::collections::HashMap<String, String>,
    added_at_by_id: &std::collections::HashMap<String, chrono::DateTime<chrono::Utc>>,
    order: ReadyOrder,
) -> Vec<String> {
    let mut out = ready;
    match order {
        ReadyOrder::Queue => {
            out.sort_by(|a, b| {
                insertion_key(a, added_at_by_id).cmp(&insertion_key(b, added_at_by_id))
            });
        }
        ReadyOrder::Priority => {
            out.sort_by(|a, b| {
                let rank = |id: &String| {
                    priority_rank(
                        priority_by_id
                            .get(id)
                            .map(String::as_str)
                            .unwrap_or("medium"),
                    )
                };
                rank(a).cmp(&rank(b)).then_with(|| {
                    insertion_key(a, added_at_by_id).cmp(&insertion_key(b, added_at_by_id))
                })
            });
        }
    }
    out
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

/// A ready spec held out of THIS wave because a sibling already claimed its
/// `serialize:<group>`. Distinct from "awaiting advisor sign-off": the spec IS
/// queued and blessed — it simply drains in a later wave. Carries the claiming
/// spec + group so the reader can see WHY it is held rather than guessing that
/// a human gate is missing.
// trace:TASK-149 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SerializeHold {
    /// The held spec's display id.
    pub(crate) spec: String,
    /// The spec that claimed the group this wave.
    pub(crate) held_by: String,
    /// The serialize group name (lowercased, `serialize:` prefix stripped).
    pub(crate) group: String,
}

/// STORY-614: substrate-enforce the `serialize:<group>` convention. Given the
/// READY fan-out set (display ids, in deterministic order) and a lookup from
/// display id → its serialize groups, collapse the set so that AT MOST ONE spec
/// per distinct group survives in this wave. The held-back members are returned
/// separately (NOT dropped) — they drain in successive waves.
///
/// TASK-149: each held member is returned as a [`SerializeHold`] naming the
/// claiming spec + group, so the caller can report it as its OWN state
/// ("queued, blessed, drains next wave") instead of folding it into the
/// awaiting-sign-off bucket, which reads as "a human still has to bless this".
///
/// Pick is deterministic: the FIRST id (in the supplied order) claims each
/// group; later members sharing any already-claimed group are held. The caller
/// supplies `ready` sorted (lowest id first) so the pick is stable across runs.
/// Specs with no serialize tag are never held — they all stay parallel. Pure +
/// order-preserving so the gate is unit-testable without a live store.
/// trace:STORY-614 | ai:claude
// trace:TASK-149 | ai:claude
pub(crate) fn collapse_serialize_groups(
    ready: Vec<String>,
    groups_by_id: &std::collections::HashMap<String, Vec<String>>,
) -> (Vec<String>, Vec<SerializeHold>) {
    let mut kept = Vec::new();
    let mut held = Vec::new();
    // group → the spec that claimed it this wave (so a hold can name its claimer).
    let mut claimed: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for id in ready {
        let groups = groups_by_id.get(&id).cloned().unwrap_or_default();
        // A spec is held if ANY of its groups is already claimed this wave; the
        // FIRST such group (in the spec's own tag order) names the hold.
        match groups
            .iter()
            .find_map(|g| claimed.get(g).map(|owner| (g.clone(), owner.clone())))
        {
            Some((group, held_by)) => held.push(SerializeHold {
                spec: id,
                held_by,
                group,
            }),
            None => {
                for g in groups {
                    claimed.insert(g, id.clone());
                }
                kept.push(id);
            }
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
    /// hierarchy subtree (the same numbers `aida graph tree` prints). `None`
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
    // `get(..n)` instead of `split_at(n)`: the prefix length is a BYTE count, and
    // a comment whose 9th byte falls INSIDE a multi-byte character made
    // `split_at` panic — "byte index 9 is not a char boundary". A real comment
    // did it: `SKETCH — awaiting independent advisor signoff` is 7 ASCII bytes
    // then an em-dash spanning bytes 7..10. That took down `aida human` and the
    // burndown's open-facts collection for the whole project, because one spec
    // carried one em-dash in one comment. `get` returns None on a non-boundary,
    // which is also the correct answer: a string that cannot be split there
    // cannot start with an ASCII prefix.
    // trace:BUG-1715 | ai:claude
    let Some(head) = trimmed.get(..WHY_OPEN_PREFIX.len()) else {
        return None;
    };
    let rest = &trimmed[WHY_OPEN_PREFIX.len()..];
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
                "an epic whose children are all still un-started — drive them forward or refine the breakdown (`aida graph tree <id>`, then groom/queue the children)".to_string(),
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
             then dispose — `aida edit <id> --status approved|rejected`, `aida defer <id> --until <condition>`, or refine the spec"
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
    /// The spec's title (for the STORY-750 interactive walk).
    pub title: String,
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
    /// The spec's title — shown in the interactive walk so the operator can
    /// decide without a separate lookup (STORY-750).
    pub title: String,
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

/// STORY-750: one resolution the interactive sweep (`aida human unblock
/// --interactive`) can offer for a hurdle. The per-hurdle MENU and the walk
/// ORDER are pure here so they're exhaustively unit-testable; the impure half
/// (the `inquire::Select` prompt + the self-invoked `aida` verb that performs
/// the write) lives in `main.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SweepChoice {
    /// Draft → approve it and queue it into the burndown.
    ApproveQueue,
    /// Draft → reject it.
    Reject,
    /// Approved-but-unqueued → queue it.
    Queue,
    /// Under-specified → open the spec to add acceptance.
    Clarify,
    /// Decision-pending / needs-design → drive the supervised guided dialog.
    LaunchGuided,
    /// Decision-pending → record a decision note without leaving the sweep.
    AddNote,
    /// Held-for-review → surface the review command.
    ShowReview,
    /// Blocked-by → surface what's blocking it.
    ShowBlocker,
    /// Print the spec's full detail (description + acceptance), then re-prompt.
    ShowDetails,
    /// Hand this one to the advisor instead of deciding it yourself — it joins
    /// the advisor-delegate prompt emitted at the end of the sweep.
    AskAdvisor,
    /// Leave this one parked and move on.
    Skip,
}

impl SweepChoice {
    /// The label shown in the `inquire::Select`.
    pub(crate) fn label(self) -> &'static str {
        match self {
            SweepChoice::ApproveQueue => "Approve & queue",
            SweepChoice::Reject => "Reject",
            SweepChoice::Queue => "Queue it into the burndown",
            SweepChoice::Clarify => "Clarify — open the spec to add acceptance",
            SweepChoice::LaunchGuided => "Resolve the design now (guided dialog)",
            SweepChoice::AddNote => "Add a decision note (leave parked)",
            SweepChoice::ShowReview => "Show the review command",
            SweepChoice::ShowBlocker => "Show what's blocking it",
            SweepChoice::ShowDetails => "Show details (description + acceptance)",
            SweepChoice::AskAdvisor => "Ask the advisor to handle it",
            SweepChoice::Skip => "Skip — leave it for now",
        }
    }
}

/// STORY-750: the ordered menu of resolutions the sweep offers for one hurdle
/// class. `Skip` is ALWAYS last. Pure.
pub(crate) fn sweep_menu(class: UnblockClass) -> Vec<SweepChoice> {
    // Every actionable hurdle ends with the universal tail: see the detail, hand
    // it to the advisor, or skip. `with_tail` keeps `Skip` last.
    let with_tail = |mut v: Vec<SweepChoice>| {
        v.push(SweepChoice::ShowDetails);
        v.push(SweepChoice::AskAdvisor);
        v.push(SweepChoice::Skip);
        v
    };
    match class {
        UnblockClass::NeedsApproval => {
            with_tail(vec![SweepChoice::ApproveQueue, SweepChoice::Reject])
        }
        UnblockClass::ApprovedUnqueued => with_tail(vec![SweepChoice::Queue]),
        UnblockClass::UnderSpecified => with_tail(vec![SweepChoice::Clarify]),
        UnblockClass::DecisionPending => {
            with_tail(vec![SweepChoice::LaunchGuided, SweepChoice::AddNote])
        }
        UnblockClass::HeldForReview => with_tail(vec![SweepChoice::ShowReview]),
        UnblockClass::BlockedBy => with_tail(vec![SweepChoice::ShowBlocker]),
        // Parked-by-choice: nothing to resolve inline — the sweep lists these as
        // informational rows rather than prompting.
        UnblockClass::BuildSupervised | UnblockClass::Deferred => vec![SweepChoice::Skip],
    }
}

/// STORY-750: is this hurdle ADVISOR grooming (draft approval / queueing) rather
/// than a genuinely-human decision? These are bulk-delegated to the advisor at
/// the top of the sweep instead of dumped on the human one at a time. Pure.
pub(crate) fn sweep_is_advisor_groomable(class: UnblockClass) -> bool {
    matches!(
        class,
        UnblockClass::NeedsApproval | UnblockClass::ApprovedUnqueued
    )
}

/// STORY-750: walk-order rank for the interactive sweep — CHEAPEST-first, so the
/// operator clears the one-keystroke approvals/queues before reaching the design
/// forks that need real thought. Lower sorts earlier. Pure.
pub(crate) fn sweep_cost_rank(class: UnblockClass) -> u8 {
    match class {
        UnblockClass::NeedsApproval => 0,
        UnblockClass::ApprovedUnqueued => 1,
        UnblockClass::BlockedBy => 2,
        UnblockClass::HeldForReview => 3,
        UnblockClass::UnderSpecified => 4,
        UnblockClass::DecisionPending => 5,
        UnblockClass::BuildSupervised => 6,
        UnblockClass::Deferred => 7,
    }
}

/// STORY-750: does the sweep offer a real inline resolution for this class, or is
/// it an informational (leave-parked) row it just lists?
pub(crate) fn sweep_is_actionable(class: UnblockClass) -> bool {
    !matches!(
        class,
        UnblockClass::BuildSupervised | UnblockClass::Deferred
    )
}

/// STORY-750: sort classified lines into the interactive walk order
/// (cheapest-first, stable within a rank so store order is preserved). Pure.
// trace:STORY-750 | ai:claude
pub(crate) fn sweep_walk_order(mut lines: Vec<UnblockLine>) -> Vec<UnblockLine> {
    lines.sort_by_key(|l| sweep_cost_rank(l.class));
    lines
}

#[cfg(test)]
#[path = "tests/burndown_story_750_sweep_tests.rs"]
mod story_750_sweep_tests;

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

/// BUG-772: render a blocker list for the footer — at most three ids, then a
/// `+N more` tail so a long chain can't run the line off the terminal.
// trace:BUG-772 | ai:claude
fn join_blockers(blockers: &[String]) -> String {
    const SHOWN: usize = 3;
    let head = blockers
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if blockers.len() > SHOWN {
        format!("{head} +{} more", blockers.len() - SHOWN)
    } else {
        head
    }
}

/// BUG-772: the next action for a BLOCKED queued item, keyed off what its
/// predecessors actually are. The old routing sent every blocked spec to
/// `AdvanceAction::Reject` — "resolve it out (`aida edit <id> --status
/// rejected`)" — which for a spec waiting on a live predecessor advised
/// destroying authorized chained work. Only a genuine dead end (Rejected /
/// dangling predecessor) is the operator's call now, and even then the verb is
/// unblock-or-re-scope, not reject. The `Clears` arm is unreachable from the
/// "needs you" loop (those items are filtered out before it) but is rendered
/// truthfully rather than panicking. Pure.
// trace:BUG-772 | ai:claude
fn blocked_action_sentence(outlook: &BlockerOutlook, id: &str) -> String {
    let list = join_blockers(&outlook.blockers);
    match outlook.kind {
        BlockerOutlookKind::DeadEnd => format!(
            "its blocker {list} will never ship — unblock or re-scope it \
             (`aida graph blocked-by {id}`)"
        ),
        BlockerOutlookKind::PredecessorParked => {
            // `aida why` takes ONE spec — point at the first blocker, not the
            // whole list (a multi-id command line would not run).
            let first = outlook.blockers.first().cloned().unwrap_or_default();
            format!(
                "its blocker {list} is not cleared to start — groom or triage the \
                 blocker first (`aida why {first}`)"
            )
        }
        BlockerOutlookKind::Clears => format!(
            "waiting on {list} — authorized work already moving; it unblocks when that ships"
        ),
    }
}

// BUG-772: one unsatisfied `BlockedBy` predecessor, as the footer sees it —
// the predecessor's display id plus its status. `status: None` means the edge
// is DANGLING (the target is no longer in the store). The caller resolves the
// edges; this stays plain data so the outlook classifier is pure.
// trace:BUG-772 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockerFact {
    /// The predecessor's display SPEC-ID (or a placeholder for a dangling edge).
    pub id: String,
    /// The predecessor's status, in any casing/spacing (`In Progress`,
    /// `inprogress`, …) — normalized by [`classify_blockers`]. `None` for a
    /// dangling edge.
    pub status: Option<String>,
}

// BUG-772: will a blocked spec's dependency clear on its own, or does it need a
// person? The queue footer used to treat EVERY blocked spec as "needs you:
// resolve it out (`--status rejected`)" — which, for a spec waiting on an
// authorized predecessor the same drain is already shipping, advises destroying
// live work. trace:BUG-772 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockerOutlookKind {
    /// Every unsatisfied predecessor is authorized, moving work (Approved /
    /// Planned / In Progress / Done). The block clears itself as the chain
    /// drains — NOT an operator action.
    Clears,
    /// A predecessor is not yet cleared to start (Draft) or is parked for
    /// triage (NeedsAttention). It needs a person, but the action is on the
    /// PREDECESSOR, not on the dependent.
    PredecessorParked,
    /// A predecessor is Rejected, or the edge dangles (target gone) — a true
    /// dead end. The dependent needs unblocking or re-scoping.
    DeadEnd,
}

/// BUG-772: the footer's verdict on a blocked spec's predecessors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockerOutlook {
    pub kind: BlockerOutlookKind,
    /// The predecessors that decided the kind, in store order.
    pub blockers: Vec<String>,
}

/// BUG-772: statuses (normalized) of a predecessor that is authorized work
/// already moving toward its own completion. `completed` is included
/// defensively — a Completed predecessor is a satisfied edge and should never
/// reach here, but if it does it must not read as a dead end.
const SELF_CLEARING_BLOCKER_STATUSES: &[&str] =
    &["approved", "planned", "inprogress", "done", "completed"];

/// BUG-772: classify a blocked spec's unsatisfied predecessors into the ONE
/// outlook that decides whether the footer may say "needs you". Precedence runs
/// loudest-first: a dead end (Rejected / dangling) outranks a parked predecessor
/// (Draft / NeedsAttention), which outranks the self-clearing case. `None` for
/// an empty slice (a spec whose Blocked classification came from the `blocked`
/// TAG rather than a graph edge — the caller keeps the legacy phrasing there).
/// Pure + exhaustively testable.
// trace:BUG-772 | ai:claude
pub(crate) fn classify_blockers(blockers: &[BlockerFact]) -> Option<BlockerOutlook> {
    if blockers.is_empty() {
        return None;
    }
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase()
    };
    let dead: Vec<String> = blockers
        .iter()
        .filter(|b| {
            b.status
                .as_deref()
                .map(&norm)
                .is_none_or(|s| s == "rejected")
        })
        .map(|b| b.id.clone())
        .collect();
    if !dead.is_empty() {
        return Some(BlockerOutlook {
            kind: BlockerOutlookKind::DeadEnd,
            blockers: dead,
        });
    }
    let parked: Vec<String> = blockers
        .iter()
        .filter(|b| {
            b.status
                .as_deref()
                .map(&norm)
                .is_none_or(|s| !SELF_CLEARING_BLOCKER_STATUSES.contains(&s.as_str()))
        })
        .map(|b| b.id.clone())
        .collect();
    if !parked.is_empty() {
        return Some(BlockerOutlook {
            kind: BlockerOutlookKind::PredecessorParked,
            blockers: parked,
        });
    }
    Some(BlockerOutlook {
        kind: BlockerOutlookKind::Clears,
        blockers: blockers.iter().map(|b| b.id.clone()).collect(),
    })
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
    /// BUG-772: for a `Blocked` item, what its predecessors imply — whether the
    /// block clears itself as the chain drains or genuinely needs a person.
    /// `None` for every other bucket (and for a spec blocked by the `blocked`
    /// TAG with no graph edge).
    // trace:BUG-772 | ai:claude
    pub blocker: Option<BlockerOutlook>,
}

impl QueuedItem {
    /// A queued item with no blocker detail — the common (non-`Blocked`) case.
    // trace:BUG-772 | ai:claude
    pub(crate) fn new(id: impl Into<String>, bucket: OpenBucket) -> Self {
        Self {
            id: id.into(),
            bucket,
            blocker: None,
        }
    }

    /// Attach the blocked-item outlook.
    // trace:BUG-772 | ai:claude
    pub(crate) fn with_blocker(mut self, blocker: Option<BlockerOutlook>) -> Self {
        self.blocker = blocker;
        self
    }
}

// BUG-753: what the LIVE drain already covers, so the footer can stop
// recommending `aida burndown run` for specs the running drain has scheduled —
// a launch the single-drain lock would refuse anyway. Built by the caller from
// the same overlay that renders the "a drain is running" banner; kept as plain
// data so the footer renderer stays pure + unit-testable.
// trace:BUG-753 | ai:claude
#[derive(Debug, Clone, Default)]
pub(crate) struct DrainFooterOverlay {
    /// UPPERCASE display SPEC-IDs the live drain has scheduled (claimed from
    /// the queue but not yet picked up).
    pub scheduled: std::collections::HashSet<String>,
    /// UPPERCASE display SPEC-IDs a live session lease is actively working.
    /// BUG-765: ids (not just a count) so the per-item stalled hint can check
    /// drain membership with the same data the banner renders from.
    pub in_flight: std::collections::HashSet<String>,
}

/// STORY-565: render the "how do I get to zero?" footer for a non-empty queue —
/// SIGNPOSTING over the SAME classifier `aida queue advance` uses, not new
/// state. Disambiguates the two meanings of "empty": DRAIN (`aida burndown run`)
/// does the work, CLEAR (`aida queue clear`) just drops queue membership — and
/// names the single next action for each non-ready (parked/blocked/held) item.
/// BUG-753: drain-aware — when a live drain is already working this queue
/// (`drain` is `Some`), the footer names the drain and offers the watch verb
/// instead of recommending a launch the single-drain lock would refuse.
/// Pure (no color, no store) so it's unit-testable without a store; the caller
/// colorizes. Returns `None` for an empty slice (caller prints the empty-queue
/// line instead).
// trace:STORY-565 trace:BUG-753 | ai:claude
pub(crate) fn render_path_to_empty(
    items: &[QueuedItem],
    drain: Option<&DrainFooterOverlay>,
) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let ready_ids: Vec<&str> = items
        .iter()
        .filter(|i| i.bucket == OpenBucket::Actionable)
        .map(|i| i.id.as_str())
        .collect();
    let ready = ready_ids.len();

    // trace:TASK-840 | ai:claude
    let bullet = crate::glyphs::get(
        crate::glyphs::Glyph::Bullet,
        crate::find_project_root().ok().as_deref(),
    );
    let mut out = String::from("To empty this queue:");
    if ready > 0 {
        match drain {
            // BUG-753: a drain is live — don't recommend `aida burndown run`
            // (the single-drain lock would refuse it) and don't count the
            // drain-scheduled specs as ready-to-drain. Name the drain, offer
            // the watch verb. trace:BUG-753 | ai:claude
            Some(d) => {
                let covered = ready_ids
                    .iter()
                    .filter(|id| d.scheduled.contains(&id.to_ascii_uppercase()))
                    .count();
                let uncovered = ready - covered;
                if covered > 0 {
                    out.push_str(&format!(
                        "\n  {bullet} a drain is already working this queue — {covered} \
                         scheduled, {} in flight; watch it: `aida drain status`",
                        d.in_flight.len()
                    ));
                }
                if uncovered > 0 {
                    out.push_str(&format!(
                        "\n  {bullet} {uncovered} ready    → waits for the running drain \
                         (it holds the drain lock)  ·  `aida queue clear` (just drops them)"
                    ));
                }
            }
            None => {
                out.push_str(&format!(
                    "\n  {bullet} {ready} ready    → `aida burndown run` (does the work)  ·  \
                     `aida queue clear` (just drops them)"
                ));
            }
        }
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
    // BUG-772: a spec blocked by a predecessor that is ITSELF authorized work
    // already moving (queued/in-flight/Approved) is not an operator action —
    // the block clears as the chain drains. Telling the operator to "resolve it
    // out (`--status rejected`)" there advises destroying authorized work.
    // trace:BUG-772 | ai:claude
    let clears_itself = |i: &QueuedItem| {
        i.bucket == OpenBucket::Blocked
            && i.blocker
                .as_ref()
                .is_some_and(|b| b.kind == BlockerOutlookKind::Clears)
    };
    for item in items.iter().filter(|i| {
        i.bucket != OpenBucket::Actionable
            && !self_resolving(i.bucket)
            && !is_drift(i.bucket)
            && !clears_itself(i)
    }) {
        // BUG-772: a genuinely-stuck blocked spec gets a blocker-aware sentence
        // — unblock or re-scope, never a bare reject.
        let sentence = match (item.bucket, item.blocker.as_ref()) {
            (OpenBucket::Blocked, Some(outlook)) => blocked_action_sentence(outlook, &item.id),
            _ => advance_action_sentence(item.bucket, &item.id),
        };
        out.push_str(&format!(
            "\n  {bullet} {:<10} needs you: {sentence}",
            item.id,
        ));
    }
    // BUG-772: the self-clearing chain — visible (so the operator knows why it
    // is sitting there) but explicitly NOT an action for them.
    // trace:BUG-772 | ai:claude
    for item in items.iter().filter(|i| clears_itself(i)) {
        let blockers = item
            .blocker
            .as_ref()
            .map(|b| join_blockers(&b.blockers))
            .unwrap_or_default();
        out.push_str(&format!(
            "\n  {bullet} {:<10} waiting on {blockers} — authorized work already moving; \
             nothing for you to do, it unblocks when that ships",
            item.id,
        ));
    }
    // BUG-765: the live drain's own specs must never read as stalled — the
    // banner above just reported them in-flight/scheduled, and following the
    // pick-it-up hint would double-dispatch a spec mid-flight. Same
    // drain-lock-derived overlay the banner renders from, so the two surfaces
    // can't contradict each other.
    // trace:BUG-765 | ai:claude
    let drain_covered = |id: &str| {
        drain.is_some_and(|d| {
            let upper = id.to_ascii_uppercase();
            d.in_flight.contains(&upper) || d.scheduled.contains(&upper)
        })
    };
    // Group 3: in-progress-without-lease drift — flag, but never as "needs you:
    // resolves through normal flow". These will NOT self-resolve. Drain-covered
    // drift is NOT drift (the drain owns it); it joins Group 2 below.
    for item in items
        .iter()
        .filter(|i| is_drift(i.bucket) && !drain_covered(&i.id))
    {
        out.push_str(&format!(
            "\n  {bullet} {:<10} in progress, but no live session lease — stalled; \
             pick it back up (`aida queue work {}`) or park it",
            item.id, item.id
        ));
    }
    // Group 2: genuinely in flight / self-resolving — no operator action.
    // Includes drain-covered drift (BUG-765): the drain is working it, so it
    // clears through normal flow like any other in-flight spec.
    let self_resolving: Vec<&str> = items
        .iter()
        .filter(|i| self_resolving(i.bucket) || (is_drift(i.bucket) && drain_covered(&i.id)))
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
#[path = "tests/burndown_tests.rs"]
mod tests;

// ── BUG-755: the drain process must not exit with residual work ──
//
// Observed 2026-07-18: a `burndown run` headless session implemented wave 1,
// printed "awaiting CI, each with a background watch. I'll merge them as their
// checks go terminal." — and ended its final turn. A headless `claude -p`
// terminates at turn end, so those "background watches" died with the session:
// three PRs sat open unwatched and four blessed specs were never started.
//
// The structural fix lives in the LAUNCHER (`handle_burndown_run`), which is
// the long-lived process holding the drain lock: after the headless session
// exits, it probes for residual work — open wave PRs (non-terminal) and
// blessed specs still unstarted — and, absent a deliberate shelve/cap stop,
// RELAUNCHES a bounded continuation turn instead of exiting. When it must exit
// early (shelve stop, or the resume budget is spent), it prints an accurate
// handoff naming every open PR and unstarted spec — never a claim of live
// watches. The pure decision/matching/rendering halves are here so they are
// exhaustively unit-testable. trace:BUG-755 | ai:claude

/// One open PR matched to a spec of the drain's blessed set — a wave PR that
/// has not merged yet.
// trace:BUG-755 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResidualPr {
    pub number: u64,
    pub spec: String,
    pub title: String,
    /// The PR's head branch — the key the launcher's Rust CI wait
    /// ([`crate::wait_for_ci_terminal`]) probes on.
    // trace:TASK-1169 | ai:claude
    pub branch: String,
}

/// The residual work a drain left behind when its headless session exited:
/// wave PRs still open (non-terminal) and blessed specs never started.
// trace:BUG-755 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ResidualWork {
    pub unmerged_prs: Vec<ResidualPr>,
    pub unstarted: Vec<String>,
}

impl ResidualWork {
    pub(crate) fn is_empty(&self) -> bool {
        self.unmerged_prs.is_empty() && self.unstarted.is_empty()
    }
}

/// Already-probed store facts about one blessed spec, used to EXCLUDE its open
/// PR from the residual set when the drain deliberately held/parked it:
/// `review:draft-only` (held for the operator), NeedsAttention (shelved), or a
/// terminal status. `status_norm` is alphanumeric-lowercase.
// trace:BUG-755 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct WaveSpecFacts {
    pub status_norm: String,
    pub tags: Vec<String>,
    /// TASK-1169: the spec's `execution_mode` makes its merge supervised
    /// (BUG-727 — anything but `drain`, or unset). Its PR is deliberately left
    /// open for a human, so it is NOT stranded work: without this, the
    /// launcher would relaunch agent turns for a PR that every merge gate is
    /// designed to refuse.
    // trace:TASK-1169 | ai:claude
    pub supervised: bool,
}

/// Does `haystack` contain `spec` (e.g. `BUG-723`) as a whole token —
/// case-insensitive, and NOT followed by another digit? The digit guard keeps
/// `TASK-11` from matching a `task-1117-…` branch.
// trace:BUG-755 | ai:claude
pub(crate) fn contains_spec_token(haystack: &str, spec: &str) -> bool {
    let hay = haystack.to_ascii_lowercase();
    let needle = spec.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return false;
    }
    let mut start = 0;
    while let Some(pos) = hay[start..].find(&needle) {
        let at = start + pos;
        let end = at + needle.len();
        let next_is_digit = hay[end..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit());
        if !next_is_digit {
            return true;
        }
        start = end;
    }
    false
}

/// Match the open PRs against the drain's blessed set → the unmerged wave PRs.
/// `open_prs` is `(number, title, head_branch)`. A PR belongs to the wave when
/// its head branch or title carries a blessed spec id as a token. A matched PR
/// is EXCLUDED when the spec's facts say the drain deliberately held or parked
/// it (`review:draft-only`, NeedsAttention, or a terminal status) — those are
/// legitimate early exits, not stranded work; missing facts keep the PR
/// (conservative: stranded-visible beats silently-orphaned). Sorted by PR
/// number for stable output.
// trace:BUG-755 | ai:claude
pub(crate) fn match_wave_prs(
    blessed: &[String],
    open_prs: &[(u64, String, String)],
    facts_by_id: &std::collections::HashMap<String, WaveSpecFacts>,
) -> Vec<ResidualPr> {
    let deliberately_held = |spec: &str| -> bool {
        facts_by_id.get(spec).is_some_and(|f| {
            f.tags
                .iter()
                .any(|t| t.trim().eq_ignore_ascii_case("review:draft-only"))
                // TASK-1169: a supervised-merge spec (BUG-727) is held for a
                // human by design — never stranded work.
                || f.supervised
                || matches!(
                    f.status_norm.as_str(),
                    "needsattention" | "completed" | "rejected"
                )
        })
    };
    let mut out: Vec<ResidualPr> = Vec::new();
    for (number, title, head_branch) in open_prs {
        let matched = blessed.iter().find(|spec| {
            contains_spec_token(head_branch, spec) || contains_spec_token(title, spec)
        });
        if let Some(spec) = matched {
            if deliberately_held(spec) {
                continue;
            }
            out.push(ResidualPr {
                number: *number,
                spec: spec.clone(),
                title: title.clone(),
                branch: head_branch.clone(),
            });
        }
    }
    out.sort_by_key(|pr| pr.number);
    out
}

/// The launcher's verdict after one headless drain turn exits.
// trace:BUG-755 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrainFollowup {
    /// No residual work — the drain genuinely finished; exit cleanly.
    Complete,
    /// Residual work + a clean exit + resume budget left — the session ended
    /// its turn early (the "background watch" failure mode); relaunch a
    /// continuation turn instead of exiting.
    Resume,
    /// Residual work, but either the drain deliberately stopped (non-zero =
    /// the shelve/cap exit-2 convention) or the resume budget is spent — exit,
    /// after printing an ACCURATE handoff (never a claim of live watches).
    Handoff,
}

/// Decide the follow-up. A non-zero drain exit is a deliberate stop (shelved
/// work / cap) — honored, never relaunched, but still handed off accurately
/// when residual work remains. A zero exit with residual work is the BUG-755
/// premature turn end: resume while budget remains.
// trace:BUG-755 | ai:claude
pub(crate) fn drain_followup(
    drain_exit_code: i32,
    residual_empty: bool,
    rounds_used: usize,
    max_rounds: usize,
) -> DrainFollowup {
    if residual_empty {
        return DrainFollowup::Complete;
    }
    if drain_exit_code != 0 {
        return DrainFollowup::Handoff;
    }
    if rounds_used < max_rounds {
        DrainFollowup::Resume
    } else {
        DrainFollowup::Handoff
    }
}

/// Build the continuation prompt for a resume round: the same skill invocation
/// (so every gate/guardrail re-applies) plus explicit resume context — the
/// open wave PRs to integrate in the FOREGROUND and the blessed specs still
/// unstarted. Pure + unit-testable.
// trace:BUG-755 | ai:claude
pub(crate) fn continuation_prompt(base_prompt: &str, residual: &ResidualWork) -> String {
    let mut s = String::from(base_prompt);
    s.push_str(
        "\n\nRESUME CONTEXT — the launcher relaunched this drain because the previous \
         session ended its turn with unfinished work. Background watches DIE when a \
         headless session ends: never promise one, and never end your final turn while \
         a wave PR is non-terminal or blessed specs are unstarted.",
    );
    if !residual.unmerged_prs.is_empty() {
        s.push_str("\n\nOpen wave PRs awaiting integration (you are the integrator):");
        for pr in &residual.unmerged_prs {
            s.push_str(&format!("\n- #{} ({}) {}", pr.number, pr.spec, pr.title));
        }
        // No --delete-branch: the implementer worktrees still hold these PR
        // branches, so the delete's local-cleanup step fails and an `&&`
        // chain would drop the auto-bump pull. Worktree pruning owns branch
        // deletion. trace:BUG-758 | ai:claude
        s.push_str(
            "\nFor each: poll `gh pr checks <n>` in the FOREGROUND until every check is \
             terminal, then merge green ones (`--squash` only — no `--delete-branch`, the \
             implementer worktree still holds the branch) and run `aida pull` as its own \
             step so the Done→Completed auto-bump fires even if merge cleanup grumbles; \
             hold `review:draft-only` PRs; park a red one per the skill. Do not fan out a \
             new implementer for a spec that already has an open PR.",
        );
    }
    if !residual.unstarted.is_empty() {
        s.push_str(&format!(
            "\n\nBlessed specs not yet started: {}. After integrating the PRs above, \
             re-run `aida burndown plan` and continue the wave loop over them until the \
             ready set is empty.",
            residual.unstarted.join(", ")
        ));
    }
    s
}

/// Render the accurate-handoff report printed when the launcher exits with
/// residual work (deliberate stop or resume budget spent). Names every open PR
/// and unstarted spec plus the manual pickup verbs — the truthful opposite of
/// "background watches will merge these".
// trace:BUG-755 | ai:claude
pub(crate) fn render_drain_handoff(residual: &ResidualWork) -> String {
    let mut s = String::new();
    s.push_str("\nDrain handoff — work left when the drain exited (no watches are running):\n");
    if !residual.unmerged_prs.is_empty() {
        s.push_str("  Open wave PRs (unmerged — CI may still be running):\n");
        for pr in &residual.unmerged_prs {
            s.push_str(&format!("    #{} ({}) {}\n", pr.number, pr.spec, pr.title));
        }
        s.push_str("    → integrate them: `aida integrate`, or merge by hand once CI is green\n");
    }
    if !residual.unstarted.is_empty() {
        s.push_str(&format!(
            "  Blessed specs never started: {}\n    → re-run `aida burndown run` to drain them\n",
            residual.unstarted.join(", ")
        ));
    }
    s
}

// ── TASK-1169: launcher-owned integration waits ─────────────────────────────
//
// BUG-755 made the launcher RELAUNCH a continuation turn when a session exited
// with residual work. That is recovery, not architecture: the CI/merge wait
// still lived inside an agent turn, where it is structurally unable to hold.
// A headless turn reaps its background tasks at turn end (the harness
// background-wait ceiling), and the foreground alternative is capped at ~10
// minutes per tool call — so neither agent-side shape can hold a 30-minute
// cross-platform CI run.
//
// ADR-21: the wait belongs to the LAUNCHER — the long-lived Rust process that
// already holds the drain lock. It blocks on `wait_for_ci_terminal` (TASK-968's
// re-arming idle timer + absolute ceiling), merges the green-and-clean PRs
// itself, and only relaunches an agent turn for work that genuinely needs an
// LLM (unstarted specs, conflicts, red CI). No harness ceiling applies to a
// Rust process, and waiting costs zero tokens.
//
// ADR-22: the ceiling the launcher sets on its children is bounded + generous +
// never zero, and hitting any bound PARKS the spec (EPIC-28 punt-and-continue)
// instead of silently terminating it. The pure halves — the ceiling resolver
// and the per-PR action gate — live here so both are exhaustively unit-testable
// without a forge, a store, or a 30-second poll. trace:TASK-1169 | ai:claude

/// The harness env var bounding how long a headless session waits for its
/// background tasks at turn end before reaping them. AIDA sets this on every
/// headless child it spawns so a hand-typed launch and a daemonized one behave
/// identically (the 2026-07-18 limp incident was exactly that divergence).
// trace:TASK-1169 | ai:claude
pub(crate) const BG_WAIT_CEILING_ENV: &str = "CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS";

/// Operator override for the value AIDA sets.
pub(crate) const BG_WAIT_CEILING_OVERRIDE_ENV: &str = "AIDA_BURNDOWN_BG_WAIT_CEILING_MS";

/// Default ceiling: 45 minutes — worst-case cross-platform CI plus margin, so
/// it never bites normal operation but a genuinely wedged task still surfaces.
pub(crate) const DEFAULT_BG_WAIT_CEILING_MS: u64 = 2_700_000;

/// Floor: 1 minute. Clamping UP is what retires the `=0` (wait-forever)
/// stopgap — a real ceiling that rarely fires beats no ceiling at all, so no
/// env value or config key can re-create wait-forever.
pub(crate) const MIN_BG_WAIT_CEILING_MS: u64 = 60_000;

/// Resolve the background-wait ceiling AIDA sets on a headless child:
/// `AIDA_BURNDOWN_BG_WAIT_CEILING_MS` → `[burndown] bg_wait_ceiling_ms` →
/// [`DEFAULT_BG_WAIT_CEILING_MS`], then clamped to [`MIN_BG_WAIT_CEILING_MS`].
/// An unparseable override is ignored (fall through) rather than fatal — a
/// typo'd env must not wedge a drain. `0` from either source clamps to the
/// floor, which is the point: wait-forever is not reachable by configuration.
// trace:TASK-1169 | ai:claude
pub(crate) fn resolve_bg_wait_ceiling_ms(
    override_env: Option<&str>,
    configured: Option<u64>,
) -> u64 {
    let chosen = override_env
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .or(configured)
        .unwrap_or(DEFAULT_BG_WAIT_CEILING_MS);
    chosen.max(MIN_BG_WAIT_CEILING_MS)
}

/// The already-probed facts the launcher's per-PR gate decides on. Every field
/// is a normalized signal so the decision stays pure — the messy `gh`/store
/// probing lives at the boundary in `lib.rs`.
// trace:TASK-1169 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WavePrFacts {
    /// `Some(label)` when the spec's `execution_mode` makes its merge
    /// supervised (BUG-727) — anything but `drain`, or unset (fail safe).
    pub supervision_label: Option<String>,
    /// `Some(reason)` when the CI wait ended on a BOUND rather than a verdict
    /// (idle-stall or absolute ceiling).
    pub wait_expired: Option<String>,
    pub ci: crate::integrate::CiState,
    pub request_changes: bool,
    // trace:TASK-3777 | ai:codex
    pub review_waiting: bool,
    pub mergeable: crate::integrate::MergeableState,
}

/// What the launcher does with one open wave PR after its CI wait returns.
// trace:TASK-1169 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WavePrAction {
    /// Clean — squash-merge it, then `aida pull` for the Done→Completed bump.
    Merge,
    /// Deliberately held for a human/advisor; NOT a defect, NOT parked. The
    /// string is the human-facing reason.
    Hold(String),
    /// Park the spec `NeedsAttention` with this reason and continue the drain
    /// (EPIC-28 punt-and-continue). The PR is left open and untouched.
    Park(String),
}

/// The pure per-PR gate. Order encodes the safety priority:
///   1. supervised merge — a non-`drain` execution_mode is a human's explicit
///      "not automatically" (BUG-727); hold, never park (nothing is wrong).
///   2. the wait hit a bound — park with the recorded reason (ADR-22's
///      punt-not-abort: the PR stays open, the work is never terminated).
///   3. otherwise delegate to the proven pre-merge gate
///      ([`crate::integrate::classify_integration_action`]) so RequestChanges /
///      CI-red / conflict all keep the exact semantics `aida integrate` uses —
///      one gate, not a second opinion.
/// A `WaitCi` from that gate means CI was still non-terminal after the bounded
/// wait already returned, so it becomes a park, never an infinite re-wait.
// trace:TASK-1169 | ai:claude
pub(crate) fn wave_pr_action(f: &WavePrFacts) -> WavePrAction {
    if let Some(label) = &f.supervision_label {
        return WavePrAction::Hold(format!(
            "merge is supervised ({label}) — left open for human/advisor review"
        ));
    }
    if let Some(reason) = &f.wait_expired {
        return WavePrAction::Park(format!(
            "the integration wait ended without a CI verdict: {reason} — PR left open, nothing terminated"
        ));
    }
    let state = crate::integrate::PrIntegrationState {
        ci: f.ci,
        request_changes_pending: f.request_changes,
        review_integrity_unproven: false,
        review_waiting: f.review_waiting,
        mergeable: f.mergeable,
    };
    match crate::integrate::classify_integration_action(&state) {
        crate::integrate::IntegrationAction::Merge => WavePrAction::Merge,
        crate::integrate::IntegrationAction::WaitCi => WavePrAction::Park(
            "CI never reached a terminal state within the bounded wait — PR left open".to_string(),
        ),
        crate::integrate::IntegrationAction::Park(reason) => {
            WavePrAction::Park(reason.message().to_string())
        }
    }
}

/// What the launcher's integration leg did to one wave PR — the report row.
// trace:TASK-1169 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IntegrationOutcome {
    pub pr: u64,
    pub spec: String,
    pub action: WavePrAction,
    /// `false` when the action was `Merge` but the merge call itself failed
    /// (the spec is parked in that case too, with the forge's error).
    pub succeeded: bool,
}

/// Render the launcher's integration-leg report. Every row names the PR, its
/// spec, and what happened — so an operator reading a drain's tail can see
/// exactly which PRs merged, which are held, and which parked, without
/// inferring it from an agent's prose.
// trace:TASK-1169 | ai:claude
pub(crate) fn render_integration_report(rows: &[IntegrationOutcome]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let merged = rows
        .iter()
        .filter(|r| r.succeeded && matches!(r.action, WavePrAction::Merge))
        .count();
    let held = rows
        .iter()
        .filter(|r| matches!(r.action, WavePrAction::Hold(_)))
        .count();
    let parked = rows.len() - merged - held;
    let mut s = format!(
        "\nIntegration (launcher-owned CI wait): {} merged, {} held, {} parked\n",
        merged, held, parked
    );
    for row in rows {
        let (glyph, detail) = match (&row.action, row.succeeded) {
            (WavePrAction::Merge, true) => (
                "merged",
                "squash-merged; `aida pull` bumped the spec".to_string(),
            ),
            (WavePrAction::Merge, false) => (
                "PARKED",
                "the merge call failed — parked for triage".to_string(),
            ),
            (WavePrAction::Hold(why), _) => ("held", why.clone()),
            (WavePrAction::Park(why), _) => ("PARKED", why.clone()),
        };
        s.push_str(&format!(
            "  #{} ({}) {} — {}\n",
            row.pr, row.spec, glyph, detail
        ));
    }
    s
}

#[cfg(test)]
#[path = "tests/burndown_bug_755_residual_tests.rs"]
mod bug_755_residual_tests;
