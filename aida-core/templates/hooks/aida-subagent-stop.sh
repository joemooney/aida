#!/bin/sh
# AIDA Claude Code Hook: release harness worktree lease on SubagentStop.
# Best-effort and keyed by the SubagentStart/SubagentStop agent_id.
# trace:TASK-702 | ai:claude

set -u

if ! command -v aida >/dev/null 2>&1; then
    exit 0
fi
if ! command -v python3 >/dev/null 2>&1; then
    exit 0
fi

payload=$(cat)
agent_id=$(printf '%s' "$payload" | python3 -c '
import json, sys
try:
    data = json.load(sys.stdin)
except Exception:
    sys.exit(1)
print(str(data.get("agent_id") or ""))
' 2>/dev/null) || exit 0

if [ -z "$agent_id" ]; then
    exit 0
fi

aida session harness-worktree-release --agent-id "$agent_id" >/dev/null 2>&1 || true
exit 0
