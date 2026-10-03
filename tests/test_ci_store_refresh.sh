#!/bin/bash
set -euo pipefail

# trace:BUG-1785 | ai:codex
# Simulates the CI job where aida-store is refreshed mid-job, but the shallow fetch
# causes unrelated histories if we try to `merge --ff-only`. We verify that `reset --hard`
# succeeds and attaches at T2.

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

REMOTE="$TMP_DIR/remote"
LOCAL="$TMP_DIR/local"

# 1. Create remote with aida-store branch
mkdir -p "$REMOTE"
cd "$REMOTE"
git init -b main
git commit --allow-empty -m "init"
git branch aida-store
git checkout aida-store
git commit --allow-empty -m "T1"
T1=$(git rev-parse HEAD)

# 2. Local fetches shallowly and creates worktree
mkdir -p "$LOCAL"
cd "$LOCAL"
git init -b main
git remote add origin "$REMOTE"
git fetch --depth 1 origin +aida-store:refs/remotes/origin/aida-store
git worktree add .aida-store refs/remotes/origin/aida-store

# Verify attached at T1
ATTACHED=$(git -C .aida-store rev-parse HEAD)
if [ "$ATTACHED" != "$T1" ]; then
    echo "Failed: not attached at T1"
    exit 1
fi

# 3. Remote advances (simulating schedule tick)
cd "$REMOTE"
git commit --allow-empty -m "T2"
T2=$(git rev-parse HEAD)

# 4. Local fetches again mid-job (shallow)
cd "$LOCAL"
git fetch --depth 1 origin +aida-store:refs/remotes/origin/aida-store

# 5. Run the refresh step (using reset --hard)
if git -C .aida-store rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git -C .aida-store reset --hard origin/aida-store
elif [ -e .aida-store ]; then
    echo "::error::.aida-store exists but is not an attached git worktree"
    exit 1
else
    git worktree add .aida-store refs/remotes/origin/aida-store
fi

# 6. Verify attached at T2
ATTACHED=$(git -C .aida-store rev-parse HEAD)
if [ "$ATTACHED" != "$T2" ]; then
    echo "Failed: not attached at T2"
    exit 1
fi

echo "OK"
