#!/bin/sh
# AIDA Claude Code Hook: TASK-1454 — clear this session's pending-approval
# marker (if any) on the next UserPromptSubmit or PostToolUse. Either event
# means the block recorded by aida-notification.sh has resolved: a human
# answered the prompt (a tool then ran, PostToolUse fires) or sent a new
# prompt (UserPromptSubmit fires).
#
# Wired on BOTH `UserPromptSubmit` and `PostToolUse` with NO matcher (every
# tool, not just Bash — the existing PostToolUse `aida-track-commits.sh`
# entry is Bash-only and left untouched) so the marker never outlives the
# turn it was raised on. `aida ps` / `aida awaiting` additionally treat an
# un-cleared marker as stale after a time floor (see
# `pending_approval::PENDING_APPROVAL_STALE_SECS` in aida-cli-lib) as a
# backstop for a session that crashes before either event fires.
#
# Deliberately prints nothing on success — this runs on essentially every
# turn (most of which have nothing to clear), and its stdout is folded into
# the agent's context on UserPromptSubmit, so a routine cleanup pass must
# stay silent rather than adding per-turn noise.
#
# Best-effort, cheap, non-blocking. Runs under /bin/sh (dash) — no bashisms.
# trace:TASK-1454 | ai:claude

command -v aida >/dev/null 2>&1 || exit 0
command -v python3 >/dev/null 2>&1 || exit 0

payload=$(cat 2>/dev/null || true)
session_id=$(printf '%s' "$payload" | python3 -c '
import json, sys
try:
    data = json.load(sys.stdin)
except Exception:
    sys.exit(0)
print(data.get("session_id") or "")
' 2>/dev/null) || exit 0

[ -z "$session_id" ] && exit 0

project_root="${AIDA_SESSION_PROJECT:-${CLAUDE_PROJECT_DIR:-$(git rev-parse --show-toplevel 2>/dev/null || printf %s "$PWD")}}"
cd "$project_root" 2>/dev/null || exit 0

aida session pending-approval-clear --session "$session_id" >/dev/null 2>&1 || true
exit 0
