# SPIKE-29: Claude Code Agent Teams — Multi-Agent Substrate Comparison and Design Sketch

**Date**: 2026-06-05
**Status**: DRAFT SKETCH (Pending Master Review)
**Target Spec**: [SPIKE-29](file:///.aida-store/objects/SPIKE/000/SPIKE-29.yaml)

---

## 1. Substrate Comparison: AIDA vs. Claude Code Agent Teams

Claude Code's experimental `agent-teams` feature (enabled via `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`) introduces a multi-process coordination layer that maps closely to AIDA's core primitives:

| Substrate Primitive | AIDA Implementation | Claude Agent Teams Equivalent | Comparison / Gaps |
|---|---|---|---|
| **Task Queue** | Git-canonical YAML store (`.aida-store/` branch) + SQLite cache | Local folder file-list under `~/.claude/tasks/{team-name}/` | AIDA is globally synchronized across machines and branches. Claude is strictly local-machine. |
| **Work Claiming** | Atomic leases (`.aida/sessions/*.toml`) | File-based lock files in the local task directory | Both prevent duplicate work. AIDA leases are Git-trackable; Claude locks are ephemeral. |
| **Inter-Agent Sync** | Mailbox database (`send_message` / `read_inbox`) | Local IPC / teammate-to-teammate socket messaging | AIDA mailbox is tool-agnostic; Claude messaging is single-session/single-runtime only. |
| **Teammate Spawn** | Supervised launcher (`aida agent new <type>`) | Team lead spawning sub-processes in `tmux` panes | AIDA supports heterogeneous agents (Claude + Codex + Antigravity); Claude only spawns Claude Code. |
| **Session Resumption** | State-json recovery + `aida session start` | Native `/resume` (which does NOT restore background teammates) | AIDA handles cross-session state recovery cleanly; Claude loses teammate progress. |

---

## 2. Strategic Verdict: COMPOSE (AIDA as the Core Substrate)

### Rationale
AIDA should **not compete** on in-session teammate spawning or UI layout (e.g. `tmux` pane splitting), as Anthropic's platform handles this natively and efficiently.
However, AIDA should **not divest** its coordination layer because Claude Code's agent teams suffer from critical architectural limitations:
*   No support for non-Claude agents (e.g. Codex, Antigravity).
*   No cross-machine or cross-session persistence (progress is lost on terminal exit).
*   Token and cost overhead is extremely high due to overlapping context windows.

### Integration Proposal
AIDA will act as the **source-of-truth compiler** that generates Claude Code `agent-teams` configuration files dynamically.

```
┌────────────────────────────────────────────────────────────┐
│                  AIDA Spec Graph Substrate                 │
│         Stores specs, dependencies, and agent roles.       │
└─────────────────────────────┬──────────────────────────────┘
                              │ ① compile specs + roles
                              ▼
┌────────────────────────────────────────────────────────────┐
│             AIDA Team Compiler (`aida team compile`)       │
│  Generates `config.json` and teammate role markdown files  │
│  under `.claude/teams/` and `.claude/agents/`.             │
└─────────────────────────────┬──────────────────────────────┘
                              │ ② read compiled config
                              ▼
┌────────────────────────────────────────────────────────────┐
│                    Claude Code Agent Teams                 │
│  Executes teammate loops, claims tasks, splits tmux panes.  │
└────────────────────────────────────────────────────────────┘
```

---

## 3. Gaps and Proposed Shape

### AIDA Gaps to Address
*   **Role template syncing**: AIDA needs a command to write its internal role definitions (implementer, reviewer, advisor, etc.) into Claude's teammate definition directories (`.claude/agents/*.md`).
*   **Shared task compilation**: AIDA needs to export its active spec backlog into Claude's task folder (`~/.claude/tasks/{team-name}/`) so teammates can discover and claim them using native `agent-teams` commands.

### Proposed Workflow
1.  Operator starts the team: `aida team start --owns EPIC-100 --name feature-auth`
2.  AIDA compiles requirements and roles:
    *   Generates `.claude/teams/feature-auth/config.json`.
    *   Scaffolds teammate template descriptions under `.claude/agents/implementer.md` and `.claude/agents/reviewer.md` containing specific tool allowlists.
    *   Writes pending spec tasks directly to `~/.claude/tasks/feature-auth/`.
3.  Operator runs Claude with teams enabled: `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1 claude`

---

## 4. Trade-Offs

*   **Pros**:
    *   Leverages Claude Code's native `tmux` split-pane visualization.
    *   Ensures role definitions and allowed tools are strictly enforced at the runner layer.
*   **Cons**:
    *   Relies on undocumented, experimental Claude Code APIs that could shift.
    *   Writing tasks directly to `~/.claude/tasks/` requires maintaining write-back compatibility with Claude's private task schema.

---

## 5. Open Decisions for Master

1.  **Do we format AIDA specs to match Claude's task JSON schema?**
    *   *Path A*: Yes. Parse AIDA spec metadata and write it as Claude-native task JSON files so teammates read them without translation.
    *   *Path B*: No. Expose a custom MCP tool for task discovery and let Claude's lead session query AIDA's database to create tasks. (Path B is safer against Claude schema updates).
2.  **How do we handle non-Claude teammates (Codex, Antigravity)?**
    *   Should we represent them as "external teammates" that are blocked/awaited using standard task dependencies, or exclude them from the team configuration entirely?
