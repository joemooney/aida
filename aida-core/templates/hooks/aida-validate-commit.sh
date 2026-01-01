#!/bin/bash
# AIDA Claude Code Hook: Validate commits reference requirements
# PreToolUse hook for Bash commands
#
# This hook intercepts git commit commands and validates that:
# 1. Commit message includes a requirement ID (REQ-ID) for feat/fix commits
# 2. The referenced requirement exists in the database
#
# Exit codes:
#   0 - Allow the command
#   2 - Block the command (show stderr to Claude)

set -euo pipefail

# Read JSON input from stdin
input=$(cat)

# Extract the command being executed
command=$(echo "$input" | jq -r '.tool_input.command // ""')

# Only validate git commit commands
if ! echo "$command" | grep -qE '^git commit'; then
    exit 0  # Not a commit, allow
fi

# Skip if it's an amend or merge commit
if echo "$command" | grep -qE '(--amend|--no-edit|Merge)'; then
    exit 0
fi

# Extract commit message from -m flag
# Handle both single and double quotes
if echo "$command" | grep -qE '\-m "'; then
    msg=$(echo "$command" | sed -n 's/.*-m "\([^"]*\)".*/\1/p')
elif echo "$command" | grep -qE "\-m '"; then
    msg=$(echo "$command" | sed -n "s/.*-m '\\([^']*\\)'.*/\\1/p")
else
    # No inline message, might be using editor - allow
    exit 0
fi

# Check commit type - only require REQ-ID for feat/fix
commit_type=$(echo "$msg" | grep -oE '^(\[AI:[^\]]+\] )?(feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)' | tail -1 || true)

case "$commit_type" in
    *feat*|*fix*)
        # Require requirement ID for features and fixes
        if ! echo "$msg" | grep -qE '\([A-Z]+-[0-9]+\)'; then
            cat >&2 <<EOF
Commit blocked: Missing requirement ID

Your feat/fix commit must reference a requirement:
  Format: type(scope): description (REQ-ID)
  Example: feat(auth): add login validation (FR-0042)

Run 'aida list --status approved' to find requirement IDs.
To skip validation, use 'chore' or 'docs' type instead.
EOF
            exit 2  # Block the commit
        fi
        ;;
    *)
        # Other types (docs, chore, etc.) don't require REQ-ID
        exit 0
        ;;
esac

# Validate that the requirement exists
req_id=$(echo "$msg" | grep -oE '\([A-Z]+-[0-9]+\)' | head -1 | tr -d '()')

if [ -n "$req_id" ]; then
    if command -v aida &> /dev/null; then
        if ! aida show "$req_id" &> /dev/null 2>&1; then
            echo "Warning: Requirement $req_id not found in database" >&2
            # Non-blocking warning - exit 0 to allow, exit 2 to block
            exit 0
        fi
    fi
fi

exit 0  # Allow commit
