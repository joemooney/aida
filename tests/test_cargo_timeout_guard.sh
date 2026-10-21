#!/bin/bash
# Pattern coverage for the warn-only cargo time-budget PreToolUse hook.
# trace:STORY-1854 | ai:claude
# trace:TASK-1967 | ai:claude
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOK="$ROOT/aida-core/templates/hooks/aida-cargo-timeout-guard.sh"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
fail=0

# Feed one Bash command through the hook as a PreToolUse payload. The command
# is JSON-encoded by python so quotes and newlines survive intact.
# RUN_DIR is the hook's working directory (where it looks for a Makefile).
RUN_DIR="$ROOT"
run() {
    python3 -c 'import json,sys; print(json.dumps({"tool_name":"Bash","tool_input":{"command":sys.argv[1]}}))' "$1" \
        | (cd "$RUN_DIR" && "$HOOK") >"$TMP/out" 2>"$TMP/err"
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

# TASK-1961: a quoted `sh -c` body is still run under the outer timeout.
expect_warn "timeout bash -c double-quoted cargo" 'timeout 3000 bash -c "cargo test"' 'AIDA_CARGO_TIMEOUT=3000 cargo ...'
expect_warn "timeout sh -c single-quoted cd && cargo" "timeout 20m sh -c 'cd w && RUST_LOG=1 cargo build'" 'AIDA_CARGO_TIMEOUT=20m cargo'
expect_warn "timeout bash -lc with nice and make" 'timeout 50m bash -lc "nice -n 10 make test-touched"' 'AIDA_CARGO_TIMEOUT=50m make <target>'
expect_warn "hint names the sh -c form" 'timeout 3000 /bin/bash -c "cargo test"' 'timeout 3000 sh -c "cargo ..."'
expect_silent "timeout bash -c body that only echoes cargo" 'timeout 30 bash -c "echo cargo test"'
expect_silent "timeout bash -c body without cargo" "timeout 30 bash -c 'ls -la'"
expect_silent "quoted prose naming an sh -c form" "aida comment add STORY-1 --body \"timeout 3 bash -c 'cargo test'\""

# TASK-1962: a make target is skipped only when its Makefile shows no cargo.
MK="$TMP/mk"
mkdir -p "$MK/sub"
printf '%s\n' \
    '.PHONY: hello docs lint wrapper recurse varcmd script both' \
    'hello: ## Prints a greeting' \
    $'\t@echo hello' \
    $'\t@printf "%s\\n" "$$HOME" \\' \
    '      | wc -c' \
    'docs: hello README.md' \
    $'\t@echo docs' \
    'lint:' \
    $'\tcargo clippy --workspace' \
    'wrapper: docs lint' \
    'recurse:' \
    $'\t@$(MAKE) hello' \
    'varcmd:' \
    $'\t$(TEST_CMD)' \
    'script:' \
    $'\t@./run-checks' \
    'both: ; echo a && cargo build' \
    'continued: hello \' \
    '    lint' >"$MK/Makefile"
RUN_DIR="$MK"
expect_silent "make target that only echoes" 'timeout 1h make hello'
expect_silent "make target and a file prerequisite" 'timeout 1h make -j4 V=1 docs'
expect_silent "sh -c body running a make target without cargo" 'timeout 1h bash -c "make hello"'
expect_warn "make target that runs cargo" 'timeout 1h make lint' 'AIDA_CARGO_TIMEOUT=1h make <target>'
expect_warn "make prerequisite that runs cargo" 'timeout 1h make wrapper' 'AIDA_CARGO_TIMEOUT=1h make'
expect_warn "make prerequisite on a continued line" 'timeout 1h make continued' 'AIDA_CARGO_TIMEOUT=1h make'
expect_warn "one of several targets runs cargo" 'timeout 1h make hello lint' 'AIDA_CARGO_TIMEOUT=1h make'
expect_warn "recursive make" 'timeout 1h make recurse' 'AIDA_CARGO_TIMEOUT=1h make'
expect_warn "recipe expands a make variable" 'timeout 1h make varcmd' 'AIDA_CARGO_TIMEOUT=1h make'
expect_warn "recipe runs a script" 'timeout 1h make script' 'AIDA_CARGO_TIMEOUT=1h make'
expect_warn "inline recipe runs cargo" 'timeout 1h make both' 'AIDA_CARGO_TIMEOUT=1h make'
expect_warn "unknown make target" 'timeout 1h make nope' 'AIDA_CARGO_TIMEOUT=1h make'
expect_warn "default goal" 'timeout 1h make' 'AIDA_CARGO_TIMEOUT=1h make'
expect_warn "make -C another directory" 'timeout 1h make -C sub hello' 'AIDA_CARGO_TIMEOUT=1h make'
expect_warn "cd before the timeout" 'cd sub && timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
printf '%s\n' 'include rules.mk' 'hello: generated' $'\t@echo hello' >"$MK/sub/Makefile"
RUN_DIR="$MK/sub"
expect_warn "prerequisite an include may define" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
RUN_DIR="$TMP"
expect_warn "no Makefile here" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
RUN_DIR="$ROOT"

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
