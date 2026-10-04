#!/usr/bin/env bash
# Exercise a normal two-remote pull and verify its output byte-for-byte against the pre-BUG-1796 state.
# trace:BUG-1796 | ai:codex
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
AIDA="$TARGET_DIR/debug/aida"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bug-1796-normal-pull-XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
export AIDA_OUTPUT_FORMAT=human
export NO_COLOR=1

(cd "$ROOT" && cargo build -p aida-cli --quiet)

bare_code="$tmp/code.git"
git init --bare --initial-branch=main "$bare_code" >/dev/null
bare_store="$tmp/store.git"
git init --bare --initial-branch=aida-store "$bare_store" >/dev/null

project="$tmp/project"
git clone "$bare_code" "$project" >/dev/null 2>&1
git -C "$project" config user.email test@example.com
git -C "$project" config user.name 'BUG-1796 test'
git -C "$project" commit --quiet --allow-empty -m 'test: initialize project'
git -C "$project" push origin main >/dev/null 2>&1

(cd "$project" && "$AIDA" init --no-skills --no-hooks --no-post-hooks --no-roles --no-agent-config --no-schedule --footprint minimal >/dev/null)

# Store init
git clone "$bare_store" "$project/.aida-store" >/dev/null 2>&1
git -C "$project/.aida-store" checkout -b aida-store >/dev/null 2>&1
git -C "$project/.aida-store" config user.email test@example.com
git -C "$project/.aida-store" config user.name 'BUG-1796 test'
git -C "$project/.aida-store" commit --quiet --allow-empty -m 'test: init store'
git -C "$project/.aida-store" push origin aida-store >/dev/null 2>&1

output="$(cd "$project" && "$AIDA" pull 2>&1)" || {
    status=$?
    printf '%s\n' "$output" >&2
    printf 'aida pull unexpectedly exited %s\n' "$status" >&2
    exit 1
}

expected="Pulling code main ← origin
Already up to date.
Pulling store aida-store ← origin
Already up to date."

if [ "$output" != "$expected" ]; then
    printf 'output mismatch!\nExpected:\n%s\nGot:\n%s\n' "$expected" "$output" >&2
    exit 1
fi

echo "BUG-1796: normal two-remote pull output is byte-identical to legacy."
