<!-- AIDA Generated: v2.0.0 | checksum:fbe2a46e | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->

# Codex Brief Pickup

Use this guide when a master/advisor session says a substrate-resident
brief is waiting for Codex. Default to the CLI/TOON read path so pickup works
without MCP discovery; use shell for git, build, test, and PR work.

## Default CLI/TOON Flow

1. Run `AIDA_AGENT_OUTPUT=toon aida brief list --for-agent codex`.
2. If no briefs are returned, report `no pending briefs for codex` and stop.
3. Pick the oldest pending brief unless the operator names a different one.
4. Run `AIDA_AGENT_OUTPUT=toon aida brief read <path>` using the path returned
   by the list command.
5. Read the full brief before editing. The `## Setup` section is the intended
   worktree/lease bootstrap; the `## Trailer reminder` section is the commit/PR
   trailer contract.
6. Once you proceed with the pickup, run
   `AIDA_AGENT_OUTPUT=toon aida brief ack <path>`. Acking is idempotent, so a
   repeated call on an already-acked path is safe.
7. Implement from the brief, verify locally, and ship with `aida pr ship`.

<!-- trace:STORY-1095 | ai:codex -->

## Optional MCP Flow

1. Call `list_briefs({agent: "codex"})`.
2. If no briefs are returned, report `no pending briefs for codex` and stop.
3. Pick the oldest pending brief unless the operator names a different one.
4. Call `read_brief({path})` using the path returned by `list_briefs`.
5. Read the full brief before editing. The `## Setup` section is the
   intended worktree/lease bootstrap; the `## Trailer reminder` section is
   the commit/PR trailer contract.
6. Once you proceed with the pickup, call `ack_brief({path})`. Acking is
   idempotent, so a repeated call on an already-acked path is safe.
7. Implement from the brief, verify locally, and ship with `aida pr ship`.

## MCP Fallback Boundary

MCP is an opt-in convenience for structured reads/writes, not a prerequisite
for the lane. If MCP is unavailable or too expensive for the current context,
keep using CLI/TOON for brief and spec lookup, and use MCP only when a typed
coordination write is worth it.

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
