# Applying the gate-vs-rule terminus to AIDA's own gates — an honest audit

- **Date:** 2026-06-19
- **Probe:** EPIC-48, L2/P1. Companion to the gate-vs-rule program (`2026-06-18-gate-vs-rule-{pilot,i2,i3,i4}.md`, STORY-655) and `SPIKE-67` (field instrumentation).
- **Purpose:** the program concluded "don't gate rules that fire in well-scoped work; audit AIDA's hard gates against the real-repo/long-autonomy bar." This is that audit — and its result is a useful corrective to a naive reading.

## The trap this avoids

The glib takeaway from "five cells, five ceilings, the gate did zero work" is *"remove AIDA's gates."* That would be wrong, and seeing why sharpens what the evidence actually says.

**The gate-vs-rule experiments tested exactly one kind of gate: a *rule-adherence* gate** — a programmatic check standing in for an agent's self-discipline on a *stated rule the agent is supposed to follow* (write the commit in this format; tag the code; run this step). The finding — a capable model self-complies at the ceiling, so the gate is idle — applies *only* to that kind.

It says **nothing** about the other kinds of gate, where "the agent would self-comply" is not even a coherent alternative:

| Gate kind | Example in AIDA | Does the evidence apply? |
|---|---|---|
| **Rule-adherence** (stand-in for agent self-discipline) | commit-format / REQ-ID strict, doc-comment trace-marker hook, glyph-lint, trace-coverage | **YES** — this is what I1–I4 tested |
| **Authorization** (a permission boundary) | advisor-authority to approve/queue, can't-approve-your-own-spec, team roster, MCP status gate | **NO** — you cannot self-comply past an auth boundary; it is not about rule-following |
| **Concurrency** (prevent races) | drain lock, cross-clone lease refusal, orchestrator token | **NO** — a second concurrent drain is not an agent "forgetting a rule" |
| **Integrity** (prevent corruption) | spec-id collision guard, lifecycle state-machine | **NO** — data-structure invariants, not agent discipline |
| **Correctness/security** (catch bugs) | cargo test, clippy -D correctness, fmt --check, bwrap confinement | **NO** — these catch *defects*, not rule-violations |

## The result: the removal-candidate set is nearly empty — AIDA gates the right things

Inventorying AIDA's ~38 hard gates and keeping only the **rule-adherence** ones (the only kind the evidence addresses), then asking the I4 question — *does it fire in well-scoped work (evidence: idle, removable) or only in the real-repo/long-autonomy regime (evidence: possibly justified)?* — almost nothing is a clean removal candidate:

- The authorization / concurrency / integrity / security gates (the large majority) are **out of scope** — keep them; the evidence does not touch them.
- The rule-adherence gates that exist (commit-format, trace-coverage, glyph-lint, doc-comment marker) **run at CI** — i.e. at the *real-repo integration point*, against a real, growing, multi-contributor codebase. That is regime **(b)**, not a fresh well-scoped task. The evidence does **not** mark regime-(b) gates removable; it is silent on them (the ablations could not reach regime b — that is the whole terminus).

**So the honest product conclusion is the opposite of the glib one: AIDA's gates survive the audit.** It gates authorization, concurrency, and integrity (where self-compliance is not even the question) and runs its rule-adherence checks at the real-repo boundary (the unmeasured regime). The probe's own evidence does not call for removing any of them. That is a non-trivial validation of the gate *design*, reached by trying to falsify it.

## The one genuine calibration target — STORY-499 (trace-coverage → `--block`)

There is exactly one place the evidence bites a live decision. The diff-level trace-coverage gate (STORY-499) ships **report-only** by deliberate design (SPIKE-47 §5/§6: "a coverage gate's failure mode is adoption death — run report-only for a calibration period to mine missed exemptions, then flip to `--block`"). The flip-to-`--block` is pending.

Two facts now bear on that flip:

1. **I2 was, literally, a trace-coverage ablation** — "every code change carries a `// trace:` comment", buried, never-restated — and it scored **100% rule-only compliance** across Claude *and* Codex. Agents self-comply on *this exact rule*.
2. **The report-only gate is already a mini-SPIKE-67 field instrument.** It has been annotating real-PR coverage ratios + uncovered hunks without blocking — i.e. collecting precisely the field data the gate-vs-rule terminus says is the only remaining evidence path, on the one rule we have an ablation for.

**Recommendation:** do **not** flip trace-coverage to `--block` on schedule by default. First read the report-only field record: if real PRs show coverage at/near the ceiling with only rare, legitimately-exempt misses (which I2 predicts), the `--block` flip buys friction, not safety — keep it report-only and treat it as the standing field channel. Flip to `--block` only if the field data shows *recurring real misses* (which would itself be the first controlled-ish evidence of rule-dropping in a real repo — a SPIKE-67 signal worth its own writeup). Either way, the decision should be **data-driven off the report-only record**, not calendar-driven.

## Carried back

- The gate-vs-rule terminus does **not** indict AIDA's gates; applied rigorously it validates them (right gate kinds, right regime).
- It refines exactly one pending decision (STORY-499 `--block` flip) and connects it to SPIKE-67: the report-only trace gate is the field instrument already running; let it, not the calendar, decide the flip.
- General principle for future gates: before adding a *rule-adherence* gate, ask "does this rule fire in well-scoped work?" If yes, the evidence says a CLAUDE.md rule suffices — don't gate it. Reserve new gates for authorization/concurrency/integrity, or for rule-adherence at the real-repo/autonomy boundary.

<!-- trace:STORY-655 EPIC-48 | ai:claude -->
