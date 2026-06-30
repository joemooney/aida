# Marketplace roster — substrate & agent-orchestration projects AIDA tracks

**Living document** · Last updated: 2026-06-29 · Refresh cadence: with each ecosystem scan (see `README.md` · `signals-to-watch.md`)

> **Why this file exists.** AIDA is built in a fast-moving field. We actively survey the agent-tooling marketplace so we **build on / interoperate with prior art rather than reinvent it** — and so anyone reviewing AIDA can see the landscape we measure ourselves against. This is the *roster* (who's in the field, by category); the dated files in this directory are the point-in-time *analyses*. Inclusion here is not an endorsement, and absence is not a judgment — it's a working list, kept honest by refresh.
>
> **Discovery method** (so the list stays comprehensive, not vocabulary-blind — see `[[feedback_competitive_discovery_multimodal]]`): sweep curated *awesome-lists* + comparison catalogs by category; watch *known builders* (a Yegge/Karpathy/etc. launch is a signal); search the *problem in its many names* ("issue-driven development", "agent memory", "coding agent fleet"), not just AIDA's vocabulary; track *star-velocity / trending / HN*; snowball from each found tool's README and "X vs Y" posts. (We previously missed Beads/Gas Town for ~weeks by searching only our own vocabulary in a fixed category — this method is the correction.)
>
> **Full landscape scan (June 2026):** a ~50-player map across frontier agents, IDEs, orchestrators, spec/record tools, SaaS trackers and standards — each with a substance-vs-hype note and confidence flag — is maintained in `docs/research/2026-06-17-agent-coordination-market-landscape.md` (lands via PR #977). This roster is the curated *lane view* (substrate / orchestration / standards + the platforms we ride); that doc is the exhaustive map.

Star counts and "verified" flags are point-in-time; treat as directional. `[V]` = a claim we've checked against primary source in a dated analysis; otherwise catalog-level. **Don't oversell competitors:** stars ≠ engineering; the most-hyped capabilities are repeatedly the least-GA (Tessl's spec→code, Jira Rovo Max, Beads' daemon), and self-reported benchmarks are not capability.

---

## A. Substrate — spec / issue / intent / memory graphs (AIDA's own lane)

The "what to build and why, tracked structurally" layer. AIDA's nearest neighbors.

| Project | What it is | Relation to AIDA |
|---|---|---|
| **Beads** (`gastownhall/beads`, ~24.5k★) `[V]` | Typed issue + memory graph for coding agents ("50 First Dates"); richest typed-link vocab (relates_to/duplicates/supersedes + deps + epics), hash IDs, `bd ready` gate, MCP | **Nearest substrate competitor.** `2026-06-12-beads-gastown-vs-aida.md`. **Storage corrected (verified via changelog):** SQLite-synced-to-JSONL-by-a-daemon through early 2026 → SQLite removed v0.58 (Mar) → **Dolt is now source-of-truth** v1.0.5 (May), JSONL demoted to optional export — so "Beads is git-native" is **stale**, and the daemon/corruption criticism is **SQLite-era, OBE**; cite as history. Author positions it as an *execution* tool, abdicating the requirements front. |
| **Gas Town / Gas City** (`gastownhall`, ~15.9k★, Yegge) `[V]` | *Listed under Orchestration (B); the store under it is Beads.* | — |
| **GitHub Spec Kit** (`github/spec-kit`, ~113k★) | Markdown spec-driven development, agent-agnostic; `/constitution→/specify→/plan→/tasks→/implement` | Near competitor (SDD lane). Explicitly "a convention, not infrastructure" — no graph/stable-IDs/trace/enforcement. `docs/positioning/vs-spec-kit.md`. |
| **Kiro** (AWS) | EARS-notation spec IDE; req→design→tasks `.md` per feature; GA May 2026 (replaced Amazon Q) | Near competitor. Flat per-feature md (no cross-spec graph), closed/metered, single-agent. `docs/positioning/vs-kiro.md`. |
| **Linear** (`linear.app`) | SaaS tracker; **mature typed graph** (4 relation types + parent/sub, stable IDs, GraphQL); "Linear for Agents" makes agents first-class members | **Honesty check:** disproves "we have a typed graph, they don't." Gaps vs AIDA: no git-portable store, no code traces, **one-issue-to-one-agent (no fleet)**. |
| **Jira / Atlassian Rovo** | SaaS tracker; directional typed links + custom types **predating its agents ~15 yrs**, REST-traversable; Rovo agents GA May 2026 | Best-resourced typed graph in the field. Gaps: no inline code traces, no git-portable store, no fleet (Rovo "Max" multistep = Early Access). |
| **Miyabi** | Framework explicitly branded **"issue-driven development"** | **Same category, by name — investigate.** Not yet analyzed. |
| **Intent** | Spec-driven macOS workspace; **Coordinator / Implementor / Verifier** agent roles over living specs | **Closest to AIDA's role + lifecycle model seen so far.** Not yet analyzed. |
| **Augment Cosmos** | Living specs + org-scale multi-agent orchestration; semantic understanding across 400k+ files | Serious adjacent (scale + living-spec). Not yet analyzed. |
| **OpenSpec** (`Fission-AI`, ~55k★) | Delta-marked specs, brownfield iteration, 3-phase state machine (proposal/apply/archive) | SDD lane; lifecycle-state-machine overlap. Genuine edge = specs-as-baseline, changes-as-deltas. |
| **Tessl** (Podjarny, $125M) | "AI-native" spec platform + hosted spec/skills **Registry** | **Vision >> shipped:** `tessl build` (spec→code) still private beta after ~18 months; spec→code is non-deterministic; pivoted to "agent enablement / skills". |
| **BMAD-METHOD** | 21+ role-based agents generating structured SDLC docs | Heavy-process SDD; role overlap. |
| **Backlog.md** (`MrLesk`, ~5.7k★) | Git/markdown task tracker for agents; stable IDs + one `dependencies[]` list | Adjacent (most-polished plain-md board); single-agent (no claiming). |
| **Claude Task Master** (`eyaltoledano`, ~27.5k★) | PRD→tasks; numeric IDs + deps + best dep *tooling* (cycle detect/repair); 36 MCP tools | Adjacent tracker; "fleet" = intra-Claude-session roles. License `NOASSERTION` — check. |
| **ergo** (`sandover`, B. Harvey, ~37★) | JSONL **event-log** task tracker, replay-reconstructed, atomic claiming | The minimalist anti-Beads reaction ("inspired by beads, but simpler, sounder, 5–15× faster"). No typed graph / traces. |
| **GNAP** (`farol-team/gnap`, ~66★) | **Git-native** coordination record: `agents.json` roster + `tasks/`/`runs/`/`messages/` JSON; poll-via-git, no server | **Nearest *file-based fleet-coordination* neighbor** (the closest to AIDA's portable-store + mailbox shape) — but **stalled** (quiet since Mar 2026), no typed depends-on edges, no traces. (`mrdummy550/gnap` = negligible derivative.) NOT IETF OAuth GNAP. |
| **Agent Mail / AgentMail** | "Gmail-like" agent-to-agent coordination + file-reservation/intent layer; pairs with Beads | Overlaps AIDA's mailbox + lease ideas. |
| **Karpathy-style `*.md`** | Structured markdown queryable by the agent | The floor AIDA builds above. `docs/positioning/vs-karpathy-md.md`. |
| Embedding/RAG memory (distinct sub-lane): **Mem0, LangMem, Graphiti, Cognee, Letta, Dreams, mnemex** | Vector/graph agent memory libraries | Different mechanism (recall, not requirement graph). `2026-05-26-agent-memory-libraries.md`. |

## B. Orchestration — multi-agent coding runners / fleets

The "drive N agents through work" layer. AIDA's orchestrator/drain lives here. **Key split:** durable, queryable coordination model vs *fan-out launcher* (humans coordinate). Almost all are fan-out.

| Project | What it is | Relation to AIDA |
|---|---|---|
| **Gas Town / Gas City** (`gastownhall`, ~15.9k★, Yegge) `[V]` | Fleet manager ("Kubernetes for agents"): Mayor/Deacon/Witness, Refinery merge-queue, Convoys, on Beads. **Gas City** = Gas Town rewritten as an SDK (composable "packs", v1.0 Apr 2026) | **Nearest orchestration competitor** + the only real *durable coordination model*. OSS is ~Claude-only; **cross-vendor is the paid Kilo cloud.** Yegge's own caveats: "cash guzzler ~$100/hr", "fixes get lost", needs a power-operator. AIDA's edge is the front approval-gate + advisor-escalation + git-canonical store, not scale. |
| **Goosetown** (`block`→`aaif-goose`, ~128★) | Multi-agent layer **on goose**, "flocks" (research→build→review) coordinating via a broadcast "Town Wall"; cross-vendor "crossfire" review | **Explicitly inspired by Gas Town.** Early; coordination is an in-flight broadcast log, not a durable graph. (goose itself = Block's single-agent harness, ~49.6k★, Linux Foundation.) |
| **Claude Flow** (`ruvnet`, ~59.8k★) | Hive/swarm meta-harness; "queen" delegates to parallel workers; durable state in SQLite (`.swarm/memory.db`) | Durable-state orchestrator, but **Claude-centric**; "84.8% SWE-bench" is **self-reported — discount.** |
| **Composio Agent Orchestrator** | Agents in isolated worktrees, PR autonomy, CI-retry milestone gates | **AIDA's drain shape** — closest orchestrator analogue. |
| **Bernstein** (Apache-2.0) | Planning→merge pipeline, deterministic scheduling, "Janitor" pre-merge quality gates | **AIDA's drain shape** — deterministic-coordination overlap. |
| **Claude Code Agent Teams** (Anthropic, native) | Built-in parallel agents + inter-agent messaging + escalation; file-locked, local, Claude-only | **Platform risk** — native absorption of the coordination layer. `[[feedback_ride_native_within_vendor_own_cross_vendor]]`. |
| **Warp** + **Oz** (`warp.dev`) | Agentic terminal + **Oz** orchestrates parallel agents *across harnesses* (Claude Code, Codex, own); BYO-key | Multi-harness orchestrator that actually ships; resells 3rd-party LLMs. |
| **Conductor Build** (Melty Labs, YC) | Native **macOS** app, parallel agents in worktrees + review/ship-PR dashboard; closed, pre-1.0 | Fan-out launcher + dashboard. *(Distinct from `ryanmac/code-conductor` below — same generic name.)* |
| **Code Conductor** (`ryanmac`) / **Baton** | GitHub-issue claim/poll-dispatch-reconcile loops; Claude-only | Issue-driven dispatch (close to "drain the queue"); thin layer, tiny adoption. |
| **gnhf** (`kunchenguid`, ~2.6k★, MIT) `[V]` | "Good Night, Have Fun" — single-objective ralph-style overnight loop: one free-text prompt → commit-on-success/reset-on-failure iterations until a cap/`--stop-when` trips; **7 agents + ACP**, `npm i -g`, sleep-prevention | **Nearest *autonomy-layer* neighbor** (same author as AXI). `2026-06-29-gnhf-vs-aida.md`. **Prompt-driven loop vs AIDA's spec-graph-driven drain:** no IDs/traces/lifecycle/coordination, no review/merge (leaves a branch to review by hand), parallelism = N uncoordinated worktree processes. Wins on simplicity/install/executor-breadth/adoption; AIDA wins on structured-traceable-self-closing. Learn: the framing, sleep-prevention + backoff, ACP executor breadth. |
| **Sculptor** (Imbue) | Desktop UI, parallel agents each in a Docker container + IDE "pairing"; Claude-first (+Codex) | Fan-out + sandbox isolation; a Claude Code wrapper at heart. |
| **container-use** (`cu`, Dagger) | MCP server giving each agent an isolated container + git branch | Isolation layer, not a coordinator; review via plain git. |
| **Devin / "manages Devins"** (Cognition) | Manager→child fan-out (map-reduce); closed/hosted | **Cognition's own blog lists cross-agent shared state as unsolved** — external validation of the gap. |
| **graft** (`coconinja2`) | Runtime **lock-manager**: OS-style claim-before-write leases at file-resource granularity (in-memory bus) | Complementary, not competing — prevents concurrent-edit conflicts, a different layer than intent coordination. |
| **Vibe Kanban** (Apache-2.0, 27k★) | Kanban web UI, MCP task decomposition, 10+ providers | **SHUT DOWN Apr 2026** ("thin launcher couldn't monetize", mostly free users) → community-maintained. Direct market signal: fan-out alone isn't a business. |
| **Claude Squad** (AGPL, ~7.8k★) | tmux+worktree TUI session manager | Parallel-session TUI (cf. AIDA's TUI); no shared graph. |
| **Nimbalyst** (← Crystal, deprecated) / **Emdash** | Desktop parallel Claude/Codex sessions, worktree isolation, ~22 providers | Desktop session managers. |
| **Multiclaude** (Dan Lorenc) · **Goose** · **Kilo Agent Manager** · **Shipyard** · **OpenClaw+Antfarm** · **amux/dmux** · **Cursor Background Agents** · **Antigravity (AGY)** | Assorted runners / multiplexers / IDE background agents | Field breadth; AGY is one of this project's own dispatch agents. |
| General frameworks (more abstract): **LangGraph, CrewAI, AutoGen→Microsoft Agent Framework (1.0 GA Apr 2026), MetaGPT, CAMEL, AgentScope, DeerFlow, OpenAI Agents SDK** | Multi-agent application frameworks (build one app's agents; state in process/service) | Substrate-agnostic; AIDA is not one of these. `category-summaries/`. |

## C. Standards / context layers AIDA rides (not competes with)

**MCP** (Anthropic → **Linux Foundation / Agentic AI Foundation**; agent↔tool) · **A2A** (Google → Linux Foundation; agent↔agent, **explicitly stateless**) · **ACP** (IBM → **merged into A2A**, 2025) · **AGENTS.md** (LF AAIF, 60k+ repos) · **Skillfold** · the **Claude Code plugin marketplace**. AIDA's posture is to *generate / speak* these, not replace them. **Crucial gap:** *none of MCP/A2A/ACP standardizes a durable, multi-vendor-readable coordination record* — that slice is the open frontier. Complementary research: **Authenticated Delegation** (South et al., arXiv:2501.09674, MIT) — agent-identity + delegation tokens, a trust layer a coordination substrate sits *atop*. See `signals-to-watch.md`.

## D. Frontier coding agents & IDEs — the platforms AIDA coordinates (mostly *not* direct competitors)

These are the agent *products* a multi-vendor fleet mixes; AIDA's posture is to ride/coordinate them, not compete. Listed so "what about X?" has an answer. (Full substance-vs-hype detail in the June-2026 landscape doc.)

| Player | Org | Cross-vendor | One-liner (honest) |
|---|---|:-:|---|
| **Claude Code** | Anthropic | ❌ | Strongest revenue (~$2.5B run-rate); subagents/teams, but Claude-only, token-heavy. |
| **OpenAI Codex** | OpenAI | ❌ | CLI + cloud parallel agents; ~4M WAU; "teammate" framing oversells. |
| **GitHub Copilot** (+ coding agent) | GitHub/MS | ✅ | One of only two frontier players with real OpenAI+Anthropic+Google choice. |
| **Google Antigravity** | Google | ✅ | The other true cross-vendor frontier play (Gemini+Claude+GPT-OSS); public preview. |
| **Cursor** | Anysphere | ✅ | Fast (own Composer); ~8 parallel agents — but single-human/keyboard, not durable coordination. |
| **Devin** / **Devin Desktop** (← Windsurf) | Cognition | 🟡 | Most-hyped/most-criticized: **~$25B val vs ~$0.5B ARR, no updated SWE-bench**; Devin Desktop opens via Agent Client Protocol (cross-vendor). |
| **Factory.ai (Droids)** | Factory AI | ✅ | Strongest startup BYO-model + real orchestration ("Missions"); long-horizon reliability unsolved (per CEO). |
| **OpenHands** (← OpenDevin) | All Hands AI | ✅ | Most benchmark-transparent open agent (~68% SWE-bench w/ Opus); single-agent loop, tiny commercial scale. |
| **Amp** | Amp Inc. (← Sourcegraph) | routing | Distinctive multi-model routing + subagents; "profitable" unaudited. |
| **Cline** / **Kilo Code** / **Aider** / **Zed** / **Goose** | OSS | ✅ | The model-agnostic OSS layer (Cline ~63k★ leader; Kilo $8M seed; Aider mature git-native; Zed native-speed; Goose local-first). |
| **Replit Agent** / **Tabnine** / **Qodo** | — | mixed | App-builder ($9B, reliability caveats) / air-gapped enterprise / code-integrity (15+ review agents). |

**Recent reversals to know cold:** Windsurf → **Devin Desktop** (Cognition, Jun 2 2026); **Roo Code → ZooCode** (archived May 2026); **Continue.dev** OSS → commercial pivot; **Vibe Kanban** shut down (Apr 2026); **ACP merged into A2A**; **Amp** spun out of Sourcegraph (Dec 2025); **Graphite** acquired by Cursor (Dec 2025).

---

## How AIDA positions against this roster

The one-line honest summary (full treatment: the dated analyses + `docs/positioning/` + the June-2026 landscape scan): the typed-graph-on-git substrate is **no longer unique** — Beads has the richest typed-link vocab, and **Linear and Jira/Rovo ship mature, API-queryable typed graphs too** (so "we have a graph, they don't" is simply false); fleet-scale orchestration is **more mature elsewhere** (Gas Town). AIDA's claimed distinct edges, narrowed to what survives the scan:

- **(a) code↔spec inline traceability** — a *clean* wedge: **none** of the 12+ spec/record tools surveyed has machine-checkable `trace:ID` links in source (all use branch/PR/commit conventions; Tessl runs the opposite direction);
- **(b) a programmatic *pre-work* approval/authority gate** (agents can't self-bless work in) — distinct from merge-time verification gates (Gas Town's Refinery);
- **(c) the fleet-coordination substrate** (queue / lease / drain / escalation / inter-agent mailbox) — a wedge vs *almost everyone* (trackers delegate one issue to one agent); matched only by Beads (DB-backed) and the **stalled** file-based farol-GNAP;
- **(d) a git-canonical, single-source store** (plain YAML + rebuildable cache, no daemon, no SQL engine) — a deliberate alternative to *both* of Beads' eras; a wedge against the SaaS incumbents, though shared with the files-in-git OSS crowd.

The **white space is well-evidenced**: *cross-vendor + free + self-hosted + durable coordination model* is unoccupied (Beads is SQL-DB-backed, Gas Town's cross-vendor is paid cloud, Claude Flow is Claude-only). Vibe Kanban's shutdown and Cognition's "shared state unsolved" admission both point at this layer. These claims are under active, adversarial review — see the dated files. **The honest open risk is distribution, not features** (Gas Town/Beads has Yegge's distribution and ~10× the traction).
