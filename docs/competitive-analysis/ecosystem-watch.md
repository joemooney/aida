# AIDA Ecosystem Watch Log

**Last updated**: 2026-05-18  
**Ecosystem Cadence**: Scan triggered by critical events or quarterly reviews

This document serves as our chronological ledger of ecosystem capability updates from platform providers (Anthropic, OpenAI) and neighbor tools (Cursor, Windsurf, Aider, Cline). Each entry evaluates a specific feature through AIDA's strategic lens, maps its classification, and documents the resulting feedback loop to our product backlog.

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
