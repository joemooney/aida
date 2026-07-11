<!-- AIDA Generated: v2.0.0 | checksum:f39d1894 | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->

# Antigravity Brief Pickup

Use this guide when a master/advisor session says a substrate-resident
brief is waiting for Antigravity. Prefer MCP over shell for the brief read path;
use shell for git, build, test, and PR work.

## MCP Flow

1. Call `list_briefs({agent: "antigravity"})`.
2. If no briefs are returned, report `no pending briefs for antigravity` and stop.
3. Pick the oldest pending brief unless the operator names a different one.
4. Call `read_brief({path})` using the path returned by `list_briefs`.
5. Read the full brief before editing. The `## Setup` section is the
   intended worktree/lease bootstrap; the `## Trailer reminder` section is
   the commit/PR trailer contract.
6. Once you proceed with the pickup, call `ack_brief({path})`. Acking is
   idempotent, so a repeated call on an already-acked path is safe.
7. Implement from the brief, verify locally, and ship with `aida pr ship`.

## Brain-Directory Integration

Antigravity operates with rich brain-directory artifacts (`implementation_plan.md`, `task.md`, `walkthrough.md`) for persistent execution tracking:
- When starting work from a brief, initialize `implementation_plan.md` and `task.md` in the session's brain directory (`<appDataDir>/brain/<conversation-id>`).
- Reference the brief's spec ID, title, and key requirements inside `implementation_plan.md` rather than duplicating the full brief text.
- To avoid race conditions or double-pickup, call `ack_brief({path})` immediately after forking the sibling worktree and drafting the implementation plan. This signals to other agents and the master session that the brief is actively claimed.
- Summarize changes, testing, and manual verification inside `walkthrough.md` upon completion.

## Fallback CLI Flow

If MCP is unavailable, use the equivalent local CLI:

```bash
aida brief list --for-agent antigravity
aida brief ack .aida/agent-briefs/antigravity/<brief>.md
```

Only use the CLI fallback for the brief channel. For spec graph and
coordination work, MCP remains the preferred Antigravity interface when it is
available.

## Safety Notes

- Treat brief files as local runtime state under `.aida/agent-briefs/`.
- Do not edit brief files by hand unless recovering from a tool failure.
- Do not auto-claim a spec merely because a brief exists; follow the setup
  section and the current worktree/session discipline.
- If a brief references architecture-class work without a sketch verdict,
  stop and ask for master sign-off before implementing.

## Resilience: commit early, push early

A vendor agent's session can end abruptly — a dropped connection, a killed
process, a vendor-side timeout, or a credit/quota stop. Anything only in the
working tree at that moment is lost, and no other agent (or a resumed session)
can pick up from it. So checkpoint durably as you go:

- **Commit each coherent step the moment it builds/tests green** — never batch a
  whole spec into one final commit. A small commit per logical step means a crash
  costs at most the current uncommitted step, not the session.
- **Push the branch early and often** — after the first commit, then after each
  step, not just at the end. A pushed branch survives the local machine and lets
  a different vendor or a fresh session resume from where the work stopped.
- **Use the CLI for the durable git steps** (`aida commit`, `git push`) rather
  than an MCP round-trip — the CLI is the resilient surface when the connection is
  flaky (SPIKE-76, the multi-vendor resilience thesis).
- **On resume**, `git fetch` and inspect the pushed branch before re-doing work —
  the prior session may have already landed the early steps.

Work that is committed and pushed is work no single-vendor failure can erase.