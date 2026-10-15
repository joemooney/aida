# Tool-only grant expiry notices

<!-- trace:BUG-2014 | ai:codex -->

Claude Code **2.1.296** (`claude --version`, 2026-10-13) was checked against
[the official hook reference](https://code.claude.com/docs/en/hooks#pretooluse-decision-control).
`PreToolUse` accepts this informational JSON, with no permission decision:

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"Expiry warning and renewal command"}}
```

A live `claude -p` probe used the actual template shell hook, a process-local
fake `aida` returning this shape, and only a Bash `echo hook-probe` tool. The
prompt requested the verification word supplied by the tool hook without
including that word. There was no UserPromptSubmit hook. Claude returned:

> The PreToolUse hook on Bash supplied the verification word **CEDAR-GRANT-2014**.

Local probe output: `/tmp/bug2014-real-hook-probe-ynzifzq1/result.json` and
`debug.log`. This verifies this installed client's context delivery, not the
new Rust binary, which is built and tested on GitHub CI. The live probe uses a
stub CLI because manual compilation is prohibited for the factory seat.

`tests/test_grant_expiry_notice_hook.py` pins exact JSON, the every-tool template
registration, unset/empty grant early exit, silent ledger-failure behavior,
partial-output suppression and the 1.8s timeout (0.1s kill grace). CI and the
fast pre-push tier both run it. Rust tests exercise the actual typed validator
and notice/ledger functions: deterministic >2h → 90m → repeat → 20m → repeat
→ expired → repeat, same-id and new-id renewal, child-parent minimum, typed
rejections, concurrent crossings and corrupt/unreadable/unwritable ledgers.
Poisoned canonical objects and absence of a cache establish that no graph or
cache is initialized; the shared validator still reads local roster metadata.
The early role dispatch is statically pinned before storage initialization.

Ledger claims are persisted before context is printed. Interruption after a
successful claim can lose that notice, but cannot flood the session. Lock
contention skips this tool call and can retry on the next. A failed ledger
read/write emits no notice; SessionStart/statusline remain the fallback. A
jump straight to expiry emits the current expired notice rather than obsolete
near-expiry warnings. Notices confer no authority. Client delivery for Codex
and Antigravity is tracked separately in TASK-1744.
