# Signals to Watch: Future Landscape Refresh Triggers

**Last updated**: 2026-05-22  
**Ecosystem Cadence**: Scan triggered by critical events defined below

Because the AI developer tooling landscape moves with extreme velocity, a static landscape scan quickly becomes outdated. To maintain AIDA's strategic positioning and prevent context rot, this document outlines the **critical signals** that will trigger an immediate re-evaluation and refresh of AIDA's competitive analysis.

---

## 1. Native Provider Coordination Primitives

We monitor platform providers (Anthropic, OpenAI) for native agent coordination features that could absorb horizontal tasks.

| Signal Trigger | What We Watch | AIDA Strategic Pivot |
|---|---|---|
| **Anthropic Agent Teams Release** | Native multi-agent collaboration, workspace sharing, or collaborative session PTYs built directly into Claude Code. | Assess if AIDA should delegate PTY session hosting to Anthropic's native Teams and focus exclusively on requirements graph compilation and trace-comment verification. |
| **OpenAI Codex / Goose Upgrades** | Major updates to the Codex unified `AGENTS.md` and Goose skill registries (`.goose/skills/`). | Expand AIDA's bilingual scaffolding pack to compile requirements-driven roles natively to Codex and Goose formats. |
| **OpenAI agent surface updates** | Releases of native terminal-agent loops or plan persistence from OpenAI. | Compare the persistence and graph capabilities of OpenAI's plans against AIDA's `docs/plans/` model. |

---

## 2. IDE-Integrated Agent Primitives

We monitor editor-integrated agents (Cursor, Windsurf) for changes in rules enforcement and structural context.

> [!NOTE]
> **Cursor Rules and MDCs:**  
> Cursor’s rules system (`.cursor/rules/*.mdc`) is rapidly transitioning from static developer instructions to dynamic, machine-executable context files. We monitor whether Cursor introduces program-addressable dependency graph tracking between rules files, which could compete with AIDA's local specification graph.

### Signals to Monitor:
*   **Cursor Agent Multi-Workspace Support**: When Cursor introduces native git worktree isolation or parallel agent sessions for background execution.
*   **Windsurf Rules Evolution**: Updates to `.windsurf/rules/` that allow rules to bind dynamically to local requirements files or external ticketing systems.

---

## 3. High-Momentum Open-Source Agent Frameworks

We monitor independent, open-source agent platforms that reach high developer adoption milestones.

> [!IMPORTANT]
> **The 10k-Star Threshold:**  
> Any open-source terminal-agent runner or PTY-multiplexer reaching **10,000 GitHub stars** (e.g., Cline, OpenHands, Aider) triggers an immediate deep-dive prior-art study. We analyze their workspace hosting models, terminal overlay systems, and state serialization paradigms.

---

## 4. Coordination Protocol Standardization

We monitor compilation languages and context protocols for consolidation events.

### Skillfold Adoption Trajectory:
We track the growth of `byronxlg/skillfold` across two key metrics:
1.  **Backend Expansion**: When skillfold adds native compilation backends beyond the existing 12 (specifically targeting IDEs like JetBrains or Xcode).
2.  **MCP Integration**: If skillfold introduces an MCP registry plugin, allowing agents to dynamically query the `skillfold.yaml` team execution flow at runtime.

### MCP Marketplace Consolidation:
Model Context Protocol (MCP) is currently a highly fragmented ecosystem of custom local servers. We monitor for:
*   **Centralized MCP Registries**: Standardized registries (e.g., Anthropic or community-driven hubs) that consolidate MCP server discovery.
*   **Unified Authentication Specs**: Standardized protocols for credentials sharing and secure local filesystem access across multiple MCP-connected applications.
