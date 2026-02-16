#!/bin/bash
# AIDA Session Context Hook
# Injects project context at session start
# Hook type: SessionStart (runs when Claude Code session begins)

# Check if aida CLI is available
if ! command -v aida &>/dev/null; then
    exit 0
fi

# Show brief project status
echo "AIDA Project Context:" >&2

# In-progress work
in_progress=$(aida list --status in-progress --format brief 2>/dev/null | head -5)
if [ -n "$in_progress" ]; then
    echo "  In Progress:" >&2
    echo "$in_progress" | sed 's/^/    /' >&2
fi

# High priority approved items
approved=$(aida list --status approved --priority high --format brief 2>/dev/null | head -3)
if [ -n "$approved" ]; then
    echo "  Ready (High Priority):" >&2
    echo "$approved" | sed 's/^/    /' >&2
fi

exit 0
