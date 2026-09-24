#!/bin/sh
# AIDA Claude Code Hook: TASK-1454 — record a pending-approval marker when
# Claude Code fires a Notification(permission_prompt) event, so `aida ps`
# and `aida awaiting` can say a seat is BLOCKED on the human right now,
# instead of a purely time/process heuristic guessing at it (see BUG-1553's
# 2026-09-23 PROXY DECISION, which shipped the safe LongToolCall/Suspended
# half and deferred exactly this marker as a follow-up — TASK-1454 is that
# follow-up).
#
# Wired on the `Notification` event with matcher `permission_prompt` — Claude
# Code itself gates which notifications reach this script by matching on the
# notification type, so this only fires when a tool call is genuinely
# waiting on the operator's approval (not on an idle-timeout or any other
# notification shape).
#
# The marker is CLEARED by a separate hook (aida-clear-pending-approval.sh,
# wired on UserPromptSubmit + PostToolUse) once the block resolves, or read
# as stale after a time floor if neither ever fires (a crashed session).
#
# Notification hooks are fire-and-forget: Claude Code does not wait on this
# script, and nothing it prints or returns can affect the prompt itself — so
# this stays best-effort by construction. A missing `aida`/python3, or
# malformed JSON, degrades to a silent no-op. Runs under /bin/sh (dash) — no
# bashisms.
# trace:TASK-1454 | ai:claude

command -v aida >/dev/null 2>&1 || exit 0
command -v python3 >/dev/null 2>&1 || exit 0

payload=$(cat 2>/dev/null || true)
fields=$(printf '%s' "$payload" | python3 -c '
import json, re, sys
try:
    data = json.load(sys.stdin)
except Exception:
    sys.exit(1)
session_id = str(data.get("session_id") or "")
message = str(data.get("message") or "")
# The notification text is typically "Claude needs your permission to use
# <Tool>" — best-effort tool-name extraction. A phrasing that does not match
# still yields a useful marker, just without a named tool.
m = re.search(r"permission to use ([A-Za-z0-9_.\-]+)", message)
tool = m.group(1) if m else ""
print(session_id)
print(tool)
print(message[:200].replace(chr(10), " "))
' 2>/dev/null) || exit 0

session_id=$(printf '%s\n' "$fields" | sed -n '1p')
tool=$(printf '%s\n' "$fields" | sed -n '2p')
message=$(printf '%s\n' "$fields" | sed -n '3p')

[ -z "$session_id" ] && exit 0

project_root="${AIDA_SESSION_PROJECT:-${CLAUDE_PROJECT_DIR:-$(git rev-parse --show-toplevel 2>/dev/null || printf %s "$PWD")}}"
cd "$project_root" 2>/dev/null || exit 0

if [ -n "$tool" ]; then
    aida session pending-approval-set --session "$session_id" --tool "$tool" --message "$message" >/dev/null 2>&1 || true
else
    aida session pending-approval-set --session "$session_id" --message "$message" >/dev/null 2>&1 || true
fi
exit 0
