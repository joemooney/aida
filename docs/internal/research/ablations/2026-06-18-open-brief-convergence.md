# Experiment: open-brief bake-off — convergence is driven by the substrate, not the brief

- **Date:** 2026-06-18
- **Probe:** EPIC-48, L1/L5. Spec SPIKE-64 (run 2). Subject: `aida health` (STORY-658), a deliberately OPEN brief.
- **Status:** Run 2 of the cross-vendor program. Pilot (n=1 task); a sharp, falsifiable claim, not a settled result.

## Setup + result

Same method as run 1 (Claude vs Codex, headless, identical brief, rubric judge) — but the brief was **deliberately open**: goal stated ("a fast, honest, at-a-glance read on project health"), with command name, signals, shape, and output **all left to the implementer.** The prior run's conjecture was: *a prescriptive brief causes convergence; an open brief should cause architectural divergence.*

**Claude won 4.5–3.5.** Decisive on correctness/robustness, test depth (14 vs 4), and integration cleanliness — and all three for the *same structural reason*: Claude **reused** the canonical lease/queue/velocity read paths, so it inherited the correct three-way `Dormant` lease classification; Codex **re-derived** a parallel two-way liveness probe and shipped a regression (a deleted-worktree lease misclassified, the `Dormant` concept lost). Synthesis call: ship Claude, graft only Codex's severity-ordered "Why:" issue ordering (~30 lines); do **not** take Codex's 0-100 vanity score or its duplicate readers.

## The finding (the conjecture is falsified)

Despite the open brief, both vendors converged on **the same command name (`aida health`), the same two axes (backlog + coordination), the same module, the same three-band verdict, the same "surface the worst, not a vanity average" philosophy, and the same signal set.** Brief openness did **not** produce divergence. The conjecture's causal arrow is wrong.

### The sharper claim that survives

> **Cross-vendor convergence is driven primarily by the constraint surface of the problem + the substrate, and secondarily by shared model priors — not by brief prescriptiveness. A prescriptive brief and a constraining substrate are *substitutable* sources of the same convergence; removing the brief's prescription does not create divergence when the substrate already supplies it. Architectural divergence appears only in the residual judgment layer the substrate leaves unspecified — and that residual is exactly where the quality gap opens.**

The diffs prove the mechanism: convergence was **total** where the substrate dictates the answer (both read the same physical paths — queues, leases, drain locks, findings — so there is one signal set and one way to read each), and divergence survived **only** in the rollup math (worst-anchored `max` vs. a 100-point deduction) and the reuse-vs-reimplement decision. The tighter the substrate couples a decision, the tighter the convergence on it.

**Corollary (testable):** to induce genuine architectural divergence from a multi-vendor run, you must **vary the substrate or pose the problem before the substrate exists** — not loosen the brief. Brief-openness is the wrong lever; substrate is the lever.

## Two consequences (one for the research, one for the product)

1. **For the research (L1/L5):** this *strengthens* the intent-substrate thesis from a new angle. The substrate doesn't just reduce coordination cost (the standing L1 claim) or hold the invariants (P1) — **it determines the SHAPE of what every vendor's agent builds.** Whoever owns the substrate shapes the fleet's output, regardless of which vendor executes. That is the deepest statement of AIDA's bet yet: *own the shared substrate → shape the multi-vendor fleet's work.* It also reframes P8a — the reason no single vendor builds portable coordination is the same reason the substrate is the lever: the substrate is the neutral ground none of them controls.

2. **For multi-vendor competition as a capability:** the value of running N vendors is **NOT architectural diversity** (you'll get convergent designs on a real codebase). It is **execution-quality variance within the converged design** — run 1 and run 2 both produced the same design implemented with materially different correctness, and only running both surfaced the regression (Codex's lease bug). So a `aida compete`-style feature should be framed as **quality-assurance / regression-catching** (run a spec through N vendors + a judge, ship the best, catch the one that drifted from the substrate), not as a design-exploration tool. That is a defensible, honest product framing — and one only a neutral substrate-owner can offer.

## Honesty / limits

Pilot, n=1 task, self-evaluating judge (a Claude instance grading Claude-vs-Codex — a cross-vendor judge is the next control). The brief, though open on surface, pre-named the two axes + example signals, which plausibly anchored axis-convergence (a minor contributor; doesn't explain the name/rollup/path convergence). Repeat on a task with a LESS-constraining substrate to test the corollary directly.

<!-- trace:SPIKE-64 | ai:claude -->
