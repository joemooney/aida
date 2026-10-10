# The economics of running agent fleets: compute and token cost axes

**Date:** 2026-06-29 · **Status:** findings synthesis · **Instrument:** AIDA (used as a testbed, not the subject)

> A research note. AIDA is the measurement instrument here, not the thesis —
> the question is general: *when you run a fleet of coding agents that fan out,
> implement, and integrate continuously, where does the money go, and which
> levers move it?* Three measurements this cycle answer two distinct cost axes.

## The two cost axes

Running a continuous agent fleet (fan-out implementers → CI → review → merge,
supervised) spends on two independent axes:

1. **Compute** — the CPU/wall-clock of the work itself, dominated for a Rust
   codebase by **build time** (each agent rebuilds in its worktree).
2. **Tokens** — the LLM spend, split between (a) the **agent surface** (how
   expensive each tool call is) and (b) the **supervision** (how often, and on
   what, you wake an LLM to watch the fleet).

These are independent: you can win one and lose the other. The findings below
move each, and they **compound** — the levers that make a *wider* fan-out
cheaper (compute) are the same width over which supervision-per-event (token)
amortizes.

---

## Finding 1 — Compute: recycled worktrees cut per-agent build cost ~30×

**Lever:** a **warm worktree pool** — recycle (reset, don't delete) worktrees on
hand-back so each agent's compiled `target/` cache survives, instead of
destroy-and-recreate (a cold `target/` per agent).

**Measurement** (2026-06-29, `docs/research/2026-06-29-warm-pool-build-delta.md`):

| Flow | Cold build | Warm reuse |
|---|---|---|
| A — destroy-recreate (one fresh worktree per spec) | 5497 ms avg — **every** spec pays it | — |
| B — warm-pool (recycled tree) | 5696 ms (first only) | **182 ms** avg |

- **30.2× faster per reused spec** (5497 → 182 ms; ~5.3 s saved each — only the
  changed crate recompiles, deps stay compiled).
- **Hit-rate 67%** on a 3-spec drain (first creates, rest reuse) → approaches
  **(N−1)/N** as drains widen.
- A 3-spec drain: 16.5 s → 6.1 s. The saving grows **linearly with fan-out
  width**.

**Methodology lesson (load-bearing):** the benefit is invisible under a shared
`CARGO_TARGET_DIR` — a shared target dir already provides cross-worktree cache
reuse and *masks* the pool's contribution. You must **unset it** (per-worktree
`target/`, the real-user default) to measure the pool's actual delta. The 5-dep
test crate is a **conservative floor**; a real workspace's minutes-long cold
build makes the delta far larger.

---

## Finding 2 — Token (surface): the structured/MCP surface costs ~2× the CLI

**Lever:** the **agent-facing output surface**. The intuition that a typed,
structured tool surface (MCP) is the "highest-leverage" agent interface does not
survive measurement on identical tasks.

**Measurement** (SPIKE-73, 72-cell matrix: 4 surfaces × 6 tasks × 3 runs;
`bench/agent-surface/results/report.md`):

| Surface | Success | Avg cost | Turns / tools |
|---|---|---|---|
| CLI (human-formatted) | 100% | **$0.0358** | 3.1 / 2.1 |
| MCP | 89% | $0.0709 (**~2×**) | 3.1 / 2.1 |
| MCP + on-demand schema loading | 100% | $0.0636 (**~1.8×**) | 4.8 / 3.8 |
| TOON (token-efficient CLI) | 100% | $0.0360 | 2.8 / 1.8 |

- MCP costs **~2× the CLI** for equal-or-worse success.
- The cost is **structural, not an upfront-schema artifact**: loading schemas
  on demand (the ~1.8× row) doesn't rescue it — the extra round-trips to load
  schemas eat the input-token savings, and turns/tools *rise*.
- A token-efficient text surface (TOON) ≈ the human CLI on these
  small-output reads; its win grows on large list/show outputs (measured 21–84%
  separately).

**Implication:** for fleet work, the **token-efficient CLI is the primary,
cheaper agent surface**; a typed/structural surface is an option you pay a
premium for, not a default. (`docs/positioning/vs-axi.md`.)

---

## Finding 3 — Token (supervision): wake on events, not on a timer

**Lever:** **how the fleet is supervised.** A long-running drain must be watched
for the moments that need a decision (CI red, a design fork, a merge). The naive
model wakes an LLM on a **timer** to poll "anything happen yet?" — and most wakes
find nothing.

**Measurement** (STORY-712 design,
`docs/plans/2026-06-29-story-712-zero-token-supervision.md`): an 8-hour, 8-spec
overnight drive supervised by a ~hourly LLM fork paid an estimated **~$6**
(cold-boot forks) to **$20+** (fork-from-live cache tax) in **idle-check wakes
that found nothing actionable** — pure overhead layered on top of the ~$3/spec
implement→review cost.

**The lever:** the drain already knows every state change. Emit it to an
append-only event stream; a **cheap, non-LLM classifier** absorbs the benign
majority (phase churn, retries) at **$0** and surfaces only **actionable verbs**
(CI-terminal, PR-done, punt, shelve, merge, drained); the supervising LLM
consumes that via a blocking watcher — **zero tokens while silent**. Supervision
cost drops from **O(time) → O(real events)**.

Corollary (observed directly while running the fleet): an idle supervisory loop
that ticks **past the ~5-minute prompt-cache window** pays a full cache-miss for
nothing — when genuinely idle, long intervals (or event-driven wakes) only.

---

## Synthesis — what makes wide fan-out economical

The three findings line up on a single thesis: **the cost of a continuous agent
fleet is set by per-unit infrastructure choices that each amortize over fan-out
width, not by model capability.**

| Axis | Naive model | Lever | Scaling |
|---|---|---|---|
| Compute | cold `target/` per agent | warm worktree pool | saving ∝ fan-out width; hit-rate → (N−1)/N |
| Token — surface | typed/structural (MCP) | token-efficient CLI (TOON) | ~2× per call, on every call |
| Token — supervision | timer-poll an LLM | event-driven wake | O(time) → O(real events) |

They **compound**: a warm pool makes a *wider* drain cheap (compute), the CLI
surface makes each agent's calls cheap (token), and event-driven supervision
makes watching that wider drain cheap (token) — together they turn "leave a
fleet draining all day" from prohibitively expensive into roughly free-while-idle
plus marginal-cost-per-real-event. That economic shift is the precondition for a
**continuously-running integrator** (a dedicated lane that keeps the main branch
moving) being practical rather than a money fire.

## Caveats / threats to validity

- **n is small** (warm-pool: 3 specs/flow; benchmark: 3 runs/cell, Sonnet 4.6;
  supervision: one estimated overnight). Directional, not publication-grade.
- **Conservative floors:** the build-delta crate has 5 deps; real workspaces are
  larger (delta larger). The supervision figure is a model estimate, not a
  metered bill.
- **Single-vendor instrument:** all three measured on the Claude/AIDA stack. The
  *shape* (the three levers) should generalize across vendors; the *magnitudes*
  are stack-specific.
- The warm-pool finding required **unsetting a confound** (`CARGO_TARGET_DIR`);
  other environments may have analogous masking optimizations to control for.

## Open questions

- Does the surface premium (Finding 2) hold under longer multi-call chains, or
  does MCP's typing amortize on complex tasks? (The chained-task rows hint at
  convergence but stay CLI-favorable.)
- What is the *actual* metered supervision saving once event-driven supervision
  ships (Finding 3 is a design estimate)?
- How do these levers interact with a **multi-vendor** fleet, where supervision
  is often a cold boot across vendor boundaries?

## Sources

- `docs/research/2026-06-29-warm-pool-build-delta.md` — compute measurement
- `bench/agent-surface/results/report.md` — the 72-cell surface benchmark
- `docs/plans/2026-06-29-story-712-zero-token-supervision.md` — supervision design + token math
- `docs/positioning/vs-axi.md` — the surface-cost positioning
- Related memory: token economics of agent fleets (the three levers)
