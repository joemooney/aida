# AI Agent Coordination — Market Landscape Roster

**Status:** Living reference · **Snapshot date:** 2026-06-17 · **Companion to:** the theory paper, the decision framework, and the research proposal in this directory · **Purpose:** a meeting-grade map of the players in and around multi-vendor AI agent coordination — so "what about X?" has an answer.

> **How to read this.** Two axes matter: **cross-vendor** (can more than one model vendor's agents participate / bring-your-own-model?) and **coordination model** (does it maintain a *durable, queryable* shared model, or just *fan agents out* and let humans coordinate?). Entries carry an **honest substance-vs-hype** note — stars and funding are *not* capability. Confidence: ✅ verified from primary sources · 🟡 likely · ⚠️ uncertain. This is a **dated snapshot** of a fast-moving field (see §"Reversals" — three names changed in the last six weeks); re-verify before quoting.

---

## 0. The one-paragraph state of play

Multi-agent *execution* (spawn N agents, isolate in worktrees/containers, show a dashboard) is now table stakes and largely commoditized. Cross-vendor model choice is the norm at the open-source layer and the exception at the frontier-lab layer (only **GitHub Copilot** and **Google Antigravity** offer real Anthropic+OpenAI+Google choice). The genuinely scarce, contested thing is a **durable, queryable, *cross-vendor* coordination record** — shared task/role/lease/dependency state that survives sessions and spans vendors. Almost nobody ships it: the only real fleet-coordination neighbors are **Beads + Gas Town** (Yegge; now a Dolt SQL store, single-vendor in OSS) and **farol-team/GNAP** (file-based, but stalled). Two 2026 market signals underline the gap: **vibe-kanban** (27k★, cross-vendor) **shut down** because a thin launcher couldn't monetize, and **Cognition (Devin) publicly lists cross-agent shared state as unsolved.**

---

## 1. Frontier-lab coding agents (mostly single-vendor)

| Player | Org | X-vendor | Coordination | Honest take |
|---|---|:-:|---|---|
| **Claude Code** | Anthropic | ❌ (Claude only; external MCP tools) | subagents (parallel) + agent teams (sequential) + Agent view | Strongest revenue (~$2.5B run-rate). Real orchestration, but subagents each burn tokens, share one filesystem, vendor-locked. ✅ |
| **OpenAI Codex** | OpenAI | ❌ | CLI single + Codex Cloud parallel | ~4M WAU (🟡). Genuinely agentic; "SWE teammate" framing oversells — still human-in-loop. ✅ |
| **Google Jules** | Google | ❌ (Gemini) | async queue, isolated VM each | Async + CI self-heal differentiated; output needs review; Gemini lock-in. ✅ |
| **Gemini CLI** | Google | served, forkable | single | Apache-2.0, ~105k★ — but free Google backend **ended ~June 18 2026**, pushing to Antigravity/BYO. ⚠️ |
| **GitHub Copilot** + coding agent | GitHub/MS | ✅ **multi-model leader** (OpenAI/Anthropic/Google) | Agent Mode (interactive) + async coding agent (isolated runners) | Model choice + GitHub-native is the real edge; autonomous PRs need review; "choice" limited to GitHub's curated shelf. ✅ |
| **Google Antigravity** | Google | ✅ (Gemini 3 + Claude Sonnet 4.5 + GPT-OSS) | "Manager Surface" spawns/orchestrates async agents | Rare cross-vendor move by a frontier lab; still **public preview**, "free" land-grab, Google-coupled. ✅ |

## 2. Agentic IDEs / editors

| Player | Org | X-vendor | Coordination | Honest take |
|---|---|:-:|---|---|
| **Cursor** | Anysphere | ✅ (+ own Composer) | up to ~8 parallel agents, worktree isolation | Fast; worktree parallelism real — but "parallel agents" = one human, one keyboard, not durable multi-vendor coordination. $50B/xAI press **unconfirmed** ⚠️. ✅ |
| **Devin Desktop** (← **Windsurf**) | Cognition | ✅ via **Agent Client Protocol** (Codex, Claude, OpenCode + own) | Agent Command Center (Kanban) default surface | Genuine cross-vendor (ACP) bet. **Churn:** 2-week-old rebrand, Cascade EOL ~July 2026, founders left for Google. ✅ |
| **Zed** | Zed Industries | ✅ (+ ACP external agents) | Parallel Agents (Apr'26) | Native Rust speed real; AI layer less turnkey (memory MCP-composed, immature rules). ✅ |
| **Trae** | ByteDance | 🟡 (managed, not true BYO-key) | mostly single | Capable free multi-model IDE; opaque routing, **data-governance concerns** (ByteDance). ✅ |
| **Replit Agent** | Replit | ❌ (Claude-centric) | single long-horizon (~200min) | $9B valuation, ~$525M ARR. **Reliability caveat:** deleted a prod DB despite instructions (Jul'25). ✅ |
| **AWS Kiro** | AWS | ❌ (Claude menu; account-gated, metered) | mostly single; intra-product parallel tasks | GA May 2026 (replaced Amazon Q). Spec-first (req→design→tasks `.md`); credit-expensive; closed IDE+model. ✅ |
| **Warp** + **Oz** | Warp.dev | ✅ (+ BYO-key) | **Oz** orchestrates parallel agents across harnesses (Claude Code, Codex, own) | Ships ahead of hype; multi-harness orchestrator works; resells 3rd-party LLMs, no fresh raise since '23. ✅ |

## 3. Enterprise / autonomous-engineer startups

| Player | Org | X-vendor | Coordination | Honest take |
|---|---|:-:|---|---|
| **Devin** | Cognition | 🟡 (closed routing) | "run many Devins" fan-out; core single | Most-hyped/most-criticized. **~$25B val vs ~$0.5B ARR; no updated public SWE-bench since launch.** ✅ (model/coord 🟡) |
| **Factory.ai (Droids)** | Factory AI | ✅ **strongest BYO-model** | true multi-agent ("Missions", always-on) | $150M Series C @ $1.5B; named logos (Nvidia/Adobe/EY). CEO admits long-horizon reliability unsolved. ✅ |
| **OpenHands** (← OpenDevin) | All Hands AI | ✅ (LiteLLM, any provider) | single-agent event-sourced loop | Most benchmark-transparent (~68% SWE-bench w/ Opus); "autonomy" ≈ the BYO model; tiny commercial scale. ✅ |
| **Augment Code** | Augment | ⚠️ closed-leaning | single deep-context agent (no public fleet) | $227M @ ~$1B. Real 500k-file Context Engine claim; **least transparent** on model/arch/customers. ⚠️ |
| **Cosine Genie** | Cosine | ❌ (own model) | single (4-step loop) | 2024 SWE-bench lead was real but dated; ~$6M total — capital far below ambition. ✅ |
| **Qodo** (← CodiumAI) | Qodo | ✅ + air-gapped | multi-agent code-*integrity* (15+ review agents) | $70M Series B; blue-chip logos; "verify not author" niche; benchmark leadership self-cited. ✅ |
| **Amp** | Amp Inc. (← Sourcegraph, Dec'25) | routing (no true BYO-key) | main + subagents (Oracle reviewer, Librarian) | Shipped, distinctive routing; "profitable" unaudited; ~6 months independent. ✅ |
| **Tabnine** | Tabnine | ✅ + **self-host / air-gapped** | "Agentic Platform" (vendor-stated) | Edge = air-gapped + model-switching for regulated buyers; Gartner "Visionary" = vision ahead of execution. ✅ |
| **Sourcegraph Cody** | Sourcegraph | ✅ (enterprise) | single | Free/Pro **terminated Jul 2025**; enterprise-only; superseded by Amp. ✅ |

## 4. Open-source coding CLIs & extensions (all cross-vendor, mostly single-agent)

| Player | Org | Coordination | Honest take |
|---|---|---|---|
| **Cline** (← Claude Dev) | cline.bot | single (Plan/Act); CLI 2.0 adds Kanban parallel | **~63k★, 5M+ installs — category leader**; "multi-agent" framing marketing-forward. ✅ |
| **Goose** | Block → Linux Foundation | single agent + native subagents | ~49.6k★, mature, strong local-first/privacy (Ollama); meta-orchestrator is **roadmap**. ✅ |
| **Aider** | Paul Gauthier | single (`/architect` = 2-model split) | ~46k★, mature, git-native atomic commits; terminal-only, no orchestration/graph. ✅ |
| **Continue.dev** | Continue Dev | single → async per-PR | $65M Series A — but **OSS repo archived/read-only**, pivoted commercial. ⚠️ |
| **Kilo Code** | Kilo-Org | Orchestrator = parallel-worktree (vendor-stated) | ~20k★, $8M seed, 500+ models, fastest-moving fork; differentiation **inherited from Roo/Cline**. ✅ |
| **Roo Code** (→ **ZooCode**) | RooCodeInc | Boomerang = sequential subtask delegation | **Repo archived May 2026** → continues as ZooCode fork. Boomerang pattern was influential. ✅ |
| **SWE-agent / mini** | Princeton/Stanford | single | ~19.5k★, most academically credible (benchmark-anchored); not a product/orchestrator. ✅ |
| **Devika** | stitionai | single | ~19.5k★ but **abandoned since Sept 2024** — stars are a hype artifact. ✅ |

## 5. Multi-agent orchestration — *durable* coordination model (the rare tier)

| Player | Org | X-vendor | State | Honest take |
|---|---|:-:|---|---|
| **Beads** (`bd`) | Yegge / gastownhall | ✅ (Claude/Codex/Factory/Cursor via AGENTS.md) | **Dolt** (versioned SQL; `.beads/dolt/`) | **Richest typed-link vocab** (relates_to/duplicates/supersedes + deps + epics). **NOT git-native anymore** (SQLite+JSONL → Dolt, v0.58+). Real criticisms: migration fragility (vendor-ack'd), auto-daemon rough in sandboxes, CLI bloat (225k-line Go). Harshest quotes are opinion; some criticism is SQLite-era & OBE. ✅ |
| **Gas Town / Gas City** | Yegge | OSS ~Claude-only; cross-vendor via paid Kilo cloud | Beads (Dolt) | Orchestration on Beads; typed roles (Mayor/Polecats/Refinery merge-queue), crash-resume. Yegge's own words: "cash guzzler ~$100/hr", "fixes get lost", needs a power-operator. ✅ |
| **Claude Flow** (→ Ruflo) | ruvnet | ❌ Claude-centric | SQLite (`.swarm/memory.db`) | ~59.8k★; hive/swarm w/ durable state; "84.8% SWE-bench" is **self-reported — treat skeptically**. 🟡 |
| **LangGraph** | LangChain | ✅ | per-**thread** checkpointer (SQLite/Postgres) | Real persisted state, but thread-scoped library for *one app's* graph — not a project-wide coordinator of independent agents. 🟡 |

## 6. Multi-agent orchestration — *fan-out launchers* (humans are the coordination layer)

| Player | Org | X-vendor | Honest take |
|---|---|:-:|---|
| **Goosetown** | Block / aaif-goose | ✅ ("crossfire" multi-model review) | Gas Town's idea rebuilt on goose; "Town Wall" broadcast log (not a durable graph); early ~128★. ✅ |
| **Claude Squad** | smtg-ai | ✅ | tmux + worktree per agent; pure launcher, no shared graph; ~7.8k★. ✅ |
| **Conductor Build** | Melty Labs (YC) | ✅ | Mac app, worktrees + review dashboard; closed, pre-1.0; "team of agents" oversells a parallel runner. ✅ |
| **Code Conductor** | ryanmac | ❌ (Claude) | coordination = GitHub Issues + labels + worktrees; thin layer, ~104★, maybe stalled. 🟡 |
| **Axel** | txtx | ✅ | macOS task queue + unified human-approval inbox; very early ~21★. ✅ |
| **vibe-kanban** | Bloop AI | ✅ | 27k★, polished — **SHUT DOWN Apr 2026** ("thin launcher couldn't monetize"); community-maintained. ✅ |
| **container-use** (`cu`) | Dagger | ✅ (MCP) | isolation layer (container+branch per agent), not a coordinator; ~3.9k★. ✅ |
| **Sculptor** | Imbue | 🟡 Claude-first (+Codex) | parallel agents in containers + IDE pairing; a Claude Code wrapper at heart; ~177★, beta. ✅ |
| **Devin-manages-Devins** | Cognition | ❌ | manager→child fan-out; **Cognition's blog lists shared cross-agent state as unsolved**. ✅ |

## 7. In-process agent app-frameworks (build one app's agents, not coordinate a fleet)

| Player | Org | Note |
|---|---|---|
| **CrewAI** | CrewAI | role-based Python framework; in-process RAM state; ~48–51k★; not a repo coordinator. |
| **AutoGen → Microsoft Agent Framework** | Microsoft | AutoGen + Semantic Kernel **consolidated into MAF 1.0 GA Apr 7 2026** (both now maintenance-only); Azure-centric; per-session, thin production record. |
| **OpenAI Agents SDK** | OpenAI | handoff-based library (Swarm deprecated); OpenAI-anchored; not durable fleet coordination. |
| **Magentic-One** | MS Research | Lead-Orchestrator pattern, folded into MAF as a mode. |
| **AutoGPT** | Significant-Gravitas | now a low-code agent *builder*; ~185k★ (2023 fame); notorious loop/token-burn. |

## 8. Spec / requirements tools (files-in-git, single-agent)

| Player | Org | Typed graph / IDs | Code traces | Honest take |
|---|---|:-:|:-:|---|
| **GitHub spec-kit** | GitHub | ❌ (feature folders, intra-feature task IDs) | ❌ | ~113k★ — distribution muscle, not depth; explicitly "a convention, not infrastructure"; forgotten when the agent moves on. ✅ |
| **OpenSpec** | Fission-AI | ❌ (flat change folders) | ❌ | ~55k★; genuine differentiator = brownfield "specs-as-baseline, changes-as-deltas". ✅ |
| **SPECLAN** | — | hierarchical md tree | ❌ | VS Code ext; Goal→Feature→Req→Scenario→AC; minor/early. 🟡 |

## 9. Task / coordination records (files-in-repo)

| Player | Org | Typed graph / IDs | Code traces | State | Honest take |
|---|---|:-:|:-:|---|---|
| **Backlog.md** | MrLesk | stable IDs + one `dependencies[]` list | ❌ | files (git-optional) | ~5.7k★; most polished plain-markdown task board agents can drive; single-agent (no claiming). ✅ |
| **Claude Task Master** | eyaltoledano | numeric IDs + deps + best dep *tooling* | ❌ | local files + LLM | ~27.5k★; deepest IDE/MCP integration; "fleet" = intra-Claude-session roles. License `NOASSERTION` — check. ✅ |
| **ergo** | sandover (B. Harvey) | sequence/epic/blocking | ❌ | JSONL event log, replay | ~37★, v1.0; "inspired by beads, but simpler, sounder, 5–15× faster" — the minimalist anti-Beads reaction. ✅ |
| **farol-team/GNAP** | Farol Labs | stable IDs, `parent`/`blocked` (no depends-on edges) | ❌ | **pure files in git** (`.gnap/`) | **The reference GNAP.** Roster+runs+messages, genuinely fleet-oriented, poll-via-git — but **stalled** (~66★, quiet since Mar 2026). ✅ |
| **mrdummy550/gnap** | mrdummy550 | ❌ | ❌ | files in git | Negligible derivative (1★, history rewritten in one day, incoherent repo topics). ✅ |
| **"ticket"** | G. Wedow | flat md+YAML | ❌ | `.tickets/` | Single bash script; the anti-Beads minimalist reaction; not a product. 🟡 |

## 10. Trackers-with-AI / SaaS (mature typed graphs — note for honesty)

| Player | Org | Typed graph / IDs | Code traces | State | Honest take |
|---|---|:-:|:-:|---|---|
| **Linear** | Linear | ✅ 4 relation types + parent/sub, stable IDs, GraphQL | ❌ (branch/PR magic-words) | SaaS/DB | **Strongest agent-hospitable tracker**; "Linear for Agents" = first-class agent members, but **one-issue-to-one-agent, no agent↔agent fleet**. Agent API in preview. ✅ |
| **Jira / Atlassian (Rovo)** | Atlassian | ✅ directional typed links + custom types (predates Rovo ~15y), REST-traversable | ❌ (Smart Commits) | SaaS/DB | Arguably **better-resourced typed graph than most OSS**; GA Rovo MCP exposes search/create, not blocked-by traversal; agents = single task-doers, Rovo "Max" multistep = Early Access. ✅ |
| **Graphite** | Graphite (**acq. by Cursor**, Dec'25) | ❌ (PR-stack DAG only) | ❌ | SaaS on git | Code-review/stacked-PR tool, not a spec/coordination platform; a merge-end neighbor only. ✅ |

## 11. Standards & foundations (the layer *beneath* coordination state)

| Item | Note |
|---|---|
| **MCP** (Model Context Protocol) | Anthropic → **Linux Foundation / Agentic AI Foundation (AAIF, Dec 2025)**. Agent↔tool. Tasks primitive (SEP-1686) is single-requestor, **not** shared coordination state. |
| **A2A** (Agent2Agent) | Google → Linux Foundation; **explicitly stateless** (discovery/delegation/messaging "without sharing internal memory, state, or tools"); 150+ orgs by Apr 2026. |
| **ACP** (Agent Communication Protocol) | IBM → **merged into A2A** (2025). |
| **AAIF** | Linux Foundation home for MCP, goose, AGENTS.md (formed Dec 9 2025). |
| **NIST CAISI** | "AI Agent Standards Initiative" (Feb 2026); interoperability/security profile planned Q4 2026. |
| **None** standardize a durable, multi-vendor-readable **coordination record** — the open slice. |

## 12. Adjacent / complementary (not direct competitors)

| Item | Note |
|---|---|
| **Tessl** | Guy Podjarny; **$125M** but `tessl build` spec→code still **private beta** after ~18 months; pivoted to "agent enablement / skills registry"; spec→code is non-deterministic (Fowler). Vision >> shipped. |
| **graft** | runtime **lock-manager** (file-resource leases, in-memory bus) — prevents concurrent-edit conflicts; complementary, different layer. |
| **Temporal** | durable-execution engine agents build *on*; $300M Series D ~$5B; gives a substrate, none of the repo/spec/role domain model. |
| **Microsoft Agent 365** | "control plane for agents," GA May 2026; cross-vendor registry sync (AWS/Google preview) — governance, not coordination state. |
| **Authenticated Delegation** (South et al., arXiv:2501.09674, 2025, MIT/Pentland) | identity/authorization trust layer (agent-ID + delegation tokens) — a substrate a coordination layer sits *atop*. A real academic citation. |
| **Portkey / Orq.ai / Galileo / Vellum** | AI gateways / control planes; state in their service, not portable. (Portkey → Palo Alto Networks acquisition Apr 2026.) |

---

## 13. Reversals to know cold (last ~6 weeks)
- **Windsurf → Devin Desktop** (Cognition rebrand, June 2 2026; OpenAI's ~$3B deal collapsed mid-2025, founders went to Google).
- **Roo Code → ZooCode** (repo archived May 15 2026).
- **Continue.dev** OSS repo archived → commercial pivot.
- **vibe-kanban shut down** (Apr 2026) — thin launcher couldn't monetize.
- **ACP merged into A2A**; **Amp spun out of Sourcegraph** (Dec 2025); **Graphite acquired by Cursor** (Dec 2025).

## 14. What this means for AIDA's positioning (honest wedges)
- **Clean wedge — code-to-spec inline traces:** *none* of the 12+ spec/record tools surveyed has machine-checkable `trace:ID` links in source. The most defensible single differentiator.
- **NOT a wedge — "we have a typed graph, they don't":** Linear, Jira/Rovo, and Beads all ship mature typed-relationship graphs (Jira's predates Rovo ~15 years). Claim only the *narrower* truth: a **git-portable** typed graph with **code traceback** on top.
- **Wedge vs SaaS incumbents — git-canonical portable store:** Linear/Jira/Graphite are pure SaaS; but the files-in-git OSS crowd (spec-kit, OpenSpec, Backlog.md, ergo, GNAP) also has portability, so this differentiates only against the *funded incumbents*.
- **Strong wedge — fleet coordination (queue/lease/drain/escalation/inter-agent mailbox):** vs nearly everyone (trackers delegate one-issue-to-one-agent). The only real fleet-coordination neighbors are **Beads** (Dolt-backed) and **farol-team/GNAP** (file-based, stalled).
- **The white space (well-evidenced):** *cross-vendor + free + self-hosted + durable coordination model* is unoccupied — Beads is SQL-DB-backed, Gas Town's cross-vendor is paid cloud, Claude Flow is Claude-only. vibe-kanban's shutdown and Cognition's "shared state unsolved" admission both point here.

---

## Confidence / uncertainty log
- Beads' *current* daemon/git-hook behavior — much-cited criticism is **SQLite-era (pre-v0.58) and likely OBE**; re-verify before repeating as current.
- The Brandon Harvey "era of vibe" Beads-criticism **interview could not be independently sourced** (the critique is real as ergo's README positioning; Harvey also *presented* Beads at Latent Space). Treat the specific quote as user-reported.
- Cross-vendor depth of OSS Gas Town (functional vs aspirational), Sculptor, Code Conductor maintenance — flagged 🟡.
- Self-reported benchmarks (Claude Flow, Devin, Qodo) — never quote as pure agent capability; SWE-bench reflects model+scaffold and is contamination-prone.
- Funding/valuation figures and star counts are point-in-time; financial figures intentionally minimized (kept academic).

*Living document — promote the durable entries into `docs/competitive-analysis/marketplace-roster.md` on a cadence; re-verify the 🟡/⚠️ items before any external citation.*
