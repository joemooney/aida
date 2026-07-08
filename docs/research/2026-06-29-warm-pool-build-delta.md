# Warm-pool build-delta: the cold-build tax on wide agent fan-out

*Date: 2026-06-29 · Status: finding (single-run pilot) · Specs: STORY-714, TASK-985, BUG-652 · Author: claude (implementer)*

> A research note for the **agent-fleet** line, not just a STORY-714 implementation log. The question it answers — *what does it actually cost to give every fanned-out agent its own worktree, and what does recycling them buy back?* — bears directly on whether wide, parallel agent fan-out is economical. Pairs with [`2026-07-08-coordinating-multi-vendor-agent-fleets.md`](2026-07-08-coordinating-multi-vendor-agent-fleets.md).

## 1. Question & hypothesis

AIDA's autonomous drain fans out one git worktree per spec so implementer agents work in parallel without colliding. The historical model is **destroy-and-recreate**: `git worktree add` a fresh sibling, work, `git worktree remove --force`. Each fresh worktree starts with an **empty `target/`**, so the first `cargo build` pays the full cold-compile cost — every spec, every time.

The warm-pool (STORY-714) replaces that with **acquire / return**: keep a pool of worktrees and, on hand-back, reset the tree to a clean base **instead of deleting it**, so its compiled `target/` survives. The next acquire reuses the warm tree.

**Hypothesis.** On a fan-out of *N* specs, destroy-recreate pays *N* cold builds; the warm-pool pays *1* cold build plus *(N−1)* warm (incremental) builds. If the cold-build tax is large, the warm-pool should turn a linear *N×cold* cost into roughly *cold + (N−1)×warm*.

## 2. Methodology

| Control | Value | Why |
|---|---|---|
| `CARGO_TARGET_DIR` | **unset** | Each worktree gets its **own** `target/` — the real-user default. (See §4: leaving it set silently destroys the measurement.) |
| Workload | throwaway Rust crate, **real dep tree**: `serde`(derive), `serde_json`, `anyhow`, `tokio`(rt-multi-thread, macros, time), `clap`(derive) | A non-trivial dependency graph so a cold build compiles something representative, not a toy. `Cargo.lock` committed so post-build trees aren't spuriously dirty. |
| Specs per flow | N = 3 | Pilot scale; enough to show cold-vs-warm separation and a non-trivial hit-rate. |
| Per-spec change | append one line to `src/main.rs`, on a fresh branch | Models the realistic drain pattern — each spec touches *different* source, so the **crate** recompiles but the **dependencies stay cached**. Not a zero-change no-op. |
| Metric | wall-clock of `cargo build -q` (ms), via `date +%s%3N` around the build | The cost an agent actually waits on. |
| Binary | in-repo `aida` (`target/debug/aida`) driving `worktree pool acquire` / `return` | Exercises the shipped pool primitives. |

**Flow A — destroy-recreate (baseline).** For each spec: `git worktree add --detach` a fresh sibling → `checkout -b` → edit → timed `cargo build` → `git worktree remove --force`. Every build is cold (empty `target/`).

**Flow B — warm-pool.** For each spec: `aida worktree pool acquire` → `checkout -b` → edit → timed `cargo build` → `aida worktree pool return`. The first acquire creates the tree (cold); subsequent acquires reuse it (warm, `target/` preserved across the reset).

The two flows run back-to-back on the same machine, same crate, same dependency set.

## 3. Results

```
FLOW A: destroy-recreate (fresh worktree per spec, COLD target/)
  spec 1: fresh worktree cold build = 5958 ms
  spec 2: fresh worktree cold build = 5228 ms
  spec 3: fresh worktree cold build = 5307 ms        avg = 5497 ms  (EVERY spec pays this)

FLOW B: warm-pool (recycled tree, WARM target/ after first)
  spec 1: aida-pool-measure-0  COLD(first) = 5696 ms
  spec 2: aida-pool-measure-0  WARM        =  183 ms
  spec 3: aida-pool-measure-0  WARM        =  181 ms        warm avg = 182 ms (specs 2–3)

pool hit-rate = 2/3 = 67%
```

| Metric | Value |
|---|---|
| Flow A cold build (avg, paid every spec) | **5497 ms** |
| Flow B warm reuse (avg) | **182 ms** |
| **Speed-up per reused spec** | **30.2× faster** (saves ~5.3 s each) |
| Pool hit-rate (N = 3) | **67%** → approaches **(N−1)/N** as drains widen |
| 3-spec drain, total build time | **16.5 s** (destroy-recreate) → **6.1 s** (warm-pool) = **10.4 s saved** |

The per-spec saving is roughly constant (`cold − warm ≈ 5.3 s`), so the total saving grows **linearly** with drain width: a 10-spec drain saves ≈ 9 × 5.3 ≈ 48 s of pure build wait; a 50-spec drain ≈ 260 s.

`cargo build` user-time of ~69 s across the run (vs ~23 s wall) confirms the cold builds did real, parallel compilation — the cold numbers are genuine compiles, not cache hits.

## 4. Methodology lesson: the `CARGO_TARGET_DIR` confound

The **first** attempt to measure this produced nonsense — a "cold" build of the same crate completed in 165 ms, ~33× faster than it should. Cause: the dev environment had `CARGO_TARGET_DIR=/home/joe/ai/aida/target` exported, so **every** worktree — pool tree or fresh — compiled into **one shared target directory**. The deps were compiled once and reused everywhere, regardless of flow.

This is a genuine finding, not just a test bug:

- **A shared `CARGO_TARGET_DIR` already delivers the warm-pool's cache benefit** — and masks the pool's *marginal* contribution to zero. If a team runs their whole fan-out against one shared target dir, the warm-pool buys them little *on build caching* (its value there is the lifecycle: no worktree churn, BUG-553 branch-stacking and TASK-0396 fingerprint-poison dissolution).
- **To measure the pool's cache contribution you must unset it**, reproducing the default single-developer / per-worktree layout. Any benchmark that leaves a shared target dir in place will under-report the pool by ~30× and conclude (wrongly) that it does nothing.

Generalization: *when benchmarking a cache, audit for an outer cache that already covers it.* A silent shared cache turns a 30× effect into a 1.0× null result.

## 5. Threats to validity

- **Single run, N = 3.** No variance bars; machine load could move any one number. The *separation* (5.5 s vs 0.18 s) is far larger than plausible noise, so the direction and order of magnitude are safe; the exact multiplier is not.
- **Throwaway crate ≠ real workspace.** Five dependencies is a deliberately **conservative floor** (see §6).
- **Warm build is a 1-line incremental,** not zero-change. That's the *realistic* per-spec case (different specs touch different files), but a spec touching a widely-included header/trait would recompile more — the warm number is a floor for "small diff," not a ceiling.
- **First-spec cold cost is unavoidable** in both flows; the warm-pool only wins from spec 2 onward. Drains of size 1 see no build benefit (only the lifecycle benefits).
- **Build wait ≠ total drain time.** Real drains also spend time in the agent, CI, and review. This measures only the build component the worktree model controls.

## 6. The floor caveat

The 5-dependency crate is the **low end**. A cold build here is ~5.5 s. The real `aida` workspace (6 crates, hundreds of transitive deps) has a cold build measured in **minutes**. The warm-pool's per-reuse saving scales with the cold-build cost it avoids, so on the actual project the absolute delta is **far larger** than the 5.3 s shown here — this note's numbers are a conservative lower bound on the payoff, chosen for fast, reproducible measurement.

## 7. Implication for the agent-fleet thesis

The cold-build tax is a **per-worktree fixed cost on parallelism**. Under destroy-recreate it scales as *N × cold* — the wider you fan out, the more total compute you burn re-compiling identical dependency graphs in throwaway trees. That cost is a quiet tax on the whole "spread substantial work across many agents" strategy: every additional parallel implementer pays the full cold build before it does any useful work.

The warm-pool removes that tax from all but the first worktree, turning *N × cold* into *cold + (N−1) × warm*. Concretely, **it is part of what makes wide agent fan-out economical** rather than wasteful — it lets the [integrator-throughput](../positioning/vs-agent-teams.md) model (many cheap parallel implementers feeding one integration lane) pay build cost *once* per pool tree instead of once per spec. The wider and more repetitive the drain, the more the warm-pool matters; for the kind of long autonomous burndowns AIDA targets, it converts a linear build-cost penalty into a near-constant one.

This is the empirical backing for taking the warm-pool from opt-in to default (TASK-985): the payoff is real, large, and grows with exactly the fan-out width AIDA is built to exploit.

## 8. Reproducibility

Unset `CARGO_TARGET_DIR`. Scaffold a Rust crate with the dep set in §2 (`cargo generate-lockfile`; commit `Cargo.lock`). For Flow A, loop: `git worktree add --detach <fresh> main`, `checkout -b`, edit `src/main.rs`, time `cargo build`, `git worktree remove --force`. For Flow B, loop: `aida worktree pool acquire --json`, `checkout -b`, edit, time `cargo build`, `aida worktree pool return`. Average Flow A; for Flow B separate the first (cold) build from the warm reuses; hit-rate = reuses / acquires. (Run in a throwaway project — never against real specs; see the no-E2E-on-real-spec discipline.)

## 9. Open items

- **Re-run at scale on the real workspace** (per-worktree target, N ≥ 5) to put an absolute number on the minutes-long-cold-build case. Tracked as a TASK-985 follow.
- **Pool hit-rate telemetry** (reuse vs create) in production drains to confirm the (N−1)/N model holds under real cap pressure (STORY-714 followup).
- **Variance**: repeat the pilot ≥5× for error bars before quoting the 30× multiplier as anything but order-of-magnitude.

## Related

- STORY-714 — worktree warm-pool (the feature measured here); `docs/plans/2026-06-28-story-714-worktree-warm-pool.md`.
- TASK-985 — flip the acquire-on-start default ON (this note is its decision evidence).
- BUG-652 — `session end --return` dirty-gate fix (without it, hit-rate collapsed to 0% — reuse is a precondition for any of this payoff).
- `docs/session-lifecycle.md` — the worktree lifecycle and the warm-pool section.
- `docs/research/2026-07-08-coordinating-multi-vendor-agent-fleets.md` — the agent-fleet line this feeds.
