# Claude exact-session wake: Step 0 investigation

Date: 2026-11-05. Outcome: **Unsupported** for AIDA live Claude mailbox wake.
The investigation is complete under Joe's binding advisor ruling recorded on
TASK-3072 at 2026-11-05 18:42 UTC. No production adapter was implemented.
<!-- trace:TASK-3072 | ai:codex -->

## Official contract gate

Source: [Claude Code cross-session messaging](https://code.claude.com/docs/en/cross-session-messaging),
read 2026-11-05, especially “The session’s inbox socket”, “Message delivery”,
“Control inbound messages”, and “Non-interactive sessions”.

The page defines endpoint export before SessionStart and optional Linux auth
framing. It does **not** define a posted socket message payload or a socket
response schema classifying acceptance, hold or refusal. The displayed auth
line is not a message contract. Built-in SendMessage outcomes do not specify
an external socket client's response contract.

Documented behavior, not live-verified here: busy recipients receive between
tool calls; idle recipients can start a turn. Explicit inbound hold retains
messages until acceptance applies, whereas refusal drops them. Non-interactive
default-held messages can expire. None of these descriptions establishes a
socket acceptance receipt for AIDA.

Step 0 therefore fails. No private schema, binary inspection, third-party
transport implementation, MCP channels, or resumed/forked session substitutes
were used to fill the gap. A successful connection or write would not prove
queue acceptance, read acknowledgement or action.

## One disposable live probe

Installed CLI: `2.1.318 (Claude Code)`.
Scratch directory: `/tmp/task-3072-probe`, outside the project.
Exactly two new `claude -p` processes were started together; no fleet session
was selected, inspected for endpoints, resumed or messaged.

| Role | Session ID | Inbound setting |
| --- | --- | --- |
| Receiver | `bc963324-d91e-427a-9a1c-54ae5abe121b` | `hold` |
| Sender | `03d80284-2a96-463f-857a-7f1c49172289` | `accept` |

Each process used stream-json input/output, `--no-session-persistence`, a fresh
explicit session ID, `--setting-sources ''`, `--strict-mcp-config` with an empty
MCP server map, `--tools ''`, and `--permission-mode dontAsk`. AIDA and inherited
Claude session/socket/token context were removed from the child environment.
No model prompt was submitted. Stdin stayed open during a bounded 35-second
startup observation. A supplied SessionStart hook attempted to capture only
session ID, socket path, hook event and parent PID, never the token.

Both processes emitted SessionStart `hook_started` and `hook_response` events.
Both hook responses had `exit_code: 1` and `outcome: "error"`. Their exact
`stderr` field values were:

```text
Failed to run: EROFS: read-only file system, mkdir '/home/joe/.claude/session-env/bc963324-d91e-427a-9a1c-54ae5abe121b'
Failed to run: EROFS: read-only file system, mkdir '/home/joe/.claude/session-env/03d80284-2a96-463f-857a-7f1c49172289'
```

The hook could not run within the workspace sandbox's read-only Claude config
boundary. Neither binding file was created. Both processes were alive at the
observation deadline and were terminated and reaped by the probe's cleanup.
No second probe was attempted.

### Request/response bytes and behavior

Socket connections: **0**. Socket request bytes: **0** (`b""`, hex empty).
Socket response bytes: **0** (`b""`, hex empty; no connection was attempted).
No auth line, token or message payload was sent. There is no response to
classify; this is not evidence of an accepted, held or refused message.

| Scenario | Live observation |
| --- | --- |
| Busy recipient | Unverified: no model/tool turn or socket message |
| Idle recipient | Unverified: processes waited for stdin; no delivery attempted |
| Explicit hold | Configured on receiver; delivery/retention unverified |
| Acceptance/refusal receipts | Unverified: no documented payload/response contract |
| Idempotence and recovery | Unverified: no adapter or submission |

The startup failure is a probe limitation, not a claim that Claude messaging
is unavailable generally. The independent documentation gap is sufficient to
trigger the ruling's stop condition; even a successful startup would not
supply the missing public contract.

## Supported path and follow-up boundary

Durable AIDA mailbox requests and existing next-turn hook delivery remain the
supported Claude path. An operator can ask the receiver to read its inbox.
AIDA does not promise to start an idle Claude turn through the watcher.
“Unsupported” here names the live-wake capability outcome; existing source
classifies Claude as `WakeAdapter::NotApplicable` for its per-turn hook route.
No registry state, inbound policy, token handling or watcher ledger changed.

Reconsider only when official documentation supplies both the socket message
payload and classifiable response semantics, with a newly authorized probe.
The rejected channels route remains outside this task. Binding validation and
fake-transport tests are conditional implementation work and were not reached.
