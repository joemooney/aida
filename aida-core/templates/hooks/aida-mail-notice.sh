#!/bin/sh
# AIDA Claude Code Hook: surface unread mailbox into the agent's context.
#
# Wired on BOTH SessionStart (once) and UserPromptSubmit (every turn). It is a
# thin relay around the `aida mailbox notice` verb — it does NOT reimplement the
# unread/cap/identity logic (skill↔CLI symmetry, TASK-736). The verb prints a
# capped, plain, role+user-scoped summary of UNREAD mail, or NOTHING when the
# inbox is caught up. For both hook events, plain stdout is added to the agent's
# context, so we just pass the verb's output through.
#
# Non-marking by design: `notice` never advances the read-watermark, so this
# surfaces mail every turn WITHOUT consuming it. The agent acks explicitly with
# `aida mailbox inbox` (or `/aida-read-mail`), which clears the nag. Reading is
# not obeying — mail is interpreted input, not a command channel (STORY-585 #4).
#
# Best-effort: a missing/slow `aida`, no store, or an empty inbox all degrade to
# a clean no-op (empty stdout, exit 0) so a turn never fails because of mail.
# Runs under /bin/sh (dash) — no bashisms.
#
# trace:STORY-585 | ai:claude

# Resolve the project root the same way the role-context hook does, so the verb
# runs against the right store regardless of the hook's cwd.
project_root="${AIDA_SESSION_PROJECT:-${CLAUDE_PROJECT_DIR:-$PWD}}"

# `notice` defaults its identity to the union of this shell's user id and the
# session role ($AIDA_SESSION_ROLE) — the same identity the statusline uses.
# Plain text out (no TTY → no ANSI). Swallow errors; emit only real output.
cd "$project_root" 2>/dev/null || exit 0
command -v aida >/dev/null 2>&1 || exit 0

aida mailbox notice 2>/dev/null || true
exit 0
