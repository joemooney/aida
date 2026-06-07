# Plan: /aida-burndown — design sketch

Date: 2026-06-07
Specs: STORY-527
Status: Draft (design sketch — NOT an implementation plan; no code in this PR)
Complexity: design-only; estimated build ~1 skill + ~1 CLI subcommand (`aida burndown plan`) + ~120 test LOC, risk medium

<!--
  DESIGN SKETCH for STORY-527, per the sketch-first-pays-for-itself discipline.
  The companion doc (docs/aida/discipline/autonomous-burndown.md, TASK-695) already
  shipped — this is the "how do we actually build the runnable command" pass that
  the master/operator signs off BEFORE implementation, not at review time.
  Prefer SYMBOL refs over LINE refs. trace:TASK-92
-->

## Approach

`/aida-burndown` encodes the empirically-working autonomous drain (memory
`feedback_parallel_implementer_fanout_burndown`) as a runnable command so the
"never stop to ask" rules are *structural*, not a thing the agent must remember
— substrate-as-bouncer. The engine is Claude-Code-native: fan out implementer
subagents with `isolation: 'worktree'`, each taking one bounded ready spec to a
PR; the main session is the integrator (merge greens, reconcile, relaunch
waves); loop via wake-up. A plain `aida` subcommand cannot spawn Claude
subagents, so the work splits across two substrates: **`aida` owns
selector-resolution + the pickability gate** (it can answer "what is ready +
unblocked + bounded" deterministically from the graph), and **a `/aida-burndown`
skill owns the fan-out + integrator orchestration** (the part only the harness
can do). The recommended vehicle is a **skill driving Agent fan-out**, with the
selector extracted into a single new read-only CLI verb (`aida burndown plan
--json`) that is independently testable. We deliberately do **not** compile a
saved `burndown.js` Workflow as the primary vehicle (see Decisions).

### Diagram

```
  operator: /aida-burndown --status approved        (selector)
                     │
                     ▼
        ┌───────────────────────────────┐
        │ aida burndown plan --json      │  ← CLI: deterministic, testable
        │   selector → candidate set     │
        │   pickability gate per spec:   │
        │     ready? unblocked? bounded? │
        │   → {ready:[...], parked:[...]}│
        └───────────────────────────────┘
                     │ ready set (JSON)
                     ▼
        ┌───────────────────────────────┐
        │ /aida-burndown skill (engine)  │  ← only the harness can do this
        │  wave loop:                    │
        │   fan out N Agent(worktree)    │──► implementer subagent → PR
        │   each: 1 bounded ready spec   │──► implementer subagent → PR
        │   integrator: merge greens,    │──► implementer subagent → PR (parks on blocker)
        │     reconcile→Completed, pull  │
        │   re-resolve selector, relaunch│
        └───────────────────────────────┘
                     │  punt-and-continue: one blocker parks ONE spec
                     ▼            (tag + note), pipeline rolls on
              loop via wakeup until ready set empty
```

## (1) Vehicle: skill driving Agent fan-out, NOT a compiled Workflow

**Decision: ship `/aida-burndown` as a skill that drives Agent fan-out. Do not
make a saved `burndown.js` Workflow the primary vehicle.**

The two candidate vehicles, and why:

- **Compiled saved Workflow (`burndown.js`, mirroring `panel-review.js`)** —
  `panel-review.js` (`.claude/workflows/panel-review.js`) is a *fixed-topology*
  pipeline: a known N of analysts → verify → reconcile, with forced structured
  output schemas. It compiles well because the phase graph is static and the
  per-phase output is schema-shaped. Burndown is **not** fixed-topology: the
  wave width is `min(ready_set, budget)` and *changes every wave* as specs land,
  park, or unblock; the loop terminates on a data condition (ready set empty),
  not a fixed phase count; and the integrator does live git/CI/merge work
  (`gh pr merge`, `aida pull`, `aida db reconcile-status`) whose results steer
  the next wave. A saved Workflow would have to re-resolve the selector and
  re-plan its own topology each iteration — i.e. it would *contain* a skill's
  worth of dynamic decisioning anyway. Forcing it into the Workflow shape buys
  determinism we don't actually want here.

- **Skill driving Agent fan-out (RECOMMENDED)** — a skill is the natural fit for
  a *dynamic-width, data-terminated* loop. It re-runs `aida burndown plan
  --json` at the top of each wave (fresh ready set), spawns `Agent` subagents
  with `isolation: 'worktree'`, and runs the integrator inline. The determinism
  that *matters* — "only dispatch ready+unblocked+bounded specs" — lives in the
  CLI gate, not in the orchestration shape, so we get the substrate-as-bouncer
  guarantee without paying the static-topology tax.

**Trade-off summary.** Workflow = saved-script lane (deterministic replay,
checked-in build artifact) — right when topology is fixed (panel-review). Skill
= dynamic-generation lane — right when the topology is *data-shaped per run*
(burndown). See memory `feedback_workflows_saved_script_lane`: don't conflate
the two lanes. The pickability gate is the part that needs a bouncer, and we put
*that* in the CLI where it's a pure, testable function — the orchestration
around it stays a skill.

Forward door (not in slice 1): if usage shows operators want a *recordable,
parameterised* burndown (e.g. `--cross-vendor`, fixed lens set like
panel-review), a thin `burndown.js` could later wrap the same `aida burndown
plan` CLI to drive a fixed-width wave. The CLI-owns-the-gate split makes both
vehicles cheap; we just don't lead with the Workflow.

## (2) Selector resolution + the pickability gate — CLI surface

**Decision: add ONE new read-only verb `aida burndown plan` (reuse-heavy), not a
new resolver from scratch.** It is the single funnel every selector axis passes
through.

### Selector axes (all funnel through one gate)

```
aida burndown plan --batch <name>        # a cluster (reuse: list --tags batch:<name>)
aida burndown plan --tag <tag>           # by tag      (reuse: list --tags)
aida burndown plan --status approved     # ready backlog (DEFAULT) (reuse: list --status)
aida burndown plan --queue               # active role's queue (reuse: queue list)
aida burndown plan "<ad-hoc description>"# ad-hoc → resolved to a filter (skill-side NL→flags, then this verb)
aida burndown plan --json                # machine output for the skill
```

Default target when no selector is given: the **ready backlog for the active
role** (`--status approved` scoped by the role's scope filters — the same
scoping `aida list` already applies; `--no-scope` to bypass).

### Reuse vs new

Almost entirely **reuse** — the candidate-set resolution is `aida list` /
`aida queue list` machinery that already exists (`--status`/`--tags`/`--parent`,
role scoping, `--json`). The genuinely **new** part is the **pickability gate**
applied per candidate, which has no single existing home:

- **ready** — status is Approved (or in the role's queue), not Draft, not
  already In-Progress/Done/Completed/Rejected. Reuse the status field from the
  cache-backed listing.
- **unblocked** — no open `BlockedBy` edge. Reuse `aida graph <id> --blocked-by
  --json` (the verb behind `aida graph`); a candidate with any non-terminal
  blocked-by spec is parked, not dispatched. This is the cross-spec query a flat
  tool can't do — exactly what the graph verb exists for.
- **bounded** — no unresolved design fork. We do NOT have a hard "has a design
  fork" bit. Sketch heuristic for slice 1: a spec is **bounded** if it has an
  `## Acceptance` section (decision-free target) AND is not tagged with a
  fork/needs-decision marker (`needs:decision`, `design-fork`, parked/
  `NeedsAttention` tags). This is intentionally conservative — a borderline spec
  is *parked, not dispatched* (false-park is cheap; false-go costs a stalled
  subagent asking a human who isn't there). Tighten the heuristic with
  calibration data post-ship.

### Output contract

`aida burndown plan --json` emits:

```jsonc
{
  "selector": { "status": "approved", "role": "implementer" },
  "ready":  [ { "spec_id": "TASK-700", "title": "...", "reason": "ready+unblocked+bounded" } ],
  "parked": [ { "spec_id": "STORY-9", "title": "...", "reason": "blocked-by:STORY-5" },
              { "spec_id": "TASK-12", "title": "...", "reason": "no-acceptance-section" } ],
  "wave_suggested": 4   // min(ready.len, default cap) — advisory, skill applies budget
}
```

The skill consumes `ready[]` to fan out and surfaces `parked[]` to the operator
(transparency: "here's what I'm NOT touching and why"). The gate is a **pure
function over the graph + spec metadata** → trivially unit-testable on a fixture
spec set (the STORY-527 acceptance test), independent of any Claude fan-out.

## (3) Fan-out + integrator loop shape (skill side)

The skill implements the wave loop. Pseudocode (illustrative, not the impl):

```
plan = sh("aida burndown plan <selector> --json")
report_parked(plan.parked)                 # transparency, never block on these
while plan.ready is non-empty:
    wave = plan.ready[: min(len, wave_cap)] # wave_cap from budget/operator flag
    # FAN OUT — each subagent isolated in its own worktree, one bounded spec:
    for spec in wave:
        Agent(isolation:'worktree',
              prompt: "/aida-pickup <spec.id>: implement to acceptance, trace markers,
                       build+test+fmt, commit (SPEC-ID) trailer, push, open PR.
                       If you hit a fork you cannot defensibly resolve: PUNT —
                       tag the spec + leave a note, do NOT ask, do NOT down tools.")
    # INTEGRATE (main session does NOT implement):
    for pr in completed_prs(wave):
        if pr.ci_green and pr.clean:
            gh pr merge pr ; aida pull ; aida db reconcile-status --spec pr.spec
        else:
            park(pr.spec, reason)           # CI red / dirty → parks, pipeline rolls on
    plan = sh("aida burndown plan <selector> --json")  # re-resolve: newly-unblocked join
# loop via wakeup so waves keep launching for hours
```

Loop properties tied to the non-negotiables:

- **worktree isolation per implementer** — `Agent(isolation:'worktree')`;
  parallel agents never collide (the empirical win, 12 specs in one push).
- **integrator never implements** — main session only merges/reconciles/pulls/
  relaunches. Keeps authorship separation and a single merge gate.
- **punt-and-continue** — a blocker parks ONE spec (tag + note) and the wave
  rolls on; a CI-red PR simply doesn't merge (CI gates `main`). One spec's
  failure never halts the pipeline.
- **never stop to ask** — re-resolving the selector after each wave means
  "I can't make further progress" is structurally false while `ready[]` is
  non-empty; the loop only ends when the gate returns empty.
- **re-resolution unblocks chains** — specs blocked-by a now-merged spec become
  ready on the next wave's `aida burndown plan` call, with no manual nudge.

Termination / exit: when `ready[]` is empty, report the final `parked[]` set for
triage (mirrors the orchestrator drain's exit-2 "something shelved" semantics)
and stop relaunching. A `--goal` integration (`aida goal --batch NAME`) can
supply the machine-checkable stop condition for `/schedule`-driven runs.

## (4) Positioning vs the orchestrator drain

`/aida-burndown` is the **RECOMMENDED** autonomous-drain path *now*, and the
skill text + the already-shipped companion doc must say so without ambiguity:

- It deliberately uses the harness's **native subagent fan-out** rather than
  `aida queue work --auto-complete` (the orchestrator-spawns-`claude` keystone
  behind BUG-431 / STORY-492 / EPIC-33). It works partly *because* it sidesteps
  the orchestrator's lease-coupled phase machinery that is being hardened.
- They are **not competitors.** The orchestrator drain is hardened in parallel
  (EPIC-33); `/aida-burndown` is the path to reach for hands-off backlog
  draining. Use the orchestrator drain where its single-spec lifecycle is what
  you want.
- **No "which drain do I trust" ambiguity:** the skill's opening lines and the
  doc's "Relationship to the orchestrator drain" section both state: pick
  `/aida-burndown` for hands-off backlog draining; orchestrator drain for
  single-spec lifecycle. Don't run both against the same set.
- **Keyboard-not-drain carve-out preserved:** reliability fixes to the autonomy
  machinery itself (orchestrator, the burndown runner) ship supervised at the
  keyboard, not via an unsupervised headless burndown — a fix riding through a
  broken drain gets caught in the breakage (memory
  `feedback_reliability_fixes_use_keyboard_not_drain`).

## Decisions

- **Vehicle = skill, not compiled Workflow.** Rationale: burndown is
  dynamic-width + data-terminated; a saved Workflow's static topology fights
  that. Put the determinism in the CLI gate, keep orchestration as a skill.
- **One new read-only CLI verb `aida burndown plan`.** Rationale: gives the gate
  a pure, testable home and a stable JSON contract for the skill; reuses
  `list`/`queue list`/`graph` for candidate resolution rather than a parallel
  resolver that would drift.
- **Pickability gate = ready ∧ unblocked ∧ bounded, conservative.** Rationale:
  false-park is cheap, false-go costs a stalled subagent. Tighten with
  calibration data, don't gold-plate the "bounded" heuristic in slice 1.
- **Default target = active role's ready backlog.** Rationale: matches the doc
  and the most common hands-off intent; explicit selectors override.
- **Positioning baked into skill + doc text, not left implicit.** Rationale: the
  "which drain" ambiguity is a known trap; state it.

## Files (in build-order) — for the eventual implementation, NOT this PR

### `aida-cli/src/main.rs` (or a `burndown.rs` module) — new `aida burndown plan` verb

- `fn handle_burndown_plan_command`: parse selector flags → resolve candidate
  set (reuse list/queue resolution) → apply pickability gate per candidate →
  emit human table or `--json`.
- `fn pickability_gate(spec, graph) -> Pick { Ready | Parked(reason) }`: the pure
  gate. Reuses the `--blocked-by` graph traversal and spec metadata.

### `aida-core/templates/skills/aida-burndown.md` — new skill (master copy)

- Frontmatter (`name`, `description`, `allowed-tools: Bash, Task/Agent, Read`),
  the wave-loop body, the punt-and-continue prompt for subagents, and the
  positioning block.

### `aida-core/templates/commands/aida-burndown.md` — slash command shim

### `.claude/skills/aida-burndown.md` + `.claude/commands/aida-burndown.md` — per-file symlinks

- Via `make sync-templates` (per the template architecture in CLAUDE.md). Edit
  ONLY the master copy under `aida-core/templates/`.

## Critical Files

- `aida-cli/src/main.rs` (new `burndown plan` verb + pickability gate)
- `aida-core/templates/skills/aida-burndown.md`
- `aida-core/templates/commands/aida-burndown.md`
- `docs/aida/discipline/autonomous-burndown.md` (already shipped — only "The
  command" section may need a one-line sync once the CLI verb name is final)

## Reusable helpers (do not reimplement)

- `aida list` candidate resolution (`--status`/`--tags`/`--parent`/role scope/
  `--json`) — do not write a parallel lister.
- `aida graph <id> --blocked-by --json` — the transitive blocked-by traversal for
  the *unblocked* check. Don't hand-roll edge-walking.
- `aida queue list --json` machinery for the `--queue` selector axis.
- `aida goal --batch/--epic` — machine-checkable stop condition for scheduled runs.
- `aida db reconcile-status --spec` — integrator's Done→Completed replay.
- The `Agent` / Task tool with `isolation: 'worktree'` — the fan-out primitive.
- `panel-review.js` as a *structural reference only* for skill packaging — NOT as
  the vehicle.

## Risks + gotchas

1. **Risk**: the "bounded" heuristic mis-classifies (drags in a forked spec → a
   subagent stalls asking a human). **Mitigation**: conservative gate (park on
   doubt) + the subagent prompt mandates PUNT-not-ask, so even a mis-dispatched
   spec parks rather than hangs.
2. **Risk**: vehicle ambiguity — someone runs `/aida-burndown` AND
   `aida queue work --auto-complete` on the same set. **Mitigation**: positioning
   text in skill + doc; consider a soft warning if a live orchestrator lease
   exists on a target spec (check `aida session leases`).
3. **Risk**: integrator merges a green-but-wrong PR. **Mitigation**: CI gates
   `main`; this is the same trust model the working drain already used (12-spec
   push). Architecture-class specs should not be in a hands-off ready set — the
   gate + grooming keep them out.
4. **Risk**: re-resolving the selector every wave is O(graph) — perf on large
   backlogs. **Mitigation**: gate runs against the cache-backed listing +
   bounded graph traversal; waves are seconds-to-minutes apart (subagent runtime
   dominates), so resolution cost is negligible.
5. **Risk**: scaffolding sprawl — another skill that muddies the interface.
   **Mitigation**: this *replaces* the manual fan-out recipe operators run by
   hand today; it's a wedge, not a pile (memory
   `feedback_capture_vs_slop_in_article_flow`). One skill + one CLI verb.

## Tests (named, not "add tests")

- `burndown_plan_selector_status_default_resolves_active_role_ready` — default
  target = active role's Approved set.
- `pickability_gate_parks_blocked_by_open_spec` — a spec with an open BlockedBy
  edge lands in `parked[]` with `blocked-by:<id>` reason.
- `pickability_gate_parks_spec_without_acceptance_section` — bounded heuristic
  negative case.
- `pickability_gate_passes_ready_unblocked_bounded` — happy path → `ready[]`.
- `burndown_plan_batch_selector_funnels_through_gate` — `--batch` candidates
  still pass the gate (selector axis ≠ gate bypass).
- `burndown_plan_json_contract_shape` — JSON has `ready`/`parked`/`wave_suggested`.

(All pure CLI/gate tests on a fixture spec set — the STORY-527 acceptance bullet
"selector resolution + pickability filtering (pure), exercised on a fixture spec
set". No Claude fan-out in the test path.)

## Verification

```bash
AIDA_BIN="$(git rev-parse --show-toplevel)/target/debug/aida"
TMP=$(mktemp -d); cd "$TMP" && git init -q && "$AIDA_BIN" init >/dev/null
# fixture: one ready, one blocked, one fork-tagged
"$AIDA_BIN" add --title "ready bounded" --type task --status approved
"$AIDA_BIN" add --title "blocked" --type task --status approved          # add BlockedBy edge
"$AIDA_BIN" add --title "forked" --type task --status approved --tags needs:decision
"$AIDA_BIN" burndown plan --json | jq '.ready | length'    # expect: 1
"$AIDA_BIN" burndown plan --json | jq -r '.parked[].reason' # expect: blocked-by:..., needs:decision/no-acceptance
```

## Followups

- Optional `burndown.js` Workflow wrapper for fixed-width/parameterised runs.
- Calibrate the "bounded" heuristic from real park/false-park rates.
- Soft warning when a target spec has a live orchestrator lease.
- `--cap N` wave-width flag wired to budget (memory
  `feedback_multi_agent_budget_dispatching`).

## Related

- Encodes: memory `feedback_parallel_implementer_fanout_burndown` (the drain that works).
- Companion: `docs/aida/discipline/autonomous-burndown.md` (TASK-695, shipped — the why+rules).
- Positioned-against (not competing): `aida queue work --auto-complete` / EPIC-33
  (orchestrator hardening, BUG-431/STORY-492).
- Structural reference: `.claude/workflows/panel-review.js` (saved-Workflow lane).
- Vehicle-lane distinction: memory `feedback_workflows_saved_script_lane`.

## Sign-off needed (operator/master) before implementation

This is an architecture-class change (new CLI verb + new skill + multi-agent
orchestration), so per `feedback_one_master_advisor_until_subsystems` it needs
sign-off BEFORE a PR opens. Specific calls to confirm:

1. **Vehicle**: agree skill-not-Workflow as the primary vehicle for slice 1.
2. **CLI surface**: agree the new `aida burndown plan` verb (vs overloading an
   existing command). Verb name (`burndown plan`) and JSON contract.
3. **Bounded heuristic**: agree the conservative "has `## Acceptance` ∧ no
   fork tag" gate for slice 1, with calibration to follow.
4. **Smallest-valuable-slice**: ship the **CLI verb + gate + its pure tests
   first** (deterministic, reviewable, the substrate-as-bouncer core), then the
   skill that drives fan-out on top — rather than landing the whole loop at
   once. The gate is the load-bearing, testable half; the orchestration is the
   harness-dependent half.
