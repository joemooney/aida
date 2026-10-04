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

bare_code="$tmp/remote.git"
git init --bare --initial-branch=main "$bare_code" >/dev/null
project="$tmp/project"
git clone "$bare_code" "$project" >/dev/null 2>&1
git -C "$project" config user.email test@example.com
git -C "$project" config user.name 'BUG-1796 test'
git -C "$project" commit --quiet --allow-empty -m 'test: initialize project'
git -C "$project" push origin main >/dev/null 2>&1

(cd "$project" && "$AIDA" init --no-skills --no-hooks --no-post-hooks --no-roles --no-agent-config --no-schedule --footprint minimal >/dev/null)

# `aida init` already creates the canonical local `.aida-store` worktree and
# configures its `origin` to the project's code remote. Publish the orphan
# branch there too, matching a real AIDA remote with both refs present. Use a
# relative remote path plus a symlink in the store worktree so any Git fetch
# header uses a stable `../remote` path instead of the mktemp directory.
ln -s "$bare_code" "$project/remote.git"
git -C "$project" remote set-url origin ../remote.git
git -C "$project/.aida-store" config user.email test@example.com
git -C "$project/.aida-store" config user.name 'BUG-1796 test'
git -C "$project/.aida-store" add -A
git -C "$project/.aida-store" commit --quiet -m 'test: initialize store'
git -C "$project/.aida-store" push origin aida-store >/dev/null 2>&1

actual="$tmp/actual-output"
if (cd "$project" && "$AIDA" pull >"$actual" 2>&1); then
    :
else
    status=$?
    cat "$actual" >&2
    printf 'aida pull unexpectedly exited %s\n' "$status" >&2
    exit 1
fi

expected="$tmp/expected-output"
cat >"$expected" <<'EXPECTED'
Pulling code main ← origin
From ../remote
 * branch            main       -> FETCH_HEAD
Already up to date.
  code pull complete
  Note: Already up to date — origin had no new commits.
  Committed pending orphan changes before pull
Pulling store aida-store ← origin
  store pull complete
  (no new commits)
EXPECTED

if ! cmp -s "$expected" "$actual"; then
    printf 'output mismatch! Full output differs from the legacy bytes.\n' >&2
    diff -u "$expected" "$actual" >&2 || true
    exit 1
fi

echo "BUG-1796: normal two-remote pull output is byte-identical to legacy."
