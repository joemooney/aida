#!/usr/bin/env bash
# scripts/pre-release-check.sh — gate a release on cross-platform CI.
#
# AIDA's PR CI (.github/workflows/ci.yml) is Linux-only during the alpha
# (TASK-257). Windows + macOS are validated by the nightly
# .github/workflows/cross-platform.yml workflow. Before tagging a release we
# want a *recent, green* cross-platform run so the release tarballs aren't
# shipping an untested win/mac regression.
#
# This script:
#   1. Looks at the most recent cross-platform.yml run on `main`.
#   2. If it succeeded within the last 24h, the gate passes immediately.
#   3. Otherwise it dispatches a fresh run (`gh workflow run`) and blocks on
#      its completion (`gh run watch --exit-status`).
#
# Exit 0      = cross-platform CI is green and recent; safe to tag.
# Exit non-0  = no green run / the dispatched run failed; do not tag.
#
# Usage:
#   scripts/pre-release-check.sh            # reuse a <24h green run if present
#   scripts/pre-release-check.sh --refresh  # always dispatch a fresh run
#
# Called by scripts/release.sh before the tag step (opt out there with
# --skip-xplat-check or AIDA_SKIP_XPLAT_CHECK=1).
#
# trace:TASK-257 | ai:claude

WORKFLOW="cross-platform.yml"
MAX_AGE_HOURS=24
refresh=0

# parse_iso_to_epoch <iso8601-timestamp> — echo Unix epoch seconds.
#
# Portable between GNU date (Linux: `date -d <str>`) and BSD date (macOS:
# `date -j -u -f <fmt> <str>`). GitHub's createdAt timestamps are ISO 8601
# UTC ("2026-05-19T12:34:56Z"); the `-u` on the BSD branch keeps the parse
# in UTC so the trailing `Z` is honoured. Echoes `0` if neither form parses
# the input — callers MUST treat `0` as "unknown age" and not as 1970.
#
# Why both branches: `date -d` is a GNU extension absent from BSD coreutils,
# so on macOS it fails silently and `created_epoch` would always be 0,
# defeating the freshness gate. trace:TASK-284 | ai:claude
parse_iso_to_epoch() {
    local iso="$1" out
    if out=$(date -d "$iso" +%s 2>/dev/null); then
        echo "$out"
        return 0
    fi
    if out=$(date -j -u -f "%Y-%m-%dT%H:%M:%SZ" "$iso" +%s 2>/dev/null); then
        echo "$out"
        return 0
    fi
    echo 0
}

# Allow `source pre-release-check.sh` from tests without running the main
# flow. Direct invocation (`./pre-release-check.sh` or `bash …`) proceeds
# as normal.
if [ "${BASH_SOURCE[0]}" != "${0}" ]; then
    return 0
fi

set -euo pipefail

for arg in "$@"; do
    case "$arg" in
        --refresh) refresh=1 ;;
        -h|--help)
            echo "usage: $0 [--refresh]"
            echo "  --refresh   dispatch a fresh cross-platform run even if a recent green one exists"
            exit 0
            ;;
        *)
            echo "error: unknown argument '$arg'" >&2
            exit 1
            ;;
    esac
done

if ! command -v gh >/dev/null 2>&1; then
    echo "error: 'gh' (GitHub CLI) is required for the cross-platform pre-release check." >&2
    echo "       Install it (https://cli.github.com) or skip with AIDA_SKIP_XPLAT_CHECK=1." >&2
    exit 1
fi

watch_run() {
    # $1 = run databaseId
    echo "→ watching run $1 ..."
    if gh run watch "$1" --exit-status; then
        echo "✓ cross-platform CI passed (run $1)."
        return 0
    fi
    echo "✗ cross-platform CI failed (run $1)." >&2
    echo "  Inspect: gh run view $1 --web" >&2
    return 1
}

dispatch_and_watch() {
    # Snapshot the most recent workflow_dispatch run id BEFORE dispatching, so
    # we can distinguish "my new run" from "a stale previous run." Without
    # this, the post-dispatch poll returns a days-old run during the few-second
    # registration window and `gh run watch` reports a stale conclusion —
    # silently bypassing the freshness gate this script exists to enforce.
    # trace:STORY-254 | ai:claude
    local prev_id
    prev_id=$(gh run list --workflow="$WORKFLOW" --branch main --event workflow_dispatch \
                --limit 1 --json databaseId -q '.[0].databaseId' 2>/dev/null || true)

    echo "→ dispatching $WORKFLOW on main ..."
    gh workflow run "$WORKFLOW" --ref main

    # Poll for a run whose id differs from the snapshot — that's the one we
    # just dispatched. The dispatched run takes a few seconds to register.
    local id=""
    for _ in $(seq 1 15); do
        sleep 4
        local candidate
        candidate=$(gh run list --workflow="$WORKFLOW" --branch main --event workflow_dispatch \
                --limit 1 --json databaseId -q '.[0].databaseId' 2>/dev/null || true)
        if [ -n "$candidate" ] && [ "$candidate" != "$prev_id" ]; then
            id="$candidate"
            break
        fi
    done
    if [ -z "$id" ]; then
        echo "error: dispatched the workflow but could not locate the new run." >&2
        echo "       Check 'gh run list --workflow=$WORKFLOW' manually." >&2
        exit 1
    fi
    watch_run "$id"
}

if [ "$refresh" = "1" ]; then
    dispatch_and_watch
    exit $?
fi

# Inspect the most recent cross-platform run on main. When there are no
# runs, `gh` emits `[]`; `.[0]` yields null which `@tsv` renders as a row
# of empty/null fields (not an empty string). The `[ -z "$line" ]` check
# below only catches the truly-empty case (`gh` errored, `|| true`
# swallowed it), treated as "no prior run". trace:TASK-283 | ai:claude
line=$(gh run list --workflow="$WORKFLOW" --branch main --limit 1 \
        --json status,conclusion,createdAt,databaseId,url \
        -q '.[0] | [.status, .conclusion, .createdAt, (.databaseId|tostring), .url] | @tsv' \
        2>/dev/null || true)

if [ -z "$line" ]; then
    echo "→ no prior cross-platform run found."
    dispatch_and_watch
    exit $?
fi

IFS=$'\t' read -r status conclusion created_at run_id run_url <<<"$line"

if [ "$status" = "completed" ] && [ "$conclusion" = "success" ]; then
    created_epoch=$(parse_iso_to_epoch "$created_at")
    now_epoch=$(date +%s)
    if [ "$created_epoch" -ne 0 ]; then
        age_hours=$(( (now_epoch - created_epoch) / 3600 ))
        if [ "$age_hours" -lt "$MAX_AGE_HOURS" ]; then
            echo "✓ cross-platform CI is green and recent (${age_hours}h old, run $run_id)."
            echo "  $run_url"
            exit 0
        fi
        echo "→ last cross-platform run is green but stale (${age_hours}h old, >${MAX_AGE_HOURS}h)."
    else
        echo "→ last cross-platform run is green but its age could not be parsed; refreshing."
    fi
else
    echo "→ last cross-platform run is not green (status=$status conclusion=${conclusion:-none})."
fi

dispatch_and_watch
exit $?
