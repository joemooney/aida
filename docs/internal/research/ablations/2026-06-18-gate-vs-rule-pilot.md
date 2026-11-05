# Gate-vs-rule pilot result — P1 is weakened; the real variable is attention-distance

- **Date:** 2026-06-18
- **Probe:** EPIC-48, L2/P1 (substrate-as-bouncer). Spec STORY-655. Runner: `scripts/ablations/gate-vs-rule.sh`. Design: `2026-06-17-gate-vs-rule.md`.
- **Status:** Pilot, n=10/arm, single invariant (I1 = commit-format), single vendor (Claude headless). A real signal with a pre-registered interpretation; not a study.

## Result

| Arm | Landed-compliance | Gate-saves |
|---|---|---|
| R (rule-only, CLAUDE.md format rule, no gate) | **100% (10/10)** | — |
| G (gate, `AIDA_COMMIT_STRICT` commit-msg hook) | 100% (10/10) | **0% (0/10)** |

Pre-registered (design §5): Arm-R ≥95% → **P1 WEAKENED**. Verdict stands: for this invariant, a capable 2026 model honored the explicit in-context rule every time, and the gate did **no work** — it never rejected a single commit.

## What this does to P1 (honest scoping)

P1 as originally stated — *"to hold an invariant against a capable LLM you need a programmatic gate, not a rule"* — is **falsified for simple, immediate, in-context format rules.** A bigger 2024-vintage truth ("agents route around rules") did not reproduce here on the easy case. P1 is not dead, but it must be **scoped**: the gate's value is *conditional*, not universal.

The pilot tested the **easiest possible case for the rule**: the commit format is explicit, short, and the commit *is the task* — the rule is right at the moment of action. That is exactly where a rule should succeed. The original P1 evidence came from *semantic, multi-step, cross-cutting* invariants (don't approve your own spec; trace every change; don't merge over a RequestChanges) where the agent must remember + apply judgment across a long task, far from where the invariant bites.

## The synthesis (this is the pivotal insight — it unifies both experiments)

Put this pilot next to the competitive bake-off (2026-06-17): there, **Codex skipped a named rule** (the `--ai` gate "like aida intent") and lost the contest; here, **Claude followed a named rule 100%.** Same class of model, opposite rule-adherence. The difference is not the model and not "rule vs gate" — it is **how far the invariant sits from the moment of action, and how much it competes for attention:**

- Commit-format pilot: the rule **is** the task, stated immediately → 100% adherence, gate idle.
- Codex `--ai` gate: the rule was one line of **fine print in a 19-line brief**, to be applied deep inside a long implementation task → skipped.

> **The real variable is attention-distance, not enforcement mechanism. A capable model reliably honors invariants that are immediate, explicit, and at the point of action; it drops invariants that are buried, deferred, or competing for attention across a long task — and *that* is where a programmatic gate earns its place.** Rule-vs-gate is the wrong axis; near-vs-far-from-the-action is the right one.

### Restated P1 (the claim that survives, sharper than the original)

**Substrate-as-bouncer is a *selective* discipline, not a blanket one. Gate the invariants with high attention-distance (far from the action, buried in context, applied across long autonomous runs, or load-bearing-for-safety); trust the model on the immediate, explicit, point-of-action ones. Gating everything is over-engineering; gating nothing is the Codex `--ai` miss. The skill is choosing *which* invariants to make the substrate enforce.**

## Why this matters for the product (not just the paper)

This is a *design principle* for AIDA, and an honest one: AIDA should **not** reflexively add a gate for every rule (surface bloat + false confidence). It should gate the few high-attention-distance invariants — the ones that bite far from where they're stated or that run unattended (merge-over-RequestChanges, approve-your-own-spec, the unattended-drain safety rails) — and leave the immediate/explicit ones to the model + CLAUDE.md. The gate-vs-rule question becomes a *triage*: for each invariant, estimate attention-distance, gate only when it's high. That is a more defensible, less-over-engineered posture than "substrate enforces everything," and it's directly testable.

## Next (to firm this up — pre-registered in the design)

- **I2 — a harder, semantic, non-format invariant** (e.g. "don't approve a spec you authored," or "every code change carries a trace") run the same way. Prediction: Arm-R compliance drops below 95% and gate-saves become non-zero — i.e. P1 *holds* as attention-distance rises. This is the decisive follow-up.
- **Dose-response** — rerun I1 with a smaller model; prediction: Arm-R compliance falls, gate-saves rise (the gate's value is inversely proportional to capability × inversely to attention-distance).
- **Cross-vendor** — does the gate help Codex more than Claude on the same invariant? (The bake-off says Codex is the one that skips fine print.)

## Honesty / limits

n=10, one invariant, one vendor, deterministic grading (good — no judge bias). The task was deliberately trivial; the result is only directly valid for trivial+immediate invariants — which is precisely the scope it establishes. The synthesis claim (attention-distance) is a conjecture supported by two data points (this pilot + the bake-off); I2 is designed to test it.

<!-- trace:STORY-655 EPIC-48 | ai:claude -->
