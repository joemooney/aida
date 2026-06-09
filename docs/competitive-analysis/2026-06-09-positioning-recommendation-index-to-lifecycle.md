# Positioning recommendation: rethink the "missing index" headline

*Filed: 2026-06-09 · Author: advisor (claude) · Status: RECOMMENDATION — decision is the operator's*

> This is a proposal, not a change. It touches the README headline and **VIS-1**
> (the vision spec) — the project's core brand — so nothing here is applied
> unilaterally. It lays out the case, steelmans keeping things as-is, offers
> concrete options, and recommends one. The decision is yours.

## The trigger

The 2026-06-09 Lane D red-team (`2026-06-09-weekly-scan.md`, Kill-shot 3) names a
positioning collision, and the analysis is sound:

> *"AIDA's own tagline — 'your project's missing index' (VIS-1) — collides head-on
> with these [auto-code-graph] tools, on their turf, where they win on
> effort-adjusted value. Meanwhile AIDA's genuinely-defensible value (intent/spec
> traceability, the why behind the code, lifecycle truth) is a harder, under-told
> story that doesn't get a hearing because the cheaper claimants already own the
> word 'index.'"*

The market verified the threat: CodeGraph, Augment's Context Engine, and CodeCompass
ship **auto-derived code graphs** (AST/dependency, built by the tool, served over
MCP) at **zero discipline cost** — and they're winning the "agent needs a
structural index" demand in benchmarks (the Navigation Paradox paper *validates*
that structure beats raw context; it just rewards the *auto* index). When a
newcomer hears "AIDA — your project's missing index," the nearest mental
neighbor is now a free, zero-ceremony tool that indexes their code for them.

AIDA's actual moat is a different index: the **intent** index — *why* the code
exists, spec↔code traceability, a maintained lifecycle. That's the under-told
story.

## What the collision is — and isn't

Be precise (overclaiming the problem is as bad as ignoring it):

- It **is** a *word*-collision. "Index" now cues "auto code graph" for the
  agent-tooling audience. AIDA pays a comprehension tax explaining "no, a
  *different* index" before its real value lands.
- It is **not** a value-collision. The auto-code-graph tools index *what exists
  in the code*; AIDA indexes *what was intended and whether the code still
  honors it*. Those are genuinely different artifacts. AIDA's "structure matters"
  premise is **validated** by the same research — it just needs to claim the
  half it actually owns.

## The case for keeping "missing index" (steelman)

This is not a slam-dunk change. Reasons to leave it:

1. **"Index" is humble and concrete.** The Trojan-horse strategy
   (`OVERVIEW.md`) depends on AIDA *sounding* modest — the intended first
   reaction is *"I could do this in 20 lines of bash."* "Index" invites that
   underestimation; "lifecycle truth" / "intent traceability" sound bigger,
   more enterprise, more ceremony — which can *raise* the perceived adoption
   cost and scare off the exact zero-discipline newcomer the funnel needs.
2. **It's woven in.** `first-project.md` lands on *"the epic is the index that
   ties them together."* The word does real work in the docs.
3. **Lane D's own conclusion is that words aren't the main lever.** The same
   report ranks **distribution/timing** as the larger risk: *"the danger isn't
   that the moat is shallow; it's that the gate is locked and the clock is
   running."* A tagline change with zero installs changes nothing for nobody.
   Don't over-invest in wording while the real bottleneck is *use*.

## The case for changing it

1. The word actively **routes the listener to a competitor's strength** before
   AIDA's differentiator gets a hearing. That's a tax on every first impression.
2. The **defensible** claim — intent traceability + lifecycle truth — is
   *under-marketed*. There's a ready, verified, incentive-anchored line for it
   (below) that ages better than any capability claim.
3. The fix is **cheap and reversible** (a headline + a vision-spec description),
   unlike the distribution problem.

## The line that already survives the convergence

From `2026-05-31-round2-moat-gaps-moves.md` — already in the substrate, already
anchored on *incentive* (which ages better than capability claims):

> *"AGENTS.md and Spec Kit standardized how agents read your project and your
> specs. AIDA is the graph underneath — stable IDs, typed relationships,
> enforced traces, and a lifecycle that keeps them all true — and it writes
> those standard files for you. It's the only one where an orchestrator drains
> that graph through a spec-grounded escalation cascade, and the only one
> portable across every vendor because it lives in git."*

That's the elevator pitch. The headline is the one open question.

## Three options for the headline

**Option A — Keep "index," qualify it.** Name *which* index, head-on:
> **AIDA — your project's missing index *of intent, not just code*.**

Keeps the humility and the existing doc weave; adds one clause that deflects the
collision. Lowest-disruption; preserves the Trojan-horse tone. Risk: a qualifier
is a weaker headline than a clean claim.

**Option B — Pivot to the durable-linkage claim.** Lead with the differentiator:
> **AIDA — where your project's specs and code stay permanently linked.**

Clean, concrete, owns the *intent↔code* ground the auto-index tools don't touch.
Risk: slightly bigger-sounding; ripples into `first-project.md` and OVERVIEW.

**Option C — Pivot to the lifecycle-truth claim.**
> **AIDA — the lifecycle that keeps your specs and code true to each other.**

Strongest differentiation, furthest from "index." Risk: most abstract; highest
ceremony-cost signal; biggest doc ripple.

## Recommendation

**Option A or B — lean A.** Option A is the smallest honest move that fixes the
exact problem (the word routing to a competitor) while preserving the
Trojan-horse humility Lane D *didn't* tell you to abandon. It keeps "index" (and
the doc weave) but adds the one clause — *"of intent, not just code"* — that
makes the differentiator legible on first contact. If you want a cleaner break,
B is the strongest replacement that stays concrete.

I'd **not** do Option C unless you've decided to deliberately reposition upmarket;
it trades the most humility for the most differentiation.

**And — heed Lane D's own ranking:** this is the cheap lever. The expensive,
higher-impact lever is *distribution* (getting to enough installs that the depth
surfaces at all). Spend ten minutes picking a headline; don't spend a week on it.

## If you pick a change, here's the blast radius

A change touches, in order:
1. `README.md` line 1 (the headline + the elevator paragraph under it).
2. `OVERVIEW.md` line 1 + the Vision section.
3. **VIS-1** (the vision spec — `aida edit VIS-1 --title/--description`).
4. A light pass on `first-project.md`'s closing "index" sentence (optional —
   Option A leaves it valid).
5. The `docs/positioning/` framing (these pages already lead with "lifecycle /
   graph," so they need little change).

This recommendation is filed as a change-request against VIS-1 so it doesn't get
lost. Pick A / B / C / keep-as-is, and I (or the next session) will apply the
chosen blast-radius in one pass.

---
*Frozen artifact — represents the advisor's read on 2026-06-09. Supersede with a
new dated recommendation rather than editing this one.*
