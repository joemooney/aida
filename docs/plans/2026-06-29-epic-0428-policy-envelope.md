# Plan: TASK-0429 — Advisor autopilot policy envelope

Date: 2026-06-29
Specs: TASK-0429 (parent EPIC-0428)
Status: Draft — **design only, needs master-advisor sign-off before any code**
Complexity: ~design doc + ~120 prod LOC + ~120 test LOC when built, 0 commits now, risk medium (governs authority — the failure mode is a confident-but-wrong auto-disposition)

<!--
  This is the FOUNDATION plan. TASK-0430 (audit/reversal), TASK-0431
  (product-role), TASK-0432 (mode composition) all depend on the envelope
  defined here. Do NOT implement — advisor-architecture sign-off first.
-->

## Approach

Advisor autopilot is **not a new disposition engine** — it is a *bounded-authority
envelope* wrapped around the existing `aida groom` / `aida groom --apply` pass
(STORY-560 / STORY-708) and its `IntakeConfig` policy (`aida-cli/src/intake.rs`).
Today `groom --apply` is **binary**: a cold-boot advisor proposes a disposition
per spec and `--apply` executes *all* of them (approvals + a `backlog groom`
queue step). The only governance is the *candidate fence*
(`select_intake_candidates`) — it decides **which specs** are touchable
(do-not-approve classes, keystone/supervised, deferred, risk-above-ceiling all
fenced out) but, once a spec is in the fence, places no per-**action** limit.

Autopilot adds the missing axis: a per-**action-class authority map** that says,
for each thing the advisor can do (approve / reject / dedupe / tag / queue /
park / route / comment / ask), whether autopilot may execute it **autonomously**,
must only **propose** it, or may **never** do it. An action auto-executes only
when four gates all pass: (1) the spec clears the existing fence, (2) the action
class is `auto` in the envelope, (3) the decision is groundable in recorded
substrate (Type-A or recorded-B — the Type A/B/C calibration from
`docs/architecture/autonomy-and-escalation.md` §3), and (4) it is under the risk
ceiling. Any gate failing routes the action to *propose* or *park/escalate*. The
default bias when uncertain is **park/escalate, never approve** — the same
conservative-escalation bias the advisor tier already runs on.

The envelope is a config struct (`AutopilotEnvelope`) that *composes with*, not
replaces, `IntakeConfig`. The reusable substrate-as-bouncer guarantee (the HARD
bounds live in the pure fence function, not in a skill prompt an LLM can talk
itself past) is preserved and extended.

### Diagram — where the envelope sits

```
 open specs
     │
     ▼
 select_intake_candidates(cfg, filters)      ◄── EXISTING fence (which SPECS)
     │   fences out: do-not-approve classes, keystone/supervised,
     │   deferred, excluded-tag, risk-above-ceiling
     ▼
 eligible specs ──► cold-boot advisor proposes a disposition per spec
     │
     ▼
 ┌───────────────── AUTOPILOT ENVELOPE (which ACTIONS) ─────────────────┐
 │  for each (spec, proposed action):                                   │
 │    gate 1  spec in fence?            ─ no ─► drop (already excluded)  │
 │    gate 2  action authority == auto? ─ no ─► PROPOSE (hold)          │
 │    gate 3  Type-A or recorded-B?     ─ no ─► PARK / ESCALATE         │
 │    gate 4  under risk ceiling?       ─ no ─► PARK / ESCALATE         │
 │    all pass ─────────────────────────────► AUTO-EXECUTE + audit      │
 └──────────────────────────────────────────────────────────────────────┘
     │                              │                       │
   executed                      proposed                parked/escalated
  (+ durable audit, TASK-0430)  (review surface)        (findings, tier 3)
```

## Decisions

- **Decision: autopilot is a config posture over `groom`, not a new command.**
  **Rationale**: EPIC-0428 explicitly says "do NOT reinvent groom; design the
  envelope that governs it." `groom` already owns the disposition judgment, the
  cold-boot caveat, the fence, the `--apply` execution path, and the
  propose-by-default gate. Autopilot is the *authority* layer. Surfaced as
  `aida groom --autopilot` (+ `[autopilot]` config); see TASK-0432 for the
  full mode-vs-flag-vs-posture decision (this plan assumes "config posture +
  one flag", TASK-0432 ratifies it).

- **Decision: authority is per-action-class with three levels — `auto` /
  `propose` / `never`.** **Rationale**: the nine advisor actions have wildly
  different blast radius. Tagging is reversible and cheap; rejecting a draft or
  approving one onto the buildable queue is not. A single "autonomy on/off"
  switch would force the most dangerous action to share a setting with the
  safest. Three levels map cleanly onto the existing three-mode ladder
  (auto-resolve / pause-and-ask / escalate).

- **Decision: the four gates are AND-composed and the fence stays the outermost,
  programmatic bound.** **Rationale**: substrate-as-bouncer
  (`feedback_substrate_as_bouncer_not_rules`). The HARD exclusions
  (keystone, security, architecture, do-not-approve classes, `needs-human` /
  `strategic`, `risk:high`, `blast-radius:high`) must be enforced in
  `select_intake_candidates`, where no LLM prompt can override them — not in the
  skill text. Autopilot adds gates *inside* the fence; it never widens it past
  the keystone bound without an explicit, separately-gated opt-in.

- **Decision: the grounding gate reuses the Type A/B/C calibration verbatim.**
  **Rationale**: this is exactly the resolve-vs-escalate primitive the advisor
  tier already runs (autonomy doc §3). Type-A (recorded principle) and
  recorded-B (recorded preference) → autopilot may resolve. Unrecorded-B and
  Type-C (synthesized in-flight context a cold boot can't reconstruct) →
  escalate. This keeps autopilot's authority *identical in kind* to the advisor
  escalation tier — autopilot is "the advisor tier, applied to grooming, with an
  explicit action-authority map" — so the corpus-growth feedback loop (§4) that
  shrinks escalations also shrinks autopilot's hold-for-human pile over time.

- **Decision: default envelope is conservative — only reversible, low-blast
  actions are `auto` out of the box.** **Rationale**: zero-config must be safe.
  Default `auto`: `tag`, `comment`, `dedupe` (flag-only, see below), `route`
  (to an existing queue), `park`, and `queue` of an *already-Approved* spec.
  Default `propose`: `approve` (draft→Approved) and `reject`. Default `never`:
  anything touching a fenced spec. A project widens explicitly
  (`approve = "auto"`), exactly as it can already widen `--risk high`.

- **Decision: `dedupe` is auto only as *propose-the-link*, never auto-reject the
  duplicate.** **Rationale**: declaring spec X a duplicate of Y and rejecting X
  is destructive and frequently wrong on cold-boot context. Autopilot may
  auto-add a `duplicate-of:<ID>` tag + comment (reversible, informational);
  the *reject* half routes through the `reject` action's authority (default
  `propose`). Splitting the action prevents the most common cold-boot error
  (wrongly killing a non-duplicate).

- **Decision: `ask` is a first-class autopilot action, not the absence of one.**
  **Rationale**: "park/escalate when uncertain" must be a recorded, durable
  action with a reason category (strategy / irreversibility / corpus-gap), not a
  silent no-op. It reuses the advisor tier's `needs-human` finding + spec
  comment (TASK-0430 owns the durability).

## Files (in build-order)

> Design plan — files listed are the *intended* build targets so the
> implementation plan is unambiguous after sign-off. No code is written now.

### `aida-cli/src/autopilot.rs` (new) — the side-effect-free envelope heart

- `enum ActionClass` — `Approve | Reject | Dedupe | Tag | Queue | Park | Route | Comment | Ask`. The canonical taxonomy of what autopilot can do.
- `enum Authority` — `Auto | Propose | Never`.
- `struct AutopilotEnvelope { authorities: BTreeMap<ActionClass, Authority>, intake: IntakeConfig, grounding_required: bool }` — composes the existing `IntakeConfig` (P1/P2/P3 fence policy) with the new per-action authority map. `grounding_required` defaults true (gate 3 active).
- `impl Default for AutopilotEnvelope` — the conservative default table above.
- `fn authority_for(&self, action: ActionClass) -> Authority` — lookup with the default fallback.
- `enum Grounding { TypeA, RecordedB, UnrecordedB, TypeC }` + `fn is_resolvable(Grounding) -> bool` (A or recorded-B). Mirrors the autonomy-doc §3 table; the *classification* is the agent's judgment (recorded in the proposal), the *gate* is pure.
- `struct Decision { spec_id, action: ActionClass, grounding: Grounding, risk: RiskLevel, reason: String, evidence: Vec<String> }` — one proposed disposition.
- `enum Outcome { Execute, Hold, Escalate(EscalateReason) }`.
- `fn evaluate(env: &AutopilotEnvelope, fenced_ids: &HashSet<String>, d: &Decision) -> Outcome` — the pure four-gate function. **This is the unit-test surface** — exhaustively testable with no I/O, exactly like `select_intake_candidates`.
- `fn parse_authority_overrides(toml_section: &str) -> BTreeMap<ActionClass, Authority>` — `[autopilot]` config parse, section-aware (mirror `intake.rs`'s hand-roll parse so it stays dependency-light).

### `aida-cli/src/intake.rs` — expose the fence set for gate 1

- Reuse `select_intake_candidates`; have the autopilot launcher pass its `eligible` set as `fenced_ids` complement so `evaluate` gate 1 is a set membership check, not a re-derivation. No fence logic is duplicated.

### `aida-cli/src/cli.rs` + `main.rs` — surface

- Add `--autopilot` to the `groom` subcommand (clap), and `[autopilot]` to config load. The launcher builds the `AutopilotEnvelope`, runs the existing groom proposal pass, then routes each proposed disposition through `autopilot::evaluate` instead of the binary "execute all on `--apply`".

### `aida-core/templates/skills/aida-assess.md` (master template) — prompt

- Add an "Autopilot envelope" section: when launched with the autopilot env vars, the cold-boot advisor must (a) classify each decision's `Grounding` (A / recorded-B / unrecorded-B / C) and cite the substrate, (b) emit a `risk` per decision, (c) honor that `propose`/`never` actions are *output only*, never executed. The HARD bounds stay in the fence; the skill text is the *soft* guidance layer.

### `docs/architecture/autonomy-and-escalation.md` — new §8 "Advisor autopilot"

- Document the envelope as the grooming-time application of the §2 cascade + §3 calibration. (TASK-0432 owns the precise edit; flagged here for build-order.)

### `aida-core/templates/docs/aida/discipline/advisor-role.md` — vocabulary

- Add "autopilot envelope" to the advisor seat's documented authority.

## Critical Files

- `aida-cli/src/autopilot.rs` (new)
- `aida-cli/src/intake.rs`
- `aida-cli/src/cli.rs`
- `aida-cli/src/main.rs`
- `aida-core/templates/skills/aida-assess.md`
- `docs/architecture/autonomy-and-escalation.md`

## Reusable helpers (do not reimplement)

- `select_intake_candidates` (`aida-cli/src/intake.rs:382`) — the pure candidate fence (gate 1 + the HARD authority bound). Autopilot consumes its `eligible`/`fenced` partition; it does **not** re-derive any exclusion.
- `IntakeConfig` / `DispositionBias` / `OnApply` / `DEFAULT_DO_NOT_APPROVE_CLASSES` / `ALWAYS_EXCLUDE_TAGS` (`aida-cli/src/intake.rs:91-130`) — the existing P1/P2/P3 policy; `AutopilotEnvelope` *embeds* `IntakeConfig`, never forks it.
- `IntakeFilters` + `keystone_marker` (`aida-cli/src/intake.rs:317`, `:349`) — per-run `--only-tag`/`--exclude-tag`/`--risk` and the keystone fence reason.
- `presence::is_keystone_class` (`aida-cli/src/presence.rs`) — the canonical keystone/supervised detector (epic type or `keystone`/`architecture`/`security`/`supervised`/`needs-supervised-build`/`blast-radius:high`/`risk:high` tag). The gate-1 keystone bound MUST route through this so autopilot, the drain, `queue integrate`, and `groom` never disagree on "keystone".
- `presence::resolve_solo_posture` / `SoloPosture` (`aida-cli/src/presence.rs:390-428`) — the existing safe-vs-keystone posture; autopilot's "default-bias-when-uncertain" composes with this (TASK-0432).
- `backlog::RiskLevel` + `RiskLevel::from_str`/`token` (`aida-cli/src/backlog.rs:55-80`) — the risk ceiling type for gate 4. Reuse, don't add a parallel enum.
- The Type A/B/C calibration model (`docs/architecture/autonomy-and-escalation.md` §3, `.claude/skills/aida-advise.md`) — the grounding classifier for gate 3.

## Risks + gotchas

1. **Risk: authority creep — a project sets every action to `auto` and recreates
   "blindly bless everything", the exact failure the cold-boot caveat warns
   against.** **Mitigation**: gate 1 (the fence) and gate 3 (grounding) are
   **not** overridable by the authority map. Even `approve = "auto"` still
   cannot touch a fenced spec and still escalates an unrecorded-B/Type-C call.
   The authority map widens *which in-fence, grounded actions* auto-execute — it
   never disarms the HARD bounds. Document this invariant prominently.

2. **Risk: the grounding gate is the agent's self-classification — an
   over-confident advisor marks a Type-C "recorded-B".** **Mitigation**: this is
   the *same* risk the advisor tier already carries; the answer is the same —
   calibration mode (`[advisor] calibration_mode`) shadows cold-boot vs
   fork-from-live and surfaces disagreements as substrate-gap signals
   (`aida findings calibration`). Recommend running autopilot with calibration on
   for its first weeks. The conservative default (most actions `propose`, not
   `auto`) means a mis-classification on an `approve` is *held for review*, not
   executed, until a project explicitly widens.

3. **Risk: divergence from `groom`'s fence if autopilot re-implements any
   exclusion.** **Mitigation**: gate 1 is set-membership against
   `select_intake_candidates`' output — zero duplicated fence logic. A unit test
   asserts `evaluate` rejects any id not in the eligible set regardless of
   authority map.

4. **Risk: config schema drift — `[autopilot]` and `[intake]` overlap
   confusingly.** **Mitigation**: `[intake]` keeps owning the *fence* (which
   specs); `[autopilot]` owns *only* the action-authority map and the
   `grounding_required` toggle. Document the split in
   `docs/environment-variables.md` + a `[autopilot]` config comment. No field
   appears in both sections.

5. **Risk: "dedupe" and "route" are under-specified verbs.** **Mitigation**:
   pin them now — `dedupe` = add `duplicate-of:<ID>` tag + comment (reject is a
   *separate* action); `route` = `aida queue move`/add to an *existing* role
   queue (never creates a new routing target, never routes to a human's
   keystone queue).

## Tests (named, not "add tests")

- `evaluate_auto_action_in_fence_grounded_under_ceiling_executes` — happy path.
- `evaluate_propose_action_holds_even_when_grounded` — authority gate dominates.
- `evaluate_never_action_escalates` — hard exclusion.
- `evaluate_unrecorded_b_escalates_even_if_authority_auto` — grounding gate dominates authority.
- `evaluate_type_c_escalates` — synthesized-context bias.
- `evaluate_risk_above_ceiling_parks` — risk gate.
- `evaluate_spec_not_in_fence_drops_regardless_of_authority` — fence is outermost.
- `default_envelope_holds_approve_and_reject` — safe zero-config.
- `default_envelope_autos_tag_comment_park` — reversible actions flow.
- `dedupe_auto_adds_link_but_reject_half_routes_through_reject_authority` — split-verb invariant.
- `parse_authority_overrides_widens_single_action` — config narrowing/widening.
- `parse_authority_overrides_cannot_override_keystone_fence` — invariant: config can't widen past gate 1.

## Verification

```bash
AIDA_BIN="$(git rev-parse --show-toplevel)/target/debug/aida"
TMP=$(mktemp -d); cd "$TMP" && git init -q && "$AIDA_BIN" init >/dev/null

# A clear, bounded draft (in-fence) and a keystone-tagged one (fenced).
"$AIDA_BIN" add --title "rename a log label" --type task --status draft --tags "risk:low"
"$AIDA_BIN" add --title "redesign the lease protocol" --type task --status draft --tags "architecture,risk:high"

# Propose-mode autopilot: NOTHING is written, both specs still draft,
# the keystone one is shown fenced, the safe one shown held-for-review (approve=propose default).
"$AIDA_BIN" groom --autopilot --dry-run    # expect: fence shows architecture spec excluded (keystone)
"$AIDA_BIN" show TASK-... | grep -i 'status: *draft'   # both unchanged

# Widen ONLY tag authority; safe spec gets an auto tag, keystone spec still untouched.
printf '\n[autopilot]\ntag = "auto"\n' >> .aida/config.toml
"$AIDA_BIN" groom --autopilot --apply
"$AIDA_BIN" show TASK-<keystone> | grep -i 'status: *draft'   # STILL draft — fence held
# approve stayed propose (default) → safe spec NOT auto-approved
"$AIDA_BIN" show TASK-<safe> | grep -i 'status: *draft'       # still draft, approve held
```

## Followups

- TASK-0430 — durable audit + one-command reversal for every `Execute` outcome (depends on the `Decision`/`Outcome` shapes defined here).
- TASK-0431 — product-role recommendations as *evidence* feeding gate 3, never as authority.
- TASK-0432 — precedence when autopilot composes with `--zen` / `--no-human` / solo / `intake --apply`; ratify the surface (flag vs posture vs mode).
- Followup TASK (file at sign-off): wire `[autopilot]` into `docs/environment-variables.md` and the scaffolded `.aida/config.toml` comment block.

## Related

- EPIC-0428 (parent), STORY-560 / STORY-708 (`groom`), BUG-594 (keystone fence), TASK-827 (solo posture), STORY-306 / STORY-347 (advisor tier + calibration).
- `docs/architecture/autonomy-and-escalation.md` §2–§4, `docs/solo-mode.md`, `.claude/skills/aida-assess.md`.

## Recommendation + smallest first slice

**Recommendation**: build the envelope as a pure, exhaustively-unit-tested
function (`autopilot::evaluate`) that sits *between* the existing groom proposal
pass and its execution, governed by an `AutopilotEnvelope` that embeds the
existing `IntakeConfig`. Do not add a new command, a new fence, a new risk enum,
or a new keystone detector — reuse all four. Keep the conservative default
(approve/reject = `propose`) so zero-config autopilot can only ever auto-execute
reversible actions; a project opts into more, action by action.

**Smallest first slice** (one PR, no behavior change to existing `groom`):
ship `aida-cli/src/autopilot.rs` with `ActionClass`, `Authority`,
`AutopilotEnvelope::default()`, and the pure `evaluate` four-gate function +
its full unit-test suite — **wired to nothing**. This lands the governing
contract and its tests as a reviewable, low-risk artifact that TASK-0430/0431/0432
build against, before any launcher or config plumbing touches a live disposition.
The flag (`--autopilot`) and config parse are the *second* slice, gated on
master-advisor sign-off of the default authority table.
