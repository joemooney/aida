# SPIKE-41: AIDA Hook Bundle for Lifecycle Integration — Design Sketch

**Date**: 2026-06-05
**Status**: DRAFT SKETCH (Pending Master Review)
**Target Spec**: [SPIKE-41](file:///.aida-store/objects/SPIKE/000/SPIKE-41.yaml)

> **⚠ Verification caveat (master, 2026-06-05):** this sketch references items
> NOT yet confirmed against primary sources — treat as proposals to validate
> before implementing:
> - The **`InstructionsLoaded`** hook event is **unverified** against the
>   Claude Code hook taxonomy (`docs/agents/session-communication.md` documents
>   SessionStart/SessionEnd/PreToolUse/PostToolUse/Stop/PreCompact). Re-confirm
>   it exists before wiring an integration to it.
> - The commands `aida snapshot`, `aida lease validate`/`check`, and
>   `aida rules audit`/`verify` **do not exist today** (real surface:
>   `aida session leases`, `aida rules sync`, internal `state-snapshot`).
> - The settings.json `{"event","action"}` array shape differs from AIDA's
>   actual keyed-object hook config (SessionStart/PreToolUse/…).

---

## 1. Goal Description

Establish a standardized hook configuration bundle scaffolded by `aida init` to automatically synchronize Claude Code session lifecycle events with AIDA's git-canonical spec-graph and database coordination substrate. This bridges process-level events (like session exits or memory compactions) directly into durable workspace updates.

---

## 2. Event Integration Architecture

We propose binding AIDA CLI operations to the following Claude Code hook events:

```
┌───────────────────┐                     ┌────────────────────┐
│ Claude Code Event │                     │ AIDA CLI Command   │
├───────────────────┤                     ├────────────────────┤
│ SessionEnd        ├────────────────────>│ aida pull          │ (Syncs spec auto-bumps)
│ PreCompact        ├────────────────────>│ aida snapshot      │ (Saves lease state before summary)
│ PreToolUse        ├────────────────────>│ aida lease check   │ (Locks file updates to owner)
│ InstructionsLoaded├────────────────────>│ aida rules audit   │ (Verifies active spec rules)
└───────────────────┘                     └────────────────────┘
```

### Proposed Mappings and Behaviors

1.  **`SessionEnd` -> `aida pull`**
    *   *Trigger*: Fires when the terminal session exits cleanly (using `clear | resume | logout` discriminators).
    *   *Action*: Execute `aida pull` asynchronously to synchronize remote git commits and automatically transition resolved specs (`Done` -> `Completed`).
2.  **`PreCompact` -> Lease State Snapshot**
    *   *Trigger*: Fires immediately before context summarization/compaction.
    *   *Action*: Export the active lease registry token, PID, and working directory path to `.aida/snapshots/last-active-lease.json`. This prevents losing the transient active state during long-running reasoning compactions.
3.  **`PreToolUse` -> Lease Validation Guard**
    *   *Trigger*: Fires before any `Write` or `Edit` tool execution.
    *   *Action*: Invoke an AIDA script to confirm the current process owns the active lease on the touched file/scope.
    *   *Behavior*: If the lease has expired or been claimed by another agent, exit with code `2` (blocking error) and return the error message: `[AIDA:LEASE_STOLEN] Active lease on this scope is owned by another process.`
4.  **`InstructionsLoaded` -> Rules Audit**
    *   *Trigger*: Fires when `CLAUDE.md` or `.claude/rules/*.md` files are loaded (using the `load_reason` discriminator).
    *   *Action*: Execute a validation audit to verify that AIDA-emitted rules for the current spec scope are properly populated in the active rules file.

---

## 3. Hook Configuration and Scaffolding (`aida init`)

AIDA's initialization routine (`aida init`) will generate or merge the following hooks definition block inside `.claude/settings.json`:

```json
{
  "hooks": [
    {
      "event": "SessionEnd",
      "type": "command",
      "action": "aida pull"
    },
    {
      "event": "PreCompact",
      "type": "command",
      "action": "aida snapshot --lease-id $AIDA_AGENT_REGISTRY_TOKEN"
    },
    {
      "event": "PreToolUse",
      "type": "command",
      "matcher": "^(Write|Edit|EditFile)$",
      "action": "aida lease validate --file {{input.path}}"
    },
    {
      "event": "InstructionsLoaded",
      "type": "command",
      "action": "aida rules verify --reason {{input.load_reason}}"
    }
  ]
}
```

---

## 4. Trade-Offs

*   **Pros**:
    *   **Automated hygiene**: No need to rely on the operator manually calling `aida pull` on exit.
    *   **Proactive safety**: Prevents race conditions or dual-editing of files without a valid lease.
*   **Cons**:
    *   **Latency overhead**: Spawning `aida lease validate` on every `Write`/`Edit` tool call adds up to 50ms per interaction.
    *   **Scope conflicts**: If the user edits files manually outside Claude Code, hook execution is skipped.

---

## 5. Open Decisions for Master

1.  **Command vs. HTTP/MCP hook type?**
    *   *Path A*: Command hooks. Simple to configure, executes via shell. But has higher process startup latency (~40-80ms).
    *   *Path B*: HTTP hooks. Fast execution, but requires AIDA to run a local daemon/server to receive POST hook endpoints.
    *   *Decision*: Standardize on command hooks for simplicity in v1; optimize to HTTP if latency becomes a bottleneck.
2.  **Validation failure behavior**:
    *   Should a failed `PreToolUse` lease check exit with code `2` (hard block/halt) or return a prompt suggestion to AIDA (`allow` with warning banner)?
    *   *Recommendation*: Hard block (exit code `2`) to prevent race conditions in highly concurrent multi-agent drains.
