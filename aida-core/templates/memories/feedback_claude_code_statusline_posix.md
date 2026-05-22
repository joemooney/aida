---
name: Claude Code statusLine and hooks run under /bin/sh, not bash
description: Write statusLine commands and unmarked hook commands as POSIX sh — bashisms silently fail to empty output on Ubuntu (dash)
type: feedback
propagation: scaffolding-pack
originSessionId: 123d0d20-197d-490d-a6fd-1332da826246
---
Claude Code's `statusLine.command` and `hooks[*].hooks[*].command` (when no `shell: bash` field is set) execute under `/bin/sh`. On Ubuntu / Debian / WSL-Ubuntu, `/bin/sh` is `dash`, not bash. Bash-only constructs raise `Bad substitution` (or similar) and Claude Code renders the statusline as empty / silently drops the hook — there's no visible error in the TUI.

**Why:** Burned on 2026-05-04 writing a custom statusLine that used `${PWD/#$HOME/\~}` (bash parameter substitution). Worked perfectly in every standalone test under `bash -c`, but Claude Code's TUI showed no statusline at all. Diagnosed by running the command under `sh -c` — got `Bad substitution`. The substitution-with-pattern syntax `${var/pat/repl}` is a bashism; dash only supports POSIX `${var#pat}` / `${var%pat}` / `${var:+...}` / `${var:-...}`.

**How to apply:**
- Default to POSIX sh syntax for `statusLine` and `hooks` commands. Common bashisms to avoid: `${var/pat/repl}`, `${var//pat/repl}`, `${var^^}`, `[[ ... ]]`, arrays, `<( )` process substitution, `==` in `[ ]`.
- For tilde-collapsing under sh, use `case "$PWD" in "$HOME"*) cwd="~${PWD#$HOME}" ;; *) cwd="$PWD" ;; esac` — POSIX-clean.
- For hooks specifically, you can opt into bash by setting `"shell": "bash"` on the hook entry — `statusLine` has no such field, so it must be POSIX.
- Test commands with `sh -c "$(jq -r '.statusLine.command' /path/to/settings.json)"` before assuming the bash test was sufficient. `bash -c` will mask the failure.
- If a statusline / hook ever silently disappears: first suspect shell incompatibility, not a config-watcher issue.
