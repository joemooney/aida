#!/usr/bin/env bash
# Self-test for the real-~/.aida guard in tests/lib/isolated_home.sh.
#
# Every case points the guard at a throwaway "real" HOME (never the operator's
# ~/.aida), mutates its roles/ during the guarded run, and asserts the guard's
# verdict via the child script's exit status:
#   - volatile role churn (last_active_at, appended [[activity]]) passes in
#     the default mode,
#   - a changed purpose / system_prompt / working_directory, or a new or
#     deleted role file, fails in the default mode,
#   - strict mode (CI=true) still fails on volatile churn.
#
# trace:BUG-1640 | ai:claude
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HELPER="$SCRIPT_DIR/lib/isolated_home.sh"
WORK=$(mktemp -d "${TMPDIR:-/tmp}/aida-guard-selftest.XXXXXX")
builtin trap 'rm -rf "$WORK"' EXIT

pass=0
fail=0

seed_home() {
    local home="$1"
    mkdir -p "$home/.aida/roles"
    cat >"$home/.aida/roles/advisor.toml" <<'EOF'
name = "advisor"
purpose = "Independent judgment seat."
created_at = "2026-05-04T04:14:39.836105946Z"
last_active_at = "2026-09-26T07:35:35.945564499Z"
working_directory = "/work/aida"
global = true
system_prompt = "You are the independent judgment gate."
EOF
    cat >"$home/.aida/roles/implementer.toml" <<'EOF'
name = "implementer"
purpose = "Build approved specs."
created_at = "2026-05-04T04:14:39.836105946Z"
last_active_at = "2026-09-25T12:24:00.000000000Z"
working_directory = "/work/aida"
global = true
system_prompt = "Implement."

[[activity]]
spec_id = "BUG-1"
action = "claim"
at = "2026-09-25T12:24:00.000000000Z"
EOF
    # Old mtimes, so any rewrite is a visible mtime change for strict mode.
    touch -d '2026-01-01 00:00:00' "$home/.aida/roles/"*.toml "$home/.aida/roles" "$home/.aida"
}

# run_case <name> <expect: pass|fail> <strict: 0|1> <mutation shell snippet>
# The snippet runs inside the guarded child with $R = the fake real ~/.aida.
run_case() {
    local name="$1" expect="$2" strict="$3" mutation="$4"
    local home="$WORK/$name"
    seed_home "$home"
    local rc=0 out ci=()
    [ "$strict" = 1 ] && ci=(CI=true)
    out=$(
        env -u CI -u AIDA_HOME_GUARD_STRICT HOME="$home" TMPDIR="$WORK" "${ci[@]}" \
            bash -c '
                set -euo pipefail
                source "$1"
                aida_isolate_home
                R="$AIDA_TEST_REAL_HOME/.aida"
                eval "$2"
                exit 0
            ' guard-child "$HELPER" "$mutation" 2>&1
    ) || rc=$?
    local got=pass
    [ "$rc" -ne 0 ] && got=fail
    if [ "$got" = "$expect" ]; then
        echo "ok   $name (guard: $got)"
        pass=$((pass + 1))
    else
        echo "FAIL $name: expected guard $expect, got $got (rc=$rc)"
        echo "$out" | sed 's/^/    /'
        fail=$((fail + 1))
    fi
}

# Atomic-rename rewrite, the way the CLI saves a role file.
rewrite='rewrite() { local f="$1"; shift; sed "$@" "$f" >"$(dirname "$f")/.$(basename "$f").tmp-x"; mv "$(dirname "$f")/.$(basename "$f").tmp-x" "$f"; }'

# (a) volatile churn from a concurrent live session: passes by default.
run_case bump-last-active pass "" "$rewrite; rewrite \"\$R/roles/advisor.toml\" -e 's/^last_active_at = .*/last_active_at = \"2026-09-26T09:00:00Z\"/'"
run_case append-first-activity pass "" "printf '\n[[activity]]\nspec_id = \"STORY-52\"\naction = \"rework\"\nat = \"2026-09-26T09:00:00Z\"\n' >>\"\$R/roles/advisor.toml\""
run_case append-more-activity pass "" "printf '\n[[activity]]\nspec_id = \"STORY-9\"\naction = \"queue-add\"\nat = \"2026-09-26T09:00:00Z\"\n' >>\"\$R/roles/implementer.toml\""
run_case bump-and-append pass "" "$rewrite; rewrite \"\$R/roles/implementer.toml\" -e 's/^last_active_at = .*/last_active_at = \"2026-09-26T09:00:00Z\"/' -e '/^\\[\\[activity\\]\\]/i [[activity]]\\nspec_id = \"X-1\"\\naction = \"a\"\\nat = \"t\"\\n'"
run_case leftover-temp-file-mid-write pass "" ": >\"\$R/roles/.advisor.toml.tmp-0192\""

# (b) real leaks: fail by default.
run_case change-purpose fail "" "$rewrite; rewrite \"\$R/roles/advisor.toml\" -e 's/^purpose = .*/purpose = \"leaked\"/'"
run_case change-system-prompt fail "" "$rewrite; rewrite \"\$R/roles/implementer.toml\" -e 's/^system_prompt = .*/system_prompt = \"leaked\"/'"
run_case change-working-directory fail "" "$rewrite; rewrite \"\$R/roles/advisor.toml\" -e 's|^working_directory = .*|working_directory = \"/tmp/leak\"|'"
run_case change-global fail "" "$rewrite; rewrite \"\$R/roles/advisor.toml\" -e 's/^global = .*/global = false/'"
run_case new-role-file fail "" "printf 'name = \"leak\"\n' >\"\$R/roles/leak.toml\""
run_case deleted-role-file fail "" "rm \"\$R/roles/implementer.toml\""
run_case purpose-change-with-activity-churn fail "" "$rewrite; rewrite \"\$R/roles/implementer.toml\" -e 's/^purpose = .*/purpose = \"leaked\"/' -e 's/^last_active_at = .*/last_active_at = \"now\"/'"

# Strict mode (CI=true) compares everything: volatile churn fails there too.
run_case strict-bump-last-active fail 1 "$rewrite; rewrite \"\$R/roles/advisor.toml\" -e 's/^last_active_at = .*/last_active_at = \"2026-09-26T09:00:00Z\"/'"
run_case strict-untouched pass 1 ":"

echo "guard self-test: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
