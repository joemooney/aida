//! Declared spec-state transition model — the single source of truth for the
//! lifecycle the Mermaid diagram is generated from.
//!
//! Phase 1 (SPIKE-56 / TASK-737) is **generate-only**: this module declares the
//! status chain (Region 1 of the lifecycle diagram in `docs/lifecycle.md`) and
//! renders it to a Mermaid `stateDiagram-v2`. Later phases reuse this same
//! declared model for guard enforcement and declared-vs-empirical diffing; they
//! are deliberately NOT implemented here. Encode only what `docs/lifecycle.md`
//! and the README "Spec lifecycle" section already document — do not invent new
//! transitions.
//
// trace:TASK-737 | ai:claude

/// Which kind of trigger most often drives a transition (and, for entry-into-a
/// -state, colours the node). Mirrors the three-trigger-kinds legend in
/// `docs/lifecycle.md`: blue = CLI/human, purple = LLM decision, green =
/// system/git-event.
// trace:TASK-737 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerKind {
    /// 🔵 A person (or script) runs an `aida` verb.
    Cli,
    /// 🟣 A Claude session decides and acts.
    Llm,
    /// 🟢 A git event or background sweep fires it, no human in the loop.
    Git,
}

impl TriggerKind {
    /// The Mermaid `classDef` name this trigger kind maps onto.
    fn class_name(self) -> &'static str {
        match self {
            TriggerKind::Cli => "cli",
            TriggerKind::Llm => "llm",
            TriggerKind::Git => "git",
        }
    }
}

/// A declared spec-state. `[*]` (the Mermaid pseudo start/end node) is modelled
/// as [`State::Start`] / it is rendered specially; the rest are the real
/// `status` values from `docs/lifecycle.md`.
// trace:TASK-737 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Start,
    Draft,
    Approved,
    Planned,
    InProgress,
    Done,
    Completed,
    Released,
    Rejected,
    NeedsAttention,
}

impl State {
    /// Map a status string as recorded inside a `history:` entry's
    /// `{field_name: "status", old_value, new_value}` triple back to a declared
    /// [`State`]. Status changes record values via [`RequirementStatus`]'s
    /// `Display` form (e.g. `"In Progress"`, `"Needs Attention"`), so we route
    /// through the same tolerant recognizer the CLI uses
    /// ([`RequirementStatus::from_filter_str`]) — casing / hyphen / space
    /// variants all collapse. Returns `None` for any string that is not a
    /// recognized status (so an unparseable history value is surfaced as such by
    /// the empirical reconstruction rather than silently mapped). Note: the
    /// declared-only `Start` and `Released` states are never produced here —
    /// they are not `RequirementStatus` values (entry is `aida add`, release is
    /// a git tag), so they only appear as dead declared edges in a `--diff`.
    // trace:TASK-742 | ai:claude
    pub fn from_status_str(s: &str) -> Option<State> {
        use crate::models::RequirementStatus as RS;
        match RS::from_filter_str(s)? {
            RS::Draft => Some(State::Draft),
            RS::Approved => Some(State::Approved),
            RS::Planned => Some(State::Planned),
            RS::InProgress => Some(State::InProgress),
            RS::Done => Some(State::Done),
            RS::Completed => Some(State::Completed),
            RS::Rejected => Some(State::Rejected),
            RS::NeedsAttention => Some(State::NeedsAttention),
        }
    }

    /// Map a [`RequirementStatus`] directly to its declared [`State`] — the
    /// in-memory companion to [`State::from_status_str`], no string round-trip.
    /// Total over `RequirementStatus`; the declared-only `Start`/`Released`
    /// states are not statuses. trace:TASK-738 | ai:claude
    pub fn from_status(rs: &crate::models::RequirementStatus) -> State {
        use crate::models::RequirementStatus as RS;
        match rs {
            RS::Draft => State::Draft,
            RS::Approved => State::Approved,
            RS::Planned => State::Planned,
            RS::InProgress => State::InProgress,
            RS::Done => State::Done,
            RS::Completed => State::Completed,
            RS::Rejected => State::Rejected,
            RS::NeedsAttention => State::NeedsAttention,
        }
    }

    /// The node label as it appears in the Mermaid diagram.
    pub fn label(self) -> &'static str {
        match self {
            State::Start => "[*]",
            State::Draft => "Draft",
            State::Approved => "Approved",
            State::Planned => "Planned",
            State::InProgress => "InProgress",
            State::Done => "Done",
            State::Completed => "Completed",
            State::Released => "Released",
            State::Rejected => "Rejected",
            State::NeedsAttention => "NeedsAttention",
        }
    }

    /// The trigger kind that most often drives **entry** into this state — this
    /// is what colours the node. `None` = not classified (the start/end
    /// pseudo-node).
    fn entry_trigger(self) -> Option<TriggerKind> {
        match self {
            State::Start => None,
            State::Draft | State::Approved | State::Planned | State::Rejected => {
                Some(TriggerKind::Cli)
            }
            State::InProgress | State::NeedsAttention => Some(TriggerKind::Llm),
            State::Done | State::Completed | State::Released => Some(TriggerKind::Git),
        }
    }
}

/// True iff `from → to` is a transition the lifecycle model
/// ([`LifecycleModel::declared`]) declares legal — the single source of truth
/// for which edges exist, so gates migrated behind it (Phase 2) ask this
/// instead of re-encoding the same edge set, and a gate can never drift from
/// the diagram. A self-edge (`from == to`) is a no-op, not a declared
/// transition; callers that allow no-ops handle that themselves.
/// trace:TASK-738 | ai:claude
pub fn is_declared(from: State, to: State) -> bool {
    LifecycleModel::declared()
        .transitions
        .iter()
        .any(|t| t.from == from && t.to == to)
}

/// A named guard on a transition — the authority/condition it must satisfy.
/// Gates migrated behind the model ask for this instead of hand-encoding the
/// same predicate, so a gate and the rendered diagram can never drift. Phase 2b
/// (TASK-739) introduces the advisor-authority guard here. The later
/// merge-evidence (Phase 2c / TASK-740 → [`git_merge_completes`]) and
/// terminal-and-unqueued (Phase 2d / TASK-741 → [`INVARIANTS`]) constraints
/// landed as their own typed predicates rather than `GuardKind` arms: they
/// answer different questions than the manual-edit authority gate (one is "does
/// a git merge-event complete this spec", the other a cross-axis invariant), so
/// folding them into the single `transition_guard` return would conflate gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardKind {
    /// No special authority — an implementer-legitimate flip.
    None,
    /// Lifting an un-triaged (`Draft`) or punted (`NeedsAttention`) spec into
    /// the approved+ pipeline: the advisor's (or interactive human's) triage
    /// decision. trace:TASK-739 | ai:claude
    RequiresAdvisorAuthority,
}

/// Whether a *target* status is one the advisor gate protects (the approved+
/// pipeline). The single source for the target half of the authority predicate
/// (`status_requires_advisor_authority`). trace:TASK-739 | ai:claude
pub fn target_requires_advisor_authority(to: State) -> bool {
    matches!(
        to,
        State::Approved | State::Planned | State::InProgress | State::Done | State::Completed
    )
}

/// The guard on the `from → to` transition. Phase 2b: a transition is an
/// advisor-authority act iff it lifts an un-triaged (`Draft`) or punted
/// (`NeedsAttention`) spec into a protected target — the model's single source
/// for `status_advance_requires_advisor_authority`. Defined over the FULL
/// (from, to) domain, not only declared edges, because the gate governs direct
/// edits too (e.g. `Draft → InProgress`). trace:TASK-739 | ai:claude
pub fn transition_guard(from: State, to: State) -> GuardKind {
    if matches!(from, State::Draft | State::NeedsAttention) && target_requires_advisor_authority(to)
    {
        GuardKind::RequiresAdvisorAuthority
    } else {
        GuardKind::None
    }
}

/// Phase 2c (TASK-740): the merge auto-bump expressed as the model's GitEvent
/// guard. A commit referencing the spec landing on the default branch is
/// authoritative evidence the work shipped — the 🟢 [`TriggerKind::Git`]
/// `→ Completed` transition (the declared mainline `Done → Completed` edge, verb
/// "merge auto-bump (aida pull)"). This is the single source for which SOURCE
/// states that git-event promotes straight to `Completed`, so the `aida pull`
/// scanner and the `aida db reconcile-status` replay ask the model instead of
/// re-encoding the set:
/// - the in-pipeline states `Approved` / `Planned` / `InProgress` / `Done` — a
///   commit is authoritative that approved/planned/in-flight/done work shipped
///   (BUG-328);
/// - a shelved `NeedsAttention` whose PR a later session/sibling-agent fixed and
///   merged — otherwise stranded, since a direct `NeedsAttention → Completed`
///   manual flip is blocked but the merge is the same "it shipped" signal
///   (BUG-405).
///
/// `Draft` is excluded — a commit does not override un-triaged intent, the
/// approval signal is preserved (BUG-328) — and the terminal states
/// (`Completed` / `Rejected`) and `Released` stay put. Defined over the full
/// state domain; total. trace:TASK-740 | ai:claude
pub fn git_merge_completes(from: State) -> bool {
    matches!(
        from,
        State::Approved | State::Planned | State::InProgress | State::Done | State::NeedsAttention
    )
}

// ────────────────────────────────────────────────────────────────────
// Phase 2d (TASK-741): cross-axis orthogonal invariants.
//
// The transition table above governs ONE axis — status. But the spec lifecycle
// is several orthogonal regions (status × visibility × queue × forge/park), and
// the bugs that hurt most were cross-axis contradictions: a combination that no
// single-axis predicate could forbid because the two axes it related were
// checked by two unrelated predicates. `archived ∧ queued` (BUG-492) and a
// `review:draft-only` hold whose draft PR is closed (BUG-493) are exactly that.
//
// Those constraints live here, as `INVARIANTS` — the one place every cross-axis
// rule is declared, so any path that mutates one axis (flips `archived`, asserts
// a held-for-review claim) consults the row it must not violate instead of
// re-deriving the rule. Same "declare-once, consult-everywhere" discipline
// STORY-538 brought to the schema. trace:TASK-741 | ai:claude
// ────────────────────────────────────────────────────────────────────

impl State {
    /// A *terminal* status — the closed long-tail (`Completed` / `Rejected`)
    /// past which no further pipeline transition fires. The single source for
    /// "is this work closed?"; the CLI's `is_terminal_status` routes through
    /// this so the archive invariant (`archived ⇒ terminal ∧ ¬queued`) and the
    /// diagram read the same definition. trace:TASK-741 | ai:claude
    pub fn is_terminal(self) -> bool {
        matches!(self, State::Completed | State::Rejected)
    }
}

/// A cross-axis lifecycle invariant: a constraint BETWEEN two orthogonal
/// regions (status × visibility × queue × forge) that no single-axis check can
/// enforce. Declared as data so the rule has one name, one statement, and a
/// pointer to the bug whose root cause was this constraint going unexpressed —
/// the same discipline STORY-538 brought to the schema. trace:TASK-741 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrthogonalInvariant {
    /// Stable kebab-case identifier the enforcing call site pins itself to.
    pub name: &'static str,
    /// The constraint, stated as an implication between axes.
    pub statement: &'static str,
    /// Spec id of the bug whose root cause was this invariant being implicit
    /// (two axes checked by two unrelated predicates).
    pub origin: &'static str,
}

/// The cross-axis illegal-combination list — the one place every constraint that
/// spans two orthogonal regions is declared. Any axis-mutating path consults the
/// row it must not violate rather than re-deriving the rule, so the illegal
/// combination becomes unconstructable instead of merely untested-against.
/// trace:TASK-741 | ai:claude
pub const INVARIANTS: &[OrthogonalInvariant] = &[
    OrthogonalInvariant {
        name: "archived-implies-terminal-and-unqueued",
        statement: "archived ⇒ (Completed ∨ Rejected) ∧ ¬queued",
        origin: "BUG-492",
    },
    OrthogonalInvariant {
        name: "held-for-review-implies-open-draft-pr",
        statement: "review:draft-only ⇒ an open draft PR exists",
        origin: "BUG-493",
    },
];

/// Look up a declared cross-axis invariant by `name`. The enforcing call sites
/// pin themselves to the row they uphold (via a test against this) so a renamed
/// or deleted invariant surfaces at its consumer rather than drifting silently.
/// trace:TASK-741 | ai:claude
pub fn invariant(name: &str) -> Option<&'static OrthogonalInvariant> {
    INVARIANTS.iter().find(|i| i.name == name)
}

/// Which axis of the `archived ⇒ terminal ∧ ¬queued` invariant (BUG-492) a
/// candidate archive would violate. `Queued` is reported in preference to
/// `NonTerminal`: a queued spec is the louder contradiction (`aida list` hides
/// it while `queue list` still points at it), matching the pre-migration
/// message precedence. trace:TASK-741 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveBlock {
    /// The spec is in the queue — archiving leaves the queue pointing at a
    /// hidden spec.
    Queued,
    /// The spec is non-terminal (not Completed/Rejected) — archive is for the
    /// closed long-tail.
    NonTerminal,
}

/// Evaluate the BUG-492 archive invariant: `Some(block)` names the axis that
/// forbids archiving this `(status, queued)` spec; `None` means the archive is
/// invariant-legal. The single source the archive path consults — the `--force`
/// override and the user-facing warning wording stay at the call site (CLI
/// concerns), but *whether the combination is legal* is decided here.
/// trace:TASK-741 | ai:claude
pub fn archive_invariant_block(status: State, queued: bool) -> Option<ArchiveBlock> {
    if queued {
        return Some(ArchiveBlock::Queued);
    }
    if !status.is_terminal() {
        return Some(ArchiveBlock::NonTerminal);
    }
    None
}

/// Class-aware "closed for archive purposes" predicate. For work specs the
/// closed set is the terminal statuses (Completed/Rejected). For the
/// decision class (ADRs) `Approved` IS the terminal state — the documented
/// ADR lifecycle records acceptance as `Approved` (draft = proposed,
/// approved = accepted), so an accepted decision is closed and belongs in
/// the archivable long-tail, not the open lens forever. Other
/// knowledge-class types (principle, term) cannot reach `Approved` at all
/// (approval is forbidden for them), so they keep the work-spec rule.
// trace:BUG-761 | ai:claude
pub fn status_is_closed_for_type(req_type: &crate::models::RequirementType, status: State) -> bool {
    status.is_terminal()
        || (*req_type == crate::models::RequirementType::Decision && status == State::Approved)
}

/// Type-aware form of [`archive_invariant_block`]: identical queued-axis
/// precedence, but the status axis consults [`status_is_closed_for_type`]
/// so an accepted (`Approved`) decision spec archives without a block while
/// every other type keeps the Completed/Rejected-only rule.
// trace:BUG-761 | ai:claude
pub fn archive_invariant_block_for_type(
    req_type: &crate::models::RequirementType,
    status: State,
    queued: bool,
) -> Option<ArchiveBlock> {
    if queued {
        return Some(ArchiveBlock::Queued);
    }
    if !status_is_closed_for_type(req_type, status) {
        return Some(ArchiveBlock::NonTerminal);
    }
    None
}

/// Evaluate the BUG-493 held-for-review invariant: a `review:draft-only` hold
/// CLAIMS an open draft PR exists for human review. Given whether the forge
/// confirms an open PR, return whether the claim HOLDS. The reconcile path
/// (`reconcile_held_for_review`, which owns the forge-specific three-way probe)
/// consults this so the *rule* lives with the other cross-axis invariants even
/// though the *probe* is forge-specific. The predicate is intentionally simple —
/// the value is that there is ONE named home for the rule the held-for-review
/// reason must not contradict, not its arithmetic. trace:TASK-741 | ai:claude
pub fn held_for_review_claim_holds(open_draft_pr_exists: bool) -> bool {
    open_draft_pr_exists
}

/// One legal transition in the declared status chain.
// trace:TASK-737 | ai:claude
#[derive(Debug, Clone, Copy)]
pub struct Transition {
    pub from: State,
    pub to: State,
    /// The precise verb / command that drives the transition (the Mermaid edge
    /// label).
    pub verb: &'static str,
}

/// The declared status-chain model: the states plus the legal transitions, each
/// with its verb. This is **the single source** — the diagram is derived from
/// it, never hand-maintained. Encodes Region 1 of `docs/lifecycle.md` exactly
/// (Draft → Approved → Planned → In Progress → Done → Completed → Released, plus
/// the Rejected / Needs Attention branches).
// trace:TASK-737 | ai:claude
#[derive(Debug, Clone)]
pub struct LifecycleModel {
    pub transitions: Vec<Transition>,
}

impl Default for LifecycleModel {
    fn default() -> Self {
        Self::declared()
    }
}

impl LifecycleModel {
    /// The canonical declared model. Order matters: the diagram renders edges in
    /// declaration order, grouped to match the committed `docs/lifecycle.md`
    /// block (mainline chain, then the Needs Attention branch, then the
    /// off-mainline edges).
    // trace:TASK-737 | ai:claude
    pub fn declared() -> Self {
        use State::*;
        let t = |from, to, verb| Transition { from, to, verb };
        LifecycleModel {
            transitions: vec![
                // Entry.
                t(Start, Draft, "aida add (LLM/human)"),
                // Mainline chain.
                t(Draft, Approved, "aida edit --status approved"),
                t(Approved, Planned, "aida edit --status planned"),
                t(Approved, InProgress, "aida queue work"),
                t(Planned, InProgress, "aida queue work"),
                t(InProgress, Done, "aida queue done / aida-pr"),
                t(Done, Completed, "merge auto-bump (aida pull)"),
                t(Completed, Released, "release tag (scripts/release.sh)"),
                // Needs Attention branch (the off-mainline pause state).
                t(InProgress, NeedsAttention, "punt (design-fork)"),
                t(NeedsAttention, InProgress, "aida edit --status in-progress"),
                t(NeedsAttention, Approved, "aida edit --status approved"),
                t(NeedsAttention, Rejected, "aida edit --status rejected"),
                // Off-mainline edges.
                t(Draft, Rejected, "aida edit --status rejected"),
                t(Approved, Rejected, "aida edit --status rejected"),
                t(Done, InProgress, "reviewer RequestChanges"),
            ],
        }
    }

    /// Render the declared status chain as a Mermaid `stateDiagram-v2`,
    /// byte-for-byte matching the committed Region 1 block in
    /// `docs/lifecycle.md` (so the doc-pin check can compare them directly).
    /// The body is grouped — entry, mainline, Needs Attention branch,
    /// off-mainline — and terminal states emit `--> [*]` edges, then the
    /// `classDef` legend + per-state class assignments derived from each
    /// state's [`State::entry_trigger`].
    // trace:TASK-737 | ai:claude
    pub fn to_mermaid(&self) -> String {
        let mut out = String::new();
        out.push_str("stateDiagram-v2\n");
        out.push_str("    direction LR\n");

        // Edges, grouped with blank lines exactly as the committed block.
        // Group boundaries are derived from the declared `from` state so the
        // grouping tracks the model, not a hand-kept layout: the entry edge,
        // then the mainline chain (everything up to the first NeedsAttention
        // edge), then the NeedsAttention branch, then the remaining
        // off-mainline edges.
        let mut emitted_entry = false;
        let mut in_needs_attention = false;
        let mut after_needs_attention = false;
        for tr in &self.transitions {
            if !emitted_entry {
                // entry edge
                out.push_str(&format!(
                    "    {} --> {}: {}\n",
                    tr.from.label(),
                    tr.to.label(),
                    tr.verb
                ));
                out.push('\n');
                emitted_entry = true;
                continue;
            }
            let touches_na = tr.from == State::NeedsAttention || tr.to == State::NeedsAttention;
            if touches_na && !in_needs_attention {
                // start of the Needs Attention branch group
                out.push('\n');
                in_needs_attention = true;
            } else if !touches_na && in_needs_attention && !after_needs_attention {
                // start of the off-mainline group
                out.push('\n');
                after_needs_attention = true;
            }
            out.push_str(&format!(
                "    {} --> {}: {}\n",
                tr.from.label(),
                tr.to.label(),
                tr.verb
            ));
        }

        // Terminal `--> [*]` edges for the end states, in the committed order.
        out.push('\n');
        for term in [State::Released, State::Rejected, State::Completed] {
            out.push_str(&format!("    {} --> [*]\n", term.label()));
        }

        // The classDef legend.
        out.push('\n');
        out.push_str("    classDef cli fill:#1f6feb,stroke:#0d3b8a,color:#fff\n");
        out.push_str("    classDef llm fill:#8957e5,stroke:#5a2ca0,color:#fff\n");
        out.push_str("    classDef git fill:#2da44e,stroke:#176b2e,color:#fff\n");

        // Per-class state assignments, derived from entry_trigger, in the
        // committed cli/llm/git order.
        out.push('\n');
        for kind in [TriggerKind::Cli, TriggerKind::Llm, TriggerKind::Git] {
            let members: Vec<&str> = self
                .states_in_order()
                .into_iter()
                .filter(|s| s.entry_trigger() == Some(kind))
                .map(|s| s.label())
                .collect();
            if !members.is_empty() {
                out.push_str(&format!(
                    "    class {} {}\n",
                    members.join(","),
                    kind.class_name()
                ));
            }
        }

        out
    }

    /// The real (non-pseudo) states in the committed presentation order, deduped
    /// — used to render the per-class `class A,B,C cli` lines deterministically.
    // trace:TASK-737 | ai:claude
    fn states_in_order(&self) -> Vec<State> {
        use State::*;
        // Fixed presentation order matching docs/lifecycle.md.
        vec![
            Draft,
            Approved,
            Planned,
            InProgress,
            Done,
            Completed,
            Released,
            Rejected,
            NeedsAttention,
        ]
    }
}

// ────────────────────────────────────────────────────────────────────
// Phase 3 (TASK-742): empirical reconstruction + declared-vs-observed diff.
//
// The DECLARED model above is the single source of truth for the *intended*
// state machine. The EMPIRICAL model below is reconstructed by walking the
// `history:` arrays inside the spec YAML — every `{field_name: "status",
// old_value, new_value}` triple is one OBSERVED status flip. Diffing the two
// surfaces: (a) observed transitions the declared model never authorized
// (undocumented / illegal flips that actually happened), and (b) declared
// transitions never observed in the substrate (dead edges).
// ────────────────────────────────────────────────────────────────────

/// One observed `from → to` status transition plus how many times it was seen
/// across all specs' history arrays. `from`/`to` carry the raw recorded status
/// string alongside the parsed [`State`] so an unrecognized status value can
/// still be reported. trace:TASK-742 | ai:claude
#[derive(Debug, Clone)]
pub struct ObservedTransition {
    /// Parsed source state; `None` if the recorded `old_value` did not parse to
    /// a known status.
    pub from: Option<State>,
    /// Parsed target state; `None` if the recorded `new_value` did not parse.
    pub to: Option<State>,
    /// The raw `old_value` string as stored in history.
    pub from_raw: String,
    /// The raw `new_value` string as stored in history.
    pub to_raw: String,
    /// How many times this exact `from_raw → to_raw` flip was observed.
    pub count: usize,
}

impl ObservedTransition {
    /// Stable key for grouping identical observed flips (by raw recorded
    /// strings, so unparseable values still group). trace:TASK-742 | ai:claude
    fn key(from_raw: &str, to_raw: &str) -> String {
        format!("{from_raw}\u{1}{to_raw}")
    }
}

/// The reconstructed observed state machine: every distinct status transition
/// seen across the history arrays, with counts, plus a tally of how many specs
/// contributed and how many status flips were walked. trace:TASK-742 | ai:claude
#[derive(Debug, Clone, Default)]
pub struct EmpiricalModel {
    /// Distinct observed transitions, sorted descending by count then by
    /// `from_raw`/`to_raw` for a stable presentation.
    pub transitions: Vec<ObservedTransition>,
    /// How many specs contributed at least one status flip.
    pub specs_with_history: usize,
    /// Total number of status flips walked (sum of all `count`s).
    pub total_flips: usize,
}

impl EmpiricalModel {
    /// Reconstruct the observed machine from an iterator of `(old_value,
    /// new_value)` status-change pairs grouped per spec. Each inner iterator is
    /// one spec's ordered status changes (the caller filters the history arrays
    /// down to `field_name == "status"` triples). Pure / storage-free so it is
    /// unit-testable without a git backend. trace:TASK-742 | ai:claude
    pub fn from_status_changes<I, S, A, B>(per_spec: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: IntoIterator<Item = (A, B)>,
        A: AsRef<str>,
        B: AsRef<str>,
    {
        use std::collections::HashMap;
        let mut counts: HashMap<String, ObservedTransition> = HashMap::new();
        let mut specs_with_history = 0usize;
        let mut total_flips = 0usize;

        for spec_changes in per_spec {
            let mut spec_contributed = false;
            for (old_v, new_v) in spec_changes {
                let from_raw = old_v.as_ref().to_string();
                let to_raw = new_v.as_ref().to_string();
                // Skip no-op rows (status recorded but unchanged) — they are
                // not transitions.
                if from_raw == to_raw {
                    continue;
                }
                spec_contributed = true;
                total_flips += 1;
                let key = ObservedTransition::key(&from_raw, &to_raw);
                counts
                    .entry(key)
                    .and_modify(|t| t.count += 1)
                    .or_insert_with(|| ObservedTransition {
                        from: State::from_status_str(&from_raw),
                        to: State::from_status_str(&to_raw),
                        from_raw,
                        to_raw,
                        count: 1,
                    });
            }
            if spec_contributed {
                specs_with_history += 1;
            }
        }

        let mut transitions: Vec<ObservedTransition> = counts.into_values().collect();
        transitions.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.from_raw.cmp(&b.from_raw))
                .then_with(|| a.to_raw.cmp(&b.to_raw))
        });

        EmpiricalModel {
            transitions,
            specs_with_history,
            total_flips,
        }
    }
}

/// The result of diffing the declared model against the empirical one.
/// trace:TASK-742 | ai:claude
#[derive(Debug, Clone, Default)]
pub struct LifecycleDiff {
    /// Observed transitions that are NOT in the declared model — illegal /
    /// undocumented flips that actually happened. Includes flips whose
    /// `old_value`/`new_value` did not parse to a known status (those can never
    /// match a declared edge).
    pub undocumented: Vec<ObservedTransition>,
    /// Declared transitions never observed in any spec's history — dead edges.
    pub unobserved: Vec<Transition>,
}

impl LifecycleDiff {
    /// `true` when at least one observed transition is undocumented — the
    /// CI-gate condition. trace:TASK-742 | ai:claude
    pub fn has_undocumented(&self) -> bool {
        !self.undocumented.is_empty()
    }
}

/// Diff a declared [`LifecycleModel`] against a reconstructed [`EmpiricalModel`].
/// A declared edge matches an observed flip when both endpoints parse to the
/// same declared [`State`]. trace:TASK-742 | ai:claude
pub fn diff(declared: &LifecycleModel, empirical: &EmpiricalModel) -> LifecycleDiff {
    // An observed flip is "documented" iff some declared transition has the
    // same (from, to) parsed states. Unparseable endpoints never match.
    let undocumented: Vec<ObservedTransition> = empirical
        .transitions
        .iter()
        .filter(|obs| {
            let (Some(of), Some(ot)) = (obs.from, obs.to) else {
                return true; // unparseable endpoint can't match a declared edge
            };
            !declared
                .transitions
                .iter()
                .any(|d| d.from == of && d.to == ot)
        })
        .cloned()
        .collect();

    // A declared edge is "dead" iff no observed flip parses to the same
    // (from, to). The Start entry-edge and any edge into Released are inherently
    // un-observable from status history (no such status value), so they are
    // expected dead edges — still reported, but that's by design.
    let unobserved: Vec<Transition> = declared
        .transitions
        .iter()
        .filter(|d| {
            !empirical
                .transitions
                .iter()
                .any(|obs| obs.from == Some(d.from) && obs.to == Some(d.to))
        })
        .cloned()
        .collect();

    LifecycleDiff {
        undocumented,
        unobserved,
    }
}

/// Wrap the generated Mermaid body in a fenced ```mermaid code block.
// trace:TASK-737 | ai:claude
pub fn fenced_mermaid(model: &LifecycleModel) -> String {
    format!("```mermaid\n{}```\n", model.to_mermaid())
}

/// Extract the **first** fenced ```mermaid block body from a markdown document
/// (Region 1 of `docs/lifecycle.md` is the first one). Returns the inner body
/// WITHOUT the fence lines, or `None` if there is no mermaid block.
// trace:TASK-737 | ai:claude
pub fn first_mermaid_block(markdown: &str) -> Option<String> {
    let mut lines = markdown.lines();
    let mut body = String::new();
    // Find opening fence.
    for line in lines.by_ref() {
        if line.trim_start().starts_with("```mermaid") {
            break;
        }
    }
    let mut found_close = false;
    for line in lines {
        if line.trim_start().starts_with("```") {
            found_close = true;
            break;
        }
        body.push_str(line);
        body.push('\n');
    }
    if body.is_empty() && !found_close {
        // We never entered a block (or it was empty with no close).
        return None;
    }
    if !found_close {
        return None;
    }
    Some(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_model_has_all_states_reachable() {
        let m = LifecycleModel::declared();
        // Every mainline + branch state appears as a `from` or `to`.
        for s in [
            State::Draft,
            State::Approved,
            State::Planned,
            State::InProgress,
            State::Done,
            State::Completed,
            State::Released,
            State::Rejected,
            State::NeedsAttention,
        ] {
            let touched = m.transitions.iter().any(|t| t.from == s || t.to == s);
            assert!(touched, "state {:?} must appear in a transition", s);
        }
    }

    #[test]
    fn mermaid_is_a_state_diagram() {
        let body = LifecycleModel::declared().to_mermaid();
        assert!(body.starts_with("stateDiagram-v2\n"));
        assert!(body.contains("[*] --> Draft"));
        assert!(body.contains("Done --> Completed: merge auto-bump (aida pull)"));
        assert!(body.contains("Completed --> [*]"));
        assert!(body.contains("class Draft,Approved,Planned,Rejected cli"));
    }

    #[test]
    fn fenced_round_trips_through_extractor() {
        let m = LifecycleModel::declared();
        let fenced = fenced_mermaid(&m);
        let extracted = first_mermaid_block(&fenced).expect("has a mermaid block");
        assert_eq!(extracted, m.to_mermaid());
    }

    #[test]
    fn extractor_returns_none_without_a_block() {
        assert!(first_mermaid_block("no fences here\njust prose\n").is_none());
    }

    #[test]
    fn extractor_grabs_first_block_only() {
        let md = "intro\n```mermaid\nstateDiagram-v2\n    A --> B\n```\nmiddle\n```mermaid\nOTHER\n```\n";
        let body = first_mermaid_block(md).unwrap();
        assert!(body.contains("A --> B"));
        assert!(!body.contains("OTHER"));
    }

    // ── Phase 2c (TASK-740): merge auto-bump GitEvent guard ──

    #[test]
    fn git_merge_completes_covers_the_in_pipeline_and_shelved_states() {
        use State::*;
        // The in-pipeline states a commit-on-default-branch ships (BUG-328) plus
        // a shelved NeedsAttention whose PR a later session merged (BUG-405).
        for s in [Approved, Planned, InProgress, Done, NeedsAttention] {
            assert!(git_merge_completes(s), "{s:?} must be merge-eligible");
        }
        // Draft preserves un-triaged intent; the terminal states + the
        // declared-only pseudo/Released states stay put.
        for s in [Start, Draft, Completed, Released, Rejected] {
            assert!(!git_merge_completes(s), "{s:?} must not be merge-eligible");
        }
    }

    // ── Phase 2d (TASK-741): cross-axis invariants ──

    #[test]
    fn invariants_declare_both_cross_axis_rows() {
        // The two cross-axis rules this phase homes must be present and
        // discoverable by name, each pointing at the bug it prevents.
        let archive = invariant("archived-implies-terminal-and-unqueued")
            .expect("archive invariant declared");
        assert_eq!(archive.origin, "BUG-492");
        let held = invariant("held-for-review-implies-open-draft-pr")
            .expect("held-for-review invariant declared");
        assert_eq!(held.origin, "BUG-493");
        // Unknown names don't resolve.
        assert!(invariant("nonexistent-invariant").is_none());
    }

    #[test]
    fn is_terminal_is_completed_or_rejected_only() {
        assert!(State::Completed.is_terminal());
        assert!(State::Rejected.is_terminal());
        for s in [
            State::Start,
            State::Draft,
            State::Approved,
            State::Planned,
            State::InProgress,
            State::Done,
            State::Released,
            State::NeedsAttention,
        ] {
            assert!(!s.is_terminal(), "{s:?} must not be terminal");
        }
    }

    #[test]
    fn archive_invariant_block_enforces_terminal_and_unqueued() {
        use State::*;
        // Terminal + unqueued is the only legal archive.
        assert_eq!(archive_invariant_block(Completed, false), None);
        assert_eq!(archive_invariant_block(Rejected, false), None);
        // Queued is the louder contradiction — reported in preference to
        // NonTerminal even when both axes are wrong.
        assert_eq!(
            archive_invariant_block(Approved, true),
            Some(ArchiveBlock::Queued)
        );
        assert_eq!(
            archive_invariant_block(Completed, true),
            Some(ArchiveBlock::Queued)
        );
        // Non-terminal + unqueued blocks on the status axis.
        for s in [Draft, Approved, Planned, InProgress, Done, NeedsAttention] {
            assert_eq!(
                archive_invariant_block(s, false),
                Some(ArchiveBlock::NonTerminal),
                "{s:?} unqueued must block as NonTerminal"
            );
        }
    }

    // An accepted ADR (Decision @ Approved) is closed for archive purposes;
    // every other type keeps the Completed/Rejected-only rule, and the
    // queued axis still blocks first even for a decision.
    // trace:BUG-761 | ai:claude
    #[test]
    fn archive_invariant_block_for_type_treats_accepted_decision_as_closed() {
        use crate::models::RequirementType as T;
        use State::*;
        // Accepted decision → archive is invariant-legal bare.
        assert_eq!(
            archive_invariant_block_for_type(&T::Decision, Approved, false),
            None
        );
        // Terminal statuses stay legal for every class.
        assert_eq!(
            archive_invariant_block_for_type(&T::Decision, Completed, false),
            None
        );
        assert_eq!(
            archive_invariant_block_for_type(&T::Task, Completed, false),
            None
        );
        // A proposed (Draft) decision is still open.
        assert_eq!(
            archive_invariant_block_for_type(&T::Decision, Draft, false),
            Some(ArchiveBlock::NonTerminal)
        );
        // Queued precedence is unchanged — even an accepted decision blocks
        // on the queue axis.
        assert_eq!(
            archive_invariant_block_for_type(&T::Decision, Approved, true),
            Some(ArchiveBlock::Queued)
        );
        // Work-spec refusal unchanged: Approved non-decision types block.
        for t in [
            T::Task,
            T::Story,
            T::Bug,
            T::Functional,
            T::Principle,
            T::Term,
        ] {
            assert_eq!(
                archive_invariant_block_for_type(&t, Approved, false),
                Some(ArchiveBlock::NonTerminal),
                "{t:?} @ Approved must still block as NonTerminal"
            );
        }
        // Parity with the untyped form for a work type across all states.
        for s in [
            Draft,
            Approved,
            Planned,
            InProgress,
            Done,
            Completed,
            Rejected,
            NeedsAttention,
        ] {
            for queued in [false, true] {
                assert_eq!(
                    archive_invariant_block_for_type(&T::Task, s, queued),
                    archive_invariant_block(s, queued),
                    "typed gate must match untyped gate for work specs at {s:?} queued={queued}"
                );
            }
        }
    }

    #[test]
    fn held_for_review_claim_holds_iff_open_pr() {
        assert!(held_for_review_claim_holds(true));
        assert!(!held_for_review_claim_holds(false));
    }

    // ── Phase 3 (TASK-742): empirical + diff ──

    #[test]
    fn status_str_parses_display_and_word_break_variants() {
        assert_eq!(State::from_status_str("Draft"), Some(State::Draft));
        assert_eq!(
            State::from_status_str("In Progress"),
            Some(State::InProgress)
        );
        assert_eq!(
            State::from_status_str("in-progress"),
            Some(State::InProgress)
        );
        assert_eq!(
            State::from_status_str("Needs Attention"),
            Some(State::NeedsAttention)
        );
        assert_eq!(State::from_status_str("Completed"), Some(State::Completed));
        assert_eq!(State::from_status_str("nonsense"), None);
    }

    #[test]
    fn empirical_counts_and_dedupes_transitions() {
        let per_spec = vec![
            vec![("Draft", "Approved"), ("Approved", "In Progress")],
            vec![("Draft", "Approved"), ("Approved", "In Progress")],
            // a no-op row (unchanged status) must not count as a transition
            vec![("In Progress", "In Progress"), ("In Progress", "Done")],
        ];
        let m = EmpiricalModel::from_status_changes(per_spec);
        assert_eq!(m.specs_with_history, 3);
        // 2 + 2 + 1 real flips (the no-op is skipped) = 5
        assert_eq!(m.total_flips, 5);
        let draft_approved = m
            .transitions
            .iter()
            .find(|t| t.from == Some(State::Draft) && t.to == Some(State::Approved))
            .expect("Draft→Approved observed");
        assert_eq!(draft_approved.count, 2);
        // sorted descending by count: the count-2 edges come first
        assert!(m.transitions[0].count >= m.transitions[m.transitions.len() - 1].count);
    }

    #[test]
    fn diff_flags_undocumented_and_dead_edges() {
        let declared = LifecycleModel::declared();
        // One legal flip (Draft→Approved) + one illegal flip (Done→Approved,
        // not in the declared model).
        let per_spec = vec![vec![("Draft", "Approved"), ("Done", "Approved")]];
        let empirical = EmpiricalModel::from_status_changes(per_spec);
        let d = diff(&declared, &empirical);

        assert!(d.has_undocumented());
        assert!(
            d.undocumented
                .iter()
                .any(|t| t.from == Some(State::Done) && t.to == Some(State::Approved)),
            "Done→Approved is undocumented"
        );
        assert!(
            !d.undocumented
                .iter()
                .any(|t| t.from == Some(State::Draft) && t.to == Some(State::Approved)),
            "Draft→Approved is declared, not undocumented"
        );

        // Completed→Released was never observed → it's a dead edge.
        assert!(
            d.unobserved
                .iter()
                .any(|t| t.from == State::Completed && t.to == State::Released),
            "Completed→Released is an unobserved declared edge"
        );
        // Draft→Approved WAS observed → not dead.
        assert!(
            !d.unobserved
                .iter()
                .any(|t| t.from == State::Draft && t.to == State::Approved),
            "Draft→Approved was observed"
        );
    }

    #[test]
    fn diff_clean_when_only_declared_flips_observed() {
        let declared = LifecycleModel::declared();
        let per_spec = vec![vec![
            ("Draft", "Approved"),
            ("Approved", "In Progress"),
            ("In Progress", "Done"),
        ]];
        let empirical = EmpiricalModel::from_status_changes(per_spec);
        let d = diff(&declared, &empirical);
        assert!(!d.has_undocumented(), "all observed flips are declared");
    }

    #[test]
    fn unparseable_observed_endpoint_is_undocumented() {
        let declared = LifecycleModel::declared();
        let per_spec = vec![vec![("Draft", "Frobnicated")]];
        let empirical = EmpiricalModel::from_status_changes(per_spec);
        let d = diff(&declared, &empirical);
        assert!(d.has_undocumented());
        assert_eq!(d.undocumented.len(), 1);
        assert_eq!(d.undocumented[0].to_raw, "Frobnicated");
        assert_eq!(d.undocumented[0].to, None);
    }
}
