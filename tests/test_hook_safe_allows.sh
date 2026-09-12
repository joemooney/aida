#!/bin/bash
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HOOK_DIR="$PROJECT_ROOT/aida-core/templates/hooks"

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

require_exit() {
    local expected="$1"
    local label="$2"
    local hook="$3"
    local payload="$4"
    shift 4

    local out status
    set +e
    out=$(printf '%s\n' "$payload" | "$@" "$hook" 2>&1)
    status=$?
    set -e

    if [ "$status" -ne "$expected" ]; then
        printf '%s\n' "$out" >&2
        fail "$label exited $status, expected $expected"
    fi
    if [ "$status" -ne 0 ] && [ -z "$out" ]; then
        fail "$label exited non-zero without a diagnostic"
    fi
}

command -v jq >/dev/null 2>&1 || fail "jq is required for hook template tests"

# The reported safe one-file restore must not be confused with `git checkout -- .`.
# trace:BUG-1092 | ai:codex
safe_checkout='{"tool_input":{"command":"git checkout -- .claude/scheduled_tasks.lock"},"command":"git checkout -- .claude/scheduled_tasks.lock"}'
require_exit 0 "safe dotted-path checkout" "$HOOK_DIR/aida-git-guardrails.sh" "$safe_checkout" bash

destructive_checkout='{"tool_input":{"command":"git checkout -- ."},"command":"git checkout -- ."}'
require_exit 2 "destructive checkout" "$HOOK_DIR/aida-git-guardrails.sh" "$destructive_checkout" bash

# Safe no-op payloads for every scaffolded Claude/Codex hook that uses
# `set -euo pipefail`; grep no-matches and command substitutions must not turn
# an allow path into a silent block.
safe_bash='{"tool_input":{"command":"git status --short"},"command":"git status --short","tool_response":""}'
require_exit 0 "validate-commit safe command" "$HOOK_DIR/aida-validate-commit.sh" "$safe_bash" bash
require_exit 0 "track-commits safe command" "$HOOK_DIR/aida-track-commits.sh" "$safe_bash" bash

safe_edit='{"file_path":"src/main.rs","session_id":"bug-1092"}'
require_exit 0 "advisor-code-guard implementer edit" "$HOOK_DIR/aida-advisor-code-guard.sh" "$safe_edit" env AIDA_SESSION_ROLE=implementer bash

echo "hook safe allow regression tests passed"
