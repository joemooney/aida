#!/usr/bin/env bash
# TASK-509: commit-msg hook accepts multi-agent [AI:tool1+tool2] attribution.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOK="$PROJECT_ROOT/aida-core/templates/hooks/aida-commit-msg"
TEST_DIR=$(mktemp -d)

cleanup() { rm -rf "$TEST_DIR"; }
trap cleanup EXIT

cd "$TEST_DIR"
git init -q -b main
git config user.email "test@example.com"
git config user.name "TASK-509 test"

cat > traced.rs <<'EOF'
// trace:TASK-509 | ai:codex
fn main() {}
EOF
git add traced.rs

run_hook() {
    local subject="$1"
    local msg_file="$TEST_DIR/.git/COMMIT_EDITMSG"
    printf '%s\n' "$subject" > "$msg_file"
    AIDA_COMMIT_STRICT=true bash "$HOOK" "$msg_file" 2>&1
}

accepted=(
    "[AI:claude] feat(hooks): accept single agent (TASK-509)"
    "[AI:antigravity] fix(hooks): accept named agent (TASK-509)"
    "[AI:antigravity+claude] test(hooks): accept two agents (TASK-509)"
    "[AI:codex+antigravity+claude] test(hooks): accept three agents (TASK-509)"
    "[AI:claude:med] fix(hooks): accept confidence (TASK-509)"
    "[AI:antigravity+claude:med] fix(hooks): accept multi-agent confidence (TASK-509)"
)

for subject in "${accepted[@]}"; do
    output=$(run_hook "$subject")
    if grep -qE 'Errors:|Warnings:' <<<"$output"; then
        echo "expected acceptance for: $subject"
        echo "$output"
        exit 1
    fi
done

rejected=(
    "[AI:+claude] fix(hooks): reject leading separator (TASK-509)"
    "[AI:claude+] fix(hooks): reject trailing separator (TASK-509)"
    "[AI:claude++codex] fix(hooks): reject empty segment (TASK-509)"
    "[AI:claude+codex:] fix(hooks): reject empty confidence (TASK-509)"
    "[AI:claude+codex:maybe] fix(hooks): reject invalid confidence (TASK-509)"
)

for subject in "${rejected[@]}"; do
    output=$(run_hook "$subject" || true)
    if ! grep -q 'conventional format' <<<"$output"; then
        echo "expected conventional-format rejection for: $subject"
        echo "$output"
        exit 1
    fi
done

echo "TASK-509 multi-agent commit tag tests passed."
