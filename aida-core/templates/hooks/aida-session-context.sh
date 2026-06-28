#!/bin/bash
# AIDA Session Context Hook
# Hook type: SessionStart (runs once when a Claude Code session begins).
#
# Emits a COMPACT, token-minimal ambient-context digest so every session opens
# already knowing the active role, how deep the queue is, how much is in flight,
# and the top-N queued items — the AXI "ambient context" principle.
#
# CHANNEL: writes to STDOUT. Claude Code injects a SessionStart hook's stdout
# into the session context; stderr is NOT injected. (The earlier version wrote
# to stderr and was never wired, so it was a dead asset — TASK-971.)
#
# BUDGET: this loads into EVERY session, so the digest is deliberately terse —
# one header line (role / queue depth / in-flight count) plus up to N queued
# spec ids with trimmed titles. No verbose dashboard.
#
# Best-effort: a missing/slow `aida`, no store, or an empty queue all degrade to
# a clean no-op (empty stdout, exit 0) so a session never fails on this hook.
#
# trace:TASK-971 | ai:claude

set -u

# Number of queued items to surface and the title trim width.
TOP_N=5
TITLE_WIDTH=56

command -v aida >/dev/null 2>&1 || exit 0

# Run against the right store regardless of the hook's cwd, mirroring the other
# SessionStart hooks (role-context / mail-notice).
project_root="${AIDA_SESSION_PROJECT:-${CLAUDE_PROJECT_DIR:-$PWD}}"
cd "$project_root" 2>/dev/null || exit 0

role="${AIDA_SESSION_ROLE:-none}"

# In-flight count: dependency-free line count of in-progress spec ids.
in_flight=$(aida list --status in-progress --format ids 2>/dev/null | grep -c .)
case "$in_flight" in '' | *[!0-9]*) in_flight=0 ;; esac

# Queue depth + top-N. `aida queue list --json` is the cache-fast machine read;
# it can include terminal (Completed/Rejected) entries, so we filter those out.
# Prefer jq, fall back to python3, and degrade to a header-only digest if
# neither is available.
queue_json=$(aida queue list --json 2>/dev/null)
queue_depth=0
queue_lines=""
if [ -n "$queue_json" ]; then
    if command -v jq >/dev/null 2>&1; then
        queue_depth=$(printf '%s' "$queue_json" | jq -r '
            [.[] | select(.status != "Completed" and .status != "Rejected")] | length' 2>/dev/null)
        queue_lines=$(printf '%s' "$queue_json" | jq -r --argjson n "$TOP_N" --argjson w "$TITLE_WIDTH" '
            [.[] | select(.status != "Completed" and .status != "Rejected")][:$n][]
            | "  " + .spec_id + "  " + ((.title // "") | if length > $w then .[:$w] + "..." else . end)' 2>/dev/null)
    elif command -v python3 >/dev/null 2>&1; then
        queue_depth=$(printf '%s' "$queue_json" | python3 -c '
import json, sys
try:
    rows = [r for r in json.load(sys.stdin)
            if r.get("status") not in ("Completed", "Rejected")]
except Exception:
    print(0); sys.exit(0)
print(len(rows))' 2>/dev/null)
        queue_lines=$(printf '%s' "$queue_json" | python3 -c '
import json, sys
try:
    rows = [r for r in json.load(sys.stdin)
            if r.get("status") not in ("Completed", "Rejected")]
except Exception:
    sys.exit(0)
n, w = '"$TOP_N"', '"$TITLE_WIDTH"'
for r in rows[:n]:
    t = (r.get("title") or "")
    if len(t) > w:
        t = t[:w] + "..."
    print("  " + str(r.get("spec_id", "")) + "  " + t)' 2>/dev/null)
    fi
fi
case "$queue_depth" in '' | *[!0-9]*) queue_depth=0 ;; esac

# Emit the compact digest to STDOUT.
printf 'AIDA · role: %s · queue: %s · in-flight: %s\n' "$role" "$queue_depth" "$in_flight"
if [ -n "$queue_lines" ]; then
    printf '%s\n' "$queue_lines"
fi

exit 0
