# Gate-vs-rule I2 — a semantic, high-attention-distance invariant (trace-coverage)

- **Date:** 2026-06-18
- **Probe:** EPIC-48, L2/P1 (substrate-as-bouncer). Spec STORY-655. Runner: `scripts/ablations/gate-vs-rule-i2.sh`. Design: `2026-06-17-gate-vs-rule.md`. Pilot it follows: `2026-06-18-gate-vs-rule-pilot.md`.
- **Status:** **COMPLETE + CROSS-VENDOR (2026-06-18).** Claude n=10/arm + Codex n=10/arm = 40 live headless runs. Two conjectures pre-registered and **both falsified**: attention-distance (by I2) and vendor (by cross-vendor I2). Both vendors honored the buried ambient rule at 100%, gate idle in all four cells. The surviving axis is **invariant type (output-shape vs procedural)**. CSVs: `results/i2-trace-coverage-e114c8a36.csv` (Claude), `results/i2-trace-coverage-codex-1cece44a3.csv` (Codex).

## Why I2 exists — the decisive test of the attention-distance conjecture

The I1 pilot (commit-format) weakened P1 on the *easiest* case for a stated rule: the rule **is** the task, stated at the point of action — Arm-R compliance hit 100%, the gate did no work. The pilot's synthesis was a conjecture, not a result:

> **The real variable is attention-distance, not enforcement mechanism. A capable model honors invariants that are immediate, explicit, and at the point of action; it drops invariants that are buried, deferred, or competing for attention across a long task — and *that* is where a programmatic gate earns its place.**

I2 is engineered to be the **opposite** of I1, to test that conjecture head-on:

| Axis | I1 (commit-format) | I2 (trace-coverage) |
|---|---|---|
| What the invariant is | the commit message format | "every code change carries a `// trace:<SPEC>` comment" |
| Where it lives | the rule *is* the task | a general project rule in CLAUDE.md |
| When it bites | immediately, at commit | deep inside a code-implementation task |
| Restated in the task? | no | no (deliberately — ambient rule, not a reminder) |
| Attention-distance | **low** | **high** |

If the attention-distance conjecture is right, I2 is exactly where the rule should start to leak and the gate should start to earn its place.

## Design (one variable: enforcement)

Same invariant, same standing rule, same model, same class of task. The **only** difference between arms is whether a gate rejects a violation.

- **Task (per iteration):** add a small, self-contained Rust function to `src/lib.rs` in a throwaway repo, then commit it. The function varies per iteration (10 variants: `double`, `is_even`, `max3`, `clamp_byte`, `reverse_str`, …) so trials are not identical — mirroring how I1 varied its input. **The task statement does NOT mention traces** — the invariant is the ambient CLAUDE.md rule, never task-restated (avoids the recency confound).
- **Arm R (rule-only):** the scratch repo's CLAUDE.md carries the standing trace rule. No gate. Whatever the agent commits lands.
- **Arm G (gate):** same CLAUDE.md PLUS a real `pre-commit` hook (substrate-as-bouncer) that rejects a commit whose staged `.rs` diff adds code lines but no `// trace:` line, forcing the agent to retry.
- **Grader (deterministic, no judge):** inspect the landed `.rs` change (seed-commit → HEAD diff). Compliant iff an added line carries `// trace:`. Landed-compliance = % of trials whose final committed code is trace-tagged. Gate-saves (Arm G) = trials where the gate rejected at least one attempt and a compliant change ultimately landed (the gate did real work).
- **Model:** `claude -p --permission-mode bypassPermissions`, headless, same as I1.
- **n:** configurable, default n=10/arm.

## Pre-registered interpretation (already fixed in the design doc, applied at higher attention-distance)

- **Arm-R landed-compliance < 95%** → **P1 HOLDS as attention-distance rises** — the conjecture is supported; the gate does real work (gate-saves > 0). This is the predicted outcome.
- **Arm-R ≥ 95%** → the attention-distance conjecture is itself **weakened** — even a buried, semantic rule held. A strong, honest negative.

## Results (n=10/arm, 20 live headless runs)

| Arm | Landed-compliance | Gate-saves |
|---|---|---|
| R (rule-only, CLAUDE.md trace rule, no gate) | **100% (10/10)** | — |
| G (gate, trace-coverage pre-commit hook) | **100% (10/10)** | **0% (0/10)** |

Every one of the 10 rule-only trials landed a `// trace:`-tagged `.rs` change **without any task reminder** — the ambient CLAUDE.md rule alone was enough. In the gated arm the hook **never fired a single rejection**: the model tagged correctly on the first commit attempt all 10 times, so the gate did exactly zero work, identical to I1.

Pre-registered verdict (design §"interpretation"): **Arm-R ≥ 95% → the attention-distance conjecture is WEAKENED.** Verdict stands. 100% ≥ 95%, at *high* attention-distance, with no recency reminder. The conjecture predicted the rule would start to leak here; it did not leak at all.

## The synthesis — the conjecture is falsified for Claude; the real variable is the *vendor*, not attention-distance

Put all three data points on one table:

| Run | Invariant | Attention-distance | Vendor | Rule-only adherence |
|---|---|---|---|---|
| I1 pilot | commit-message format | **low** (rule *is* the task) | Claude | **100%** |
| I2 (this) | trace-coverage on code | **high** (ambient, buried, never restated) | Claude | **100%** |
| Bake-off (2026-06-17) | use the `--ai` gate (fine print in a 19-line brief) | high | **Codex** | **dropped** |

Attention-distance was my conjecture to explain the bake-off miss. I2 was engineered to be the high-attention-distance case where, if the conjecture were right, the rule should leak — and **it held at the ceiling.** So attention-distance does **not** separate the held cases from the dropped one. The variable that *does* track the split is the **vendor**: both Claude runs held regardless of attention-distance; the only observed drop was a different model (Codex). 

> **[SUPERSEDED by the cross-vendor run below — kept for the honest record.]** *Restated claim (at the time): for a capable, rule-adherent model (Claude, 2026), a stated invariant is honored across both immediate AND buried/semantic cases — the gate does zero work in both. Substrate-as-bouncer is justified by (a) the vendor you cannot trust, (b) safety-critical invariants, (c) unattended drains. The right axis is adherence-confidence — mostly a property of the model.* **The "vendor is the axis" half of this was a fresh inference from a single Codex data point; the cross-vendor I2 run was built to test it, and falsified it too.**

This is a real loss for the original P1 ("you need a gate to hold an invariant against a capable LLM"): on two invariants spanning the attention-distance range, a capable LLM needed no gate. P1 survives only in its narrowed, conditional form.

## Cross-vendor I2 — the vendor hypothesis is ALSO falsified (Codex, n=10/arm)

I built the `--vendor codex` adapter and ran I2 unchanged with **Codex** as the implementer (`codex exec --dangerously-bypass-approvals-and-sandbox`, AGENTS.md+CLAUDE.md carrying the same ambient trace rule, identical task set, deterministic grader). Prediction (from the vendor hypothesis): Codex rule-only adherence drops below Claude's, gate logs real saves.

| Arm | Vendor | Landed-compliance | Gate-saves |
|---|---|---|---|
| R (rule-only) | Codex | **100% (10/10)** | — |
| G (gate) | Codex | **100% (10/10)** | **0% (0/10)** |

**Identical to Claude.** Codex honored the buried, never-restated ambient trace rule on every single trial; the gate fired zero rejections. CSV: `results/i2-trace-coverage-codex-1cece44a3.csv`. **The vendor hypothesis is falsified for this invariant** — two vendors, both at the ceiling, gate idle in all four cells (I1-Claude, I2-Claude, I2-Codex).

## The synthesis that finally survives — it's the *invariant type*, not vendor and not distance

Four data points now bracket the question:

| Run | Vendor | Invariant **type** | Rule-only adherence |
|---|---|---|---|
| I1 (commit-format) | Claude | **output-shape** | 100% |
| I2 (trace-coverage) | Claude | **output-shape** | 100% |
| I2 cross-vendor | Codex | **output-shape** | 100% |
| Bake-off (`--ai` gate) | Codex | **procedural / tool-use** | **dropped** |

Two conjectures have now been pre-registered and falsified — attention-distance (by I2) and vendor (by cross-vendor I2). What is left is the one axis the four points actually separate on: **the *kind* of invariant.**

- **Output-shape invariants** — rules about how the model shapes *its own output* (format the commit message, tag the code with `// trace:`, name the function this way). These are inseparable from producing the answer at all, so a capable model self-complies at the ceiling — **across both vendors and the entire attention-distance range.** All three controlled trials are this type; all three hit 100%, gate idle.
- **Procedural / tool-use invariants** — rules to perform an *extra external step* that is not part of naturally producing the answer (call this specific gate, run the reviewer, push-and-open-the-PR, invoke this flag). This is the *one* place leakage was ever observed (the bake-off's `--ai` gate, skipped as fine print).

> **The surviving claim (third conjecture, now the best-supported): for a capable 2026 model, a stated *output-shape* invariant is honored at the ceiling regardless of vendor or attention-distance — substrate-as-bouncer buys nothing for this whole class. The gate earns its place only for *procedural / tool-use* invariants — "did you actually perform the extra step" — and even then chiefly under unattended autonomy or low-tolerance stakes. The axis is output-shape-vs-procedural, not rule-vs-gate, not near-vs-far, not vendor.**

This is not yet proven: it rests on **three controlled "output-shape holds" points + one uncontrolled "procedural drops" hint.** The honest status is a well-isolated hypothesis, not a result — but it is the only one of the three conjectures that all four data points are consistent with.

### The decisive next test (I3 — a procedural invariant, controlled)

Run the same R-vs-G design on a **procedural** invariant: e.g. "after editing code you must run `<tool>` and paste its output into the commit" (an extra step, not an output shape), graded on whether the step was actually performed. Prediction: rule-only adherence drops below 95% and the gate logs real saves — i.e. the gate finally earns its place, and it does so on *procedural* invariants specifically. Cross-vendor I3 (both vendors) would test whether the procedural-leak is vendor-sensitive. This is now the highest-value research follow-up.

## Why this matters for the product (it sharpens the design rule)

The instinct "make the substrate enforce every rule" is contradicted by three controlled trials for the **output-shape** class. The defensible posture, now evidence-backed: **author output-shape invariants as plain CLAUDE.md rules — never gate them (pure surface bloat + false confidence, with zero measured benefit across two vendors). Reserve programmatic gates for *procedural* invariants** — and even there, gate the narrow set that runs unattended or is safety/data-loss-critical. Tellingly, AIDA's actual hard gates are nearly all procedural: did-you-push-and-PR (BUG-269), don't-merge-over-RequestChanges, run-the-reviewer-phase, the unattended-drain rails. The output-shape nudges AIDA ships (the trace-comment pre-commit *nudge*, the commit-format check) are exactly the class this evidence says don't need to be hard gates. The probe's design instinct moves from "gate invariants" to "gate *procedures*."

## Honesty / limits

n=10/arm/vendor, deterministic grading (no judge bias — good). The output-shape-vs-procedural synthesis rests on three controlled output-shape points (all at 100%) + a single uncontrolled procedural data point (the bake-off, different task + different rule + different harness) — I3 is what turns it from inference into a controlled result. Tasks are deliberately small; a harder multi-step task might leak even for output-shape (untested). What is now firmly established: **both the attention-distance and the vendor conjectures are falsified for output-shape invariants** — that part is a clean, replicated, two-vendor negative.

<!-- trace:STORY-655 EPIC-48 | ai:claude -->
