# Weekly competitive scan — Lane B: Agent orchestration & swarm frontier (2026-06-09)

**Lane:** B (agent orchestration & swarm frontier) · **Status:** lane fragment, pending A/C/D + synthesis · **Run by:** claude (implementer seat, dispatched)
**Provenance key:** ✅ *verified* = I read the primary source (linked) · 🟡 *secondary* = read a secondary writeup, not the primary · 🔸 *inferred* = my reasoning, not a sourced claim
**Coverage caveat (no silent truncation):** I sampled the named seeds (Claude Code Agent Teams/Workflows, Codex, Ruflo/Claude-Flow, Gas Town/Beads, Vibe Kanban, Claude Squad, Wit) plus the two provider changelogs, and **Devin/Cognition (added 2026-06-09 after an initial omission — operator-caught; see B5)**. I did **not** survey the long tail of the `awesome-agent-orchestrators` list, nor verify star counts beyond the two I fetched live. Star figures from secondary sources are flagged.

## Pre-scan expectation vs. reality (the surprises are the value)

| I expected | What I found |
|---|---|
| Workflows mature but stay *within-task* → layer story holds | ✅ Confirmed — dynamic Workflows are research-preview, within-task, end in an answer |
| OSS swarm orchestrators grow, no typed substrate | **Partly wrong** — Ruflo fits, but **Beads/Gas Town ships a typed, git-backed, vendor-neutral dependency graph an orchestrator drains** — a *substrate-axis* competitor, not just mechanical |
| A provider primitive is the top commoditization risk | ✅ Confirmed and **broader than expected** — Anthropic **Agent Teams** shipped a full coordination layer (mailbox + shared task-list + dependencies + plan-gates + quality hooks); Codex shipped parallel-worker + worktree thread coordination. **Both vendors, both single-vendor.** |

The two genuine surprises — **Agent Teams' depth** and **Beads as a substrate competitor** — are the load-bearing findings below.

---

## Finding B1 — Anthropic Agent Teams: the signals-to-watch §1 tripwire **FIRED** ✅

`CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS` shipped (Claude Code ≥ v2.1.32, experimental, default-off). It is **far more than parallel subagents** — per the official docs it provides, natively, most of AIDA's *mechanical* coordination layer:

- **Mailbox** — direct inter-agent messaging, auto-delivered (no polling). → overlaps AIDA **P3** (MCP `send_message`/`read_inbox`).
- **Shared task list** — pending/in-progress/completed, **self-claim or lead-assign**, **file-locking to prevent claim races**, **task dependencies that auto-unblock** when a blocker completes. → overlaps AIDA's **queue + lease race-prevention + blocked-by graph** at the within-team level.
- **Plan-approval gate** — a teammate works in read-only plan mode until the lead approves; reject-with-feedback loops. → rhymes with AIDA's **implementer→advisor escalation / reviewer verdict gating**.
- **Quality-gate hooks** — `TaskCreated` / `TaskCompleted` / `TeammateIdle`, exit-code-2 to block + feedback. → this *is* AIDA's **substrate-as-bouncer** principle, now a native Claude Code primitive.
- **Subagent-definition reuse** — a `.claude/agents/` role can be spawned as a teammate.

**What it is NOT** (the precise open slice — no overclaim): state is local ephemeral JSON (`~/.claude/teams/…`, `~/.claude/tasks/…`), explicitly *"don't edit by hand, overwritten on next state update."* **No session resumption**, **one team at a time, no nested teams, fixed lead, Claude-only.** Tasks are within-team transient TODOs — **not** stable-ID specs with typed relationships, history, or code↔spec traces. Nothing survives the session or crosses a vendor.

**Incentive anchor (ages well):** Agent Teams is Claude-only and ephemeral **because Anthropic's business is selling Claude** — they have no incentive to persist a *vendor-neutral* graph or to make coordination outlive a session. That is the durable wedge, not a capability gap they'll "fix next release."

- ✅ Source (primary): <https://code.claude.com/docs/en/agent-teams>
- 🟡 launch context: [InfoQ — Code with Claude: Managed Agents, Proactive Workflows](https://www.infoq.com/news/2026/05/code-with-claude/) · [MindStudio — What is Claude Code Agent Teams](https://www.mindstudio.ai/blog/what-is-claude-code-agent-teams)

## Finding B2 — Beads / Gas Town: a **substrate-axis** competitor (dents the keystone moat claim)

[`gastownhall/beads`](https://github.com/gastownhall/beads) (✅ live repo: **24.4k stars** — up from ~18.7k in older secondary writeups; momentum is the signal) by **Steve Yegge** is *"a distributed, git-backed issue tracker designed specifically for AI coding agents."* [Gas Town](https://github.com/gastownhall/gastown) is the orchestration layer over it; Beads is *"the central agent coordination persistence."* Verified against the keystone's "DIFFERENTIATED" list:

| Keystone (2026-05-31) called this AIDA-differentiated | Beads, verified 2026-06-09 |
|---|---|
| **Typed inter-spec relationship graph** ("absent from agents") | **PRESENT** — `relates_to`, `duplicates`, `supersedes`, `replies_to`, `blocks`, `parent-child`, hierarchical dot-IDs (`bd-a3f8.1.1`). The "absent from agents" line is **now stale.** |
| **Multi-vendor substrate** ("the only one") | **CONTESTED** — `bd setup codex / claude / cursor / droid / mux`; vendor-neutral (Claude-spotlighted). |
| **Stable IDs as identity** | **PRESENT** — hash-based (`bd-a1b2`), collision-safe across multi-agent branches. |
| Durable graph an orchestrator drains | **PRESENT** — persists across sessions; Gas Town drains it. |

**The precise surviving differentiation (what Beads does NOT have — ✅ verified from its README):**
1. **Code↔spec trace enforcement** — *not addressed* in Beads. AIDA's commit-gated code↔spec loop remains distinctive.
2. **Rich lifecycle state machine + auto-bump-on-merge + history audit** — Beads is task-status-minimal (`claim`/`close`).
3. **Plain-git, zero-extra-infra portability** — Beads **requires Dolt** (versioned-SQL engine; git optional). AIDA needs only git + YAML. *Cuts both ways:* Dolt gives Beads cell-level merge (better data-conflict handling) at the cost of a new dependency — a genuine trade-off, not a pure AIDA win.
4. **Requirements modeling vs. issue tracking** — Beads is fundamentally an *issue tracker / agent memory*; AIDA models a *requirement graph* (functional/non-functional/epic/story + typed relations + traces + lifecycle).

**Honest read for the synthesizer (disagreement = signal):** the keystone's headline — *"the only durable, typed, multi-vendor spec graph an orchestrator drains"* — is **dented**. Beads is a durable, typed, multi-vendor dependency graph that an orchestrator drains. The defensible moat narrows to **trace-enforcement + lifecycle/auto-bump/history + plain-git-zero-infra + the requirements (not issues) framing**. **Incentive anchor:** Beads' center of gravity is *agent task-memory*, not *enforced requirement traceability* — it *could* add traces (weaker incentive moat than the provider case, since Beads is already cross-vendor and already typed). This is the most important delta of the scan and Lane D should hammer it.

- ✅ Sources (primary): [Beads repo/README](https://github.com/gastownhall/beads) · [Gas Town repo](https://github.com/gastownhall/gastown)
- 🟡secondary: [Starlog — Beads: a version-controlled task graph](https://starlog.is/articles/ai-dev-tools/gastownhall-beads/) · [DoltHub — A Day in Gas Town](https://www.dolthub.com/blog/2026-01-15-a-day-in-gas-town/)

## Finding B3 — Codex (OpenAI): native parallel + worktree coordination, also single-vendor

Codex changelog (✅ fetched): **June 2026 (26.527)** — *"Added thread coordination for local projects and worktrees, including separate background threads when explicitly requested."* Plus **Multi-agent v2** ("runtime choice per thread, cleaner follow-up/metadata for spawned agents"), Codex CLI usable as an **MCP server** inside OpenAI Agents-SDK multi-agent workflows. **Codex/OpenAI-specific** — no cross-vendor coordination protocol.

**Implication:** worktree-per-agent isolation is now *doubly* table-stakes (Anthropic Agent Teams + Codex). Both vendors built coordination *inside their own runtime*; **cross-vendor coordination remains unclaimed** — the keystone's portability wedge holds, and is the one axis neither provider is incentivized to build.

- ✅ Source (primary): <https://developers.openai.com/codex/changelog>

## Finding B4 — OSS swarm leaders: mechanical axis still commoditizing (no surprise)

- **Ruflo** (formerly Claude Flow, [`ruvnet/ruflo`](https://github.com/ruvnet/ruflo)) — 🟡 secondary star counts vary widely (**~40k–56k**, sources disagree; high momentum). Rust/WASM rewrite, 100+ agents, hierarchical/mesh/adaptive topologies, adaptive memory + RAG, native Claude **and** Codex. Mechanical orchestration + vector memory; **no typed spec graph, no trace enforcement.** Confirms "mechanical axis commoditized, substrate axis is the bet."
- **Vibe Kanban** (Kanban UI for parallel agents; Bloop shut down early 2026, project continues OSS), **Claude Squad** (zero-setup terminal parallelism), **Wit** (🟡 locks *functions* not files via Tree-sitter AST — finer-grained than AIDA's spec/worktree lease; interesting prior art for sub-file coordination). All mechanical; none carry a persistent typed requirement graph.
- 🟡 Source: [Nimbalyst — best multi-agent coding tools 2026](https://nimbalyst.com/blog/best-multi-agent-coding-tools-2026/) · [awesome-agent-orchestrators](https://github.com/andyrewlee/awesome-agent-orchestrators)

## Finding B5 — Devin (Cognition): the managed autonomous **implementer** — a Lane B executor competitor, not a substrate one

*(Added 2026-06-09 after initial omission — flagged honestly; Devin was missing from the first sweep.)*

Devin is *"an autonomous AI software engineer that can write, run and test code"* — consumes Linear/Jira tickets + prompts with explicit completion criteria, implements features, fixes bugs, **self-reviews PRs**, works via Slack/Teams/web/CLI. **Devin 2.0 (Feb 2026) added parallel sessions** — *"10 tickets → 10 Devins in parallel"* — plus planning tools and faster startup; price dropped to ~$20 entry. Strong Lane B credentials: autonomous implementation, parallel backlog execution, PR workflow, team handoff.

**But it is NOT a substrate competitor (✅ verified from Devin's intro docs):** no persistent requirements graph, no stable IDs, no typed relationships, no code↔spec trace enforcement. It *consumes* tickets and *produces* work; "Knowledge"/"Playbooks" are guidance/automation, not a spec graph. It rides existing trackers (Jira/Linear) rather than maintaining its own typed requirement layer.

**Where it sits:** Devin commoditizes the **autonomous-implementer + parallel-backlog-execution** layer — the managed-cloud, polished analog of AIDA's *drain*. **AIDA's response is not to out-implement Devin** (a cloud labor product billed per-ACU, single-vendor Cognition) but to be the **substrate the implementer drains**: the same "graph feeds the executor tickets, harvests its PRs back with trace enforcement + lifecycle bump" pattern as the Agent-Teams bridge (SPIKE-52) generalizes to Devin/Codex as executors. **Incentive anchor:** Devin monetizes *execution* (ACU billing), so it integrates with trackers rather than replacing them with a durable requirements graph — it *won't* build the substrate because its product is the engineer-as-a-service, not the spec layer. (Ages well.)

**Cross-lane:** also a **Lane A** adjacency (buyer alternative, *not* a spec-driven-dev artifact system) and a **Lane C** distribution/integration data point (repo indexing, Knowledge, API, MCP docs, enterprise Jira/Linear/GitHub/GitLab/Bitbucket reach).

- ✅ Source (primary): [Devin intro docs](https://docs.devin.ai/get-started/devin-intro) · [Devin doc index](https://docs.devin.ai/llms.txt)
- 🟡 2026 deltas: [Devin 2.0 review (price drop, parallel)](https://weavai.app/blog/en/2026/05/13/devin-2-0-review-2026-ai-engineer-price-drops-to-20/) · [Kiro vs Devin — spec-IDE vs autonomous engineer](https://www.augmentcode.com/tools/kiro-vs-devin)

---

## Commoditized vs differentiated — Lane B deltas only (vs the 2026-05-31 keystone)

| Layer | Keystone status | **Lane B delta (2026-06-09)** |
|---|---|---|
| Inter-agent mailbox | DIFFERENTIATED gap-to-build (P3) | **COMMODITIZING within-session** — Agent Teams ships a native Mailbox. Re-scope P3 to *cross-session/cross-vendor/git-canonical* only. |
| Shared task-claiming + lease race-prevention | DIFFERENTIATED (mechanical moat) | **COMMODITIZING within-session** — Agent Teams shared task list + file-locking + auto-unblocking dependencies. AIDA's edge = the *durable, cross-session* version. |
| Substrate-as-bouncer (quality gates) | DIFFERENTIATED principle | **VALIDATED + COMMODITIZING** — Agent Teams `TaskCompleted`/`TaskCreated`/`TeammateIdle` hooks are the same idea, native. Ride them. |
| Worktree-per-agent isolation | TABLE STAKES | **Doubly table-stakes** — Anthropic + Codex both native now. |
| **Autonomous implementer + parallel-backlog execution** | (AIDA's drain) | **COMMODITIZING** — Devin 2.0 (parallel sessions, self-reviewing PRs), Codex app, Agent Teams. AIDA's edge ≠ being a better implementer; it's the **substrate the implementer drains**. Don't out-implement Devin — feed it. |
| **Typed relationship graph** | DIFFERENTIATED ("absent from agents") | **CONTESTED** — Beads has one. Stale line. |
| **Multi-vendor durable graph** | DIFFERENTIATED ("the only one") | **CONTESTED** — Beads is vendor-neutral + git-backed. |
| **Code↔spec trace enforcement** | DIFFERENTIATED | **STILL DIFFERENTIATED** — absent from Beads, Agent Teams, Codex, Ruflo. The sharpest surviving edge. **Defend loudly.** |
| **Lifecycle state machine + auto-bump + history** | DIFFERENTIATED | **STILL DIFFERENTIATED** — Beads is status-minimal; providers are ephemeral. |
| **Plain-git, zero-extra-infra** | (implementation) | **NEWLY SALIENT** — vs Beads' Dolt dependency. A real portability/adoption edge, honestly trade-off-laden. |

## Adopt / adapt / avoid (Lane B — each "adopt" filed as a draft spec)

| Capability | Source | Why | How it lands on AIDA's substrate | Effort | Draft spec |
|---|---|---|---|---|---|
| **vs-agent-teams positioning doc** | Agent Teams docs | Newest, most-overlapping provider primitive; the "am I reinventing the wheel?" worry is now sharpest here (closer than subagents). The existing `vs-claude-code-subagents.md` predates it. | Honest layer doc: ephemeral local Claude-only task-list vs durable git-canonical cross-vendor requirement graph; *they compose* (AIDA above). | S (doc) | **filed** |
| **Ride Agent Teams as the within-session executor; bridge it to the spec graph** | Agent Teams docs | Stop building within-session mailbox/task-claiming from scratch (P3); Anthropic now owns it. AIDA's job is to *feed* it from the graph and *harvest* outcomes back. | SPIKE: seed the Agent-Teams shared task list from `aida queue`; map teammates→AIDA implementers; harvest mailbox/outcomes into spec history; register AIDA reviewer/precommit gates as `TaskCompleted`/`TeammateIdle` hooks. | M (spike) | **filed** |
| **Beads/Gas Town substrate-competitor deep-dive + moat re-scope** | Beads/Gas Town repos | First substrate-axis competitor; dents the keystone's "typed/multi-vendor" claim. Need a verified head-to-head + a sharpened defend/differentiate plan before the next positioning paper ships a stale line. | SPIKE: feature-matrix AIDA vs Beads (trace enforcement, lifecycle, plain-git vs Dolt, requirements vs issues); decide which edges to headline; consider a Beads import/export bridge as a Trojan-horse. | M (spike) | **filed** |
| **Adopt Agent Teams quality-gate hook pattern** (RIDE) | Agent Teams hooks | Substrate-as-bouncer is now a Claude-native primitive; conform so AIDA gates fire inside native teams too. | Emit AIDA gate scripts as `TaskCompleted`/`TaskCreated` hook handlers (subset of the bridge spike above). | S | folded into bridge spike |
| **AVOID** building an AIDA-native within-session swarm/topology engine | Ruflo | Mechanical orchestration is commoditized (Ruflo 40k+★, providers native). Building our own swarm topology layer is yak-shaving below AIDA's line. | n/a — defer to providers + Ruflo; keep AIDA at the cross-session graph layer. | — | — |

## Positioning impact (does the keystone one-liner still survive?)

The keystone line — *"…the only one where an orchestrator drains that graph through a spec-grounded escalation cascade, and the only one portable across every vendor because it lives in git"* — **needs a precision edit.** "Only … portable across every vendor" is now contestable (Beads is vendor-neutral + git-backed). Proposed Lane-B revision for synthesis:

> *"Others now coordinate agents — Anthropic Agent Teams within a Claude session, Beads as a git-backed task graph. AIDA is the one that makes the graph a **requirement** graph: stable IDs + **typed relationships + enforced code↔spec traces + a lifecycle that auto-tracks merge state** — drained by an orchestrator with a spec-grounded escalation cascade, on **plain git with no extra engine**, across every vendor."*

The surviving, defensible core is **enforcement + lifecycle + requirements-framing + plain-git**, *not* "the only typed/portable graph."

## Tripwires (per §7)

**FIRED:**
- **signals-to-watch §1 "Anthropic Agent Teams Release"** — shipped. Recommended pivot per that doc ("delegate session hosting to native Teams; focus on the requirements-graph + trace verification") is now **actionable**, not hypothetical — captured in the bridge spike.

**NEW (recommend adding to `signals-to-watch.md` at synthesis — not mutated here to avoid clobbering parallel lanes):**
- **Beads/Gas Town adds code↔spec trace enforcement OR a lifecycle state machine** → would close the last substrate gap; highest-impact Lane-B tail risk. Watch `gastownhall/beads` releases + roadmap.
- **MCP standardizes an inter-agent communication protocol** (keystone already flagged the workstream) — Agent Teams' Mailbox is a de-facto implementation; if it generalizes cross-vendor, AIDA's mailbox content (spec-anchored) is the defense, not the transport.
- **Codex/Agent-Teams add persistence/resumption** (Agent Teams' "no session resumption" is an explicit current limitation) — if either makes coordination durable, the cross-session edge narrows toward the cross-*vendor* + requirements edge only.
- **Devin turns Jira/Linear/issues into a durable, queryable requirements graph with stable IDs + trace-to-code checks** — would move Devin from the executor lane *into AIDA's core substrate lane*. Today it rides trackers and consumes tickets; watch Cognition's "Knowledge"/spec features + doc index for any move toward an owned typed requirement graph. (Operator-surfaced 2026-06-09.)

## Honest meta-read

- **Confidence: high** on Agent Teams (primary docs), Codex coordination (primary changelog), and Beads' capability surface (primary README). **Medium** on star/momentum figures (secondary, conflicting — flagged) and on Gas Town's exact drain mechanics (secondary).
- **Could not verify:** Agent Teams' real-world reliability (it's experimental, with documented resumption/status-lag limits); Beads adoption *depth* vs star count; whether Ruflo's "adaptive memory" is graph-typed or vector-only (read as vector/RAG).
- **The single most important thing for synthesis:** the substrate moat is **narrower than the 2026-05-31 keystone asserts.** Beads closed the "typed + multi-vendor graph" gap; the honest, defensible differentiation is now **trace-enforcement + lifecycle + requirements-framing + plain-git**. Lane D should attack this; positioning papers should stop claiming "the only typed/portable graph."
- **Chase next run:** a verified AIDA-vs-Beads feature matrix (the deep-dive spike); whether Agent Teams gains persistence; whether any tool bridges code↔spec trace enforcement (the last open slice).
