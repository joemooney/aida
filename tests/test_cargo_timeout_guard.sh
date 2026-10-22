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
# RUN_DIR is the hook process's working directory; PAYLOAD_CWD, when set, is
# the payload's `cwd` (the Bash tool's directory), which the hook prefers.
RUN_DIR="$ROOT"
PAYLOAD_CWD=""
run() {
    python3 -c 'import json,sys
d={"tool_name":"Bash","tool_input":{"command":sys.argv[1]}}
if sys.argv[2]: d["cwd"]=sys.argv[2]
print(json.dumps(d))' "$1" "$PAYLOAD_CWD" \
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
    '      | cat' \
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
# The payload's cwd, not the hook process's, picks the Makefile.
PAYLOAD_CWD="$MK"
expect_silent "payload cwd names the Makefile" 'timeout 1h make hello'
RUN_DIR="$MK"
PAYLOAD_CWD="$TMP"
expect_warn "payload cwd without a Makefile" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
PAYLOAD_CWD=""

# PR-2801 round 1: every segment counts, and the resolver fails toward warning.
RUN_DIR="$MK"
expect_warn "safe make then a timeout cargo" 'timeout 1h make hello && timeout 1h cargo test' 'AIDA_CARGO_TIMEOUT=1h cargo ...'
expect_warn "safe make then ; timeout cargo" 'timeout 1h make hello; timeout 1h cargo test' 'AIDA_CARGO_TIMEOUT=1h cargo ...'
expect_warn "sh -c body: safe make then cargo" 'timeout 1h bash -c "make hello && cargo test"' 'AIDA_CARGO_TIMEOUT=1h cargo ...'
expect_warn "second sh -c runs cargo" 'timeout 30 bash -c "ls" && timeout 30 bash -c "cargo test"' 'AIDA_CARGO_TIMEOUT=30 cargo ...'
expect_warn "safe sh -c make then sh -c cargo" "timeout 30 sh -c 'make hello'; timeout 30 sh -c 'cargo test'" 'AIDA_CARGO_TIMEOUT=30 cargo ...'
expect_silent "quoted sh -c prose before a harmless sh -c" "echo 'timeout 3 bash -c \"cargo x\"'; timeout 30 bash -c 'ls'"
expect_warn "sh -o option before -c" 'timeout 3000 bash -o pipefail -c "cargo test"' 'AIDA_CARGO_TIMEOUT=3000 cargo ...'
expect_warn "cargo after then" 'timeout 3000 bash -c "if true; then cargo test; fi"' 'AIDA_CARGO_TIMEOUT=3000 cargo ...'
expect_warn "cargo after do" 'timeout 3000 bash -c "for p in a b; do cargo test -p \$p; done"' 'AIDA_CARGO_TIMEOUT=3000 cargo ...'
mk_case() { mkdir -p "$MK/$1"; shift_dir="$MK/$1"; shift; printf '%s\n' "$@" >"$shift_dir/Makefile"; RUN_DIR="$shift_dir"; }
mk_case vardeps 'DEPS = lint' 'hello: $(DEPS)' $'\t@echo hi' 'lint:' $'\tcargo clippy'
expect_warn "variable prerequisite" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
mk_case incl 'include extra.mk' 'hello: ; @echo hi'
printf '%s\n' 'hello: lint' 'lint:' $'\tcargo clippy' >"$MK/incl/extra.mk"
expect_warn "include may add to a known target" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
mk_case dashincl '-include extra.mk' 'hello:' $'\t@echo hi'
expect_warn "-include fails closed" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
for recipe in 'bin/check' 'python3 -m tool' 'npm run build' 'just ci' '@echo a && rustup run stable cargo build' 'X=1 echo hi' 'echo $$(cargo --version)'; do
    mk_case allow 'hello:' $'\t'"$recipe"
    expect_warn "recipe outside the inert allowlist: $recipe" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
done
mk_case inert 'hello: a' $'\t@mkdir -p out && cp a out/ && echo "done; ok" > out/log' $'\t-rm -f stale; touch out/stamp' $'\t@test -f a || printf \'%s\\n\' missing' 'a:' $'\t@true'
expect_silent "only inert recipe commands" 'timeout 1h make hello'
# TASK-1955 (TASK-2075): a conditional inside a recipe hides its lines from
# the resolver, so any conditional directive fails closed.
# trace:TASK-1955 | ai:claude
mk_case cond 'hello:' $'\t@echo hi' 'ifeq ($(CI),1)' $'\tcargo test' 'endif'
expect_warn "recipe inside a conditional fails closed" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
mk_case ifdef 'ifdef FAST' 'hello:' $'\t@echo hi' 'endif'
expect_warn "ifdef fails closed" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
# An unresolvable line after a safe target used to fall through awk's END.
mk_case lateincl 'hello:' $'\t@echo hi' 'include extra.mk'
printf '%s\n' 'hello: lint' 'lint:' $'\tcargo clippy' >"$MK/lateincl/extra.mk"
expect_warn "include after a safe target fails closed" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
# TASK-2076: a pattern or suffix rule may build an unknown prerequisite.
mk_case pattern 'hello: page.html' $'\t@echo hi' '%.html: %.md' $'\tcargo run --bin render $<'
expect_warn "pattern rule may build a prerequisite" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
mk_case suffix 'hello: page.o' $'\t@echo hi' '.c.o:' $'\tcargo run --bin cc $<'
expect_warn "suffix rule may build a prerequisite" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
mk_case phony '.PHONY: hello' 'hello: a.txt' $'\t@echo hi'
expect_silent "special target is no suffix rule" 'timeout 1h make hello'
# TASK-2077: awk/sed bodies and process substitution can run anything;
# .RECIPEPREFIX and $(eval) change what a recipe is.
for recipe in "awk 'BEGIN{system(\"cargo build\")}'" "sed -n '1e cargo build' x" 'cat <(cargo metadata)'; do
    mk_case exec 'hello:' $'\t'"$recipe"
    expect_warn "not inert: $recipe" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
done
mk_case prefix '.RECIPEPREFIX = >' 'hello:' '>cargo test'
expect_warn ".RECIPEPREFIX fails closed" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
for directive in 'override' 'export' 'private' 'override export'; do
    mk_case prefix "$directive .RECIPEPREFIX = >" 'hello:' '>cargo test'
    expect_warn "$directive .RECIPEPREFIX fails closed" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
done
mk_case evalrule '$(eval hello: ; cargo test)' 'other:' $'\t@echo hi'
expect_warn "\$(eval) rule generation fails closed" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
# TASK-2128: the remaining fail-opens of the PR-2817 round-2 review.
# trace:TASK-1955 | ai:claude (TASK-2128, TASK-2129)
mk_case ifneq 'hello:' $'\t@echo hi' 'ifneq ($(CI),)' $'\tcargo test' 'endif'
expect_warn "ifneq fails closed" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
mk_case ifndef 'ifndef FAST' 'hello:' $'\tcargo test' 'endif'
expect_warn "ifndef fails closed" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
for directive in '' 'override ' 'export '; do
    mk_case defprefix "${directive}define .RECIPEPREFIX" '>' 'endef' 'hello:' '>cargo test'
    expect_warn "${directive}define .RECIPEPREFIX fails closed" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
done
mk_case varrule 'X := hello: ; cargo test' 'hello:' $'\t@echo hi' '$(X)'
expect_warn "a bare \$(X) line that expands to a rule fails closed" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
mk_case vartarget 'T := hello' '$(T): build' $'\t@echo hi' 'hello:' $'\t@echo hi'
expect_warn "a computed target name fails closed" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
for sfx in .C .S .F; do
    mk_case upsuffix "hello: page$sfx.o" $'\t@echo hi' "$sfx.o:" $'\tcargo run --bin cc $<'
    expect_warn "upper-case suffix rule $sfx.o may build a prerequisite" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
    mk_case upsingle "hello: page" $'\t@echo hi' "$sfx:" $'\tcargo run --bin cc $<'
    expect_warn "upper-case single-suffix rule $sfx may build a prerequisite" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
done
mk_case declared '.SUFFIXES: .md .html' 'hello: page.html' $'\t@echo hi' '.md.html:' $'\tcargo run --bin render $<'
expect_warn "a .SUFFIXES-declared suffix rule may build a prerequisite" 'timeout 1h make hello' 'AIDA_CARGO_TIMEOUT=1h make'
# TASK-2129: a dot target with an unknown suffix is a plain target.
mk_case venv '.venv: requirements.txt' $'\tpython3 -m venv .venv' 'hello: notes.txt' $'\t@echo hi'
expect_silent ".venv is no suffix rule" 'timeout 1h make hello'
mk_case dotenv '.env:' $'\t@touch .env' 'hello: notes.txt' $'\t@echo hi'
expect_silent ".env is no suffix rule" 'timeout 1h make hello'
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
