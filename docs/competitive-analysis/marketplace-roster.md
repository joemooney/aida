# Marketplace roster — substrate & agent-orchestration projects AIDA tracks

**Living document** · Last updated: 2026-06-12 · Refresh cadence: with each ecosystem scan (see `README.md` · `signals-to-watch.md`)

> **Why this file exists.** AIDA is built in a fast-moving field. We actively survey the agent-tooling marketplace so we **build on / interoperate with prior art rather than reinvent it** — and so anyone reviewing AIDA can see the landscape we measure ourselves against. This is the *roster* (who's in the field, by category); the dated files in this directory are the point-in-time *analyses*. Inclusion here is not an endorsement, and absence is not a judgment — it's a working list, kept honest by refresh.
>
> **Discovery method** (so the list stays comprehensive, not vocabulary-blind — see `[[feedback_competitive_discovery_multimodal]]`): sweep curated *awesome-lists* + comparison catalogs by category; watch *known builders* (a Yegge/Karpathy/etc. launch is a signal); search the *problem in its many names* ("issue-driven development", "agent memory", "coding agent fleet"), not just AIDA's vocabulary; track *star-velocity / trending / HN*; snowball from each found tool's README and "X vs Y" posts. (We previously missed Beads/Gas Town for ~weeks by searching only our own vocabulary in a fixed category — this method is the correction.)

Star counts and "verified" flags are point-in-time; treat as directional. `[V]` = a claim we've checked against primary source in a dated analysis; otherwise catalog-level.

---

## A. Substrate — spec / issue / intent / memory graphs (AIDA's own lane)

The "what to build and why, tracked structurally" layer. AIDA's nearest neighbors.

| Project | What it is | Relation to AIDA |
|---|---|---|
| **Beads** (`steveyegge/beads`, ~24.5k★) | Git/Dolt-backed issue + memory graph for coding agents ("50 First Dates" problem); typed links, hash IDs, `bd ready` gate, MCP | **Nearest substrate competitor.** Deep-dived: `2026-06-12-beads-gastown-moat-rescope.md` (+erratum), `2026-06-12-beads-gastown-vs-aida.md`. Author positions it as an *execution* tool, abdicating the planning/requirements front. |
| **GitHub Spec Kit** (`github/spec-kit`, ~112k★) | Markdown spec-driven development, agent-agnostic, now with a YAML workflow engine + Issues integration | Near competitor (SDD lane). See `2026-05-31-round2-moat-gaps-moves.md`, `docs/positioning/vs-spec-kit.md`. |
| **Kiro** (AWS) | EARS-notation spec IDE, agent hooks, greenfield-oriented | Near competitor. `docs/positioning/vs-kiro.md`. |
| **Miyabi** | Framework explicitly branded **"issue-driven development"** | **Same category, by name — investigate.** Not yet analyzed. |
| **Intent** | Spec-driven macOS workspace; **Coordinator / Implementor / Verifier** agent roles over living specs | **Closest to AIDA's role + lifecycle model seen so far.** Not yet analyzed. |
| **Augment Cosmos** | Living specs + org-scale multi-agent orchestration; semantic understanding across 400k+ files | Serious adjacent (scale + living-spec). Not yet analyzed. |
| **OpenSpec** | Delta-marked specs, brownfield iteration, 3-phase state machine (proposal/apply/archive) | SDD lane; lifecycle-state-machine overlap. |
| **BMAD-METHOD** | 21+ role-based agents generating structured SDLC docs | Heavy-process SDD; role overlap. |
| **Backlog.md** | Git/markdown task tracker for agents | Adjacent (lightweight tracker). |
| **Agent Mail / AgentMail** | "Gmail-like" agent-to-agent coordination + file-reservation/intent layer; pairs with Beads | Overlaps AIDA's mailbox + lease ideas. |
| **Karpathy-style `*.md`** | Structured markdown queryable by the agent | The floor AIDA builds above. `docs/positioning/vs-karpathy-md.md`. |
| Embedding/RAG memory (distinct sub-lane): **Mem0, LangMem, Graphiti, Cognee, Letta, Dreams, mnemex** | Vector/graph agent memory libraries | Different mechanism (recall, not requirement graph). `2026-05-26-agent-memory-libraries.md`. |

## B. Orchestration — multi-agent coding runners / fleets

The "drive N agents through work" layer. AIDA's orchestrator/drain lives here.

| Project | What it is | Relation to AIDA |
|---|---|---|
| **Gas Town** (`gastownhall/gastown`, ~15.9k★, Yegge) | Fleet manager ("Kubernetes for agents"): Mayor/Deacon/Witness, Refinery merge-queue, Convoys; 10+ vendors (Claude/Gemini/Codex/Cursor/AMP/Copilot/…) | **Nearest orchestration competitor.** Broader/more mature at fleet scale than AIDA's burndown; AIDA's edge is the front approval-gate + advisor-escalation, not scale. Same dated analyses as Beads. |
| **Composio Agent Orchestrator** | Agents in isolated worktrees, PR autonomy, CI-retry milestone gates | **AIDA's drain shape** — closest orchestrator analogue. |
| **Bernstein** (Apache-2.0) | Planning→merge pipeline, deterministic scheduling, "Janitor" pre-merge quality gates | **AIDA's drain shape** — deterministic-coordination overlap. |
| **Claude Code Agent Teams** (Anthropic, native) | Built-in parallel agents + inter-agent messaging + escalation | **Platform risk** — native absorption of the coordination layer. `[[feedback_ride_native_within_vendor_own_cross_vendor]]`. |
| **Conductor** (Melty) / **Microsoft Conductor** | YAML-defined workflow orchestrators, parallel groups, dashboards | Workflow-DAG orchestration. |
| **Code Conductor** / **Baton** | GitHub-issue claim/poll-dispatch-reconcile loops | Issue-driven dispatch (close to "drain the queue"). |
| **Vibe Kanban** (Apache-2.0) | Kanban web UI, MCP task decomposition, 10+ providers | Visual parallel-agent board. |
| **Claude Squad** (AGPL, ~611★) | tmux+worktree TUI session manager | Parallel-session TUI (cf. AIDA's TUI). |
| **Nimbalyst** (← Crystal, deprecated) / **Emdash** | Desktop parallel Claude/Codex sessions, worktree isolation, ~22 providers | Desktop session managers. |
| **Multiclaude** (Dan Lorenc) · **Goose** · **Kilo Agent Manager** · **Shipyard** · **OpenClaw+Antfarm** · **amux/dmux** · **Cursor Background Agents** · **Antigravity (AGY)** | Assorted runners / multiplexers / IDE background agents | Field breadth; AGY is one of this project's own dispatch agents. |
| General frameworks (more abstract): **LangGraph, CrewAI, AutoGen, MetaGPT, CAMEL, AgentScope, DeerFlow, OpenAI Agents SDK** | Multi-agent application frameworks | Substrate-agnostic; AIDA is not one of these. `category-summaries/`. |

## C. Standards / context layers AIDA rides (not competes with)

**AGENTS.md** (LF Agentic AI Foundation, 60k+ repos) · **MCP** (~97M installs) · **Skillfold** · the **Claude Code plugin marketplace**. AIDA's posture is to *generate / speak* these, not replace them. See `signals-to-watch.md`.

---

## How AIDA positions against this roster

The one-line honest summary (full treatment: the dated analyses + `docs/positioning/`): the typed-graph-on-git substrate is **no longer unique** (Beads has it; Spec Kit/Kiro have spec scaffolding); fleet-scale orchestration is **more mature elsewhere** (Gas Town). AIDA's claimed distinct edges are **(a)** a programmatic *pre-work* approval/authority gate (agents can't self-bless work in), **(b)** enforced *code↔spec* traceability + lifecycle history, and **(c)** the requirements/intent altitude that the nearest substrate competitor's author explicitly abdicates. Those claims are under active, adversarial review — see the dated files. **The honest open risk is distribution, not features.**
