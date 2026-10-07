#!/bin/bash
# End-to-end proof that the vendor-agnostic advisor-no-code-write gate
# (STORY-684) binds at the COMMIT boundary, independent of any vendor hook.
#
# Drives the real `aida internal advisor-code-gate` binary against a throwaway
# git repo with files actually staged in the index — i.e. exactly what the git
# pre-commit hook calls, with no Claude PreToolUse hook in the loop. This is the
# substrate-level counterpart to the role-gated queue tests; it proves the gate
# refuses an advisor-seat code commit and lets an implementer through, for ANY
# vendor (the path under test is plain `git` + `aida`, not Claude-specific).
#
# trace:STORY-684
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$PROJECT_ROOT"
cargo build -p aida-cli --quiet

TARGET_DIR=$(cargo metadata --no-deps --format-version 1 \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
AIDA_BIN="$TARGET_DIR/debug/aida"

TMP=$(mktemp -d)
FAKEHOME=$(mktemp -d)
trap 'rm -rf "$TMP" "$FAKEHOME"' EXIT
fail=0

# Throwaway git repo with an initial commit so HEAD exists.
cd "$TMP"
git init -q -b main
git config user.email t@t.t
git config user.name t
echo seed >seed.txt
git add seed.txt
git commit -qm "chore: seed"

# STORY-1473 / ADR-66: the role env is only a hint — the gate resolves the
# seat from a validated session grant. Seed this throwaway repo with a store
# roster + grant record so the ADVISOR cases resolve as a real granted
# advisor seat (the implementer cases ride the least-privilege default).
# Subject is "default": `env -i` strips USER, so that is what the invoked
# binary resolves. trace:STORY-1473 | ai:claude
# shellcheck source=lib/seat_grant.sh
source "$SCRIPT_DIR/lib/seat_grant.sh"
ADVISOR_GRANT=$(aida_seat_grant "$TMP" "$FAKEHOME" default advisor)

# gate <ROLE> [EXTRA_ENV...] — stage what's already `git add`-ed and run the
# gate with a fresh, isolated env (FAKEHOME so no real solo.toml leaks in;
# the advisor cases pass their validated grant as EXTRA_ENV).
gate() {
    local role="$1"
    shift
    env -i PATH="$PATH" HOME="$FAKEHOME" AIDA_SESSION_ROLE="$role" "$@" \
        "$AIDA_BIN" internal advisor-code-gate >/dev/null 2>&1
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

stage() {
    git reset -q
    for f in "$@"; do
        mkdir -p "$(dirname "$f")"
        echo "// content" >"$f"
        git add "$f"
    done
}

# 1. advisor stages CODE → REFUSED (exit non-zero).
stage src/feature.rs
assert_exit "advisor staging code is refused" 1 "$(gate advisor AIDA_SESSION_GRANT="$ADVISOR_GRANT")"

# 2. advisor stages only DOCS/SPECS/CONFIG → ALLOWED.
stage README.md docs/plan.md .aida/notes.toml
assert_exit "advisor staging docs/config is allowed" 0 "$(gate advisor AIDA_SESSION_GRANT="$ADVISOR_GRANT")"

# 3. advisor stages mixed docs + one code file → REFUSED.
stage README.md src/lib.rs
assert_exit "advisor staging mixed (one code file) is refused" 1 "$(gate advisor AIDA_SESSION_GRANT="$ADVISOR_GRANT")"

# 4. IMPLEMENTER stages code → ALLOWED (the sanctioned coder).
stage src/feature.rs
assert_exit "implementer staging code is allowed" 0 "$(gate implementer)"

# 5. advisor + AIDA_AUTO_COMPLETE (drain child) staging code → ALLOWED.
stage src/feature.rs
assert_exit "advisor in --auto-complete drain is allowed" 0 "$(gate advisor AIDA_AUTO_COMPLETE=1 AIDA_SESSION_GRANT="$ADVISOR_GRANT")"

# 6. advisor + explicit escape hatch staging code → ALLOWED.
stage src/feature.rs
assert_exit "advisor with AIDA_ALLOW_ADVISOR_CODE=1 is allowed" 0 "$(gate advisor AIDA_ALLOW_ADVISOR_CODE=1 AIDA_SESSION_GRANT="$ADVISOR_GRANT")"

# 7. advisor + nothing staged → ALLOWED (empty index never refuses).
git reset -q
assert_exit "advisor with empty stage is allowed" 0 "$(gate advisor AIDA_SESSION_GRANT="$ADVISOR_GRANT")"

# 8. The pre-commit hook wiring binds a raw `git commit` (no Claude hook).
#    Install the scaffolded pre-commit hook and prove an advisor `git commit`
#    that stages code is aborted by it.
mkdir -p .git/hooks
# Re-create the hook body inline matching the scaffolded gate step, pointing at
# the just-built binary so the test is hermetic.
cat >.git/hooks/pre-commit <<EOF
#!/bin/bash
if ! "$AIDA_BIN" internal advisor-code-gate; then
    exit 1
fi
exit 0
EOF
chmod +x .git/hooks/pre-commit

stage src/via_hook.rs
set +e
env -i PATH="$PATH" HOME="$FAKEHOME" AIDA_SESSION_ROLE=advisor AIDA_SESSION_GRANT="$ADVISOR_GRANT" \
    git -c user.email=t@t.t -c user.name=t commit -qm "feat(x): code (TASK-1)" >/dev/null 2>&1
hook_rc=$?
set -e
assert_exit "raw git commit by advisor is aborted by pre-commit hook" 1 "$hook_rc"

# 9. Same commit with --no-verify (git-native escape) → succeeds (proves the
#    escape hatch exists and is honored).
set +e
env -i PATH="$PATH" HOME="$FAKEHOME" AIDA_SESSION_ROLE=advisor AIDA_SESSION_GRANT="$ADVISOR_GRANT" \
    git -c user.email=t@t.t -c user.name=t commit --no-verify -qm "feat(x): code (TASK-1)" >/dev/null 2>&1
noverify_rc=$?
set -e
assert_exit "advisor git commit --no-verify bypasses the gate" 0 "$noverify_rc"

if [ "$fail" -ne 0 ]; then
    echo "ADVISOR CODE GATE (STORY-684): FAILURES"
    exit 1
fi
echo "ADVISOR CODE GATE (STORY-684): all checks passed"
