#!/bin/bash
# TASK-144: the pre-commit `///`-provenance gate must not false-positive on
# PURE MOVES. The gate (TASK-135/BUG-624/BUG-629/TASK-903) scans added diff
# lines only, so a mechanical file-split used to re-add pre-existing
# `/// trace:` debt and refuse the commit (~85 false-flagged lines during the
# STORY-771 extraction) even though BUG-624's design note says debt a commit
# did not introduce must not block it. The fix credits `-`-removed provenance
# lines in the SAME staged diff against added candidates (trim-insensitive,
# counted), so a move is excused while genuinely NEW debt is still refused.
#
# Drives the REAL template (aida-core/templates/hooks/aida-pre-commit.sh)
# installed as .git/hooks/pre-commit in a throwaway repo, via real `git commit`.
#
# trace:TASK-144 | ai:claude
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOK_TEMPLATE="$PROJECT_ROOT/aida-core/templates/hooks/aida-pre-commit.sh"

TMP=$(mktemp -d)
FAKEHOME=$(mktemp -d)
STDERR_LOG="$FAKEHOME/stderr.txt"
trap 'rm -rf "$TMP" "$FAKEHOME"' EXIT
fail=0

cd "$TMP"
git init -q -b main
git config user.email t@t.t
git config user.name t
mkdir -p .git/hooks src
cp "$HOOK_TEMPLATE" .git/hooks/pre-commit
chmod +x .git/hooks/pre-commit

# commit <msg> — run a real hermetic `git commit` (FAKEHOME so no user config /
# aida state leaks in; no AIDA_SESSION_ROLE so the advisor-code-gate, if an
# aida binary is even resolvable, allows). Echoes the exit code; stderr is
# captured OUTSIDE the repo so `git add -A` never stages it.
commit() {
    set +e
    env -i PATH="$PATH" HOME="$FAKEHOME" \
        git -c user.email=t@t.t -c user.name=t commit -qm "$1" \
        >/dev/null 2>"$STDERR_LOG"
    local rc=$?
    set -e
    echo "$rc"
}

assert_exit() {
    local desc="$1" expected="$2" actual="$3"
    if [ "$actual" -eq "$expected" ]; then
        echo "ok   - $desc (exit $actual)"
    else
        echo "FAIL - $desc (expected $expected, got $actual)"
        [ -s "$STDERR_LOG" ] && sed 's/^/       | /' "$STDERR_LOG"
        fail=1
    fi
}

assert_stderr_contains() {
    local desc="$1" needle="$2"
    if grep -q "$needle" "$STDERR_LOG"; then
        echo "ok   - $desc"
    else
        echo "FAIL - $desc (stderr lacks '$needle')"
        fail=1
    fi
}

assert_stderr_lacks() {
    local desc="$1" needle="$2"
    if grep -q "$needle" "$STDERR_LOG"; then
        echo "FAIL - $desc (stderr unexpectedly contains '$needle')"
        fail=1
    else
        echo "ok   - $desc"
    fi
}

# Seed: a file already carrying `///` provenance debt (committed with
# --no-verify, exactly how such debt predates the gate).
cat >src/lib.rs <<'EOF'
/// trace:TASK-101 | ai:claude
pub fn alpha() {}
/// trace:TASK-102 | ai:claude
pub fn beta() {}
pub fn gamma() {}
EOF
git add src/lib.rs
git commit --no-verify -qm "chore: seed pre-existing debt"

# 1. PURE MOVE (the STORY-771 false-positive shape): both debt lines leave
#    lib.rs and reappear verbatim in a new split.rs, same staged diff → ALLOWED.
cat >src/lib.rs <<'EOF'
pub fn gamma() {}
EOF
cat >src/split.rs <<'EOF'
/// trace:TASK-101 | ai:claude
pub fn alpha() {}
/// trace:TASK-102 | ai:claude
pub fn beta() {}
EOF
git add -A
assert_exit "pure file-split move of /// trace debt is allowed" 0 "$(commit "refactor: split")"

# 2. GENUINELY NEW debt (no matching staged removal) → still REFUSED.
cat >>src/split.rs <<'EOF'
/// trace:BUG-999 | ai:claude
pub fn delta() {}
EOF
git add -A
assert_exit "genuinely new /// trace debt is refused" 1 "$(commit "feat: new debt")"
assert_stderr_contains "refusal names the new debt line" "BUG-999"
git reset -q --hard

# 3. MOVE + NEW debt in one commit → REFUSED, but ONLY the new line is flagged;
#    the moved line is excused.
cat >src/lib.rs <<'EOF'
/// trace:TASK-101 | ai:claude
pub fn alpha() {}
/// trace:BUG-777 | ai:claude
pub fn epsilon() {}
pub fn gamma() {}
EOF
cat >src/split.rs <<'EOF'
/// trace:TASK-102 | ai:claude
pub fn beta() {}
EOF
git add -A
assert_exit "move mixed with new debt is refused" 1 "$(commit "feat: mixed")"
assert_stderr_contains "refusal flags the new line" "BUG-777"
assert_stderr_lacks "refusal does NOT flag the moved line" "TASK-101"
git reset -q --hard

# 4. RE-INDENTED move: the line moves into an indented context (indentation
#    changes, content identical after trimming) → still a move → ALLOWED.
cat >src/lib.rs <<'EOF'
pub fn gamma() {}
pub mod inner {
    /// trace:TASK-102 | ai:claude
    pub fn beta() {}
}
EOF
cat >src/split.rs <<'EOF'
/// trace:TASK-101 | ai:claude
pub fn alpha() {}
EOF
git add -A
assert_exit "re-indented move is still recognized as a move" 0 "$(commit "refactor: reindent move")"

# 5. CREDIT CONSUMPTION: one removal cannot excuse TWO identical added copies —
#    the duplicate beyond the removal count is NEW debt → REFUSED.
cat >src/lib.rs <<'EOF'
pub fn gamma() {}
pub mod inner {
    pub fn beta() {}
}
EOF
cat >src/split.rs <<'EOF'
/// trace:TASK-102 | ai:claude
pub fn alpha() {}
/// trace:TASK-102 | ai:claude
pub fn zeta() {}
EOF
git add -A
assert_exit "duplicating a moved line beyond its removal count is refused" 1 "$(commit "feat: dup")"
git reset -q --hard

if [ "$fail" -ne 0 ]; then
    echo "PRE-COMMIT PROVENANCE MOVE GATE (TASK-144): FAILURES"
    exit 1
fi
echo "PRE-COMMIT PROVENANCE MOVE GATE (TASK-144): all checks passed"
