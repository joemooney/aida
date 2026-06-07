# Backlog triage sweep — 235 approved → decision batch (manual 523 prototype)

- **Date:** 2026-06-05 · **Author:** advisor (master) · **Purpose:** front-load the human decisions so the autonomous burn-down runs through instead of stalling one-fork-at-a-time. A manual prototype of STORY-523's sweep.
- **Method:** partition approved specs into `workable-now` / `advisor-defaultable` / `needs-you` / `archive`; for `needs-you`, attach a **recommended disposition** so you confirm in seconds, not deliberate.

## The reframe (why this is the speedup)

The drain's rate-limiter is **your decision bandwidth**, not drain throughput (BUG-453, now fixed, was the last throughput bug). So speed = *minimize* decisions that reach you + make the rest *fast*. This sweep does both: it removes the archive tail and the advisor-defaultable forks from your inbox, and reduces the genuinely-strategic items to **confirm/defer/drop** picks.

## Composition (235 approved)

| Bucket | Count | What happens |
|---|---|---|
| 🗄️ **archive tail** (low-priority + observations) | ~117 | One bulk decision: archive (revisit on recurrence) |
| 🧭 **needs-you** (12 epic + 3 ADR + 1 principle + 16 spike) | 32 | The decision batch below — confirm/defer/drop |
| 🟢 **workable-now** (crisp Tasks/Bugs) | ~20–25 | Feed the headless drain (BUG-453 unblocked) |
| 🤖 **advisor-defaultable** (forks w/ a safe default) | ~30 | I pick + record w/ a revisit-trigger; not your inbox |
| 📋 **stories** (mostly umbrella) | 64 | Decompose into child tasks (most need a decision or are containers) |

## 🧭 The decision batch — confirm/defer/drop (the part you answer)

### Epics (12) — recommended disposition

| Epic | Recommended | Why |
|---|---|---|
| EPIC-35 GitLab forge | **do-now (nearly done)** | routing complete this session; remainder = GitLab `glab` impls (STORY-509/510/511) |
| EPIC-33 Orchestrator correctness floor | **do-now** | the trust floor for autonomy; BUG-453 was part of this |
| EPIC-37 Onboarding hardening | **do-now** | BUG-445/447 (high) are its symptoms; first-user-critical |
| EPIC-38 MCP gate parity | **do-now** | BUG-449 (high) is its symptom — MCP bypasses the ADR-3 gate |
| EPIC-28 Dependency-aware drain | **defer** | partially shipped; sequence after the correctness floor |
| EPIC-31 Agent registry/launcher | **defer** | large; not blocking |
| EPIC-27 MCP modernization | **defer** | sequence after EPIC-38 gate parity |
| EPIC-34 Trace-coverage merge gate | **defer** | provenance hardening; not urgent |
| EPIC-29 Split aida-tui to own repo | **defer** | infra reorg; do when TUI stabilizes |
| EPIC-24 Living documentation | **defer** | vision; revisit post-stability |
| EPIC-25 Release-as-composite | **defer** | revisit when release cadence picks up |
| EPIC-22 Cross-project primitives | **defer** | future; no current multi-project need |

### Decisions + Principle (4) — these are *recorded*, not *open work*

| Spec | Recommended | Why |
|---|---|---|
| ADR-1 orphan-branch storage | **complete** | the decision is adopted + shipped (current architecture) |
| ADR-2 role onboarding default | **complete or → EPIC-37** | decided; implementation tracked under onboarding |
| ADR-3 intake-triage gate | **complete or → EPIC-38** | decided; TASK-647 shipped it; MCP parity is EPIC-38 |
| PRIN-1 graph-is-canonical | **complete (permanent)** | a standing principle, not work — shouldn't sit in the backlog |

> These 4 sitting "Approved" is graph noise — decisions/principles are stateless. Completing them removes 4 from the count with zero risk.

### Spikes (16) — investigate-or-defer

- **do-now / recent:** SPIKE-50 (ECC deep-dive — other agents already producing it).
- **defer (Claude-Code research batch, likely partly OBE):** SPIKE-14/15/18/19/20/21/22/23 — research dumps from the 2026-05-29 overlap analysis; revisit when a feature actually needs them.
- **defer (real but not urgent):** SPIKE-47 (trace-coverage def — feeds EPIC-34), SPIKE-48 (sandbox store), SPIKE-40 (aida-channel MCP), SPIKE-38 (review GH Action), SPIKE-13 (budget dispatch), SPIKE-8 (/ultraplan quality), SPIKE-42 (rewind compose), SPIKE-12 (pack-scale).

> Spike default: **defer unless it gates an active epic.** Only SPIKE-47 (→ EPIC-34) and SPIKE-50 (competitive, in-flight) are arguably do-soon.

### High bugs (3) — these are real, route them

- **BUG-449** MCP update_requirement bypasses the ADR-3 gate (agent self-completes uncommitted code) → **EPIC-38, do-now** (reliability hole).
- **BUG-447** session-resume matches by spec-id globally (cross-project bleed) → **do-now** (reliability; bounded fix).
- **BUG-445** onboarding scaffolding breaks under worktree isolation → **EPIC-37, do-now** (first-user-critical).

## 🟢 Workable-now (feed the headless drain)

Crisp, bounded, non-reliability Tasks/Bugs the headless drain can ship (BUG-453 fixed). Validating now: TASK-673 (+TASK-671). Candidates from the med-task pool to batch next: the display/docs/hint papercuts. (I'll enumerate the exact drain set from the 33 med-tasks once the validation drain confirms the success rate.)

## 🗄️ Archive (one bulk decision)

~117 low-priority specs (incl. ~13 captured-observations). Recommend `aida archive --older-than 30d` style sweep over the low-priority + observation set (revisit on recurrence — not deletion). **Master-gated per the grooming convention; needs your one approval.**

## Next actions (in speed order)

1. **You:** confirm the epic do-now/defer picks + complete the 4 ADR/PRIN recorded-decisions (clears 4 + sequences the rest). ~5 min.
2. **You:** approve the archive sweep (clears ~100 from the count). 1 decision.
3. **Me (advisor):** resolve the ~30 advisor-defaultable with recorded defaults; enumerate + drain the workable-now set headlessly (BUG-453 fixed).
4. **Result:** the 235 collapses to a small active set (do-now epics + their child work), and the drain runs without per-fork stalls.

> This report is the manual proof of STORY-523. Automating it (the sweep + DecisionRequest attachment, with a recommended default per question) is what makes this a one-command, repeatable batch.
