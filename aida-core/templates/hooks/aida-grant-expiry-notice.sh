#!/bin/sh
# Informational PreToolUse context for tool-only Claude sessions.
# trace:BUG-2014 | ai:codex
# No grant means zero CLI processes, including on the common implementer path.
[ -n "${AIDA_SESSION_GRANT:-}" ] || exit 0
# Linux coreutils timeout is mandatory; never fall back to an unbounded call.
if ! command -v timeout >/dev/null 2>&1; then
    echo 'Expiry notice unavailable: timeout command missing' >&2
    exit 0
fi
# Capture stdout so a failed/timed-out command cannot leak partial context.
# The CLI emits a single diagnostic on ledger errors and returns success.
notice=$(timeout -k 0.1s 1.8s aida role expiry-notice)
status=$?
if [ "$status" -eq 0 ]; then
    [ -z "$notice" ] || printf '%s\n' "$notice"
else
    echo 'Expiry notice unavailable: notice command failed or timed out' >&2
fi
exit 0
