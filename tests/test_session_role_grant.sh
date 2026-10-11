#!/bin/bash
# SessionStart banners show the role the session actually holds: a role label
# with no valid seat grant reads as "(no grant)", a granted seat reads as
# before. Runs the template masters with a stub `aida` whose `role current`
# prints $STUB_SEAT (the validated grant's seat; empty = no grant). No cargo
# build, no network. trace:BUG-1924 | ai:claude
set -u

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SESSION_HOOK="$ROOT/aida-core/templates/hooks/aida-session-context.sh"
ROLE_HOOK="$ROOT/aida-core/templates/hooks/aida-role-context.sh"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
fail=0

export HOME="$TMP/home"
mkdir -p "$HOME" "$TMP/project/.aida/roles" "$TMP/bin"
cat >"$TMP/bin/aida" <<'STUB'
#!/bin/sh
if [ "${1:-}" = "role" ] && [ "${2:-}" = "current" ]; then
    [ -n "${STUB_SLEEP:-}" ] && sleep "$STUB_SLEEP"
    printf '%s\n' "${STUB_SEAT:-}"
fi
exit 0
STUB
chmod +x "$TMP/bin/aida"
cat >"$TMP/project/.aida/roles/advisor.toml" <<'TOML'
purpose = "Route work and groom the queue."
TOML

# run <hook> [env...]: the hook's stdout, run from the fixture project.
run() {
    local hook="$1"
    shift
    (cd "$TMP/project" && env -u AIDA_SESSION_ROLE -u AIDA_SESSION_GRANT \
        -u AIDA_SESSION_PROJECT -u CODEX_PROJECT_DIR \
        CLAUDE_PROJECT_DIR="$TMP/project" PATH="$TMP/bin:$PATH" \
        AIDA_MAIN_CHECKOUT_DRIFT=off "$@" bash "$hook")
}

# Additional context the role hook injects (decoded from its JSON envelope).
role_ctx() {
    run "$ROLE_HOOK" "$@" | python3 -c '
import json, sys
raw = sys.stdin.read()
print(json.loads(raw)["hookSpecificOutput"]["additionalContext"] if raw.strip() else "")'
}

check() {
    local desc="$1" expected="$2" actual="$3"
    if [ "$actual" = "$expected" ]; then
        echo "ok   - $desc"
    else
        echo "FAIL - $desc"
        echo "       expected: [$expected]"
        echo "       actual:   [$actual]"
        fail=1
    fi
}

contains() {
    local desc="$1" needle="$2" haystack="$3"
    case "$haystack" in
        *"$needle"*) echo "ok   - $desc" ;;
        *)
            echo "FAIL - $desc"
            echo "       missing: [$needle]"
            echo "       in:      [$haystack]"
            fail=1
            ;;
    esac
}

absent() {
    local desc="$1" needle="$2" haystack="$3"
    case "$haystack" in
        *"$needle"*)
            echo "FAIL - $desc"
            echo "       unexpected: [$needle]"
            fail=1
            ;;
        *) echo "ok   - $desc" ;;
    esac
}

# --- session-context digest header ---
out=$(run "$SESSION_HOOK" AIDA_SESSION_ROLE=advisor STUB_SEAT=)
check "advisor label, no grant: header marks the role ungranted" \
    "AIDA · role: advisor (no grant) · queue: 0 · in-flight: 0" "$(printf '%s\n' "$out" | head -n 1)"
contains "advisor label, no grant: says how to enter the role" \
    'eval "$(aida role enter advisor)"' "$out"

out=$(run "$SESSION_HOOK" AIDA_SESSION_ROLE=advisor STUB_SEAT=advisor)
check "advisor label with a valid grant: header unchanged" \
    "AIDA · role: advisor · queue: 0 · in-flight: 0" "$out"

out=$(run "$SESSION_HOOK" AIDA_SESSION_ROLE=dialog STUB_SEAT=)
check "deprecated alias canonicalizes before the grant check" \
    "AIDA · role: advisor (no grant) · queue: 0 · in-flight: 0" "$(printf '%s\n' "$out" | head -n 1)"

out=$(run "$SESSION_HOOK" STUB_SEAT=reviewer)
check "no label, valid grant: the granted seat is shown" \
    "AIDA · role: reviewer · queue: 0 · in-flight: 0" "$out"

out=$(run "$SESSION_HOOK" AIDA_SESSION_ROLE=Guest STUB_SEAT=)
check "persona labels compare case-insensitively and need no grant" \
    "AIDA · role: guest · queue: 0 · in-flight: 0" "$out"

out=$(run "$SESSION_HOOK" AIDA_SESSION_ROLE=Advisor STUB_SEAT=advisor)
check "a mixed-case label with a valid grant is unchanged" \
    "AIDA · role: advisor · queue: 0 · in-flight: 0" "$out"

out=$(run "$SESSION_HOOK" AIDA_SESSION_ROLE=implementer STUB_SEAT=)
check "implementer baseline needs no grant" \
    "AIDA · role: implementer · queue: 0 · in-flight: 0" "$out"

out=$(run "$SESSION_HOOK" STUB_SEAT=)
check "no label, no grant: role none" \
    "AIDA · role: none · queue: 0 · in-flight: 0" "$out"

# --- role-context addendum ---
ctx=$(role_ctx AIDA_SESSION_ROLE=advisor STUB_SEAT=)
check "role addendum header marks an ungranted advisor" \
    "# AIDA active role: advisor (no grant)" "$(printf '%s\n' "$ctx" | head -n 1)"
absent "the full grant note appears once, in the digest, not the addendum" \
    "aida role enter" "$ctx"

ctx=$(role_ctx AIDA_SESSION_ROLE=advisor STUB_SEAT=advisor)
check "role addendum header with a valid grant is unchanged" \
    "# AIDA active role: advisor" "$(printf '%s\n' "$ctx" | head -n 1)"
absent "role addendum with a valid grant has no grant warning" "no valid grant" "$ctx"
contains "role addendum still carries the role purpose" \
    "Purpose: Route work and groom the queue." "$ctx"

ctx=$(role_ctx AIDA_SESSION_ROLE=Advisor STUB_SEAT=advisor)
check "role addendum compares the label case-insensitively" \
    "# AIDA active role: advisor" "$(printf '%s\n' "$ctx" | head -n 1)"

mkdir -p "$HOME/.aida/roles"
printf 'purpose = "Global reviewer."\n' >"$HOME/.aida/roles/reviewer.toml"
ctx=$(run "$ROLE_HOOK" AIDA_SESSION_ROLE=reviewer STUB_SEAT= \
    AIDA_SESSION_PROJECT="$TMP/missing" | python3 -c '
import json, sys
raw = sys.stdin.read()
print(json.loads(raw)["hookSpecificOutput"]["additionalContext"] if raw.strip() else "")')
check "an unreachable project root skips the grant check (no false tag)" \
    "# AIDA active role: reviewer" "$(printf '%s\n' "$ctx" | head -n 1)"

ctx=$(role_ctx STUB_SEAT=advisor)
check "no label: role addendum stays silent" "" "$ctx"

# A grant check that times out is unvalidated, not a missing grant.
if command -v timeout >/dev/null 2>&1; then
    out=$(run "$SESSION_HOOK" AIDA_SESSION_ROLE=advisor STUB_SEAT= STUB_SLEEP=4)
    check "timed-out grant check shows the label untagged in the digest" \
        "AIDA · role: advisor · queue: 0 · in-flight: 0" "$out"
    ctx=$(role_ctx AIDA_SESSION_ROLE=advisor STUB_SEAT= STUB_SLEEP=4)
    check "timed-out grant check shows the label untagged in the addendum" \
        "# AIDA active role: advisor" "$(printf '%s\n' "$ctx" | head -n 1)"
fi

exit "$fail"
