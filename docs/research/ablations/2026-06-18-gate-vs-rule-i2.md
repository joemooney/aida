# Gate-vs-rule I2 — a semantic, high-attention-distance invariant (trace-coverage)

- **Date:** 2026-06-18
- **Probe:** EPIC-48, L2/P1 (substrate-as-bouncer). Spec STORY-655. Runner: `scripts/ablations/gate-vs-rule-i2.sh`. Design: `2026-06-17-gate-vs-rule.md`. Pilot it follows: `2026-06-18-gate-vs-rule-pilot.md`.
- **Status:** **PENDING full n=10 run.** Harness built + smoke-tested (1 trial/arm, live headless `claude -p`); the table below is empty until the expensive run lands.

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

## Results

| Arm | Landed-compliance | Gate-saves |
|---|---|---|
| R (rule-only, CLAUDE.md trace rule, no gate) | _pending_ | — |
| G (gate, trace-coverage pre-commit hook) | _pending_ | _pending_ |

**PENDING full n=10 run.** Run with:

```bash
scripts/ablations/gate-vs-rule-i2.sh --trials 10
```

## Smoke test (mechanism check, NOT evidence)

A 1-trial-per-arm smoke run (2026-06-18) fired a real headless `claude -p` in both arms; both produced a commit, both landed a trace-tagged `.rs` change, the deterministic grader emitted CSV rows. The `--dry-run` self-check independently confirms the gate hook rejects an untagged `.rs` commit and allows a trace-tagged one. The harness is wired end-to-end; n=1 is a mechanism check, not a result, and is not recorded as evidence here.

## Honesty / limits (carried forward from the pilot)

n=10 (planned), one invariant, one vendor, deterministic grading (good — no judge bias). If I2 also lands at the ceiling in Arm R, that is itself the decisive datum: it would say the attention-distance conjecture does not survive a genuinely buried semantic rule, and P1's scope shrinks further. The point of I2 is that it *can* embarrass the conjecture.

<!-- trace:STORY-655 EPIC-48 | ai:claude -->
