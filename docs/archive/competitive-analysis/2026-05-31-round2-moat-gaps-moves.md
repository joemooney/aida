# Competitive round-2 — the moat, the gaps, the moves (2026-05-31)

**Specs:** SPIKE-43, SPIKE-45 · **Status:** living (dated snapshot) · **Evidence:** web-current May 2026, graded in the source agent reports
**Inputs:** four parallel competitive deep-dives — Spec Kit (nearest competitor), the multi-agent-coordination frontier, AGENTS.md/MCP convergence, and git-canonical beyond-software use cases. Complements `2026-05-31-git-canonical-substrate-thesis.md` (round-1).

> Frozen at time T per the immutability discipline. Supersede with a new dated file.

## The integrated picture

Four findings, one coherent read:

1. **Spec Kit (github/spec-kit, ~90–106k stars, GitHub first-party) is the nearest competitor — and confirms our wedge.** It is the "structured-markdown floor": **all five** AIDA primary differentiators (stable cross-cutting IDs, typed relationship graph, code↔spec trace enforcement, rebuildable query cache, MCP spec graph) are **CONFIRMED absent**. Don't overclaim: it *does* have within-feature FR-/SC-/T- IDs + a one-shot `/speckit.analyze`. **The risk it poses is distribution (~100× our reach), not feature-convergence** — its roadmap aims at GitHub Issues + agent prompts, not a structured layer.
2. **The multi-agent frontier splits into 3 families; AIDA uniquely spans all 3.** General frameworks (LangGraph/CrewAI/AutoGen→MAF), coding-swarm orchestrators (Cursor/Windsurf/Composio AO/claude-flow), and SDD tools (Kiro/Spec Kit/OpenSpec). **The confirmed moat:** the only durable, typed, multi-vendor spec graph an orchestrator *drains*, with a spec-grounded escalation/shelving cascade. **But worktree isolation is now table stakes**, **Kiro matched "specs-as-artifacts"** (EARS + task→requirement traceability), and **Composio AO reproduced our drain loop without the substrate.** The mechanical axis is commoditizing; the *substrate* axis holds.
3. **The context-file + transport layers are now commoditized standards.** AGENTS.md + MCP + goose sit under the **Linux Foundation's Agentic AI Foundation** (formed Dec 2025). AGENTS.md is near-universal (60k+ repos, ~all agents) and *deliberately stays unstructured* (v1.1 adds only `description`/`tags` frontmatter; structured energy goes to scoping/permissions, **not** a requirement graph). MCP is universal plumbing (~97M installs) — a **tailwind**: every agent can already query our graph. The latent model/threat is **ReqIF** (OMG standard: typed source→target spec relations) — conceptually "AIDA's graph for enterprise ALM," but **absent from the agent world**. No ReqIF↔agent bridge exists yet.
4. **"Amenable to many use cases" is true-weak, false-strong.** Git-canonical is a *structural* advantage only where the value is *the auditable, branchable, multi-vendor diff-history of a structured graph* — not fast recall, real-time collab, or low-friction capture. That filter turns six tempting expansions into **one feature, two watches, three avoids** (below).

## Commoditized vs differentiated (what to ride vs defend)

| Layer | Status | Move |
|---|---|---|
| Portable context file (AGENTS.md) | **COMMODITIZED** (LF-governed standard) | **RIDE** — be the best AGENTS.md *generator*, projecting a rich, always-current file from the graph |
| Transport (MCP) | **COMMODITIZED** (LF-governed, ~97M installs) | **RIDE** — invest in the *graph payload*, not the pipe |
| Spec→Plan→Tasks scaffolding | **COMMODITIZED** (Spec Kit/Kiro/BMAD/OpenSpec) | not a differentiator |
| Worktree-per-agent isolation | **TABLE STAKES** (Cursor/Windsurf/Composio) | keep, but don't pitch as an edge |
| "Specs as artifacts" / ID-based traceability *concept* | **TABLE-STAKES VOCABULARY** (Kiro says the words) | talked-about ≠ enforced — the *enforcement* is the edge |
| **Stable IDs as identity (not positional/path)** | **DIFFERENTIATED** | **DEFEND — loud headline** |
| **Typed inter-spec relationship graph** | **DIFFERENTIATED** (only ReqIF has the model; absent from agents) | **DEFEND + ship graph-query** |
| **Trace-enforcement loop (code↔spec, commit-gated)** | **DIFFERENTIATED** (Spec Kit's documented drift *is* its gap) | **DEFEND — the anti-drift answer** |
| **Lifecycle state machine + auto-bump + history audit** | **DIFFERENTIATED** | **DEFEND** |
| **Orchestrator that drains the typed graph + escalation/shelving cascade** | **DIFFERENTIATED** (the real moat) | **DEFEND + close durability gap** |
| **Git-canonical multi-vendor substrate** | **DIFFERENTIATED (implementation)** | **DEFEND — the portability mechanism** |

## Capability gaps to close (prioritized — SPIKE-45 children)

Surfaced by the multi-agent-frontier dive; ordered by "what a technical evaluator hits first":
- **P1 — Resumable orchestrator checkpointing** (close the LangGraph/CrewAI/MAF gap). A crashed drain isn't step-resumable today; formalize phase transitions into a replayable execution log keyed to spec+worktree. Edge: *git-canonical AND crash-resumable*.
- **P2 — Graph-query surface + vs-Kiro/vs-Spec-Kit positioning** (flagship outsmart move). `aida graph` (BlockedBy / epic-rollup / cross-feature impact) over MCP — the query Kiro's flat per-feature markdown *structurally cannot answer*. Make it the demo.
- **P3 — Live inter-agent mailbox on the substrate** (git-canonical + replayable, vs Agent-Teams/Ruflo ephemeral local mailboxes). MCP `send_message`/`read_inbox`. Aligns with SPIKE-10.
- **P4 — Spec-graph-aware semantic recall** (embeddings as a cache projection over descriptions+traces+history) — match Ruflo HNSW/CrewAI recall, grounded in the typed graph.
- **P5 — Parallel-drain legibility** (`aida queue progress`/`findings`: N draining / M shelved / K escalated + blocked-by graph) — surface the moat on demand vs Composio's dashboard, quiet-depth-compatible.

Plus two from the convergence dive:
- **RIDE: rich AGENTS.md generator** — emit a spec-conformant, graph-projected AGENTS.md (+ `.agents/rules/` when it lands) so the standard everyone adopted is *fed by* AIDA's structured truth, and the drift Spec Kit suffers is eliminated at the AGENTS.md layer. Adopt v1.1 frontmatter early (cheap conformance insurance).
- **OPTION: agent-native ReqIF import/export bridge** (SPIKE, gated on the bugs/marketing phase) — plant AIDA as ReqIF-for-agents before anyone bridges it; opens regulated/ALM markets; pre-empts the highest-impact tail risk.

## Beyond-software use cases: one feature, two watches, three avoids

- **PURSUE (as a feature, not a pivot): ADRs / architecture-decision records as a graph.** The only candidate where git-canonical is a *genuine* structural advantage AND underserved (log4brains/adr-tools are flat ID-less markdown) AND a *short reach* (AIDA already has the `doc` type + typed relationships + cross-repo IDs). Frame as *the substrate naturally also holds your decisions* — depth on use — NOT "AIDA is an ADR tool."
- **WATCH: compliance-as-code on git** — strongest structural fit (git IS the audit artifact regulators want; FDA/OSCAL precedent), but the value is 80% the validation/e-sig/RBAC wrapper AIDA lacks. Keep the audit substrate clean + namable; watch for an *agent-angle* GRC wedge; don't build a QMS.
- **WATCH (defensive): portable agent memory.** ⚠️ **Stale-line correction:** round-1's "no production library ships portable git memory" is **now false — Letta shipped git-based "Context Repositories."** Hold the *structured*-memory differentiation (IDs + typed graph + traces + MCP), not the *git*-memory idea. Track Letta adding any graph/ID layer.
- **AVOID: ELN, internal wikis, PKM/Zettelkasten** — all fail the structural-fit gate (value = validation / collab / recall, not diffable graphs) and face entrenched incumbents *on AIDA's own pitch* (Benchling, Notion/Confluence, Obsidian).

## The positioning line that survives the convergence

> *"AGENTS.md and Spec Kit standardized how agents read your project and your specs. AIDA is the graph underneath — stable IDs, typed relationships, enforced traces, and a lifecycle that keeps them all true — and it writes those standard files for you. It's the only one where an orchestrator drains that graph through a spec-grounded escalation cascade, and the only one portable across every vendor because it lives in git."*

Anchored on **incentive** (single-vendor runtimes won't make memory portable; SDD tools won't enforce the trace because their value is generation, not maintenance) — which ages better than any capability claim.

## Tripwires to monitor (refresh ~6 weeks)

- agents.md Issues **#135** (v1.1 frontmatter), **#179** (`.agents/rules/`), **#105** (tool permissions) — *structure creep* toward relationship fields.
- MCP 2026 roadmap **agent-communication workstream** — could standardize an inter-agent handshake overlapping our escalation substrate (defend on *content*, not transport).
- **Spec Kit roadmap** — any move toward stable IDs / relationship graph / trace enforcement (currently aimed elsewhere).
- **Letta Context Repositories** adding a graph/ID layer; any **ReqIF↔agent bridge** announcement (highest-impact tail risk).

## Honest meta-read

The mechanical edges (isolation, "specs as artifacts," MCP-exposed, context files) are commoditized or table-stakes. **The durable, un-commoditized core is the *structured graph on git, drained by an orchestrator with a spec-grounded escalation cascade, portable across vendors* — and the enforcement/lifecycle machinery that keeps it true.** AIDA's real risk is **distribution**, not differentiation. The build roadmap (P1–P5 + AGENTS.md-generator) defends and sharpens the moat; the distribution problem is the bugs→stability→**marketing** phase the operator already sequenced.
