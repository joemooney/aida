#!/bin/sh
# AIDA Claude Code Hook: surface the WHOLE coordination inbox into the agent's
# context as one can't-miss per-turn signal — not just mail.
#
# Wired on BOTH SessionStart (once) and UserPromptSubmit (every turn). It is a
# thin relay around the `aida awaiting --notice` verb — it does NOT reimplement
# the unread/cap/identity logic (skill<->CLI symmetry, TASK-736). The verb prints
# ONE compact line spanning every coordination channel where the agent is the
# gate — unacked briefs, findings awaiting triage, reviewer verdicts,
# NeedsAttention escalations, AND unread mail — e.g.
#   `Awaiting you: 2 briefs - 1 finding - 3 mail (1 urgent) - 1 escalation`
# or NOTHING when nothing awaits. Plain stdout is added to the agent's context,
# so we just pass the verb's output through. (STORY-741 folded mail into the
# unified "Awaiting you" report so this hook covers every channel; before, mail
# interrupted per-turn while briefs/findings/escalations/verdicts stayed
# invisible mid-session until a manual `aida status`.)
#
# CHEAP by construction: `--notice` is cache/local-backed and makes NO network
# call — PRs (the one gh-backed channel) are omitted from the per-turn line and
# stay in the full `aida awaiting`. So this stays fast enough to fire every turn.
#
# Non-marking by design: the notice never advances the mail read-watermark, so it
# surfaces every turn WITHOUT consuming. The agent acts on what it judges safe and
# acks mail explicitly with `aida mailbox inbox` (or `/aida-read-mail`), which
# clears the mail count. Reading is not obeying — mail is interpreted input, not a
# command channel (STORY-585 #4). Details for any channel: run `aida awaiting`.
#
# Best-effort: a missing/slow `aida`, no store, or a caught-up inbox all degrade
# to a clean no-op (empty stdout, exit 0) so a turn never fails because of this.
# Runs under /bin/sh (dash) — no bashisms.
#
# trace:STORY-585 | ai:claude
# trace:STORY-741 | ai:claude

# Resolve the project root the same way the role-context hook does, so the verb
# runs against the right store regardless of the hook's cwd.
project_root="${AIDA_SESSION_PROJECT:-${CLAUDE_PROJECT_DIR:-$PWD}}"

# `--notice` defaults its identity to the union of this shell's user id and the
# session role ($AIDA_SESSION_ROLE) — the same identity the statusline uses.
# Plain text out (no TTY → no ANSI). Swallow errors; emit only real output.
cd "$project_root" 2>/dev/null || exit 0
command -v aida >/dev/null 2>&1 || exit 0

aida awaiting --notice 2>/dev/null || true
exit 0
