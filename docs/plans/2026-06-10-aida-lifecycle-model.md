# Plan: `aida lifecycle` — one declared spec-state transition model (SPIKE-56)

<!-- trace:SPIKE-56 -->

Date: 2026-06-10
Specs: SPIKE-56
Status: Complete
Complexity: design-only (SPIKE) — no prod LOC; estimates downstream-only, risk medium

<!--
  AIDA plan template. This is a SPIKE deliverable: a DESIGN DOC, not code.
  Nothing here is enacted. The "Files" section describes the *proposed* shape
  of the future implementation, gated on operator approval of the phased path
  in the Recommendation section. Prefer SYMBOL refs over LINE refs.
-->

## Approach

The spec lifecycle state machine **already exists and runs in production** — but
it is *scattered* across at least eight independent predicates in two crates,
none of which reference a shared model. The authority gate, the parking-tag
predicate, the pickability gate, the NeedsAttention transition guard, the
done→completed auto-bump, the archive guard, the held-for-review reconcile, and
the queue-add sign-off gate each encode one slice of "what transition is legal
from what state, driven by what trigger." Today they agree by careful authorship
and a suite of per-slice unit tests; nothing *structurally* prevents two slices
from drifting apart, and the bug-class that escapes is exactly the
illegal-orthogonal-state combination (archived AND queued = BUG-492;
held-for-review AND PR-closed = BUG-493).

This spike proposes consolidating the scatter into **one declarative transition
table** — a Rust data structure (`LIFECYCLE: &[Transition]`) living in
`aida-core`, each row carrying `(from, to, guard, trigger_kind)` plus the
orthogonal-region constraints — exactly mirroring how STORY-538's `aida schema`
made the storable substrate *derived from one reflected source* rather than
hand-maintained at each consumer. From that single source we can **generate** the
un-driftable state diagram (the machine version of TASK-733's hand-drawn
lifecycle), **enforce** the guards (replacing the ad-hoc per-call-site checks
with one `validate_transition` bouncer), and — as a stretch — **render and diff**
the *empirical* machine reconstructed from the per-spec `history:` arrays
(TASK-121) against the declared one, surfacing every place reality has outrun the
model. This is the `aida schema` analog for the lifecycle: the substrate stops
being reverse-engineered from prose and code, and becomes one queryable,
test-pinned, generated artifact.

The recommendation is **phased and non-destructive**: ship generate-diagram-only
first (zero behavior change, immediate drift-proofing of the docs), then migrate
the guards one slice at a time behind the new validator, leaving the empirical
diff as a third opt-in phase. Nothing is enacted in this spike.

### Diagram (the thing we'd generate from the single source)

The spec lifecycle is **not one axis** — it is five orthogonal regions that a
single spec occupies simultaneously. The current scatter exists partly *because*
no one place models all five at once. The declared model names them explicitly:

```
 status     :  Draft → Approved → Planned → InProgress → Done → Completed
                  │                            │            ▲        ▲
                  │                            ▼            │   (merge auto-bump,
                  └──(reject)──► Rejected   NeedsAttention──┘    git-event trigger)
                                              (punt, only from InProgress;
                                               exits only to Appr/InProg/Rej)

 visibility :  active ⇄ archived          (archived legal ONLY for terminal+unqueued)
 queue      :  unqueued ⇄ queued          (queued ⇒ NOT archivable; BUG-492)
 lease      :  free ⇄ leased(session)     (leased ⇒ in-flight, pickability=blocked)
 park       :  pickable ⇄ parked(tag)     (park tags / human_only / blocked-by / decision)

 ILLEGAL combinations the model makes unreachable:
   archived ∧ queued                       ← BUG-492
   review:draft-only ∧ PR-closed asserted  ← BUG-493
   NeedsAttention ∧ status-advance-by-non-advisor ← BUG-482
```

Each `status` edge carries a **trigger-kind** tag — `CLI-command` (an operator /
agent ran `aida edit --status`), `LLM-decision` (a punt / advisor verdict), or
`git-event` (a referencing commit landed on the default branch) — and a
**guard** predicate that must hold for the edge to fire.

## Decisions

These are the calls this spike makes for the proposed model. They are
recommendations for operator sign-off, not enacted code.

- **Decision: one declarative table in `aida-core`, not a trait-per-state or a
  config file.** Chosen shape: a `pub const LIFECYCLE: &[Transition]` slice of
  plain structs (`Transition { from, to, guard: GuardKind, trigger: TriggerKind }`)
  in a new `aida-core/src/lifecycle.rs`. **Rationale**: `aida-core` is the crate
  both `aida-cli` and the MCP server already depend on, so a single source there
  is reachable from every consumer without a new dependency edge. A `const` Rust
  table (vs a TOML/YAML data file) keeps the guards as real, compiled,
  exhaustively-matched code — the `RequirementStatus` enum is already there, so
  `match`-exhaustiveness gives a free compile-time check that no status is left
  unmodelled. A data file would re-introduce a parse/validate step and lose the
  enum coupling. This mirrors STORY-538's choice to derive from reflected Rust
  types rather than a hand-kept manifest.

- **Decision: model the five regions as orthogonal, not as a single flattened
  state.** **Rationale**: flattening status × visibility × queue × lease × park
  into one enum is a combinatorial explosion (8 × 2 × 2 × 2 × N) and is exactly
  what the current scatter *avoids* by keeping each axis in its own predicate.
  The win is not collapsing them — it is putting the *cross-axis constraints*
  (the illegal combinations) in one place that every axis-mutating call must
  consult. So the table is `status` transitions **plus** a separate
  `INVARIANTS: &[OrthogonalInvariant]` list ("archived ⇒ terminal ∧ unqueued").

- **Decision: guards are named variants, not closures.** `GuardKind::RequiresAdvisorAuthority`,
  `GuardKind::MergeEvidenceOnDefaultBranch`, `GuardKind::TerminalAndUnqueued`,
  `GuardKind::None`, etc. **Rationale**: a named guard is renderable into the
  generated diagram ("Done→Completed [git-event: merge evidence]"), testable in
  isolation, and lets the existing predicate bodies (`has_advisor_authority`,
  `auto_bump_eligible_status`, `archive_guard_decision`) move *under* the guard
  variant unchanged — the migration is a re-home, not a rewrite. Closures
  couldn't be rendered or compared.

- **Decision: phase the enactment — diagram first, guards second, empirical
  diff third.** **Rationale**: the diagram generator is pure-additive (a new
  read-only `aida lifecycle --diagram` surface + a doc-gen step) with zero
  behavior risk, so it can land and immediately drift-proof TASK-733's hand-drawn
  diagram while the guard migration is still being reviewed slice-by-slice. See
  Recommendation.

- **Decision: keep `NeedsAttention` and the merge auto-bump's BUG-405
  exception in the declared model, not as special-cases bolted on.** **Rationale**:
  the current `auto_bump_eligible_status` *already* allows
  `NeedsAttention → Completed` via a git-event (BUG-405) even though the
  CLI-command path forbids it (`forbidden_attention_transition`). That is not a
  contradiction — it is *the same edge with two different trigger-kinds and two
  different guards*. The declared model expresses this cleanly: the
  `NeedsAttention → Completed` edge exists with `trigger: GitEvent, guard:
  MergeEvidenceOnDefaultBranch`, and is *absent* for `trigger: CliCommand`. The
  scatter hides that this is one coherent rule; the table makes it legible.

## Files (in build-order — PROPOSED, gated on approval)

Symbol-anchored. This is the future implementation shape, not work done in this
spike.

### `aida-core/src/lifecycle.rs` — NEW, the single source (Phase 1+2)

- `enum TriggerKind { CliCommand, LlmDecision, GitEvent }` — who/what fires the edge.
- `enum GuardKind { None, RequiresAdvisorAuthority, MergeEvidenceOnDefaultBranch, PunctFromInProgress, TriageOutcome, TerminalAndUnqueued, … }` — the named guard per edge.
- `struct Transition { from: RequirementStatus, to: RequirementStatus, trigger: TriggerKind, guard: GuardKind }`.
- `pub const LIFECYCLE: &[Transition]` — the declared status state machine, every legal edge as one row.
- `struct OrthogonalInvariant { name, predicate_desc }` + `pub const INVARIANTS: &[OrthogonalInvariant]` — the cross-axis illegal-combination list (archived⇒terminal∧unqueued, etc.).
- `pub fn legal_transitions(from: &RequirementStatus, trigger: TriggerKind) -> Vec<&Transition>` — query the table.
- `pub fn validate_transition(from, to, trigger) -> Result<(), String>` — the bouncer the guards migrate behind.
- `#[cfg(test)] mod tests` — a `lifecycle_table_is_exhaustive` drift-guard (every `RequirementStatus` appears as some `from`), mirroring STORY-538's `schema_enums_match_reflection`.

### `aida-cli/src/lifecycle_cmd.rs` — NEW, the read-only surface (Phase 1)

- `fn render_diagram(fmt: DiagramFormat) -> String` — emit the Mermaid/ASCII state diagram from `aida_core::lifecycle::LIFECYCLE` (the un-driftable TASK-733 diagram).
- `fn handle_lifecycle_command(...)` — wires `aida lifecycle [--diagram] [--json] [--empirical] [--diff]`.

### `aida-cli/src/main.rs` — re-home the scattered guards behind the validator (Phase 2)

- `fn status_requires_advisor_authority` / `fn status_advance_requires_advisor_authority`: become the body of `GuardKind::RequiresAdvisorAuthority`; the call sites (`main.rs` edit handler, the MCP `update_requirement`) call `validate_transition` instead of the bespoke check.
- `fn auto_bump_eligible_status`: becomes the body of `GuardKind::MergeEvidenceOnDefaultBranch` evaluated for `trigger: GitEvent`.
- `fn archive_guard_decision`: becomes `INVARIANTS` row `archived ⇒ terminal ∧ unqueued`, consulted by the archive path.

### `aida-core/src/models.rs` — re-home the NeedsAttention guard (Phase 2)

- `fn forbidden_attention_transition`: folds into `GuardKind::PunctFromInProgress` + `GuardKind::TriageOutcome` rows; the function becomes a thin shim over `validate_transition` (keep the shim for back-compat call sites).

### `aida-cli/src/burndown.rs` — park/sign-off axes referenced, not moved (Phase 2)

- `fn parking_tag`, `fn split_by_signoff`, `fn reconcile_held_for_review`: the park + queue-sign-off + held-for-review axes are documented *in* the model as orthogonal-region rules; the predicate bodies stay here but gain a doc-link to the single source.

### `aida-core/src/lifecycle_empirical.rs` — NEW, the stretch diff (Phase 3)

- `fn empirical_machine(history: &[HistoryEntry]) -> ObservedMachine` — reconstruct the *actual* edges from per-spec `history:` status-change rows (TASK-121).
- `fn diff_declared_vs_empirical(declared: &[Transition], observed: &ObservedMachine) -> Vec<Divergence>` — every edge taken in history that the table forbids (a real bug) or every declared edge never taken (dead rule).

## Critical Files

- `aida-core/src/lifecycle.rs` (new)
- `aida-cli/src/lifecycle_cmd.rs` (new)
- `aida-cli/src/main.rs` (`status_requires_advisor_authority`, `status_advance_requires_advisor_authority`, `auto_bump_eligible_status`, `archive_guard_decision`)
- `aida-core/src/models.rs` (`RequirementStatus`, `forbidden_attention_transition`)
- `aida-cli/src/burndown.rs` (`parking_tag`, `split_by_signoff`, `reconcile_held_for_review`)
- `aida-core/src/pickability.rs` (`pickability`, `blocked_by_incomplete`)
- `aida-cli/src/schema.rs` (the STORY-538 prior-art pattern to mirror)

## Reusable helpers (do not reimplement)

The whole point of the spike is that these *already encode the rules* — the
model re-homes them, it does not re-author them.

- `auto_bump_eligible_status` (`aida-cli/src/main.rs`) — the Done→Completed (and BUG-405 NeedsAttention→Completed) git-event guard. Already pure + tested.
- `auto_bump_enabled` / `auto_bump_done_to_completed` / `print_auto_bump_summary` (`aida-cli/src/main.rs`) — the merge-evidence scan + apply, called from `handle_pull_command` and `handle_db_reconcile_status` (TASK-226).
- `has_advisor_authority` / `advisor_authority_from` / `status_requires_advisor_authority` / `status_advance_requires_advisor_authority` (`aida-cli/src/main.rs`) — the ADR-3 / TASK-647 / BUG-482 authority gate, already factored into a pure core (`advisor_authority_from`).
- `forbidden_attention_transition` (`aida-core/src/models.rs`) — the STORY-332 NeedsAttention entry/exit guard; *the only existing function that already returns a transition verdict* — the seed of the validator.
- `archive_guard_decision` (`aida-cli/src/main.rs`) — the BUG-492 `terminal ∧ unqueued` archive invariant; already pure (`status, queued, force) -> ArchiveGuard`).
- `reconcile_held_for_review` + `DraftPrObservation` (`aida-cli/src/burndown.rs`) — the BUG-493 held-for-review-vs-PR-state reconcile; pure + testable.
- `parking_tag` / `classify` / `split_by_signoff` / `OpenBucket` / `explain_open` (`aida-cli/src/burndown.rs`) — the park axis + queue-sign-off axis + the "why is this open" classifier already enumerate the parked reasons.
- `pickability` / `Pickability` / `BlockedReason` (`aida-core/src/pickability.rs`) — STORY-333's *already-centralized* pre-pickup gate (human_only > NeedsTriage > BlockedBy). Proof-of-concept that one-truth consolidation works; the lease/park region of the model wraps this.
- `aida-cli/src/schema.rs` (STORY-538) — the exact prior-art pattern: derive a queryable surface from one source + a drift-guard test (`schema_enums_match_reflection`). The lifecycle model copies this discipline.

## Inventory: the scattered transition logic, mapped onto the model

The acceptance-criteria core — proving one source can replace today's scatter
without losing behavior. Each row is a current predicate, the rule it enforces,
the trigger-kind, and where it lands in the proposed model.

| # | Current predicate (file · symbol) | Rule it enforces today | Trigger-kind | Lands in model as |
|---|-----------------------------------|------------------------|--------------|-------------------|
| 1 | `main.rs · status_requires_advisor_authority` | Producing Approved/Planned/InProgress/Done/Completed needs advisor authority | CLI-command | `GuardKind::RequiresAdvisorAuthority` on every edge *into* those statuses |
| 2 | `main.rs · status_advance_requires_advisor_authority` (BUG-482) | A non-advisor may not advance Draft *or* NeedsAttention into the pipeline | CLI-command | Same guard, keyed on `from ∈ {Draft, NeedsAttention}` |
| 3 | `models.rs · forbidden_attention_transition` (STORY-332) | NeedsAttention enters only from InProgress; exits only to Approved/InProgress/Rejected | CLI-command / LLM-decision (punt) | `GuardKind::PunctFromInProgress` (entry) + `GuardKind::TriageOutcome` (exit) rows |
| 4 | `main.rs · auto_bump_eligible_status` (BUG-328/BUG-405) | A referencing commit on default branch bumps Approved/Planned/InProgress/Done/NeedsAttention → Completed | git-event | `… → Completed [trigger: GitEvent, guard: MergeEvidenceOnDefaultBranch]` rows |
| 5 | `main.rs · auto_bump_done_to_completed` + `handle_db_reconcile_status` (STORY-86/TASK-226) | The scan that *applies* #4 on `aida pull` / reconcile | git-event | The runtime that *fires* the GitEvent edges (calls the validator, doesn't re-encode it) |
| 6 | `main.rs · archive_guard_decision` (BUG-492) | Archive (visibility flip) legal only for terminal ∧ unqueued | CLI-command | `INVARIANTS` row: `archived ⇒ (Completed ∨ Rejected) ∧ ¬queued` |
| 7 | `burndown.rs · parking_tag` + `classify` (STORY-527) | Park tags / pending decision / blocker / epic make a spec un-pickable | (read-side filter) | Park region: parked-reason enumeration referenced by the model's lease/park axis |
| 8 | `burndown.rs · split_by_signoff` (STORY-546) + queue-add gate `main.rs · QueueCommand::Add` guarded by `has_advisor_authority` (TASK-647) | Queue membership *is* advisor sign-off; queue-add is advisor-authority-gated | CLI-command | Queue region: `queued` edge carries `GuardKind::RequiresAdvisorAuthority` (same guard as #1, one source) |
| 9 | `burndown.rs · reconcile_held_for_review` + `DraftPrObservation` (BUG-493) | A `review:draft-only` claim must reconcile against real PR state | git-event (forge observation) | Park/visibility cross-check: `INVARIANTS` row `review:draft-only ⇒ open draft PR` |
| 10 | `pickability.rs · pickability` (STORY-333) | human_only > NeedsTriage > BlockedBy → un-pickable | (read-side gate) | Lease/park axis: the *already-centralized* sub-model the new model wraps |

**Key finding: there is no existing central validator.** A repo-wide search
(`rg "valid.?transition|can_transition|state.?machine"`) finds nothing — the only
function that returns a transition verdict is `forbidden_attention_transition`,
and it covers exactly one of the eight statuses' edges. Every other rule is a
standalone predicate consulted at its own call sites. The agreement between them
today is *authorial discipline + per-slice unit tests*, not a structural
guarantee. That absence is the spike's whole justification.

**Behavior-preservation proof sketch:** every row above is already a *pure*
function (`status, queued, force) -> ArchiveGuard`, `(from, to) -> Option<String>`,
`(status) -> bool`, …). Re-homing a pure predicate under a named `GuardKind`
variant and routing the (unchanged) call sites through `validate_transition`
preserves the exact same verdict for the same inputs — the existing per-slice
unit tests become the regression net for the migration (run them green before and
after each slice moves). No rule is *merged* or *weakened*; the consolidation is
purely "same predicates, one index."

## What's generated / enforced from the single source

The acceptance-criteria deliverable (a/b/c):

- **(a) The state diagram — un-driftable.** `aida lifecycle --diagram` renders
  the Mermaid/ASCII state machine *from* `LIFECYCLE` + `INVARIANTS`. This is the
  machine version of TASK-733's hand-drawn lifecycle diagram: edit the table, the
  diagram regenerates; a doc-gen step (or a `plan verify`-style check) pins the
  committed diagram against the generated one so the docs *cannot* drift from the
  enforced rules. Phase 1 ships this alone, zero behavior risk.

- **(b) Guard enforcement — substrate-as-bouncer.** The ad-hoc per-call-site
  checks (`if status_advance_requires_advisor_authority(...) && !has_advisor_authority()`,
  the inline archive guard, the `forbidden_attention_transition` shim) are
  replaced by a single `validate_transition(from, to, trigger)?` bouncer that
  consults the table. This is the project's stated discipline (the
  `substrate-as-bouncer` memory): one programmatic gate, not N hand-copied checks
  a confident LLM (or a careless edit) could let drift. Phase 2, migrated one
  slice at a time behind green per-slice tests.

- **(c) STRETCH — empirical machine + declared-vs-actual diff.** Every spec's
  YAML carries a `history:` array of status-change rows (TASK-121 — the
  source-of-truth time series). `aida lifecycle --empirical` reconstructs the
  *observed* state machine (every status edge any spec actually took) and
  `--diff` compares it to `LIFECYCLE`: an observed edge the table *forbids* is a
  real bug (a transition that happened but shouldn't have — e.g. a pre-BUG-482
  self-re-approve), and a declared edge *never observed* is a dead rule worth
  pruning. This closes the loop STORY-538 opened for schema: the substrate stops
  being asserted and starts being *measured against reality.* Phase 3, opt-in.

## Illegal orthogonal-state combinations the model makes unreachable

The bug-class the single source prevents. Each is a cross-axis contradiction that
slipped past *because* the two axes were checked by two unrelated predicates:

1. **`archived ∧ queued`** (BUG-492). Visibility flip and queue membership were
   independent; the Session-63 reset swept 128 Approved specs (4 of them queued)
   into archived, leaving the queue pointing at hidden specs. The `INVARIANTS`
   row `archived ⇒ terminal ∧ ¬queued`, consulted by *any* path that sets
   `archived` or adds to the queue, makes the combination unconstructable.

2. **`review:draft-only ∧ PR-closed-but-asserted-open`** (BUG-493). The
   held-for-review *claim* (a tag) and the real *forge state* were derived
   independently; `aida why TASK-715` asserted draft PR #709 was held for review
   after #709 had been closed. The `INVARIANTS` cross-check `review:draft-only ⇒
   open draft PR exists` forces the reconcile (`reconcile_held_for_review`) at
   the one source rather than hoping each reader re-derives it.

3. **`NeedsAttention ∧ status-advanced-by-non-advisor`** (BUG-482). The punt
   state and the authority gate were separate; a non-advisor could self-re-approve
   a freshly-punted spec, bypassing the triage the punt exists to request.
   Modelling NeedsAttention's exit edges with `GuardKind::RequiresAdvisorAuthority`
   in the *same* table that defines them makes the bypass unmodelled.

4. **(latent) `Draft → Completed` direct, or any skip-state jump by a
   non-merge trigger.** Today nothing structurally forbids a hand `aida edit
   --status completed` from Draft (only the advisor-authority gate, which is about
   *who* not *whether the edge is legal*). The declared `LIFECYCLE` table, by
   *enumerating* the legal edges, makes "Completed is reachable only via the
   merge git-event guard, or via the documented manual path" a structural fact —
   the empirical diff (c) would have *caught* any such jump that already happened.

The general statement: **every bug in the list is a missing cross-axis
constraint, and the table's job is to be the one place all cross-axis
constraints live.** Centralizing the constraints is what makes the illegal
combinations unreachable rather than merely untested-against.

## Risks + gotchas

1. **Risk: the migration silently changes a verdict.** Re-homing a predicate
   could subtly alter behavior (precedence order, an edge case). **Mitigation**:
   migrate one slice per PR; the existing per-slice unit tests
   (`pickability::tests`, the `auto_bump_*` tests, the `archive_guard` tests, the
   `forbidden_attention_transition` tests) are the regression net — run them green
   before and after, and add a parity test asserting `validate_transition`
   returns the identical verdict as the old predicate over a sweep of inputs
   *before* deleting the old predicate.

2. **Risk: orthogonal regions over-coupled into one flattened enum.** The
   temptation to model `status × visibility × queue × …` as one giant enum
   explodes combinatorially and would make the table unmaintainable.
   **Mitigation**: keep the regions separate (status transitions in `LIFECYCLE`,
   cross-axis rules in `INVARIANTS`); only the *constraints between* axes are
   centralized, not the axes themselves.

3. **Risk: trigger-kind ambiguity — the same edge fires from two triggers with
   different guards** (the `NeedsAttention → Completed` CLI-forbidden /
   git-allowed case). **Mitigation**: make `trigger` part of the `Transition`
   key, so the table holds *one row per (from, to, trigger)* — the model
   expresses "legal by merge, illegal by hand" as two distinct rows, not a
   contradiction.

4. **Risk: empirical-diff false positives from pre-model history.** Old YAML
   `history:` rows may record edges that *were* legal under earlier rules
   (pre-BUG-482) but the current table forbids — the diff would flag them as
   "illegal," which is technically true but historically expected. **Mitigation**:
   the diff is a stretch/opt-in surface for *investigation*, not a gate; annotate
   divergences with the edge's first/last observed timestamp so an operator can
   tell "happened once, in March, before the fix" from "happening now."

5. **Risk: `aida-core` ↔ `aida-cli` boundary.** Some guards (`has_advisor_authority`)
   read process/env state (TTY, role, orchestrator detection) that lives CLI-side,
   not in `aida-core`. **Mitigation**: keep the *pure core* of each guard in the
   table (`advisor_authority_from(role, is_tty, orchestrated)` is already pure);
   the CLI passes the resolved booleans in. The table holds the *rule*; the CLI
   supplies the *context*. This is already how `advisor_authority_from` is
   factored — the spike just follows the existing seam.

## Tests (named, not "add tests")

- `lifecycle_table_is_exhaustive` — every `RequirementStatus` variant appears as some `from` in `LIFECYCLE` (the STORY-538 drift-guard analog).
- `validate_transition_matches_forbidden_attention_transition` — parity: the new validator returns the same verdict as the old `forbidden_attention_transition` over all (from, to) pairs.
- `validate_transition_matches_archive_guard` — parity over (status, queued, force).
- `validate_transition_matches_auto_bump_eligible` — parity over all statuses for `trigger: GitEvent`.
- `diagram_render_is_deterministic` — `render_diagram` over `LIFECYCLE` is stable (sortable, byte-identical across runs) so the committed-vs-generated doc check is reliable.
- `committed_diagram_matches_generated` — the lifecycle diagram checked into `docs/lifecycle.md` equals `render_diagram(...)` (the un-driftable pin).
- `invariant_archived_implies_terminal_and_unqueued` — the BUG-492 cross-axis rule holds for every (visibility, status, queue) triple.
- `empirical_diff_flags_forbidden_observed_edge` (Phase 3) — a synthetic history with an illegal edge is reported as a divergence.

## Verification

This is a SPIKE — the deliverable is this doc, so verification is "the doc
exists, traces SPIKE-56, and the inventory maps every named predicate to a real
symbol." Executable check that the cited symbols still exist (run from the repo
root):

```bash
ROOT="$(git rev-parse --show-toplevel)"
for sym in \
  status_requires_advisor_authority \
  status_advance_requires_advisor_authority \
  auto_bump_eligible_status \
  archive_guard_decision \
  forbidden_attention_transition \
  reconcile_held_for_review \
  parking_tag \
  split_by_signoff ; do
  rg -q "fn $sym" "$ROOT/aida-cli/src" "$ROOT/aida-core/src" \
    && echo "OK   $sym" || echo "MISS $sym"
done
# Expect: all OK — proves the inventory's symbol-refs are live, not drifted.

rg -q "<!-- trace:SPIKE-56 -->" "$ROOT/docs/plans/2026-06-10-aida-lifecycle-model.md" \
  && echo "OK   trace marker present"
```

## Followups

These are candidate child specs for operator approval — the phased enactment
path. Do NOT enact in this spike.

- Phase 1: `aida lifecycle --diagram` + the committed-vs-generated doc pin (generate-only, zero behavior change).
- Phase 2a: introduce `aida-core/src/lifecycle.rs` with `LIFECYCLE` + `validate_transition`, migrate `forbidden_attention_transition` behind it with a parity test.
- Phase 2b: migrate the advisor-authority gate (`status_requires_advisor_authority`) behind `validate_transition`.
- Phase 2c: migrate the merge auto-bump eligibility (`auto_bump_eligible_status`) behind the model's GitEvent guards.
- Phase 2d: express the BUG-492 / BUG-493 cross-axis invariants as `INVARIANTS` rows consulted by the archive + held-for-review paths.
- Phase 3: `aida lifecycle --empirical --diff` — reconstruct the observed machine from `history:` arrays (TASK-121) and diff against the declared table.

## Related

- Mirrors: STORY-538 (`aida schema`) — derive a queryable surface from one source + a drift-guard test; the lifecycle model is the schema analog.
- Builds on: STORY-333 (pickability.rs — the one already-centralized gate, proof the pattern works).
- Prevents the bug-class of: BUG-492 (archived ∧ queued), BUG-493 (held-for-review ∧ PR-closed), BUG-482 (NeedsAttention self-re-approve).
- Reads the substrate of: TASK-121 (per-spec `history:` arrays — source for the empirical diff).
- Un-drifts: TASK-733 (the hand-drawn lifecycle diagram).
- See also: `docs/lifecycle.md` (the current prose model), CLAUDE.md storage/lifecycle sections, `aida-core/templates/docs/aida/discipline/substrate-as-bouncer.md`.
