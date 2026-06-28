# Competitive note: the AXI ecosystem (Kun Chen / kunchenguid)

**Date:** 2026-06-28 · **Trigger:** operator surfaced the "L8 Principal's
Agentic Engineering Workflow" video + the `kunchenguid` GitHub. **Type:** new
high-signal builder sighting + a direct interface-layer challenge. **Deep dive:**
`docs/positioning/vs-axi.md`.

## Why this is logged

Per the multi-modal discovery discipline (watch known *builders*; a credible
practitioner shipping is a signal). Kun is ex-L8 Principal (Meta / Microsoft /
Atlassian), now building frontier coding agents at Atlassian, self-reports
40-50 tested PRs/day. He has shipped a **coherent ecosystem with real traction**,
not a one-off — and it lands squarely in AIDA's arena.

## The ecosystem (2026-06-28 stars)

- **`axi`** (~1.1k) — "Agent eXperience Interface": 10 design principles for
  agent-native CLIs that treat token budget as first-class, **with benchmarks
  claiming agent-CLIs beat MCP** on cost/success/turns.
- **`no-mistakes`** (~3.8k) — git-push proxy → disposable-worktree validation
  pipeline → clean PR + CI auto-fix. (≈ our `--auto-complete` drain.)
- **`gnhf`** (~2.5k) — long-running agents with precise stop caps.
- **`firstmate`** (~330) — one-liaison-agent runs a worktree-isolated crew;
  opt-in persistent "secondmates" = domain supervisors. (≈ our advisor +
  orchestrator + SPIKE-10 subsystem advisors.)
- **`treehouse`** (~430) — worktree management. (≈ `isolation:worktree`.)
- **`tasks-axi`** (~11) — **task/backlog manager for agents** — the direct
  neighbor. `backlog.md` as source of truth, **built on the Beads dep-graph +
  ready-query model**.
- **`gh-axi`** (~95) — token-efficient GitHub CLI for agents. (≈ our forge
  layer.)

## The two findings

1. **The MCP bet is challenged with data.** AXI benchmarks (Sonnet 4.6, 100s of
   runs): `gh-axi` 100% / $0.050 / 3 turns vs GitHub **MCP** 87% / $0.148 / 6
   turns. Our README calls MCP "the highest-leverage surface." If this
   replicates, the token-efficient CLI is the better agent surface and MCP is
   over-weighted. **Action: reproduce on AIDA's own tools before more MCP work.**

2. **The neighbor cluster is consolidating.** `tasks-axi` borrows **Beads**
   (`gastownhall/beads` = the Gas Town that went cross-vendor in the 2026-06-26
   snapshot). AXI + Beads + Gas Town are converging on a shared model:
   markdown/structured backlog + dependency graph + token-ergonomic agent
   interface + zero-install skill/hook distribution. None of them has AIDA's
   **stable IDs + code traces + typed graph + lifecycle** — that remains the
   differentiated core.

## Convergent validation (the upside)

The video independently confirms AIDA's architecture: worktree-per-agent (the
*exact* same-directory-collision problem we solved this cycle with BUG-637 /
STORY-711), long-running supervised agents, an orchestrated first-pass→clean-PR
pipeline, an agent-activity status bar (≈ our EPIC-53 cockpit), a meta-orchestrator
("first mate" ≈ advisor). An independent expert building the same shapes is the
strongest signal the architecture is right. The gap is **interface ergonomics**,
not architecture.

## Implications for AIDA

- **Adopt, don't just defend.** AXI's 10 principles are a free, benchmarked
  spec for an agent-facing output mode. We score ~4-5/10; the misses (token-
  efficient output, content-first no-args) are concrete and cheap.
- **Re-weight the MCP investment** pending the reproduced benchmark.
- **The moat is the substrate** — keep the graph/IDs/traces/lifecycle as the
  defensible center; that's what the whole cluster lacks.
- **Distribution is a gap** — their zero-install skill/hook funnel
  (`npx skills add`) beats "build the Rust binary" on adoption friction.

## Refresh signals to watch

- `tasks-axi` / Beads adding stable IDs, code traces, or a lifecycle → tripwire (the cluster grows a graph).
- AXI principles cited as table stakes by other tools.
- The AXI-vs-MCP benchmark independently reproduced or contested.
- Kun's star velocity / any "AXI standard" framing gaining adoption.

## Filed follow-ups

- **SPIKE** — reproduce MCP-vs-token-efficient-CLI on AIDA's tools + scope an
  agent-facing AXI output mode (the load-bearing investigation).
- **TASK** — token-efficient agent-output mode (`--toon` / minimal schemas /
  content-first), agent path only, emoji human path unchanged.
- `docs/positioning/vs-axi.md` — the full head-to-head.
