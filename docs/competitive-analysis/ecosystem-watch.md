# AIDA Ecosystem Watch Log

**Last updated**: 2026-09-23
**Ecosystem Cadence**: Scan triggered by critical events or quarterly reviews

This document serves as our chronological ledger of ecosystem capability updates from platform providers (Anthropic, OpenAI) and neighbor tools (Cursor, Windsurf, Aider, Cline). Each entry evaluates a specific feature through AIDA's strategic lens, maps its classification, and documents the resulting feedback loop to our product backlog.

---

## 2026-09-23: CodeGraph re-classification — Compete → Complement (SPIKE-84)

**Trigger**: the operator, running CodeGraph locally, reported first-hand that "it works well for me and plays nicely with aida" — direct coexistence evidence from someone running both, which does not match how this repo currently classifies the tool.

**The frozen prior read, left untouched**: `2026-06-09-weekly-scan.md` (Lane D, "Kill-shot 3") classified CodeGraph/Augment/CodeCompass-class auto-derived code graphs as a **Compete**-shaped threat to the "your project's missing index" tagline — a *positioning* collision (who owns the word "index"), not a claim that the tools conflict functionally in a working repo. That entry is frozen per the dated-artifact convention; this is a new, separate read, not an edit to it.

### Classification

**Complement** (revised from the implied Compete read of the 2026-06-09 tagline analysis). The two tools operate on different graphs by construction:

- CodeGraph: an **auto-derived** AST/dependency graph — self-freshening, answers "what calls this / what depends on this," zero discipline cost.
- AIDA: a **maintained** requirement↔code intent graph — answers "why does this exist / what decided it," costs discipline, cannot be derived from the code itself.

This distinction was already present in this repo before the trigger — `docs/plans/2026-09-12-entry-memory-lane.md` names it under Risks: *"Authored-memory rot — unlike codegraph (derived, self-fresh), this needs capture + staleness or it misleads."* That plan used CodeGraph as the reference case for "derived and self-fresh" specifically to contrast with AIDA's own authored layer — i.e., the repo's own architecture reasoning already treated the two as different *kinds* of graph, independent of the operator's report. `aida why <file:line>` (STORY-754) is the concrete seam where this composes rather than competes: a derived code graph can find the call site; only AIDA's traces/comments can say why it was written.

### Question 1 — Compete, Complement, Integrate, or Ignore?

**Complement.** The 2026-06-09 tripwire was explicitly scoped to a *wording* risk ("the 'index' framing being won by zero-discipline tools in benchmarks or mindshare"), not a functional-overlap claim. Nothing in that scan asserted the two tools conflict when run together, and the operator's report plus the plan's own pre-existing "derived vs. authored" framing both point the same direction. **[I]** — this is architecture/documentation analysis, not a benchmarked or logged usage study (see Question 2).

### Question 2 — What "plays nicely" means mechanically

**Not fully answerable offline**, and this entry does not claim otherwise. What can be established from this repo alone: AIDA's own dogfood `.mcp.json` registers exactly one MCP server (`aida`); it does not bundle or reference CodeGraph, so there is no shared tool-name surface to collide on *in this repo's own config*. Beyond that — whether an agent reaches for one server over the other, whether CodeGraph's index reduces the number of AIDA queries issued in a session, or any context-budget contention — depends on the operator's actual paired session logs, which are outside this repo's tracked docs and were not available to this offline pass (no web research performed; not required to answer Questions 1/3/4). **Could not verify**: session-level interleaving behavior, query-count reduction, any tool-name or vocabulary collision in a live dual-MCP setup.

### Question 3 — Does the CR-6 / STORY-551 positioning conclusion survive?

**Yes, and it sharpens.** CR-6/STORY-551 dropped the "index" headline in favor of intent traceability + lifecycle truth — that stays correct either way, since it was a wording fix for a comprehension tax, not a technical retraction. If the tools are complementary in practice (per Question 1), the honest claim is *stronger* than the defensive one already shipped: **"AIDA is the intent layer on top of whatever code graph you already run"** is a positive claim rather than an avoidance, and it is consistent with — not a reversal of — the 2026-06-09 recommendation. Worth carrying into `docs/positioning/` as a considered addition rather than a rewrite of an existing page.

### Question 4 — Is the auto-population followup still unfiled?

**No — it is filed.** `docs/plans/2026-09-12-entry-memory-lane.md`'s Followups bullet ("Codegraph → requirements-graph auto-population as an interactive hygiene session") now has a spec: **STORY-1359** (`status: draft`, `related: SPIKE-84`, priority low). The filing gap this spike's title names has already been closed; what remains open is the advisor's approve/reject/park disposition on STORY-1359 itself, which is a separate decision from this classification.

### Action and Backlog Loop

- Classification revised here; the frozen 2026-06-09 scan is left as-is (immutability convention).
- STORY-1359 already exists and awaits advisor disposition (approve / reject / park) — not actioned by this entry.
- A `docs/positioning/` addition for the "intent layer on top of your code graph" framing (Question 3) is a candidate follow-up, not filed by this entry — the spike's non-goal excludes building any integration, and a positioning-doc change is a product-facing decision the advisor should make deliberately rather than as a side effect of a re-classification note.
- Question 2's mechanical specifics remain an open observation gap — worth a lightweight capture (e.g. `aida doc add`) the next time the operator notices something concrete about the coexistence, rather than a constructed benchmark (per this spike's own METHOD constraint).

**Confidence**: Question 1 and Question 3 medium-high (architecture/positioning analysis grounded in this repo's own prior docs). Question 4 high (verified directly via `aida show STORY-1359`). Question 2 low/open (no live-session evidence available offline).

---

## 2026-05-26: Marketplace and MCP Distribution Scan

See [2026-05-26-marketplace-research.md](2026-05-26-marketplace-research.md) for the full memo.

### Classification

**Integrate / Compete-on-substrate**. Agent extension marketplaces and MCP registries are now meaningful distribution surfaces, but they mostly distribute horizontal capabilities. AIDA should integrate with those channels while competing on the repo-local intent/control-plane layer: spec graph, lifecycle state, leases, briefs, punts, findings, traceability, status, and doctor.

### Key Signals

- Claude Code plugin marketplaces package skills, agents, hooks, MCP servers, and commands into installable bundles.
- Linear, Sourcegraph, GitHub Copilot cloud agent, Windsurf, Continue, and Cline all treat MCP as an agent integration seam.
- Enterprise-facing tools emphasize OAuth, tool whitelists, scoped access, audit logs, and consumption controls.
- Sourcegraph and Windsurf both show that MCP tool-surface size is an operational constraint, not just an implementation detail.

### Action and Backlog Loop

Filed and linked under TASK-565:

- STORY-473: publish AIDA as a Claude Code plugin/marketplace package.
- STORY-474: add MCP tool profiles and a safe default surface.
- STORY-475: add remote/auth-capable AIDA MCP transport.
- TASK-566: document AIDA MCP install matrix for major agent clients.
- TASK-567: create marketplace publication security checklist.
- STORY-476: add external issue refs for Linear/Jira/GitHub composition.
- STORY-477: add agent-lift metrics report for dogfood proof.

---

## 2026-05-18: Claude Code Platform Scan

Following the latest Anthropic platform updates, we conducted a targeted evaluation of new native primitives. AIDA's vertical architecture is highly complementary to these horizontal CLI enhancements.

### 1. `/goal` — Persistent Task Execution
- **Competitor/Source**: Anthropic Claude Code (v0.5.0)
- **AIDA Classification**: **Complement**
- **Technical Analysis**:
  Claude Code's `/goal` command introduces a native long-running, multi-turn task loop designed to run until a specified condition is achieved. This is a horizontal utility for linear, single-agent task completion. In contrast, AIDA focuses on requirements-driven, multi-node graph coordination with strict lease boundaries and trace-comment advisory verification.
- **Action & Backlog Loop**:
  **Monitor**. No direct architectural changes are required. Filed `TASK-319` in AIDA's backlog to monitor UX patterns of long-running tasks, ensuring our own autonomous queue execution (`/aida-drain-queue`) remains more transparent and manageable.

### 2. `/agents` — Subagent Spawning
- **Competitor/Source**: Anthropic Claude Code (v0.5.0)
- **AIDA Classification**: **Complement**
- **Technical Analysis**:
  Claude Code now supports spawning isolated subagent loops to run background research or targeted editing tasks. This validates our own multi-agent design patterns (e.g. `research` and `self` subagents). AIDA's unique advantage lies in our unified runtime, which coordinate these subagents through a shared git-native state instead of black-box chat contexts.
- **Action & Backlog Loop**:
  **In Progress**. We are finalizing `TASK-337` to detail this exact positioning in `docs/positioning/vs-claude-code-subagents.md`, defining how AIDA orchestrators split requirements and enforce advisory lease contracts across spawned subagents.

### 3. `/remote-control` — Remote Terminal Interaction
- **Competitor/Source**: Anthropic Claude Code (v0.5.0)
- **AIDA Classification**: **Complement / Integrate**
- **Technical Analysis**:
  `/remote-control` enables users to connect and interact with running agent sessions remotely (e.g., from mobile devices or a standby terminal). This matches our own `--zen` mobile standby concept. However, there is a minor integration gap between Claude's remote session attachment and AIDA's local workspace locking model.
- **Action & Backlog Loop**:
  **Action Required**. We must check and close this integration gap. The architecture for AIDA's remote session handling is documented in `docs/autonomous-drain.md` and aligns with the taxonomy established in the `STORY-287` design. Filed `TASK-321` to verify that AIDA's local sqlite session and lease locks handle remote PTY attachment safely.

### 4. Agent Teams — Collaborative Multi-Agent Pools
- **Competitor/Source**: Anthropic Platform Announcement
- **AIDA Classification**: **Complement / Validation**
- **Technical Analysis**:
  Anthropic's previews of Agent Teams validate AIDA's foundational thesis: complex, professional software engineering requires multiple specialized roles (advisors, implementers, reviewers) rather than a single monolithic chat agent. It also underscores the importance of low-latency communication channels between these roles.
- **Action & Backlog Loop**:
  **Action Required**. To capitalize on this, we filed `SPIKE-9` to prototype using the Model Context Protocol (MCP) as a lightweight, local message bus. This will allow AIDA agents in a workspace to communicate instantly using structured MCP notifications instead of costly disk-bound polling or file-watching.
