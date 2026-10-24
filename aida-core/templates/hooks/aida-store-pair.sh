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

COMMON_DIR=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)
MAIN_ROOT=""
[ -n "$COMMON_DIR" ] && MAIN_ROOT=$(dirname "$COMMON_DIR")

# BUG-2117: the store is resolved only from git's own common directory (the
# main checkout), never from a path under this commit's toplevel, whose tree
# a sandboxed session may write: a planted nested `.aida-store/.git` would
# otherwise forge the trailer. A commit in the store itself pins the store's
# own HEAD, read from the repository git is already committing in.
# trace:TASK-1952 | trace:BUG-2117 | ai:claude
[ -z "$MAIN_ROOT" ] && exit 0
case "$PROJECT_ROOT" in
    "$MAIN_ROOT/.aida-store"|"$MAIN_ROOT/aida-store"|*/.aida-store)
        STORE_SHA=$(git rev-parse -q --verify HEAD 2>/dev/null) || exit 0
        ;;
    *)
        # TASK-2127: with --separate-git-dir or a submodule, the common
        # directory's parent is not the checkout. Fall back to what git itself
        # records, never to a path under a linked worktree's tree: a commit in
        # the main worktree (git dir == common dir) runs in the checkout, so
        # its toplevel is the main root; a linked worktree reads core.worktree
        # from the common directory's config (set for submodules). A linked
        # worktree of a --separate-git-dir or bare repository records no main
        # checkout, so its commits carry no trailer.
        # trace:TASK-1952 | trace:TASK-2127 | ai:claude
        LISTED_MAIN=""
        GIT_DIR_ABS=$(git rev-parse --path-format=absolute --git-dir 2>/dev/null)
        if [ "$GIT_DIR_ABS" = "$COMMON_DIR" ]; then
            LISTED_MAIN="$PROJECT_ROOT"
        else
            CORE_WORKTREE=$(git config --file "$COMMON_DIR/config" --get core.worktree 2>/dev/null)
            if [ -n "$CORE_WORKTREE" ]; then
                LISTED_MAIN=$(cd "$COMMON_DIR" 2>/dev/null && cd "$CORE_WORKTREE" 2>/dev/null && pwd -P)
            fi
            case "$LISTED_MAIN" in
                "$PROJECT_ROOT"|"$PROJECT_ROOT"/*) LISTED_MAIN="" ;;
            esac
        fi
        STORE_PATH=""
        for candidate in "$MAIN_ROOT/.aida-store" "$MAIN_ROOT/aida-store" \
            ${LISTED_MAIN:+"$LISTED_MAIN/.aida-store" "$LISTED_MAIN/aida-store"}; do
            if [ -d "$candidate/.git" ] || [ -f "$candidate/.git" ]; then
                STORE_PATH="$candidate"
                break
            fi
        done
        [ -z "$STORE_PATH" ] && exit 0
        STORE_SHA=$(env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE git -C "$STORE_PATH" rev-parse HEAD 2>/dev/null) || exit 0
        ;;
esac
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
