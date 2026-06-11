# Plan: SPIKE-57 — formalize 'human' as a first-class role (escalation terminus) with an `aida human` vector

Date: 2026-06-11
Specs: SPIKE-57 (composes with TASK-130, STORY-562, STORY-561, STORY-559, STORY-547)
Status: Draft (design spike — no code enacted)
Complexity: design doc only; phased follow-ups sized in the Recommendation section, risk low

<!--
  SPIKE-57 design doc. Deliverable is THIS document, not code.
  Symbol refs over line refs (trace:TASK-92). Grounded against the code surface
  as of 2026-06-11 (main @ STORY-562 merged via PR #767).
-->

## Approach

AIDA's **agent** roles (advisor / implementer / reviewer / integrator / product) are first-class: routable (`queue add --for <role>`), enterable (`aida role enter`), queryable (`aida role show/list`). The **human** — the permanent terminus of the escalation cascade (implementer → advisor → human, per `docs/architecture/autonomy-and-escalation.md` §2) — is *not* first-class. "A human is required" is instead expressed five different ways across the codebase, none of which is a single queryable predicate or a role-vector verb. This spike designs `human` as the escalation-terminus role with one unifying idea: **a single "human-required" classification predicate** (the bottleneck signal) plus an **`aida human` vector** that is the home for both the bottleneck view *and* operator presence. The hard discipline the design holds throughout is the operator's line: **CLASSIFICATION** ("a human is required" — universal, newsworthy to everyone, the bottleneck signal) is in scope; **ASSIGNMENT** ("which human, what permissions" — multi-human RBAC) is explicitly a named follow-up, not this spike. The good news the inventory surfaces: the predicate already effectively exists as `burndown::OpenBucket::needs_human()`, and STORY-562 (`aida list human`, just shipped) is already the MVP view over it. This spike's job is to *name* that predicate as the canonical "human-required" classification, design the `aida human` role-vector that houses the view + presence, and stage the rest as operator-approvable follow-ups.

### Diagram

```
  FIVE scattered expressions of "a human is required" today
  ──────────────────────────────────────────────────────────
   (1) human_only marker (TASK-130) ─────┐
   (2) needs-human / needs-design-       │
       signoff / operator-action tags ───┤
   (3) review:draft-only tag ────────────┼──►  ONE predicate:
   (4) --escalate-blocks parking         │     human_required(spec)
       (→ NeedsAttention) ───────────────┤        │
   (5) burndown "needs a human nudge" ───┘        │ already ≈ OpenBucket::needs_human()
       bucket                                     ▼
                                          ┌──────────────────────┐
                                          │  aida human  (vector) │
                                          ├──────────────────────┤
                                          │ (view)  bottleneck    │  ← STORY-562 = MVP today
                                          │         grouped by WHY │
                                          │ (state) home / away    │  ← absorbs STORY-561
                                          └──────────────────────┘
            classification ───────────────┘     └─────────── assignment (which human / RBAC)
            (THIS spike)                                       = NAMED FOLLOWUP, out of scope
```

## Decisions

- **Decision: the unifying predicate is `human_required(spec) -> bool`, and its canonical implementation is the existing `burndown::OpenBucket::needs_human()`.** **Rationale**: the inventory (below) shows that `aida burndown explain` already classifies *every* open spec into an `OpenBucket` and `needs_human()` already returns true for exactly the human-attention set (`HeldForReview | AwaitingDecision | Ungroomed | Umbrella`). STORY-562 already filters on it. Inventing a parallel predicate would re-derive what `burndown.rs` owns and risk drift (the exact failure STORY-562 was careful to avoid — it reuses `explain_reasons` verbatim). So the design *names* `needs_human()` as the canonical "human-required" classification rather than building a new one. The `human_only` marker (signal 1) is the one input that is **orthogonal** to status — it is a permanent unpickability flag, not an open-bucket — so it folds in as an explicit additional clause (see Inventory note), not by collapsing it into a bucket.

- **Decision: `aida human` is a new dedicated role-vector verb — NOT a generalized `aida <role>` dispatch.** **Rationale**: the inventory confirms there is no general `aida <role>` pattern today. `aida advisor` exists but is *advisor-registration* (`AdvisorCommand`: register/status/schedule/handoff — fork-from-live plumbing), not a role vector; `aida role <verb>` is the generic role-lifecycle surface (enter/show/list/add). Building a fully general `aida <any-role>` dispatch is a larger, speculative surface (what would `aida implementer` even do as a bare verb?) and would collide with the existing `aida advisor` semantics. A dedicated `aida human` verb is the smallest-valuable-slice: it gives the terminus role a first-class home without committing to a role-dispatch framework. If a general pattern later proves its weight, `aida human` becomes its first instance, not a special case to unwind.

- **Decision: presence (`home`/`away`) lives UNDER `aida human` as subcommands, absorbing STORY-561.** **Rationale**: the operator's 2026-06-11 refinement (recorded on both SPIKE-57 and STORY-561) is explicit — the human role is the home for *both* "what needs a human" (the bottleneck view) and "is the human here" (presence), exactly symmetric with how agent roles carry session state via `aida role enter`/`role show`. `aida human away` / `aida human home` set presence; `aida human` (bare) shows the bottleneck view; `aida home` / `aida away` stay as optional short aliases. This makes the human role structurally parallel to agent roles (state + queryable surface) instead of bolting presence on as a disconnected pair of verbs.

- **Decision: hold the classification/assignment line — design ONLY "a human is required" + the view + presence.** **Rationale**: the operator's KEY PRINCIPLE. "Which human (A vs B), permissions, hierarchy" is multi-human RBAC and needs none of the formalization this spike does. Formalizing the *universal* "a person is needed" signal is valuable on a single-operator project today (it is the bottleneck dashboard); assignment is only valuable once there is more than one human, which AIDA does not assume. Assignment is a named follow-up (§Followups), deliberately undesigned here.

- **Decision: STORY-562 ships as-is and is *absorbed*, not restated.** **Rationale**: STORY-562 is Completed and merged (PR #767). It already implements the MVP view (`handle_list_human`) over the canonical predicate (`needs_human()`), grouped by reason. Restating it would be churn. The design treats `aida list human` as the shipped MVP view and `aida human` (bare) as the role-vector front door that *surfaces the same set* — the two converge rather than compete (see §Files: `aida human` bare can delegate to `handle_list_human`).

## Files (in build-order)

> This is a SPIKE. No code is enacted here. This section names *where the
> follow-up work would land* if the operator approves the phased path — it is
> a build-order map for the candidate follow-ups, not edits this PR makes.

### `docs/plans/2026-06-11-human-as-first-class-role.md` — this doc (the only artifact this PR writes)

- The inventory, the unified-predicate design, the `aida human` vector spec, the presence surface, and the phased follow-up recommendation.

### (Follow-up, Phase 2) `aida-cli/src/burndown.rs` — name the canonical predicate

- `fn human_required(facts: &OpenFacts) -> bool`: a thin public alias/wrapper over the existing `explain_open` → `OpenBucket::needs_human()` path, *plus* the orthogonal `human_only` clause. Single source the rest of the codebase can call by an intention-revealing name instead of re-checking five tag/marker forms. Keeps `needs_human()` as the bucket-level primitive.

### (Follow-up, Phase 2) `aida-cli/src/cli.rs` — the `aida human` verb + presence subcommands

- `enum HumanCommand { /* bare → view */ Home, Away, Status }` and a `Command::Human(Option<HumanCommand>)` arm. Short aliases `Home`/`Away` at the top level delegate here.

### (Follow-up, Phase 2) `aida-cli/src/main.rs` — wire the vector

- `fn handle_human_command`: bare → delegate to existing `handle_list_human`; `home`/`away`/`status` → presence (Phase 4).
- `fn handle_list_human` (exists, STORY-562): unchanged — `aida human` bare reuses it.

### (Follow-up, Phase 3) `aida-cli/src/main.rs` + `queue` surface — `--for human` routing

- Extend `QueueEntry::for_role` acceptance to include `human` as a valid route target; `aida queue add --for human` files into the human-attention set explicitly rather than only via derived buckets.

### (Follow-up, Phase 4) `aida-cli/src/...` — presence state file + consumers

- A timestamped presence file under `.aida/` (no daemon), TTL + auto-flip-to-home on interactive TTY, consumed by burndown-run / escalation defaults / questions surfacing (per STORY-561 acceptance, now under the human namespace).

## Critical Files

- `docs/plans/2026-06-11-human-as-first-class-role.md` (this PR)
- `aida-cli/src/burndown.rs` — `OpenBucket`, `needs_human()`, `explain_open`, `explain_reasons` (the predicate's home; follow-up only)
- `aida-cli/src/main.rs` — `handle_list_human` (STORY-562 view), `resolve_human_only`, role/queue handlers (follow-up only)
- `aida-cli/src/cli.rs` — `Command` enum, `RoleCommand`, `AdvisorCommand`, `--human-only` flags (follow-up only)
- `aida-core/src/models.rs` — `Requirement::human_only` (follow-up only)
- `aida-core/src/pickability.rs` — `BlockedReason::HumanOnly` (follow-up only)
- `aida-cli/src/pr_ship.rs` — `DRAFT_ONLY_TAG`, `is_draft_only_tagged` (follow-up only)
- `aida-cli/src/auto_complete.rs` — `EscalateMode` (follow-up only)

## Reusable helpers (do not reimplement)

The single most important finding of this spike: **the predicate already exists — do not build a new classifier.**

- `burndown::explain_open` / `burndown::explain_reasons` (`aida-cli/src/burndown.rs`) — the single classifier that buckets every open spec; STORY-562 reuses it verbatim, and any future `human_required` must too.
- `burndown::OpenBucket::needs_human()` (`aida-cli/src/burndown.rs`) — the canonical "human-required" predicate over open specs (`HeldForReview | AwaitingDecision | Ungroomed | Umbrella`). This *is* signals (2)–(5) unified already.
- `handle_list_human` (`aida-cli/src/main.rs`) — the STORY-562 MVP view; `aida human` (bare) delegates here, not a reimplementation.
- `resolve_human_only` (`aida-cli/src/main.rs`) + `Requirement::human_only` (`aida-core/src/models.rs`) + `BlockedReason::HumanOnly` (`aida-core/src/pickability.rs`) — the permanent marker (signal 1); the orthogonal clause the predicate must OR-in.
- `pr_ship::DRAFT_ONLY_TAG` / `pr_ship::is_draft_only_tagged` (`aida-cli/src/pr_ship.rs`) — the `review:draft-only` matcher (signal 3); already feeds `OpenBucket::HeldForReview`.
- `burndown::parking_tag` (`aida-cli/src/burndown.rs`) — the case-insensitive matcher for `needs-human` / `needs-design-signoff` / `operator-action` (signal 2); already feeds `OpenBucket::AwaitingDecision`.
- `auto_complete::EscalateMode` (`aida-cli/src/auto_complete.rs`) — `Blocks` (park → `NeedsAttention`, default) vs `Defaults`; the parking path (signal 4) lands specs in `AwaitingDecision`.
- `canonical_role_name` / `resolve_role_name` (`aida-cli/src/main.rs`) — role-name normalization (`dialog`→`advisor`); the pattern `human` routing should follow.
- `RequirementStatus::expand_filter_spec` (`aida-cli/src/main.rs`) — the `open`/`closed` status-alias expansion; `human` is a *positional* sibling to these (not a status), per STORY-562's implementation.

---

## Part 1 — Inventory: the five scattered "human-required" signals → one predicate

Each of the five maps onto the single `human_required` predicate. Four of the five already flow through `burndown`'s `OpenBucket` classification and so are *already unified* by `needs_human()`; the fifth (`human_only`) is orthogonal to status and folds in as an explicit clause.

| # | Signal | Where it lives (symbol) | What it means | Maps to predicate via |
|---|--------|------------------------|---------------|----------------------|
| 1 | **`human_only` marker** (TASK-130) | `Requirement::human_only` (`models.rs`); `BlockedReason::HumanOnly` (`pickability.rs`); `resolve_human_only` + `--human-only`/`--no-human-only` (`main.rs`/`cli.rs`) | Permanent "only a human may pick this up" flag; never auto-clears; Spikes default true | **Orthogonal clause.** Not an open-bucket (it is a pickability flag independent of status). The predicate ORs it in: `human_required = needs_human(bucket) OR req.human_only`. |
| 2 | **`needs-human` / `needs-design-signoff` / `operator-action` tags** | `burndown::parking_tag` (`burndown.rs`); written in auto-complete/escalation flows (`main.rs`) | Tag-borne "parked on a human decision" | → `OpenBucket::AwaitingDecision` → `needs_human()` = true. **Already unified.** |
| 3 | **`review:draft-only` tag** (built-awaiting-review) | `pr_ship::DRAFT_ONLY_TAG`, `pr_ship::is_draft_only_tagged` (`pr_ship.rs`); the STORY-529 draft-for-review gate (`main.rs`) | Work done; a draft PR is held for a human's review | → `OpenBucket::HeldForReview` → `needs_human()` = true. **Already unified.** |
| 4 | **`--escalate-blocks` parking** | `auto_complete::EscalateMode::Blocks` (`auto_complete.rs`); parks spec `NeedsAttention` | The headless advisor escalated; spec parked for morning triage | `NeedsAttention` → `OpenBucket::AwaitingDecision` → `needs_human()` = true. **Already unified.** |
| 5 | **`burndown explain` "needs a human nudge" bucket** | `OpenBucket::needs_human()` (`burndown.rs`) | The classifier's own "needs you" grouping | **IS the predicate.** This is the canonical implementation the other four feed into. |

**Key inventory finding.** Signals (2), (3), (4), (5) are *not actually five independent expressions at the data layer* — they are four inputs that already converge on `OpenBucket::needs_human()`. The fragmentation is at the **surface/vocabulary** layer (five different names, no single queryable predicate, no role verb), not the logic layer — exactly the diagnosis STORY-562 made for the *view*. Only signal (1), `human_only`, is genuinely orthogonal (a permanent unpickability marker, not a transient open-bucket), and it folds in as one explicit OR clause. So the "unify onto one predicate" work is mostly *naming* what `burndown.rs` already computes, plus OR-ing in `human_only`.

### The unified predicate (design)

```rust
// CANONICAL — names what burndown already computes; adds the orthogonal marker.
fn human_required(facts: &OpenFacts, human_only: bool) -> bool {
    let (bucket, _) = burndown::explain_open(facts);
    bucket.needs_human()        // (2)(3)(4)(5) — HeldForReview | AwaitingDecision | Ungroomed | Umbrella
        || human_only           // (1) — permanent marker, orthogonal to status
}
```

This is the *single predicate* SPIKE-57 asks for. It is universal (classifies any spec), newsworthy to all (the bottleneck signal), and re-derives nothing (delegates to `explain_open`).

---

## Part 2 — The `aida human` vector

### Surface

| Invocation | Behavior | Backing |
|-----------|----------|---------|
| `aida human` (bare) | The unified bottleneck view: every human-required spec, **grouped by WHY** | delegates to STORY-562's `handle_list_human` |
| `aida human away` | Set operator presence = away (timestamped, TTL) | presence file under `.aida/` (Phase 4) |
| `aida human home` | Set operator presence = home; auto-flip on interactive TTY | presence file (Phase 4) |
| `aida human status` | Show current presence + a one-line bottleneck count | presence file + predicate |
| `aida away` / `aida home` | Optional short aliases for the two presence verbs | delegate to `aida human {away,home}` |
| `aida list human` | The shipped MVP view (STORY-562) — sibling to `open`/`closed` | `handle_list_human` (unchanged) |

### Grouped by WHY

The view groups the human-required set by the reason a human is needed — the four buckets `needs_human()` returns, in most-actionable-first precedence order (matching `handle_list_human`'s existing `order` array):

- **held-for-review** (`OpenBucket::HeldForReview`) — built; a draft PR awaits your review (`review:draft-only`).
- **awaiting-decision** (`OpenBucket::AwaitingDecision`) — parked on a human decision: pending DecisionRequest, design-signoff, `operator-action`, `needs-human`, or `NeedsAttention` triage (this is where `--escalate-blocks` parking lands).
- **needs-triage** — surfaced within awaiting-decision as the `NeedsAttention`/shelved sub-reason (the resilient-drain shelf, EPIC-28). *Note: the operator's framing named "needs-triage" as a distinct WHY; today it is a reason-string under `AwaitingDecision`. A Phase-3 follow-up could promote it to its own bucket label if the triage queue grows enough to warrant separate grouping — flagged as a follow-up, not enacted.*
- **ungroomed** (`OpenBucket::Ungroomed`) — drafts needing an approve/reject decision.
- **(umbrella** (`OpenBucket::Umbrella`) — epics driven by children; surfaced because `needs_human()` includes them, though they are a softer "needs you" than the others.)

Plus the orthogonal **human-only** set (`req.human_only`) — specs a human must drive regardless of status — shown as its own group.

### Relationship to `--for human` routing and the `human_only` marker

- **`--for human` routing** is a *push*: explicitly filing a spec into the human-attention set (`aida queue add --for human`), distinct from the *derived* membership the predicate computes from status/tags. The vector view should show derived + explicitly-routed members together. This requires `human` to be an accepted `QueueEntry::for_role` target — the Phase 3 work. It establishes `human` as a route target symmetric with `--for advisor` / `--for implementer`, completing the "human is a first-class role" claim on the routing axis.
- **`human_only` marker** is the *permanent* membership input (vs the transient status-derived buckets). The vector ORs it in (the predicate above) and surfaces it as its own group, so a `human_only` Spike shows up in `aida human` even when its status would otherwise classify it as self-resolving.

### Why a dedicated `aida human` verb (the design choice the spec asks us to discuss)

There is no general `aida <role>` pattern today (confirmed in the inventory: `aida advisor` is advisor-*registration*, `aida role <verb>` is the generic role-lifecycle surface). SPIKE-57 must either (a) establish a general `aida <role>` dispatch or (b) add a dedicated `aida human`. This design chooses **(b)** for three reasons: (1) smallest-valuable-slice — the terminus role gets a first-class home without committing to a speculative role-dispatch framework; (2) the other roles have no obvious bare-verb semantics (`aida implementer`? `aida reviewer`?) — only `human` has a clear "show me the bottleneck + set my presence" meaning, because only the human is an *actor* with presence and an *attention queue* rather than a session seat; (3) it avoids colliding with the established `aida advisor` registration verb. If a general pattern later earns its weight, `aida human` is its first instance, not debt to unwind.

---

## Part 3 — Holding the classification / assignment line

This spike designs **only**: the `human_required` predicate (classification), the `aida human` view over it, and presence. It deliberately does **not** design:

- **Which human** a spec is for (assignee identity beyond the binary "a human").
- **Permissions / RBAC** — who may approve / merge / decide.
- **Human hierarchy / teams / escalation-to-a-specific-person.**

Rationale (the operator's KEY PRINCIPLE): the *universal* "a person is required" signal is the bottleneck dashboard — valuable on a single-operator project today, and it needs none of the assignment machinery. Assignment is only meaningful with more than one human, which AIDA does not assume. Assignment is a **named follow-up** (§Followups), explicitly undesigned here so the classification work can land without dragging RBAC in.

---

## Part 4 — Presence folds in (absorbs STORY-561)

Per the operator refinement on both SPIKE-57 and STORY-561, presence is a subcommand of the human role-vector — the human role is the home for both the bottleneck view *and* presence, symmetric with how agent roles carry session state.

- **`aida human away` / `aida human home`** set the operator's availability — a timestamped file under `.aida/` (no daemon), with a TTL (config, default ~8h) and auto-flip-to-home on any interactive TTY `aida` command, so a stale `away` can't leave the system acting unsupervised when the operator is back (STORY-561 acceptance §2).
- **`aida home` / `aida away`** remain as optional short aliases.
- **Consumers** (STORY-561 §3, now under the human namespace): burndown-run defaults headless when away / prompts when home; punt-escalation defaults to `--escalate-defaults` when away, surfaces interactively when home; decision-required specs accumulate quietly in `aida questions` when away; `human_only` / keystone work is not offered for autonomous pickup when away.
- **Presence is advisory** input to mode selection — it NEVER overrides integrity gates (CI, merge-on-green, the authority gate) (STORY-561 §4).

This **absorbs STORY-561** under the human namespace. STORY-561's acceptance is preserved verbatim; only the verb surface moves from standalone `aida home`/`aida away` to `aida human {home,away}` (+ the short aliases). STORY-561 should be restated/retagged as "the presence subcommand of SPIKE-57's `aida human` vector," or closed and re-filed as a child of the human-role work — operator's call (flagged in §Recommendation).

---

## Risks + gotchas

1. **Risk: re-deriving the predicate instead of reusing `needs_human()`.** A Phase-2 implementer might re-implement the five-signal check by hand. **Mitigation**: the design mandates `human_required` wrap `explain_open` → `needs_human()` + the `human_only` OR clause; the Reusable-helpers section names this as the #1 finding. Test parity against `burndown explain` output.
2. **Risk: `aida human` bare-view drift from `aida list human`.** Two front doors over the same set could diverge. **Mitigation**: `aida human` (bare) *delegates to* `handle_list_human`, not a copy. One implementation, two entry points.
3. **Risk: scope creep into assignment/RBAC.** The "which human" pull is strong once the view exists. **Mitigation**: §Part 3 draws the line explicitly; assignment is a named follow-up, not a Phase.
4. **Risk: presence as a daemon.** STORY-561 already warns against this. **Mitigation**: timestamped file + TTL + TTY auto-flip; no process. Carried verbatim into the design.
5. **Risk: `human_only` (orthogonal) double-counting.** A `human_only` spec whose status also lands it in `AwaitingDecision` should appear once, not twice. **Mitigation**: group precedence — show under its status bucket if it has one, else under the human-only group; OR the predicate, but the *view* de-dupes by SPEC-ID.
6. **Risk: `--for human` route conflicting with derived membership.** An explicitly-routed spec that the predicate would *not* classify as human-required (or vice-versa). **Mitigation**: union semantics — show derived ∪ explicitly-routed; the route is additive, never subtractive.

## Tests (named, not "add tests")

(For the candidate follow-ups, not this PR — this PR is docs-only.)

- `human_required_matches_needs_human_for_open_buckets` — parity: predicate agrees with `OpenBucket::needs_human()` across every status/signal (extend the existing `explain_open_buckets_every_status_and_signal` matrix).
- `human_required_ors_in_human_only_marker` — a `human_only` Spike in a self-resolving status is still human-required.
- `aida_human_bare_matches_list_human` — the bare vector view returns the same SPEC-ID set as `aida list human`.
- `presence_away_has_ttl_and_tty_autoflip` — stale `away` flips to home on interactive command (STORY-561 §2).
- `presence_advisory_never_overrides_integrity_gate` — away does not bypass CI/merge/authority gates (STORY-561 §4).
- `for_human_route_unions_with_derived_membership` — `--for human` spec appears in the view even when the predicate wouldn't derive it.

## Verification

This PR is **docs-only** — no Rust change. Verification is that the doc lints clean and the build is unbroken:

```bash
# From the repo (worktree-aware binary path — TASK-388)
AIDA_BIN="$(git rev-parse --show-toplevel)/target/debug/aida"

# 1. Nothing incidental broke (docs-only change expected to be a no-op for the build)
cargo build 2>&1 | tail -5

# 2. Plan lints clean (drifted refs / missing sections)
"$AIDA_BIN" plan verify docs/plans/2026-06-11-human-as-first-class-role.md   # || aida plan verify ...
```

Definition of done for the spike: the doc (a) inventories all five signals and maps each onto the single predicate, (b) specifies the `aida human` vector incl. grouping + `--for human` + `human_only` relationship, (c) holds the classification/assignment line with assignment as a named follow-up, (d) folds presence in (absorbs STORY-561), (e) confirms STORY-562 ships-as-is-and-is-absorbed, (f) presents a phased path as operator-approvable follow-ups without enacting any.

## Followups

> The phased path. **Candidate follow-ups for operator approval — none enacted in this spike.** Presented most-foundational-first; each is independently shippable.

- **Phase 1 (DONE): view-first.** `aida list human` — shipped via STORY-562 (PR #767). The MVP view over the predicate. Nothing to do; absorbed.
- **Phase 2: name the predicate + the `aida human` vector.** Add `burndown::human_required`, `Command::Human`, `handle_human_command` (bare → delegate to `handle_list_human`). The `aida human` front door + canonical predicate.
- **Phase 3: `--for human` queue routing.** Accept `human` as a `QueueEntry::for_role` target; union explicitly-routed with derived membership in the view. Establishes `human` as a first-class route target symmetric with agent roles.
- **Phase 4: presence (absorbs STORY-561).** `aida human home/away/status` + `aida home/away` aliases; presence file + TTL + TTY auto-flip; wire the STORY-561 consumers (burndown-run / escalation-defaults / questions surfacing). Advisory only; never overrides integrity gates.
- **Phase 5 (deferred, NAMED — not designed here): assignment / RBAC.** Which-human assignment, permissions, human hierarchy. Out of scope per the classification/assignment line; only meaningful with >1 human.
- **Housekeeping: restate or re-parent STORY-561** as the presence subcommand of SPIKE-57's `aida human` vector (operator's call: restate-and-retag vs close-and-re-file as a child of the human-role work).
- **Optional: promote "needs-triage" to its own `OpenBucket` label** if the `NeedsAttention` shelf grows enough to warrant separate grouping (today it is a reason-string under `AwaitingDecision`).

## Related

- Builds on: STORY-562 (`aida list human` — the MVP view, absorbed), STORY-547 (`burndown explain` classification — the predicate's source), TASK-130 (`human_only` marker)
- Absorbs: STORY-561 (presence → `aida human home/away`)
- Composes with: STORY-559 (advisor situational dashboard — `aida human` is its human-attention slice), the role model (`aida role` / `--for`), `aida advisor` (the registration verb that establishes there is no general `aida <role>` pattern yet)
- See also: `docs/architecture/autonomy-and-escalation.md` §2 (the implementer → advisor → human escalation cascade; `human` is the permanent Tier 3 terminus this spike formalizes)

<!-- trace:SPIKE-57 | ai:claude -->
