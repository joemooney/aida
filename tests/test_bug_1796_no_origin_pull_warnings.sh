#!/usr/bin/env bash
# Exercise the real `aida pull` no-origin paths in a freshly initialized repo.
# trace:BUG-1796 | ai:codex
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
AIDA="$TARGET_DIR/debug/aida"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bug-1796-no-origin-XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
export AIDA_OUTPUT_FORMAT=human
export NO_COLOR=1

(cd "$ROOT" && cargo build -p aida-cli --quiet)

project="$tmp/project"
mkdir -p "$project"
git -C "$project" init --quiet
git -C "$project" config user.email test@example.com
git -C "$project" config user.name 'BUG-1796 test'
git -C "$project" commit --quiet --allow-empty -m 'test: initialize project'

(cd "$project" && "$AIDA" init --no-skills --no-hooks --no-post-hooks \
    --no-roles --no-agent-config --no-schedule --footprint minimal >/dev/null)

if output="$(cd "$project" && "$AIDA" pull 2>&1)"; then
    :
else
    status=$?
    printf '%s\n' "$output" >&2
    printf 'aida pull unexpectedly exited %s\n' "$status" >&2
    exit 1
fi

for expected in \
    'Warning: no `origin` remote — skipping code pull' \
    'Warning: orphan store has no `origin` — skipping store pull'; do
    if ! grep -Fq "$expected" <<<"$output"; then
        printf '%s\n' "$output" >&2
        printf 'missing real pull warning: %s\n' "$expected" >&2
        exit 1
    fi
done

echo 'BUG-1796: real no-origin code and store pull paths both report Warning.'
