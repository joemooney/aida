#!/bin/bash
# Pattern coverage for the warn-only cargo time-budget PreToolUse hook.
# trace:STORY-1854 | ai:claude
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOK="$ROOT/aida-core/templates/hooks/aida-cargo-timeout-guard.sh"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
fail=0

# Feed one Bash command through the hook as a PreToolUse payload. The command
# is JSON-encoded by python so quotes and newlines survive intact.
run() {
    python3 -c 'import json,sys; print(json.dumps({"tool_name":"Bash","tool_input":{"command":sys.argv[1]}}))' "$1" \
        | "$HOOK" >"$TMP/out" 2>"$TMP/err"
    echo $? >"$TMP/status"
}
expect_warn() {
    local desc="$1" cmd="$2" needle="$3"
    run "$cmd"
    if [ "$(cat "$TMP/status")" = 0 ] && [ ! -s "$TMP/err" ] \
        && python3 -c 'import json,sys; d=json.load(open(sys.argv[1]))["hookSpecificOutput"]; assert d["hookEventName"]=="PreToolUse"; assert sys.argv[2] in d["additionalContext"], d; assert "permissionDecision" not in d' "$TMP/out" "$needle" 2>/dev/null; then
        echo "ok   - $desc"
    else
        echo "FAIL - $desc (exit $(cat "$TMP/status"); out: $(cat "$TMP/out"); err: $(cat "$TMP/err"))"; fail=1
    fi
}
expect_silent() {
    local desc="$1" cmd="$2"
    run "$cmd"
    if [ "$(cat "$TMP/status")" = 0 ] && [ ! -s "$TMP/out" ] && [ ! -s "$TMP/err" ]; then
        echo "ok   - $desc"
    else
        echo "FAIL - $desc (exit $(cat "$TMP/status"); out: $(cat "$TMP/out"); err: $(cat "$TMP/err"))"; fail=1
    fi
}

# Positive: an outer timeout around cargo or a cargo-running make target.
expect_warn "timeout 3000 cargo test" 'timeout 3000 cargo test' 'AIDA_CARGO_TIMEOUT=3000 cargo ...'
expect_warn "timeout 50m make test-touched" 'timeout 50m make test-touched' 'AIDA_CARGO_TIMEOUT=50m make <target>'
expect_warn "timeout options and env prefix" 'cd w && timeout -k 10 --preserve-status 3000 env RUST_LOG=1 cargo test -p aida-core' 'AIDA_CARGO_TIMEOUT=3000 cargo'
expect_warn "long signal option and cargo path" 'timeout --signal=KILL 20m ~/.cargo/bin/cargo build' 'AIDA_CARGO_TIMEOUT=20m cargo'
expect_warn "gtimeout and gmake" 'gtimeout 900 gmake check-ci-compile' 'AIDA_CARGO_TIMEOUT=900 gmake'
expect_warn "after a ; separator" 'git fetch; timeout 3000 cargo clippy --workspace' 'AIDA_CARGO_TIMEOUT=3000 cargo'
expect_warn "hint says background mode is the only protection" 'timeout 3000 cargo test' 'background mode is the only protection'
expect_warn "hint says check cargo ps before retrying" 'timeout 3000 cargo test' 'check `cargo ps` before retrying'

# Negative: non-cargo timeouts, the replacement form, and prose.
expect_silent "timeout 540 gh pr checks" 'timeout 540 gh pr checks 12 --required --watch --fail-fast'
expect_silent "AIDA_CARGO_TIMEOUT=3000 cargo test" 'AIDA_CARGO_TIMEOUT=3000 cargo test'
expect_silent "plain cargo test" 'cargo test -p aida-core foo'
expect_silent "timeout on an unrelated command mentioning cargo later" 'timeout 5 aida role current && cargo build'
expect_silent "a cargo-prefixed binary is not cargo" 'timeout 30 cargo-audit-report'
expect_silent "quoted prose" 'aida comment add STORY-1 --body "never write timeout 3000 cargo test"'
expect_silent "heredoc prose" $'aida mailbox send --to advisor --stdin <<EOF\nI ran timeout 3000 cargo test\nEOF'
expect_silent "comment prose" 'cargo ps # was: timeout 3000 cargo test'
expect_silent "non-Bash payload" ''

# Malformed input never blocks.
printf 'not json' | "$HOOK" >/dev/null 2>&1
if [ $? -eq 0 ]; then echo "ok   - malformed input exits 0"; else echo "FAIL - malformed input"; fail=1; fi

# Self-contained helpers stay byte-identical to the git-guardrails masters.
for fn in __aida_json_field strip_shell_literals; do
    awk -v f="$fn" '$0 ~ "^" f "\\(\\) \\{" { p=1 } p { print } p && /^\}/ { exit }' "$HOOK" >"$TMP/a"
    awk -v f="$fn" '$0 ~ "^" f "\\(\\) \\{" { p=1 } p { print } p && /^\}/ { exit }' "$ROOT/aida-core/templates/hooks/aida-git-guardrails.sh" >"$TMP/b"
    if [ -s "$TMP/a" ] && diff -q "$TMP/a" "$TMP/b" >/dev/null; then echo "ok   - $fn matches aida-git-guardrails.sh"
    else echo "FAIL - $fn differs from aida-git-guardrails.sh"; fail=1; fi
done

exit $fail
