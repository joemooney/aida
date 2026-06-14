#!/bin/bash
# AIDA Git Safety Guardrails — PreToolUse hook for Claude Code
#
# Blocks destructive git operations that could cause data loss:
# - git reset --hard (discards uncommitted work)
# - git clean -f (deletes untracked files)
# - git checkout -- . (discards all changes)
# - git push --force / -f / --force-with-lease to a protected branch
#   (main/master/develop/aida-store); plain --force/-f to any branch
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

    # Force-push handling (BUG-548).
    # A force-push of ANY form — --force, -f, AND --force-with-lease — to a
    # PROTECTED branch is blocked outright. --force-with-lease is NOT safe here:
    # the lease only checks the ref you last fetched, so once you (or a sibling
    # worktree) have fetched a newer main, the lease passes and the push can
    # still clobber commits merged after that fetch (e.g. a merged PR). This is
    # exactly the 2026-06-13 incident: `git push --force-with-lease origin main`
    # off a failed `cd` dropped a merged PR from main. --force-with-lease to a
    # feature branch stays allowed — that's the legitimate post-rebase path.
    local is_force_push=0
    if echo "$cmd" | grep -qE 'git\s+push\b.*--force'; then
        is_force_push=1
    elif echo "$cmd" | grep -qE 'git\s+push\s+-[a-zA-Z]*f\b'; then
        is_force_push=1
    fi
    if [ "$is_force_push" = "1" ]; then
        # Protected = the shared branches that must only move forward via normal
        # push / merge. Match the branch as a distinct push argument or refspec
        # component (`origin main`, `HEAD:main`) — NOT as a substring of a
        # feature name (`story-610-main-fix` must not trip it).
        local protected_re='(^|[[:space:]:/])(main|master|develop|aida-store)([[:space:]]|$)'
        local target_protected=0
        if echo "$cmd" | grep -qE "$protected_re"; then
            target_protected=1
        else
            # No protected branch named — best-effort: is the current branch
            # itself protected (a bare `git push -f` / `--force-with-lease`)?
            local cur
            cur=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || true)
            if [ -n "$cur" ] && echo " $cur " | grep -qE "$protected_re"; then
                target_protected=1
            fi
        fi
        if [ "$target_protected" = "1" ]; then
            echo "BLOCKED: force-push to a protected branch (main/master/develop/aida-store)."
            echo "This includes --force-with-lease — the lease only checks the ref you last"
            echo "fetched, so it can still clobber commits merged after that fetch (a merged PR)."
            echo "Protected branches advance only via normal push / merge — never a force-push."
            return 1
        fi
        # Not a protected target: a plain --force/-f (no lease) can still
        # overwrite a feature branch's history — keep nudging toward the lease.
        if ! echo "$cmd" | grep -qF -- '--force-with-lease'; then
            echo "BLOCKED: 'git push --force' can overwrite remote history."
            echo "Alternative: 'git push --force-with-lease' is safer (checks remote hasn't changed)."
            return 1
        fi
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
