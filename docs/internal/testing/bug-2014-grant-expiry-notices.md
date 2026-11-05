# Tool-only grant expiry notices

<!-- trace:BUG-2014 | ai:codex -->
<!-- trace:TASK-1744 | ai:antigravity -->

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

Codex CLI **0.163.0** (`codex --version`, 2026-10-18) supports stable `hooks`
in `.codex/hooks.json` using the same un-matchered `PreToolUse` hook event and
`hookSpecificOutput.additionalContext` JSON payload as Claude Code. `aida init`
and `scaffold_codex_hooks` install `.codex/hooks/aida-grant-expiry-notice.sh`
and wire it with `--client codex`, a 2-second timeout, and a fail-open
`top=$(git rev-parse --show-toplevel 2>/dev/null) || exit 0; h="$top/.codex/hooks/aida-grant-expiry-notice.sh"; [ -x "$h" ] || exit 0`
guard when a tool runs outside a git checkout or inside a git checkout that
lacks the gitignored handler copy.

> **Codex hook-trust re-approval**: Codex records per-hook-entry SHA-256 trust
> hashes under `[hooks.state."<repo>/.codex/hooks.json:pre_tool_use:<entry>:<hook>"]`
> in `~/.codex/config.toml`. When `aida init --force` or `scaffold_codex_hooks`
> adds or updates the `PreToolUse` entry in `.codex/hooks.json`, interactive
> Codex sessions must re-approve the updated hook via `/hooks` (or headless test
> harnesses must pass `--dangerously-bypass-hook-trust`) before Codex executes
> the new entry.

A live `codex exec` probe (session `01a15113-114a-7d42-bd39-f504a16dc73b`) used
the actual template shell hook, `.codex/hooks.json` wired with `--client codex`,
`AIDA_SESSION_GRANT=probe-grant-1744`, and a process-local fake `aida` returning:

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"The advisor seat grant expires in 18m. Verification word: MAPLE-CODEX-1744. Renew: aida role enter advisor"}}
```

With only a shell `echo hook-probe` tool call and no `UserPromptSubmit` hook,
Codex logged `hook: PreToolUse` / `hook: PreToolUse Completed` before
`/bin/bash -lc 'echo hook-probe'` and returned:

> `MAPLE-CODEX-1744`

Local probe output: `/home/joe/.cache/task1744-codex-probe/codex_result.txt` and
`codex_hook_stdout.log`.

Antigravity CLI **1.4.2** (`agy --version`, 2026-10-18) reads `.agents/hooks.json`
and executes `PreInvocation` hooks before every model invocation in a tool-only
loop, as documented in Antigravity CLI 1.4.2's built-in hook reference at
`~/.gemini/antigravity-cli/builtin/skills/agy-customizations/docs/hooks.md`
(sections `## Input/Output Contract`, `### 2. PostToolUse Contract`, and
`### 4. PreInvocation Contract`). `scaffold_antigravity_hooks` non-destructively
merges the `"aida-grant-expiry-notice"` named hook key into existing valid
`.agents/hooks.json` objects (preserving third-party keys and their original
order, and leaving malformed/non-object files untouched with a warning),
installs `.agents/hooks/aida-grant-expiry-notice.sh` (ignored along with
`.agents/hooks.json` in downstream repos via `.gitignore`), and wires
`.agents/hooks.json` with `--client agy`, a 2-second timeout, and a fail-open
`{}` guard (`top=$(git rev-parse --show-toplevel 2>/dev/null) || { printf '{}\n'; exit 0; }; h="$top/.agents/hooks/aida-grant-expiry-notice.sh"; [ -x "$h" ] || { printf '{}\n'; exit 0; }`)
outside git checkouts or in git checkouts without the handler copy:

```json
{"injectSteps":[{"ephemeralMessage":"Expiry warning and renewal command"}]}
```

When no notice is emitted (unset/empty grant, threshold already claimed, or
ledger failure), `--client agy` outputs `{}` on stdout (the documented no-op
JSON object in `agy-customizations/docs/hooks.md`) and exits 0 without a
permission decision or `terminationBehavior`.

A live `agy -p --output-format json` probe (conversation
`edd09216-b364-4e5b-8c04-32cca53fb3b5`) used the actual template shell hook,
`.agents/hooks.json` wired with `--client agy`, `AIDA_SESSION_GRANT=probe-grant-1744`,
and a process-local fake `aida` that returned `{}` on the initial `PreInvocation`
before `echo hook-probe` and then returned:

```json
{"injectSteps":[{"ephemeralMessage":"The advisor seat grant expires in 18m. Verification word: BIRCH-AGY-1744. Renew: aida role enter advisor"}]}
```

on the post-tool `PreInvocation`. Antigravity executed `echo hook-probe` and
returned:

> `{"conversation_id":"edd09216-b364-4e5b-8c04-32cca53fb3b5","status":"SUCCESS","response":"BIRCH-AGY-1744\n",...}`

Local probe output: `/tmp/task1744-real-hook-probe-_xxvxvoq/agy_result.json` and
`agy_hook_stdout.log`.

`tests/test_grant_expiry_notice_hook.py` pins exact JSON for Claude/Codex
(`--client codex`) and Antigravity (`--client agy`), the every-tool/pre-invocation
template registrations, unset/empty grant early exit across all three clients,
missing-`timeout` fail-open behavior, non-git working directory fail-open behavior
for the template commands, silent ledger-failure behavior, partial-output
suppression and the 1.8s timeout (0.1s kill grace). CI and the fast pre-push tier
both run it. Rust tests exercise the actual typed validator, client protocol
renderer (`NoticeClient`), non-destructive `.agents/hooks.json` merge and re-init
repair, and notice/ledger functions: deterministic >2h → 90m → repeat → 20m →
repeat → expired → repeat, same-id and new-id renewal, child-parent minimum,
typed rejections, concurrent crossings and corrupt/unreadable/unwritable ledgers.
Poisoned canonical objects and absence of a cache establish that no graph or
cache is initialized; the shared validator still reads local roster metadata.
The early role dispatch is statically pinned before storage initialization.

Ledger claims are persisted before context is printed. Interruption after a
successful claim can lose that notice, but cannot flood the session. Lock
contention skips this tool call and can retry on the next. A failed ledger
read/write emits no notice; SessionStart/statusline remain the fallback. A
jump straight to expiry emits the current expired notice rather than obsolete
near-expiry warnings. Notices confer no authority.
