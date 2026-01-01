#!/bin/bash
# AIDA Claude Code Hook: Track commits and update requirement status
# PostToolUse hook for Bash commands
#
# This hook runs after successful git commit commands and:
# 1. Extracts requirement IDs from the commit message
# 2. Updates those requirements to "in-progress" status
# 3. Adds a comment noting the commit
#
# Exit codes:
#   0 - Success (always, this is informational only)

set -euo pipefail

# Read JSON input from stdin
input=$(cat)

# Extract command and response
command=$(echo "$input" | jq -r '.tool_input.command // ""')
tool_response=$(echo "$input" | jq -r '.tool_response // ""')

# Only process git commit commands
if ! echo "$command" | grep -qE '^git commit'; then
    exit 0
fi

# Check if commit was successful (look for success indicators in response)
# This is a heuristic - adjust based on actual tool_response format
if echo "$tool_response" | grep -qiE '(error|failed|abort)'; then
    exit 0  # Commit failed, don't update
fi

# Extract requirement IDs from commit message
req_ids=$(echo "$command" | grep -oE '\([A-Z]+-[0-9]+\)' | tr -d '()' | sort -u || true)

if [ -z "$req_ids" ]; then
    exit 0  # No requirements referenced
fi

# Check if aida CLI is available
if ! command -v aida &> /dev/null; then
    exit 0
fi

# Get the commit hash (if available)
commit_hash=$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")

# Update each requirement
for req_id in $req_ids; do
    # Check current status
    current_status=$(aida show "$req_id" 2>/dev/null | grep -oE 'Status:\s*\w+' | awk '{print $2}' || true)

    case "$current_status" in
        Draft|Approved|Planned)
            # Transition to InProgress
            aida edit "$req_id" --status in-progress 2>/dev/null || true
            echo "Updated $req_id status to in-progress"
            ;;
        InProgress)
            # Already in progress, just add comment
            ;;
        Completed)
            # Already completed, skip
            continue
            ;;
    esac

    # Add commit reference as comment
    aida comment add "$req_id" "Commit $commit_hash references this requirement" 2>/dev/null || true
done

exit 0
