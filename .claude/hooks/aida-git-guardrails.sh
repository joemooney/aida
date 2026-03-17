#!/bin/bash
# AIDA Git Safety Guardrails — PreToolUse hook for Claude Code
#
# Blocks destructive git operations that could cause data loss:
# - git reset --hard (discards uncommitted work)
# - git clean -f (deletes untracked files)
# - git checkout -- . (discards all changes)
# - git push --force (overwrites remote history)
# - git branch -D (force-deletes branches)
# - git stash drop (permanently drops stashed work)
# - git rebase without confirmation context
#
# Install: add to .claude/settings.json hooks.PreToolUse
# The hook reads the tool input from stdin as JSON.

set -euo pipefail

# Read the tool use from stdin
INPUT=$(cat)

# Extract the command from the Bash tool input
COMMAND=$(echo "$INPUT" | grep -oP '"command"\s*:\s*"\K[^"]*' 2>/dev/null || echo "")

# If no command found (not a Bash tool call), allow
if [ -z "$COMMAND" ]; then
    exit 0
fi

# Patterns that indicate destructive git operations
# Each pattern has an explanation of why it's blocked
check_destructive() {
    local cmd="$1"

    # git reset --hard — discards all uncommitted changes
    if echo "$cmd" | grep -qE 'git\s+reset\s+--hard'; then
        echo "BLOCKED: 'git reset --hard' discards all uncommitted changes."
        echo "Alternative: 'git stash' to save changes, or 'git checkout -- <file>' for specific files."
        return 1
    fi

    # git clean -f — permanently deletes untracked files
    if echo "$cmd" | grep -qE 'git\s+clean\s+-[a-zA-Z]*f'; then
        echo "BLOCKED: 'git clean -f' permanently deletes untracked files."
        echo "Alternative: 'git clean -n' to preview what would be deleted."
        return 1
    fi

    # git checkout -- . — discards all working tree changes
    if echo "$cmd" | grep -qE 'git\s+checkout\s+--\s+\.'; then
        echo "BLOCKED: 'git checkout -- .' discards all working tree changes."
        echo "Alternative: 'git checkout -- <specific-file>' for targeted restore."
        return 1
    fi

    # git push --force or -f (not --force-with-lease)
    if echo "$cmd" | grep -qE 'git\s+push\s+.*--force'; then
        if ! echo "$cmd" | grep -qF -- '--force-with-lease'; then
            echo "BLOCKED: 'git push --force' can overwrite remote history."
            echo "Alternative: 'git push --force-with-lease' is safer (checks remote hasn't changed)."
            return 1
        fi
    fi
    if echo "$cmd" | grep -qE 'git\s+push\s+-[a-zA-Z]*f\b'; then
        echo "BLOCKED: 'git push -f' can overwrite remote history."
        echo "Alternative: 'git push --force-with-lease' is safer."
        return 1
    fi

    # git branch -D — force-deletes a branch regardless of merge status
    if echo "$cmd" | grep -qE 'git\s+branch\s+-D\b'; then
        echo "BLOCKED: 'git branch -D' force-deletes a branch even if not merged."
        echo "Alternative: 'git branch -d' (lowercase) only deletes if merged."
        return 1
    fi

    # git stash drop/clear — permanently removes stashed changes
    if echo "$cmd" | grep -qE 'git\s+stash\s+(drop|clear)\b'; then
        echo "BLOCKED: 'git stash drop/clear' permanently removes stashed changes."
        echo "Alternative: 'git stash list' to review, 'git stash pop' to apply and remove."
        return 1
    fi

    # rm -rf on git directory
    if echo "$cmd" | grep -qE 'rm\s+-[a-zA-Z]*r[a-zA-Z]*f?\s+\.git\b'; then
        echo "BLOCKED: Removing .git directory destroys the entire repository history."
        return 1
    fi

    return 0
}

if ! check_destructive "$COMMAND"; then
    echo ""
    echo "To proceed anyway, ask the user to confirm the destructive operation."
    exit 2
fi

exit 0
