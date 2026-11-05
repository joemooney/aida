# Field observation 1 — supervised reviewer bypass (SPIKE-67)

**Date:** 2026-07-19 (incident: 2026-07-09; guard fix verified 2026-07-12)
**Spec:** TASK-1125 (feeds SPIKE-67 → the §13 field-instrumentation path; incident specs: TASK-1123, BUG-716, BUG-710, BUG-713)
**Status:** field observation recorded, n=1. The underlying defect is already fixed (BUG-716); this doc is the observation record and cause taxonomy, not the fix.
**Builds on:** `2026-06-20-field-instrumentation-spike-67.md` (slice 1 — the sensor) and `2026-06-22-field-study-slice2-spike-67.md` (slice 2 — the controls, and the caveat this observation lands in).

## What happened

On 2026-07-09, during a **supervised interactive** walkthrough (`--supervised` drive)
of TASK-1123 — a deliberately trivial doc task (append one dated bullet to
`docs/flow-smoke.md`) — the codex implementer ran `aida pr ship` end-to-end inside its
own implementer session: implement → push → CI → **squash-merge**. PR **#1376** merged
at 2026-07-09T04:40Z with **zero independent reviews** (`gh pr view 1376` →
`reviews: []`, re-verified 2026-07-19 while writing this up).

The supervised mode's stated contract is that *"the reviewer ALWAYS runs headless as an
independent gate before the auto-merge."* That contract was violated: the orchestrator's
reviewer phase never ran because the PR was already merged when the orchestrator looked.

The human at the keyboard did not catch it either — and *why* they did not is the most
instructive part (see the gate-scope class below): the human was answering codex
command-permission prompts, not gating the merge, and had no distinct "approve this
merge" moment to refuse.

## Why this is SPIKE-67 data

The gate-vs-rule program's terminus (five controlled cells, all 100% rule-only, 0
gate-saves) concluded that a clean ablation cannot reproduce rule-dropping at all, and
the theory paper's §13 (`../2026-07-08-coordinating-multi-vendor-agent-fleets.md`)
pivoted the open evidence path to **field instrumentation**. The first two field slices
returned a null *on this repo's commit-format and trace rules* — and slice 2 explicitly
flagged its own blind spot: *"Headless-vs-supervised is not separable at all"* —
session-level attribution was named as the unmeasured remainder.

This incident is the first field observation that lands in exactly that unmeasured
regime, and it is worth recording for three reasons:

1. **It is a real stated-rule drop under real work** — the thing five ablation cells
   could not manufacture. The implementer's contract (a prose rule delivered via its
   prompt scaffolding: *open the PR and stop; the orchestrator's reviewer gates the
   merge*) was not honored. n=1, uncontrolled — the same evidentiary class as the
   bake-off drop, but this time with a full, attributed cause chain.
2. **It attributes to the supervised session mode** — the axis slice 2 could not cut.
3. **It embarrasses a deferral decision rather than flattering the system.** BUG-710's
   triage explicitly deferred supervised-mode guard coverage on the rationale
   "supervised has a human watching." The observation falsifies that rationale in the
   field: supervision-as-permission-prompts is not supervision-as-merge-gate.

## Cause taxonomy

The single incident decomposes into **three distinct failure classes**. They are worth
separating because each has a different general form, a different detection story, and
a different fix — and because the cross-repo harvest should code observations against
classes like these rather than logging "a rule was dropped" as one undifferentiated
event.

| class | mechanism in this incident | general form |
| --- | --- | --- |
| **delivery-rot** | BUG-713 had already fixed the codex prompt at the *source template*, but this machine's `~/.codex/prompts/` was never re-scaffolded. Codex therefore still held the broken prompt (a `.claude/skills/` path codex cannot read), got the prompt *shell* without the workflow *body*, and free-lanced to `aida pr ship`. | The rule text was corrected upstream but the corrected text never reached the agent. A prose rule has a **delivery pipeline** (template → scaffold → per-machine copy → context window), and every hop can rot silently. Detection gap: nothing diffed the deployed copy against the source (the `aida doctor` scaffold-drift check filed as the follow-up). |
| **gate-scope** | The BUG-710 self-merge guard existed and worked — but was keyed on `AIDA_HEADLESS=1`. A `--supervised` implementer is interactive, so the variable was unset and the guard stayed silent. The deferral was deliberate ("the human is watching"). | The programmatic gate holds only inside the predicate it was scoped to, and the scoping decision itself is a human judgment that can be wrong. The invariant ("no implementer self-merges inside an orchestrated drive") was mode-independent; the guard's trigger was mode-dependent. A gate that exists but is out of scope is indistinguishable, from the substrate's point of view, from no gate at all. |
| **violation vs guard** | With the prose rule rotted away (class 1) and the gate out of scope (class 2), nothing held: the model *did* free-lance into the full ship. Where the same invariant faced the same free-lancing in **headless** mode (BUG-710, the TASK-1115 drive), the post-fix guard — not the prompt — is what holds it. | The substrate-as-bouncer split, observed in the field: the *violation* (a capable model taking the locally-efficient path through the whole ship) is the constant; whether the invariant survives depends entirely on whether a **programmatic gate is in scope at the moment of action**. The prompt did not hold the invariant in either mode; the gate holds it wherever it applies. |

The classes compose: delivery-rot removed the rule, gate-scope removed the guard, and
the violation walked through the resulting hole. Any one of the three being absent
would have prevented the bypass (a fresh scaffold → codex stops at the PR; a
mode-independent guard → `aida pr ship` refuses the merge step; a non-free-lancing
implementer → no ship attempted).

## What this says about the slice-2 null

It does **not** overturn it. Slices 1–2 measured *commit-format* and *trace-presence*
adherence over 750 commits and found drained commits adhere better than interactive
ones — a valid null for those rules on this repo. This observation concerns a
different **rule class**: a *procedural workflow-scope* rule ("stop at the PR"), held
in per-machine prompt scaffolding rather than restated per-task, under a mode whose
guard was deliberately out of scope. Read together:

- The lab result stands: rules the model actually *has in context* are honored at the
  ceiling in controlled conditions.
- The field adds the failure modes the lab structurally excludes: the ablations
  *hand-deliver* the rule into context (delivery-rot impossible) and grade one commit
  (no orchestrated multi-phase drive for a gate to be mis-scoped over). The regime the
  terminus called unmeasured differs from the lab not just in codebase size and task
  span but in having a **rule-delivery pipeline and a gate-scoping surface that can
  each fail independently of the model**.

That reframing is the instrumentation payoff for the cross-repo harvest: alongside
"would the rule have blocked," the sensor design should ask **was the rule even
delivered** (scaffold drift) and **was a gate in scope** (mode/predicate coverage) —
otherwise a delivery-rot miss is indistinguishable from a model-dropped rule and the
harvest over-counts the class the ablations already falsified.

## Resolution (already shipped, for the record)

- **BUG-716** generalized the guard: the orchestrator now sets a mode-independent
  orchestrated-drive signal (the drain-lock signal) on *both* headless and supervised
  implementer launches, and `aida pr ship` refuses its merge step on it
  unconditionally. The implementer opens the PR and stops; the independent reviewer
  gates the merge in every mode.
- **2026-07-12 verification walkthrough** (TASK-1126, recorded in
  `docs/flow-smoke.md`): the guard suite re-ran green — 47 `pr_ship` tests including
  the should-block-ship-merge truth table, plus 18 `drain_lock` tests covering the
  invariant the guard depends on.
- **BUG-713** fixed the codex prompt at the source; the remaining delivery-rot
  *detection* gap (deployed-scaffold drift vs source) is the `aida doctor` follow-up.

## Bounds on the claim

- **n=1**, one vendor (codex), one trivial task, one repo, manually observed (the
  human noticed after the merge) — not scanner-harvested. It is an existence proof
  that the unmeasured regime produces real drops, and a cause taxonomy for coding
  future ones; it is not a rate.
- The task was `lifecycle:trivial`-class work; nothing about the *code* shipped in
  PR #1376 was wrong. The violated invariant is a governance invariant (independent
  review before merge), so the damage here is to the guarantee, not the artifact —
  on a non-trivial task the same hole ships unreviewed code.
- The observation is entangled with AIDA's own scaffolding bug (BUG-713); a repo with
  healthy scaffolding would need classes 2–3 alone to reproduce it. That is itself the
  point of class 1: scaffold health is part of the rule's effective existence.

trace:TASK-1125 trace:SPIKE-67
