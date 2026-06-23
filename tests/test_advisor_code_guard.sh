#!/bin/bash
# Smoke test for the advisor-code-guard PreToolUse hook (STORY-670).
# Exercises the fire/suppress decision matrix. Exits non-zero on any failure.
# trace:STORY-670

HOOK="$(cd "$(dirname "$0")/.." && pwd)/aida-core/templates/hooks/aida-advisor-code-guard.sh"
TMP=$(mktemp -d)
FAKEHOME=$(mktemp -d)
mkdir -p "$FAKEHOME/.aida"
fail=0

# run <json> — runs the hook with the ambient test env, prints its exit code.
run() {
    printf '%s' "$1" | TMPDIR="$TMP" HOME="$FAKEHOME" "$HOOK" >/dev/null 2>&1
    echo $?
}

assert_exit() {
    local desc="$1" expected="$2" actual="$3"
    if [ "$actual" -eq "$expected" ]; then
        echo "ok   - $desc (exit $actual)"
    else
        echo "FAIL - $desc (expected $expected, got $actual)"
        fail=1
    fi
}

RS='{"session_id":"%s","tool_input":{"file_path":"/repo/aida-cli/src/x.rs"}}'
MD='{"session_id":"%s","tool_input":{"file_path":"/repo/aida-cli/src/x.md"}}'
DOCS_RS='{"session_id":"%s","tool_input":{"file_path":"/repo/docs/plans/x.rs"}}'
NOFILE='{"session_id":"%s","tool_input":{"command":"ls"}}'

# 1. advisor + code, fresh session, no solo/auto-complete → soft-block (2)
export AIDA_SESSION_ROLE=advisor
unset AIDA_AUTO_COMPLETE
assert_exit "advisor edits code → soft-block" 2 "$(run "$(printf "$RS" S1)")"

# 2. same session repeats → marker set → allow (0)
assert_exit "advisor repeats same session → allowed (fire-once)" 0 "$(run "$(printf "$RS" S1)")"

# 3. advisor + non-code (.md) → allow (0)
assert_exit "advisor edits .md → allowed (specs/docs are advisor work)" 0 "$(run "$(printf "$MD" S2)")"

# 4. advisor + code under docs/ → allow (0)
assert_exit "advisor edits docs/**/*.rs → allowed (doc tree)" 0 "$(run "$(printf "$DOCS_RS" S3)")"

# 5. non-advisor role + code → allow (0)
export AIDA_SESSION_ROLE=implementer
assert_exit "implementer edits code → allowed" 0 "$(run "$(printf "$RS" S4)")"

# 6. advisor + code + AIDA_AUTO_COMPLETE (drain child) → allow (0)
export AIDA_SESSION_ROLE=advisor
export AIDA_AUTO_COMPLETE=1
assert_exit "advisor in --auto-complete drain → allowed" 0 "$(run "$(printf "$RS" S5)")"
unset AIDA_AUTO_COMPLETE

# 7. advisor + code + solo mode active → allow (0)
touch "$FAKEHOME/.aida/solo.toml"
assert_exit "advisor in solo mode → allowed" 0 "$(run "$(printf "$RS" S6)")"
rm -f "$FAKEHOME/.aida/solo.toml"

# 8. advisor + no file_path (Bash-like input) → allow (0)
assert_exit "no file_path → allowed" 0 "$(run "$(printf "$NOFILE" S7)")"

rm -rf "$TMP" "$FAKEHOME"
if [ "$fail" -ne 0 ]; then
    echo "ADVISOR CODE GUARD: FAILURES"
    exit 1
fi
echo "ADVISOR CODE GUARD: all checks passed"
