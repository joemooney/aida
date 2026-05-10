#!/bin/bash
# AIDA prepare-commit-msg hook — pin the orphan-store HEAD SHA into
# every code commit's trailer. Lets `aida store status` (and future
# time-travel commands) align code commits with the store version they
# were written against.
# trace:EPIC-21 | ai:claude

# git invokes prepare-commit-msg with: $1 = path-to-msg, $2 = source
# (message|template|merge|squash|commit), $3 = sha (for amend/squash).
COMMIT_MSG_FILE="$1"
COMMIT_SOURCE="$2"

# Skip non-message commits (merges, amends with -C, etc.) — those
# inherit trailers from the source commit. Skip squashes for the same
# reason.
case "${COMMIT_SOURCE}" in
    merge|squash|commit) exit 0 ;;
esac

# Find the orphan-store worktree. By convention it's at .aida-store/
# (or aida-store/ for sibling-mode projects). Quietly exit if we can't
# find one — degrade gracefully so the hook never fails a commit.
PROJECT_ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || exit 0
[ -z "$PROJECT_ROOT" ] && exit 0

STORE_PATH=""
for candidate in "$PROJECT_ROOT/.aida-store" "$PROJECT_ROOT/aida-store"; do
    if [ -d "$candidate/.git" ] || [ -f "$candidate/.git" ]; then
        STORE_PATH="$candidate"
        break
    fi
done
[ -z "$STORE_PATH" ] && exit 0

STORE_SHA=$(git -C "$STORE_PATH" rev-parse HEAD 2>/dev/null) || exit 0
[ -z "$STORE_SHA" ] && exit 0

# Skip if the trailer is already there (handles re-edit of an in-progress
# commit message).
if git interpret-trailers --parse "$COMMIT_MSG_FILE" 2>/dev/null \
    | grep -qE '^Aida-Store: '; then
    exit 0
fi

# Append the trailer. `git interpret-trailers` handles the blank-line
# separation between body and trailers correctly.
NEW_MSG=$(git interpret-trailers \
    --trailer "Aida-Store: $STORE_SHA" \
    "$COMMIT_MSG_FILE" 2>/dev/null) || exit 0

if [ -n "$NEW_MSG" ]; then
    printf '%s\n' "$NEW_MSG" > "$COMMIT_MSG_FILE"
fi

exit 0
