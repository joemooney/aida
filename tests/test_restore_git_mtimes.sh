#!/bin/sh
set -eu

# Regression for TASK-1274: warm GitLab checkouts must normalize both blobs and
# trees, because Cargo's recursive rerun-if-changed watches directory mtimes.
repo_root=$(git rev-parse --show-toplevel)
fixture=$(mktemp -d)
trap 'chmod -R u+w "$fixture" 2>/dev/null || true; rm -rf "$fixture"' EXIT

git -C "$fixture" init -q
git -C "$fixture" config user.name test
git -C "$fixture" config user.email test@example.invalid
mkdir -p "$fixture/templates/nested"
printf 'one\n' > "$fixture/templates/nested/input.txt"
git -C "$fixture" add .
git -C "$fixture" commit -qm initial

(
    cd "$fixture"
    "$repo_root/ci/restore-git-mtimes"
)
file_before=$(stat -c %Y "$fixture/templates/nested/input.txt")
nested_before=$(stat -c %Y "$fixture/templates/nested")
templates_before=$(stat -c %Y "$fixture/templates")

touch "$fixture/templates/nested/input.txt" "$fixture/templates/nested" "$fixture/templates"
(
    cd "$fixture"
    "$repo_root/ci/restore-git-mtimes"
)

test "$(stat -c %Y "$fixture/templates/nested/input.txt")" = "$file_before"
test "$(stat -c %Y "$fixture/templates/nested")" = "$nested_before"
test "$(stat -c %Y "$fixture/templates")" = "$templates_before"

printf 'two\n' > "$fixture/templates/nested/input.txt"
git -C "$fixture" add .
git -C "$fixture" commit -qm changed
(
    cd "$fixture"
    "$repo_root/ci/restore-git-mtimes"
)

test "$(stat -c %Y "$fixture/templates/nested/input.txt")" != "$file_before"
test "$(stat -c %Y "$fixture/templates/nested")" != "$nested_before"
test "$(stat -c %Y "$fixture/templates")" != "$templates_before"

echo "restore-git-mtimes: blob and tree mtimes are content-stable"
