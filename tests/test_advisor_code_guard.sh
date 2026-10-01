#!/bin/bash
# Smoke test for the advisor-code-guard PreToolUse hook (STORY-670).
# Exercises the fire/suppress decision matrix. Exits non-zero on any failure.
# trace:STORY-670

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"
cargo build -p aida-cli --quiet || exit 1
TARGET_DIR=$(cargo metadata --no-deps --format-version 1 \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
export PATH="$TARGET_DIR/debug:$PATH"

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

# 7. Hook, internal resolver, and solo status agree across all stored states.
STATUS_REPO="$TMP/status-repo"
mkdir -p "$STATUS_REPO"
git -C "$STATUS_REPO" init -q -b main
# trace:BUG-1748 | ai:codex
# Use python3 because date -d is GNU-only and this test runs on the macOS matrix leg.
now=$(python3 -c 'import datetime; print(datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"))')
expired=$(python3 -c 'import datetime; print((datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(days=2)).strftime("%Y-%m-%dT%H:%M:%SZ"))')
if [ -z "$now" ] || [ -z "$expired" ]; then
    echo "FAIL - could not compute solo fixture timestamps"
    exit 1
fi
for state in absent active expired inactive; do
    solo_on=false
    case "$state" in
        absent) rm -f "$FAKEHOME/.aida/solo.toml" ;;
        active)
            cat >"$FAKEHOME/.aida/solo.toml" <<EOF
active = true
set_at = "$now"
ttl_secs = 86400
EOF
            solo_on=true
            ;;
        expired)
            cat >"$FAKEHOME/.aida/solo.toml" <<EOF
active = true
set_at = "$expired"
ttl_secs = 86400
EOF
            ;;
        inactive)
            cat >"$FAKEHOME/.aida/solo.toml" <<EOF
active = false
set_at = "$now"
ttl_secs = 0
EOF
            ;;
    esac

    if [ "$solo_on" = true ]; then hook_expected=0; status_expected='solo mode ON'; internal_expected=0
    else hook_expected=2; status_expected='solo mode off'; internal_expected=1; fi

    hook_err="$TMP/hook-err-$state"
    hook_actual=$(printf '%s' "$(printf "$RS" "solo-$state")" | \
        env -u AIDA_AUTO_COMPLETE AIDA_SESSION_ROLE=advisor HOME="$FAKEHOME" TMPDIR="$TMP" \
        "$HOOK" >/dev/null 2>"$hook_err"; echo $?)
    assert_exit "$state hook agrees with solo state $solo_on" "$hook_expected" "$hook_actual"
    if grep -Fq 'could not consult' "$hook_err"; then
        echo "FAIL - $state hook fell back instead of consulting the resolver"
        fail=1
    else
        echo "ok   - $state hook consulted the resolver (no fallback)"
    fi

    internal_actual=$(env -i PATH="$PATH" HOME="$FAKEHOME" \
        "$TARGET_DIR/debug/aida" internal solo-active >/dev/null 2>&1; echo $?)
    if [ "$internal_actual" -eq "$internal_expected" ]; then
        echo "ok   - $state internal solo-active agrees with solo state $solo_on (exit $internal_actual)"
    else
        echo "FAIL - $state internal solo-active disagrees with solo state $solo_on (expected $internal_expected, got $internal_actual)"
        fail=1
    fi

    status_actual=$(cd "$STATUS_REPO" && env -i PATH="$PATH" HOME="$FAKEHOME" \
        "$TARGET_DIR/debug/aida" solo status 2>&1 || true)
    if printf '%s' "$status_actual" | grep -Fq "$status_expected"; then
        echo "ok   - $state solo status agrees with solo state $solo_on"
    else
        echo "FAIL - $state solo status disagrees with solo state $solo_on (expected '$status_expected'; got '$status_actual')"
        fail=1
    fi
done

# 8. Missing aida binary fails safe even when the stored solo flag is active.
cat >"$FAKEHOME/.aida/solo.toml" <<EOF
active = true
set_at = "$now"
ttl_secs = 86400
EOF
SHIM="$TMP/no-aida-bin"
mkdir -p "$SHIM"
printf '#!/bin/sh\nexit 127\n' >"$SHIM/aida"
chmod +x "$SHIM/aida"
fallback_err="$TMP/hook-err-fallback"
fallback_actual=$(printf '%s' "$(printf "$RS" solo-fallback)" | \
    env -u AIDA_AUTO_COMPLETE AIDA_SESSION_ROLE=advisor HOME="$FAKEHOME" TMPDIR="$TMP" \
    PATH="$SHIM:/usr/bin:/bin" "$HOOK" >/dev/null 2>"$fallback_err"; echo $?)
assert_exit "missing resolver with active solo flag fails safe" 2 "$fallback_actual"
if grep -Fq 'could not consult' "$fallback_err"; then
    echo "ok   - missing resolver reports fallback"
else
    echo "FAIL - missing resolver did not report fallback"
    fail=1
fi
rm -f "$FAKEHOME/.aida/solo.toml"

# 9. advisor + no file_path (Bash-like input) → allow (0)
assert_exit "no file_path → allowed" 0 "$(run "$(printf "$NOFILE" S7)")"

# BUG-1751: prove the parser works when grep has no PCRE support.
REAL_GREP=$(command -v grep)
PCRE_SHIM="$TMP/no-pcre"
mkdir -p "$PCRE_SHIM"
cat >"$PCRE_SHIM/grep" <<EOF
#!/bin/sh
for arg do case "\$arg" in --perl-regexp|-*P*) echo 'PCRE unavailable' >&2; exit 2;; esac; done
exec "$REAL_GREP" "\$@"
EOF
chmod +x "$PCRE_SHIM/grep"
run_pcre() {
    printf '%s' "$1" | PATH="$PCRE_SHIM:$PATH" TMPDIR="$TMP" HOME="$FAKEHOME" "$HOOK" >/dev/null 2>&1
    echo $?
}
assert_exit "PCRE unavailable: advisor code still soft-blocks" 2 "$(run_pcre "$(printf "$RS" pcre-a)")"
assert_exit "PCRE unavailable: advisor .md allowed" 0 "$(run_pcre "$(printf "$MD" pcre-md)")"
assert_exit "PCRE unavailable: same session remains fire-once" 0 "$(run_pcre "$(printf "$RS" pcre-a)")"
assert_exit "PCRE unavailable: first distinct session soft-blocks" 2 "$(run_pcre "$(printf "$RS" pcre-b)")"
assert_exit "PCRE unavailable: second distinct session soft-blocks" 2 "$(run_pcre "$(printf "$RS" pcre-c)")"
assert_exit "escaped quote in file_path still reaches code decision" 2 "$(run_pcre '{"session_id":"escaped-path","tool_input":{"file_path":"/repo/x\\\"y.rs"}}')"
# A late key after a large unrelated value still reaches the same policy decision.
python3 - "$TMP/late-key.json" <<'PYJSON'
import json, pathlib, sys
payload = {"session_id": "late-key", "tool_input": {"content": "x" * 200_000, "file_path": "/repo/late.rs"}}
pathlib.Path(sys.argv[1]).write_text(json.dumps(payload))
PYJSON
late_key_actual=$(env AIDA_SESSION_ROLE=advisor TMPDIR="$TMP" HOME="$FAKEHOME" "$HOOK" <"$TMP/late-key.json" >/dev/null 2>&1; echo $?)
assert_exit "file_path after 200 KB content still soft-blocks" 2 "$late_key_actual"
# A JSON-escaped slash in a docs path must be decoded before suppression.
assert_exit "escaped slash decodes before docs suppression" 0 "$(run_pcre '{"session_id":"escaped-slash","tool_input":{"file_path":"/repo/docs\/x.rs"}}')"

# The self-contained hook copies must keep their parser implementation identical.
awk '/^__aida_json_field\(\) \{/ { p=1 } p { print } p && /^\}/ { exit }' "$PROJECT_ROOT/aida-core/templates/hooks/aida-advisor-code-guard.sh" >"$TMP/guard-parser"
awk '/^__aida_json_field\(\) \{/ { p=1 } p { print } p && /^\}/ { exit }' "$PROJECT_ROOT/aida-core/templates/hooks/aida-git-guardrails.sh" >"$TMP/git-parser"
if diff -q "$TMP/guard-parser" "$TMP/git-parser" >/dev/null; then echo "ok   - hook JSON helper definitions are byte-identical"; else echo "FAIL - hook JSON helper definitions differ"; fail=1; fi

rm -rf "$TMP" "$FAKEHOME"
if [ "$fail" -ne 0 ]; then
    echo "ADVISOR CODE GUARD: FAILURES"
    exit 1
fi
echo "ADVISOR CODE GUARD: all checks passed"
