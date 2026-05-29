# SPIKE-32 Workflow POC

**Status**: narrow happy-path POC  
**Target spec**: SPIKE-30  
**Artifact**: `spec-30-drain.workflow.js`

This directory probes the SPIKE-32 thesis: AIDA can compile a spec graph into a saved Claude Code workflow script that replays AIDA's drain shape.

## What The POC Proves

The happy-path orchestration shape is clean:

1. Build a prompt from spec description and acceptance criteria.
2. Spawn an implementer `agent()` call.
3. Stub CI wait.
4. Spawn a reviewer `agent()` call.
5. Branch on approximate approval.
6. Spawn a merger `agent()` call.
7. Return a structured result with phase outcomes.

The script is valid JavaScript and can be run locally:

```bash
node docs/architecture/spike-32-workflow-poc/spec-30-drain.workflow.js
```

That local run uses a mock `agent()` fallback. It validates the data contract, not Claude Code's runtime API.

## Runtime API Caveat

Claude's public workflows docs confirm the runtime model, saved project path, background execution, resumability limits, and that scripts coordinate agents rather than directly accessing shell/filesystem. They do **not** publish the full JavaScript helper signature. The installed math-olympiad workflow guidance says to set `opts.label` on each `agent()` call, so the POC uses:

```js
agent(prompt, { label: "SPIKE-30:implementer" })
```

The next design pass still needs a real saved `.claude/workflows/*.js` sample from Claude Code to confirm exact syntax.

## Failure-Routing Probe

- **Punt path**: advisor reroute is easy if the implementer returns structured `{status: "punt"}`. Resuming the *same* implementer context after an out-of-band AIDA punt resolution is not proven.
- **Shelve path**: RequestChanges branching is easy. Marking the spec `NeedsAttention` requires an AIDA CLI/MCP callback or external supervisor; pure workflow variables are insufficient substrate.
- **Resume path**: awkward. Claude workflow resume is run-local; AIDA's punt resolution is substrate-local and may happen after the workflow stops.

## Verdict

Reshape, do not kill. The happy path is simple enough to justify SPIKE-32 as months-not-weeks scoped. The design pass should focus on runtime API confirmation, CLI/MCP substrate callbacks, and recoverability semantics before any divestment from `auto_complete.rs`.
