# Codex Brief Pickup

Use this guide when a master/advisor session says a substrate-resident
brief is waiting for Codex. Prefer MCP over shell for the brief read path;
use shell for git, build, test, and PR work.

## MCP Flow

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

## Fallback CLI Flow

If MCP is unavailable, use the equivalent local CLI:

```bash
aida brief list --for-agent codex
aida brief ack .aida/agent-briefs/codex/<brief>.md
```

Only use the CLI fallback for the brief channel. For spec graph and
coordination work, MCP remains the preferred Codex interface when it is
available.

## Dispatch Recovery Discipline

- Make an early local WIP commit as soon as the first coherent edit compiles.
- Push the branch at the first verified boundary, such as a passing targeted
  test or build, so a fresh Codex launch can resume from git instead of a dirty
  local diff.
- If a previous agent died with only a dirty worktree, salvage that worktree
  first and commit the recovered diff before rebriefing another vendor.
- Use `aida agent dispatch-health` for a read-only recovery snapshot; in
  `AIDA_AGENT_OUTPUT=1` mode it emits TOON for machine parsing.

## Safety Notes

- Treat brief files as local runtime state under `.aida/agent-briefs/`.
- Do not edit brief files by hand unless recovering from a tool failure.
- Do not auto-claim a spec merely because a brief exists; follow the setup
  section and the current worktree/session discipline.
- If a brief references architecture-class work without a sketch verdict,
  stop and ask for master sign-off before implementing.
