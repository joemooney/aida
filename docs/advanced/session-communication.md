# Agent Session Communication Reference

This document captures operational facts about communicating with running
agent sessions. Keep it updated when Claude Code, Codex, Antigravity, or AIDA
launcher behavior changes. The goal is to prevent agents and operators from
rediscovering the same pause/abort/resume semantics through production
failures.

## Claude Code Hook Control Flow

Claude Code hook control is event-ordered. A hook that stops the current event
does not create a later event that can ask whether to continue.

Key rules:

- `continue: false` is terminal for the current Claude run after the hook
  returns. It takes precedence over event-specific decision fields.
- `PreToolUse` fires before a tool call. If a `PreToolUse` hook blocks the
  tool, the tool never completes, so `PostToolUse` will not fire for that
  call.
- `PostToolUse` is downstream of successful tool execution. Do not design a
  "block in PreToolUse, then ask in PostToolUse" sequence; the ordering makes
  that impossible.

Use one of these patterns instead:

| Situation | Hook response | Result |
|---|---|---|
| Interactive user can answer now | `permissionDecision: "ask"` | Claude shows a permission prompt for the tool call. |
| Unattended run should stop and alert | Notify inside the hook, then return `continue: false` | Alert is sent as hook side effect; Claude stops. |
| Headless run needs external approval and later resume | `permissionDecision: "defer"` | Claude exits with `stop_reason: "tool_deferred"`; caller resumes after external decision. |

### Pattern 1: Interactive Ask

In an interactive Claude Code session, return:

```json
{ "permissionDecision": "ask" }
```

Claude prompts the user to allow or deny the tool call. This is the built-in
"should this continue?" path.

Do not use this for unattended `claude -p` runs. There is no terminal user to
answer, so the run cannot make progress.

### Pattern 2: Notify, Then Halt

For unattended aborts where no external approval service will resume the
session, send the notification inside the hook and then stop Claude:

```bash
#!/usr/bin/env bash
set -euo pipefail

if [ -f "${CLAUDE_PROJECT_DIR}/.aida-abort" ]; then
  payload="$(jq -n \
    --arg event "runaway_abort" \
    --arg session "${CLAUDE_CODE_SESSION_ID:-unknown}" \
    '{event: $event, session: $session}')"

  curl -sS -X POST "$AIDA_ALERT_URL" \
    -H 'Content-Type: application/json' \
    -d "$payload" \
    >/dev/null 2>&1 || true

  jq -n '{continue: false, stopReason: "Runaway detected; ops notified"}'
  exit 0
fi
```

This is the right shape for fully unattended AIDA drains when the safe action
is "alert the operator and stop." The notification is not a later hook; it is a
side effect of the hook that made the halt decision.

### Pattern 3: Defer And Resume

For headless runs that need a real external approval gate, return:

```json
{ "permissionDecision": "defer" }
```

In `claude -p` mode, Claude exits with `stop_reason: "tool_deferred"` and
preserves the pending tool call in the session state. The caller then:

1. Reads the deferred tool-use payload.
2. Asks an external authority: web UI, approval service, advisor, or human.
3. Resumes with `claude -p --resume <session-id>`.
4. Ensures the hook now returns allow or deny according to the external
   decision.

Constraints:

- Defer only works when Claude makes a single tool call in that turn. If the
  model emits several parallel tool calls, defer is ignored with a warning
  because resume can only replay one pending tool call.
- There is no useful "PostToolUse after defer" step. The caller owns the
  external approval loop.
- Deferred sessions remain on disk until resumed or cleaned by Claude Code's
  normal retention sweep.

## AIDA Implications

AIDA's substrate gates should choose the mechanism by runtime mode:

- Interactive Claude Code: use `ask` when a human at the terminal can decide.
- Headless fire-and-forget: notify or record durable evidence, then
  `continue: false`.
- Headless approval workflow: use `defer`, then let the AIDA caller/advisor
  resume with an explicit decision.

Do not rely on prompt text alone for invariants. If an invariant matters,
enforce it in the launcher, hook, orchestrator, pre-commit hook, or doctor
heal path. Skill-template wording informs the model; substrate gates enforce.

## Per-Turn Coordination Signal (One Line, Every Channel)

The `UserPromptSubmit` / `SessionStart` hook `aida-mail-notice.sh` is the agent's
can't-miss per-turn signal that *something awaits it* — across **every**
coordination channel, not just mail. It is a thin relay around `aida awaiting
--notice`, which prints one compact line spanning every channel where the agent
is the gate:

```
⦿ Awaiting you: 2 briefs · 1 finding · 3 mail (1 urgent) · 1 escalation — run `aida awaiting`
```

It is **silent** when nothing awaits (empty stdout, exit 0), so the line
appearing is itself the signal.

- **Why unified.** Mail already had a per-turn hook while unacked briefs,
  findings awaiting triage, reviewer verdicts, and `NeedsAttention` escalations
  were invisible mid-session until a manual `aida status`. Folding unread mail
  into the same `AwaitingReport` the "Awaiting you" section renders makes the
  whole coordination inbox one surface (STORY-741).
- **Cheap by construction.** `--notice` is cache/local-backed and makes **no
  network call** — it never loads the full store and never shells `gh`. PRs (the
  one gh-backed channel) are omitted from the per-turn line; run the bare `aida
  awaiting` to include them. This keeps a per-prompt hook fast and fail-open: any
  misstep prints nothing rather than failing the turn.
- **Non-marking.** The notice never advances the mail read-watermark, so it
  surfaces every turn without consuming. The agent acts on what it judges safe,
  then acks mail explicitly with `aida mailbox inbox` (or `/aida-read-mail`),
  which clears the mail count. Reading is not obeying — mail is interpreted
  input, not a command channel.
- **Details on demand.** The compact line is a breadcrumb, not the inbox. Run
  `aida awaiting` for the full multi-channel report (or `aida awaiting --json`),
  and `aida mailbox inbox` for mail bodies. `aida status` still leads with the
  same "Awaiting you" section.

## Codex And Antigravity (Agy) Notes

Codex and Antigravity ("Agy" in some operator notes) do not share Claude
Code's hook event model. For these agents, AIDA communication should stay
substrate-first:

- Use `aida brief <agent> <SPEC>` or MCP brief tools for pickup routing.
- Use AIDA leases, agent registry, and `aida status` for process visibility.
- Use MCP tools for spec graph writes, comments, punts, findings, and brief
  acknowledgement.
- Use CLI lifecycle verbs (`aida queue work`, `aida pr ship`, `aida session
  end`, `aida doctor`) for stateful orchestration.

If a non-Claude agent gains comparable hook/defer primitives, add the exact
semantics here before building automation on top of them.
