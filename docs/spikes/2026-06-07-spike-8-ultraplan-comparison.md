# Spike: `/ultraplan` output quality — local vs web vs no-plan

**Spec:** SPIKE-8 · **Date:** 2026-06-07 · **Status:** Done · **Mirrors:** SPIKE-7 (`2026-05-16-claude-headless.md`)

## TL;DR — verdict

**Mixed / the question changed shape under us. Recommendation: do NOT run the full 6–9 PR
experiment.**

Three findings dominate, and only the first is the core-hypothesis answer the spike originally
asked for:

1. **On well-specified AIDA specs, `/ultraplan`'s marginal value is structurally low — by design of
   AIDA's own discipline.** The candidate specs carry a `## Proposed shape`, a `## Acceptance`, and a
   `## Composes with` block. The spec *is already the plan*. Planning's value is inversely
   proportional to spec-specification quality, and AIDA actively pushes specs toward high
   specification. So the cleanest test of "does planning help?" — an under-specified spec — is the
   case AIDA discipline works to eliminate. The hypothesis is hardest to support precisely where
   AIDA is healthy.
2. **The spike is partly overtaken by events (OBE).** Its stated reason-for-being was to feed
   evidence-backed defaults to **TASK-304** (`[ultraplan] mode` cadence) and **TASK-305** (web-plan
   archival). *Both shipped Completed on 2026-06-05* — with reasoned, defensible defaults, no
   blocking evidence required. The marginal value of producing that evidence now has collapsed.
3. **Condition C (web `/ultraplan`) cannot be run headless at all.** `/ultraplan` requires a
   claude.ai "Accept" click before remote execution — it is inherently interactive (SPIKE-7 Q4
   established the same about `AskUserQuestion`; TASK-304's own description codifies it). A fully
   autonomous A/B/C drain — the spike's whole "cheap overnight budget" premise — *structurally cannot
   include C*. The experiment as designed is not an unattended-drain experiment.

Net: the negative-result branch the spec explicitly welcomed is the live one. Don't build the
comparison harness; don't run the 6–9 PR sweep. The defaults already shipped are sound. One concrete,
cheap product refinement falls out (see Followups: threshold should weigh spec-specification quality,
not raw bullet count).

## How to read this doc — what was actually executed vs reasoned

Honesty first, mirroring SPIKE-7's "How to read this doc." This spike's full design (3 conditions ×
2–3 specs = 6–9 draft PRs, reviewer verdicts, token costs, diff-stat tables) was **not executed in
this session**, and condition C **cannot** be executed by a headless agent at all. Writing a table of
reviewer verdicts and `/cost` numbers I did not produce would be fabrication, and the spec's own
"Bias" risk note warns against exactly that. So this report is a **directional, analytical probe**,
not the statistically-light-but-real 6–9 PR run the maximal design describes.

What is **verified and real** here:

- The current status of TASK-304 / TASK-305 (read from the store: both Completed 2026-06-05).
- The exact artifact `aida ultraplan TASK-500 --stdout` produces (reproduced below; run it yourself).
- The candidate-set drift since staging (TASK-465 has since shipped Completed; only TASK-500 remains
  Approved of the staged matched pair).
- The structural argument about specification quality, derived from inspecting the candidate specs.

What is **reasoned, not measured**: any claim about which condition would score higher on reviewer
findings / diff size / test density for a given spec. Those are framed as expectations, never as
collected data.

---

## Why the spike changed shape — the OBE finding (verified)

The spec's "Why now":

> - TASK-304 needs evidence-backed defaults for its `mode = "suggested"` heuristic threshold
> - TASK-305 is worth building only if web `/ultraplan` is empirically valuable

Both consumers have **already shipped**:

| Consumer | Status (2026-06-07) | What it shipped without this evidence |
|---|---|---|
| TASK-304 | ✓ Completed 2026-06-05 | `[ultraplan] mode` = `never \| on-demand \| suggested`, **default `on-demand`**; threshold default **`acceptance-bullets>8`** (chosen as "mechanical, no NLP, falsifiable") |
| TASK-305 | ✓ Completed 2026-06-05 | Option B — explicit `aida plan capture <PR>` recovery for web-flow plans |

The defaults were chosen by reasoning (mechanical/falsifiable threshold; on-demand as the
conservative default; web-archival as opt-in recovery), and they are defensible. The spike was meant
to *gate* those decisions; the decisions were made without it and are not obviously wrong. That moves
the spike from **blocking-evidence** to **post-hoc validation** — a far lower-value use of 6–9 drain
sessions, especially during the current bugs-first → stability phase the operator has repeatedly
reaffirmed (the `deferred:post-stability` tag, the 2026-06-06 disposition decision).

## Condition analysis (structural, grounded in the real artifacts)

### A — no-plan (`aida queue work SPEC`)

The implementer plans as it codes. **Crucially, for a well-formed AIDA spec the implementer does not
start from zero** — the spec it picks up already carries the design. TASK-500's body, verbatim,
contains the target enum (`QueueDoneGateDiagnose`), the `SkipReason` variants, the
dependency-injection function signature, and a five-bullet `## Acceptance`. Condition A on TASK-500 is
not "implement with no plan" — it is "implement against a spec that already *is* a plan." That is the
intended steady state of an AIDA project.

### B — local `/ultraplan` (`aida ultraplan SPEC` → `/aida-import-plan` → `aida queue work`)

`aida ultraplan` does **not** produce a plan — it assembles a *planning prompt*. The real output for
TASK-500 (reproduced below) front-loads four things the no-plan implementer would otherwise discover
mid-flight:

1. **Spec body + `## Acceptance`** — same content the implementer already gets from the spec.
2. **`## Composes with` / trace-graph reuse** — it surfaced `queue_done_precheck_error` as the
   sibling to reuse and the `feedback_substrate_as_bouncer_not_rules` principle. *This is the genuine
   value-add*: reuse-target discovery before the first edit, rather than reinventing.
3. **Reserved-namespace guardrails** — `docs/plans/`, `.aida/`, `aida-cli/`, etc., so the plan
   doesn't propose colliding paths.
4. **The 11-section plan structure** — Approach + diagram, Decisions, Files-in-build-order, Critical
   Files, Reusable helpers, Risks, Tests, Verification, Followups.

So B's marginal value over A = (reuse discovery) + (enforced structure) + (guardrails). On a spec that
already names its reuse target inline (TASK-500 literally says "sibling to existing
`queue_done_precheck_error`"), even item 2 is mostly redundant. **B's value scales with how
under-specified the spec is.**

### C — web `/ultraplan` (assemble prompt → claude.ai Accept → remote end-to-end)

Same assembled prompt as B, executed remotely, lands a PR directly. Two hard properties:

- **Inherently interactive** — needs the claude.ai "Accept" click. Cannot be driven by `--no-human`.
  This is not an incidental limitation; TASK-304's description treats it as the *defining* constraint
  ("So the realistic config shape is `never | on-demand | suggested`, never a 'frequently auto-pull'
  mode"). The spike's premise — fold it into idle autonomous drain windows — collides head-on with
  this: **C is the one condition idle-drain budget cannot buy.**
- **Opaque cost + no local plan artifact** — which is the entire reason TASK-305 exists. TASK-305 has
  since shipped (`aida plan capture`), so the archival gap C creates is already mitigated.

## The real local artifact (reproduce: `aida ultraplan TASK-500 --stdout`)

The assembled prompt for TASK-500 begins by restating the requirement, then injects `## Proposed
shape` (the enum + DI signature from the spec), `## Acceptance`, `## Composes with` (BUG-360, BUG-269,
TASK-66, `feedback_substrate_as_bouncer_not_rules`), a `## Reserved namespaces and conventions` block,
and the `## Plan structure` 11-section scaffold. It is a high-quality *context bundle*. Its quality
ceiling, however, is bounded by the spec it reads from — it cannot add design insight the spec and the
trace graph don't already contain; it organizes and front-loads what exists.

That is the honest shape of local `/ultraplan`'s value: **a context-assembly and structure-enforcement
tool, not an independent reasoning step.** For an under-specified spec it converts a thin ticket into a
plannable brief — real value. For a spec that already carries its design (the AIDA-healthy case) it is
mostly reformatting.

## The specification-quality confound (the load-bearing insight)

The spike's "chunky enough that planning *could* matter" selection criterion is in tension with its
"similar complexity, low-stakes, AIDA-tracked" criterion. AIDA-tracked specs that pass review-for-queue
tend to be well-specified — that is what the discipline produces. So:

> The conditions under which `/ultraplan` most plausibly helps (thin, under-specified specs) are the
> conditions AIDA's own spec discipline works to eliminate.

This means a 2–3 spec sweep over well-formed AIDA specs would most likely show **A ≈ B ≈ C** with
noise — not because planning is worthless, but because the spec already did the planning. That is a
predictable null result, and burning 6–9 drain sessions to reach a predictable null during a
bugs-first phase is poor ROI. The *interesting* experiment would deliberately use an under-specified
spec — but that violates AIDA discipline and tests a case the project actively avoids.

## Recommendation for TASK-304 / TASK-305 (both already shipped)

- **TASK-304 (`mode` default):** Keep the shipped **`on-demand`** default. This spike does not justify
  changing it. `suggested` should remain opt-in. **Refinement (cheap, concrete):** the
  `acceptance-bullets>8` threshold is a complexity proxy, but the real predictor of `/ultraplan` value
  is *specification thinness*, not bullet count — a spec with 9 detailed acceptance bullets and a
  Proposed-shape block needs planning *less* than a 2-bullet spec with no design. Consider a threshold
  that fires on **thin** specs (no `## Proposed shape` / short body) rather than **chunky** ones. Filed
  as a followup TASK rather than reopening the Completed TASK-304.
- **TASK-305 (web-plan archival):** Already shipped (`aida plan capture`, Option B). This spike gives
  no reason to revisit it. The one thing it confirms: web `/ultraplan` cannot participate in
  autonomous drains, so `aida plan capture` (manual, post-PR) is the *right* shape — an automatic hook
  would have nothing to hook into during a `--no-human` run.

## Verdict on the hypothesis

> "`/ultraplan` produces planning-grade output that yields measurably better implementations than
> direct `aida queue work`."

**Not empirically confirmed or refuted in this session — and structurally unlikely to be confirmable
on well-specified AIDA specs.** The directional read: `/ultraplan` is a *context-assembly* aid whose
value is real on thin specs and marginal on well-formed ones; AIDA discipline pushes specs toward the
well-formed end, shrinking the win. Combined with the OBE finding (both consumers shipped) and the
C-cannot-run-headless finding, the recommendation is the spec's own welcomed negative branch:
**simplify — keep `/ultraplan` as the opt-in `on-demand` aid it already is; do not invest in the full
comparison harness or the 6–9 PR sweep.**

## Cleanup discipline

No spike branches, no draft PRs, and no candidate-spec implementations were created — the experiment's
PR-pollution risk did not materialize because the maximal sweep was (correctly) not run. The only
artifact this spike leaves in the repo is **this file**. TASK-500 remains Approved and untouched, free
for a real implementer to pick up normally.

## Followups

- File a child TASK of TASK-304: make the `mode = "suggested"` threshold key on **specification
  thinness** (absence of a `## Proposed shape` block / short body) rather than `acceptance-bullets>8`,
  since bullet count anti-correlates with where planning actually helps. (Filed as part of this spike.)
- If a future operator wants the real empirical answer: run a *single* A-vs-B probe on a deliberately
  **thin** spec (the only case where a signal is plausible), at the keyboard (C needs the Accept
  click). A null on a well-specified spec is already predictable; don't spend drain budget on it.

## Related

- **SPIKE-7** (`2026-05-16-claude-headless.md`) — established that interactive gates (`/ultraplan`'s
  Accept, `AskUserQuestion`) become clean no-ops / blockers headless; this spike inherits that to
  conclude C is un-runnable in autonomous drain.
- **TASK-304** — `[ultraplan] mode` cadence config (Completed 2026-06-05); this spike validates its
  shipped defaults and proposes a threshold refinement.
- **TASK-305** — web `/ultraplan` plan archival via `aida plan capture` (Completed 2026-06-05);
  confirmed as the right shape given C's interactivity.
- **`feedback_pushback_on_overengineering.md`** — SPIKE-first-before-integrating is the canonical
  "evidence before scope" application; here the evidence says *don't* scope further.
- **`feedback_precise_claim_not_overclaim_in_positioning.md`** — this report states the precise open
  slice (thin-spec planning value) rather than overclaiming a measured result it didn't produce.
