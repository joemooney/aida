#!/usr/bin/env bash
# tests/test_release_prepare_only.sh — static regression for STORY-1125.
# `scripts/release.sh --prepare-only` is the queueable release-prep boundary:
# it may leave a prepared dirty tree, but it must exit before release commit,
# tag, or push. trace:STORY-1125 | ai:codex

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$ROOT/scripts/release.sh"

echo "=== STORY-1125: release prepare-only boundary ==="

help="$("$SCRIPT" --help)"
case "$help" in
    *"--prepare-only"*) ;;
    *) echo "FAIL: release help does not advertise --prepare-only" >&2; exit 1 ;;
esac

prepare_line=$(grep -n 'if \[ "$prepare_only" = "1" \]; then' "$SCRIPT" | head -n1 | cut -d: -f1)
commit_line=$(grep -n '^git commit -m "chore: release v\$new"' "$SCRIPT" | head -n1 | cut -d: -f1)
tag_line=$(grep -n '^git tag -a "v\$new"' "$SCRIPT" | head -n1 | cut -d: -f1)
push_line=$(grep -n '^git push origin HEAD' "$SCRIPT" | head -n1 | cut -d: -f1)

if [ -z "$prepare_line" ] || [ -z "$commit_line" ] || [ -z "$tag_line" ] || [ -z "$push_line" ]; then
    echo "FAIL: could not locate prepare-only or publish boundary lines" >&2
    exit 1
fi

if [ "$prepare_line" -ge "$commit_line" ] || [ "$prepare_line" -ge "$tag_line" ] || [ "$prepare_line" -ge "$push_line" ]; then
    echo "FAIL: --prepare-only check must occur before commit/tag/push" >&2
    exit 1
fi

prepare_block=$(sed -n "${prepare_line},${commit_line}p" "$SCRIPT")
case "$prepare_block" in
    *"exit 0"*) ;;
    *) echo "FAIL: --prepare-only block must exit before publish commands" >&2; exit 1 ;;
esac

echo "PASS: --prepare-only is advertised and exits before commit/tag/push"
