#!/bin/bash
set -euo pipefail

# // trace:BUG-1810 | ai:antigravity

echo "Setting up isolated test environment..."
TEST_DIR=$(mktemp -d)
trap 'rm -rf "$TEST_DIR"' EXIT

cd "$TEST_DIR"

git init main-repo
cd main-repo
git config user.name "Test User"
git config user.email "test@example.com"
# Remove the custom core.hooksPath, git handles worktree hooks natively via common dir.

echo "Initial" > README.md
git add README.md
git commit -m "Initial commit"
CODE_HEAD=$(git rev-parse HEAD)

git worktree add .aida-store -b aida-store
cd .aida-store
echo "Store" > store.txt
git add store.txt
git commit -m "Store initial"
STORE_HEAD=$(git rev-parse HEAD)
cd ..

cp /home/joe/ai/aida-worktrees/aida-bug-1810/aida-core/templates/hooks/aida-store-pair.sh .git/hooks/prepare-commit-msg
chmod +x .git/hooks/prepare-commit-msg

git worktree add ../linked-worktree -b linked-branch
cd ../linked-worktree
echo "Linked edit" > README.md
git commit -am "Linked commit"

LINKED_COMMIT=$(git rev-parse HEAD)
LINKED_TRAILER=$(git log -1 --format=%B | grep "Aida-Store: " | sed 's/Aida-Store: //' || true)

echo "CODE_HEAD: $CODE_HEAD"
echo "STORE_HEAD: $STORE_HEAD"
echo "LINKED_TRAILER: $LINKED_TRAILER"

if [ -z "$LINKED_TRAILER" ]; then
    echo "FAIL: Trailer is missing"
    exit 1
fi

if [ "$LINKED_TRAILER" = "$CODE_HEAD" ] || [ "$LINKED_TRAILER" = "$LINKED_COMMIT" ]; then
    echo "FAIL: Trailer is code HEAD, not store HEAD"
    exit 1
fi

if [ "$LINKED_TRAILER" != "$STORE_HEAD" ]; then
    echo "FAIL: Trailer $LINKED_TRAILER != $STORE_HEAD"
    exit 1
fi

echo "PASS: Trailer correctly matches store HEAD!"
