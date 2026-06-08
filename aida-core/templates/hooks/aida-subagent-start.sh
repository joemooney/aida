#!/bin/sh
# AIDA Claude Code Hook: register harness worktree lease on SubagentStart.
# Best-effort and passive-observe only; Claude Code owns worktree provisioning.
# trace:TASK-702 | ai:claude

set -u

if ! command -v aida >/dev/null 2>&1; then
    exit 0
fi
if ! command -v python3 >/dev/null 2>&1; then
    exit 0
fi

payload=$(cat)
fields=$(printf '%s' "$payload" | python3 -c '
import json, sys
try:
    data = json.load(sys.stdin)
except Exception:
    sys.exit(1)
for key in ("agent_id", "agent_type", "cwd"):
    value = data.get(key) or ""
    print(str(value))
' 2>/dev/null) || exit 0

agent_id=$(printf '%s\n' "$fields" | sed -n '1p')
agent_type=$(printf '%s\n' "$fields" | sed -n '2p')
cwd=$(printf '%s\n' "$fields" | sed -n '3p')

if [ -z "$agent_id" ] || [ -z "$cwd" ]; then
    exit 0
fi

if [ -n "$agent_type" ]; then
    aida session harness-worktree-register --agent-id "$agent_id" --agent-type "$agent_type" --cwd "$cwd" >/dev/null 2>&1 || true
else
    aida session harness-worktree-register --agent-id "$agent_id" --cwd "$cwd" >/dev/null 2>&1 || true
fi

exit 0
