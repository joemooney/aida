#!/bin/sh
# Informational tool/invocation context for Claude, Codex, and Antigravity tool-only sessions.
# trace:BUG-2014 | ai:codex
# trace:TASK-1744 | ai:antigravity
client=""
if [ "${1:-}" = "--client" ] && [ -n "${2:-}" ]; then
    client="$2"
fi
# Antigravity CLI 1.4.2 PreInvocation hook contract (documented in
# `~/.gemini/antigravity-cli/builtin/skills/agy-customizations/docs/hooks.md`,
# sections "2. `PostToolUse` Contract" and "4. `PreInvocation` Contract"):
# stdout must be a JSON object; `{}` is a no-op and
# `{"injectSteps":[{"ephemeralMessage":"..."}]}` injects an ephemeral notice
# before the model invocation without a permission decision.
empty_out() {
    case "$client" in
        agy|antigravity|antigravity-cli) printf '{}\n' ;;
    esac
}
# No grant means zero CLI processes, including on the common implementer path.
if [ -z "${AIDA_SESSION_GRANT:-}" ]; then
    empty_out
    exit 0
fi
# Linux coreutils timeout is mandatory; never fall back to an unbounded call.
if ! command -v timeout >/dev/null 2>&1; then
    echo 'Expiry notice unavailable: timeout command missing' >&2
    empty_out
    exit 0
fi
# Capture stdout so a failed/timed-out command cannot leak partial context.
# The CLI emits a single diagnostic on ledger errors and returns success.
if [ -n "$client" ]; then
    notice=$(timeout -k 0.1s 1.8s aida role expiry-notice --client "$client")
else
    notice=$(timeout -k 0.1s 1.8s aida role expiry-notice)
fi
status=$?
if [ "$status" -eq 0 ]; then
    if [ -n "$notice" ]; then
        printf '%s\n' "$notice"
    else
        empty_out
    fi
else
    echo 'Expiry notice unavailable: notice command failed or timed out' >&2
    empty_out
fi
exit 0
