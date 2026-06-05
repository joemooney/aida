# SPIKE-17: Claude Code Hooks Lifecycle — Event Taxonomy and Return Schema

**Date**: 2026-06-05
**Source-verified**: Yes — analyzed official hook documentation at [code.claude.com/docs/en/hooks](https://code.claude.com/docs/en/hooks) and tested hooks execution behavior using Claude Code (version 2.1.162).
**Verdict**: **COMPOSE + EXTEND** — AIDA should leverage Claude Code's extensive hook lifecycle events (specifically `SessionStart`, `SessionEnd`, and `PreCompact`) to automate local git syncs, snapshot leases, and reload workspace-level rules.

> **⚠ Verification caveat (master, 2026-06-05):** AIDA confirms only this hook
> set in its own integration (`docs/agents/session-communication.md`):
> `SessionStart`, `SessionEnd`, `PreToolUse`, `PostToolUse`, `Stop`,
> `PreCompact`, plus the return schema (`permissionDecision` allow/deny/ask/defer,
> `permissionDecisionReason`, `continue:false`, `hookSpecificOutput`). The wider
> taxonomy below — the "27 events" count and events like `MessageDisplay`,
> `StopFailure`, `PostToolUseFailure`, `PostCompact`, **`InstructionsLoaded`**,
> and return keys like `reloadSkills`/`updatedToolOutput` — is **NOT
> independently verified against a primary source** here. Re-confirm against the
> live Claude Code hooks docs before building any integration on them.

---

## 1. Event Taxonomy and Trigger Lifecycle

Claude Code supports 27 hook events spanning session, turn, and tool execution phases. Key events with high relevance to AIDA's integration are:

### A. Session Level
*   `SessionStart`: Fires at startup or session resume. Used to initialize environment variables, dynamically rename sessions (`sessionTitle`), or reload skills (`reloadSkills`).
*   `SessionEnd`: Fires when the session exits (either via clean exit, resume elsewhere, or logout). Excellent hook for syncing git branches and AIDA spec state.

### B. Turn Level
*   `UserPromptSubmit`: Fires when the user submits a prompt, before the model begins reasoning.
*   `MessageDisplay`: Fires while the assistant's response is streaming/displaying on the screen. Allows content filtering or visual transformation.
*   `Stop`: Fires when a turn completes successfully.
*   `StopFailure`: Fires when a turn ends in an error.

### C. Tool and MCP Level
*   `PreToolUse`: Fires before tool invocation. Can block execution (`deny`), request confirmation (`ask`), or defer (`defer`).
*   `PostToolUse`: Fires after a tool succeeds. Can rewrite tool outputs (`updatedToolOutput`) or feed blocks to the model (`continueOnBlock`).
*   `PostToolUseFailure`: Fires after a tool fails.
*   `PreCompact` / `PostCompact`: Gates context compaction. Pre-compaction is a critical hook for capturing/syncing transient states to disk.

---

## 2. Output and Decision Schema

Hooks return control parameters via stdout JSON formatting. The schema varies by lifecycle phase:

### A. Common Response Schema (All Events)
```json
{
  "continue": true,
  "stopReason": "Optional message when continue is false"
}
```
*   `continue: false` terminates the execution loop immediately.

### B. Tool Hook Control Schema (`PreToolUse` / `PostToolUse`)
Returned inside a `hookSpecificOutput` block:
```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "allow | deny | ask | defer",
    "permissionDecisionReason": "Reason for denial or deferral",
    "updatedInput": {
      "param_name": "modified_value"
    }
  }
}
```

*   **`allow`**: Auto-approves tool run, skipping interactive permission prompts.
*   **`deny`**: Blocks tool execution, feeding `permissionDecisionReason` to the model.
*   **`ask`**: Explicitly prompts the operator for approval.
*   **`defer`**: Punts decision to default system permission handlers.

### C. Session Control Schema (`SessionStart`)
Allows adjusting the session's workspace context:
```json
{
  "sessionTitle": "Feature/Auth Sandbox (TASK-123)",
  "reloadSkills": true
}
```

*   `reloadSkills`: Forces mid-session skill catalog reload.

---

## 3. Comparative Map: AIDA vs. Claude Code Hooks

| AIDA Mechanism | Claude Code Hook Integration Path | Gap / Action |
|---|---|---|
| **Lease Validation** | `Stop` or `PreToolUse` | AIDA can run local CLI checks in `Stop` to verify that active leases have not gone stale or expired. |
| **Spec Graph Sync (`aida pull`)** | `SessionEnd` | When a Claude session exits cleanly, AIDA can run `SessionEnd -> aida pull` to sync speculative rules and auto-bump completed tasks. |
| **Lease Snapshots** | `PreCompact` | Context compaction discards oldest memory; AIDA can use `PreCompact` to write a snapshot of active leases and working directories to `.aida/snapshots/`. |
| **Rule Validation** | `InstructionsLoaded` | Fires when `CLAUDE.md` and rules load. AIDA can audit whether active specs and constraints are correctly bound in the system context. |

---

## 4. Hook Scaffolding Recommendations for AIDA

AIDA should update its scaffolding engine (`aida init`) to configure the following default hook scripts in the local `.claude/settings.json` profile:

1.  **`SessionEnd` -> `aida pull`**
    *   Fires a command-type hook that executes `aida pull` on clean exit to ensure the local main branch remains cleanly aligned with remote PR updates.

2.  **`PreCompact` -> `aida snapshot`**
    *   Saves the session state and active lease tokens to `.aida/snapshots/` before Claude's context is pruned, preventing data loss in long-running sessions.

3.  **`PreToolUse` -> Lease Validator**
    *   Checks whether the active SPEC-ID lease is still owned by the current PID. Denies or defers files updates (`deny`) if another agent has stolen the lease or the lease has expired.
