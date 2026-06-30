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

# The repo's default branch (resolved dynamically — covers a repo whose default
# is not "main"). Empty when it can't be resolved; the static set in
# is_protected_branch still covers the common names so protection never depends
# on this lookup succeeding.
DEFAULT_BRANCH=$(git symbolic-ref --short refs/remotes/origin/HEAD 2>/dev/null || true)
DEFAULT_BRANCH=${DEFAULT_BRANCH#origin/}

# Is a branch name a PROTECTED branch whose force-push must be blocked?
# Protected = main, master, develop, aida-store, AND the repo's actual default
# branch. The match is on the EXACT branch name (callers pass a parsed target
# ref), never a substring — so a feature like `story-610-main-fix` is NOT
# protected. trace:BUG-661
is_protected_branch() {
    local b="${1#refs/heads/}"
    case "$b" in
        main|master|develop|aida-store) return 0 ;;
    esac
    if [ -n "$DEFAULT_BRANCH" ] && [ "$b" = "$DEFAULT_BRANCH" ]; then
        return 0
    fi
    return 1
}

# Resolve the destination branch(es) of ONE `git push ...` segment.
# Echoes one target branch per line, or the sentinel "__FAILCLOSED__" when the
# target cannot be reliably determined (the caller treats that as protected, so
# an unparseable push fails CLOSED / is blocked). Handles: explicit
# `origin <branch>`, `origin HEAD:<branch>`, `origin <src>:<dst>`, a leading `+`
# force marker on a refspec, a `-C <dir>` worktree override, and the
# implicit/no-refspec case (resolves the real current branch of the TARGETED
# working tree, not the CWD's HEAD blindly). trace:BUG-661
resolve_one_push_segment() {
    local seg="$1"
    local -a toks=()
    read -ra toks <<< "$seg"   # word-split without glob expansion
    local n=${#toks[@]}
    local i=1                  # toks[0] is "git"
    local cdir=""
    local subcmd_found=0
    # Walk git's pre-subcommand global options until we reach `push`, capturing
    # a `-C <dir>` so a bare push resolves HEAD in the right worktree.
    while [ "$i" -lt "$n" ]; do
        local t="${toks[$i]}"
        case "$t" in
            push) subcmd_found=1; i=$((i + 1)); break ;;
            -C) cdir="${toks[$((i + 1))]:-}"; i=$((i + 2)) ;;
            -c|--git-dir|--work-tree|--namespace) i=$((i + 2)) ;;
            *) i=$((i + 1)) ;;
        esac
    done
    if [ "$subcmd_found" = "0" ]; then
        echo "__FAILCLOSED__"; return
    fi
    # Remaining tokens are the push arguments: first non-flag = the remote, the
    # rest are refspecs.
    local remote_seen=0
    local found_refspec=0
    local out=""
    while [ "$i" -lt "$n" ]; do
        local t="${toks[$i]}"
        i=$((i + 1))
        case "$t" in
            -*) continue ;;   # flag (incl. --force-with-lease=<ref>); skip
        esac
        if [ "$remote_seen" = "0" ]; then
            remote_seen=1; continue   # the remote name (e.g. origin)
        fi
        found_refspec=1
        t="${t#+}"                    # drop a leading '+' force marker
        local dst
        if [ "${t#*:}" != "$t" ]; then
            dst="${t##*:}"            # destination = part after the last ':'
        else
            dst="$t"
        fi
        dst="${dst#refs/heads/}"
        out="${out}${dst}"$'\n'
    done
    if [ "$found_refspec" = "1" ]; then
        printf '%s' "$out"
        return
    fi
    # No refspec → push the current branch of the targeted tree.
    # symbolic-ref --short returns the branch name, or empty + non-zero on a
    # detached HEAD (or any unknown state) → fail closed.
    local cur
    if [ -n "$cdir" ]; then
        cur=$(git -C "$cdir" symbolic-ref -q --short HEAD 2>/dev/null || true)
    else
        cur=$(git symbolic-ref -q --short HEAD 2>/dev/null || true)
    fi
    if [ -z "$cur" ]; then
        echo "__FAILCLOSED__"; return   # detached / unknown → fail closed
    fi
    printf '%s\n' "$cur"
}

# Resolve the destination branch(es) across ALL push segments in a command.
# Echoes every target (one per line); echoes "__FAILCLOSED__" if any segment is
# unparseable or no push is found. A chained command
# (`a && git push --force origin main`) is split on shell separators so a
# force-push to a protected branch anywhere in the command is still caught.
# trace:BUG-661
resolve_push_targets() {
    local cmd="$1"
    local segs
    segs=$(printf '%s' "$cmd" | grep -oE 'git[^&|;]*\bpush\b[^&|;]*' || true)
    if [ -z "$segs" ]; then
        echo "__FAILCLOSED__"; return
    fi
    local seg
    while IFS= read -r seg; do
        [ -z "$seg" ] && continue
        resolve_one_push_segment "$seg"
    done <<EOF
$segs
EOF
}

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

    # Force-push handling (BUG-548, BUG-661).
    # A force-push of ANY form — --force, -f, AND --force-with-lease — to a
    # PROTECTED branch is blocked outright. --force-with-lease is NOT safe here:
    # the lease only checks the ref you last fetched, so once you (or a sibling
    # worktree) have fetched a newer main, the lease passes and the push can
    # still clobber commits merged after that fetch (e.g. a merged PR). This is
    # exactly the 2026-06-13 incident: `git push --force-with-lease origin main`
    # off a failed `cd` dropped a merged PR from main. --force-with-lease to a
    # feature branch stays allowed — that's the legitimate post-rebase path.
    # BUG-552: match the short `-f` ANYWHERE in the push command, not just
    # immediately after `push` — `git push origin main -f` (trailing flag) must
    # be caught too. The `\s` before the dash keeps a branch name containing a
    # hyphen (`feat-f`) from tripping it (a flag is always space-separated).
    # The `[^&|;]*` between `git` and `push` tolerates global options
    # (`git -C <dir> push ...`) without spanning a shell separator into a
    # neighbouring command. A leading `+` on a refspec (`git push origin +main`)
    # is also a force-push and is caught so it can't sneak past as a protected
    # target.
    local is_force_push=0
    if echo "$cmd" | grep -qE 'git[^&|;]*\bpush\b[^&|;]*--force'; then
        is_force_push=1
    elif echo "$cmd" | grep -qE 'git[^&|;]*\bpush\b[^&|;]*\s-[a-zA-Z]*f\b'; then
        is_force_push=1
    elif echo "$cmd" | grep -qE 'git[^&|;]*\bpush\b[^&|;]*\s\+[A-Za-z0-9_./-]'; then
        is_force_push=1
    fi
    if [ "$is_force_push" = "1" ]; then
        # BUG-661: decide protected-ness from the ACTUAL PUSH TARGET, not the
        # CWD's current branch. An agent sitting on `main` in the primary
        # checkout that force-pushes a FEATURE branch living in a worktree used
        # to trip the old "is the CWD HEAD protected?" heuristic. We now parse
        # the destination ref(s) from the push invocation and block only when a
        # TARGET is a protected branch; an unparseable target fails CLOSED.
        local targets
        targets=$(resolve_push_targets "$cmd")
        local target_protected=0
        if printf '%s\n' "$targets" | grep -q '__FAILCLOSED__'; then
            target_protected=1   # could not parse the target → fail closed
        else
            local _t
            while IFS= read -r _t; do
                [ -z "$_t" ] && continue
                if is_protected_branch "$_t"; then
                    target_protected=1
                fi
            done <<EOF
$targets
EOF
        fi
        if [ "$target_protected" = "1" ]; then
            echo "BLOCKED: force-push to a protected branch (main/master/develop/aida-store or the repo default)."
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
