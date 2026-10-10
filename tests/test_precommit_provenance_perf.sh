#!/bin/bash
# BUG-1925: the pre-commit `///`-provenance gate must stay fast on huge diffs
# and must not re-check content a merge brings in from a parent.
#
# 1. PERF: a staged move of a ~250k-line .rs file (with 25k `/// trace:` debt
#    lines to exercise move credits) finishes the hook in under 10 s. The old
#    gate spawned `printf | grep` per diff line and scanned the removal-credit
#    array linearly per added candidate, which held the TASK-1538 lib.rs split
#    merge for 20+ minutes. The scan also reports progress on stderr.
# 2. MERGE DELTA: with MERGE_HEAD present, only lines that differ from every
#    parent (the conflict resolution) are checked. Debt that already landed on
#    the merged-in branch is not re-checked; new debt typed into the resolution
#    is still refused.
#
# Drives the REAL template (aida-core/templates/hooks/aida-pre-commit.sh)
# installed as .git/hooks/pre-commit in a throwaway repo, via real `git commit`.
#
# trace:BUG-1925 | ai:claude
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOK_TEMPLATE="$PROJECT_ROOT/aida-core/templates/hooks/aida-pre-commit.sh"
BUDGET_SECONDS=10
MOVE_LINES=250000

TMP=$(mktemp -d)
FAKEHOME=$(mktemp -d)
STDERR_LOG="$FAKEHOME/stderr.txt"
STDOUT_LOG="$FAKEHOME/stdout.txt"
trap 'rm -rf "$TMP" "$FAKEHOME"' EXIT
fail=0

# A no-op `cargo` keeps the hook's rustfmt step out of the measurement, so the
# timing is the provenance gate's.
SHIMS="$FAKEHOME/shims"
mkdir -p "$SHIMS"
printf '#!/bin/sh\nexit 0\n' >"$SHIMS/cargo"
chmod +x "$SHIMS/cargo"

cd "$TMP"
git init -q -b main
git config user.email t@t.t
git config user.name t
mkdir -p .git/hooks src
cp "$HOOK_TEMPLATE" .git/hooks/pre-commit
chmod +x .git/hooks/pre-commit

# commit <msg> — a real hermetic `git commit`; echoes the exit code. Output is
# captured OUTSIDE the repo so `git add -A` never stages it.
commit() {
    set +e
    env -i PATH="$SHIMS:$PATH" HOME="$FAKEHOME" \
        git -c user.email=t@t.t -c user.name=t commit -qm "$1" \
        >"$STDOUT_LOG" 2>"$STDERR_LOG"
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
        [ -s "$STDERR_LOG" ] && head -n 40 "$STDERR_LOG" | sed 's/^/       | /'
        fail=1
    fi
}

assert_stderr_contains() {
    local desc="$1" needle="$2"
    if grep -q -- "$needle" "$STDERR_LOG"; then
        echo "ok   - $desc"
    else
        echo "FAIL - $desc (stderr lacks '$needle')"
        fail=1
    fi
}

assert_stderr_lacks() {
    local desc="$1" needle="$2"
    if grep -q -- "$needle" "$STDERR_LOG"; then
        echo "FAIL - $desc (stderr unexpectedly contains '$needle')"
        fail=1
    else
        echo "ok   - $desc"
    fi
}

# ---------------------------------------------------------------------------
# 1. PERF: ~250k-line move.
# ---------------------------------------------------------------------------
# Seed: one huge file, every 10th line a `/// trace:` debt line (unique ids, so
# each moved debt line needs its own credit). Committed with --no-verify, the
# way such debt predates the gate.
awk -v n="$MOVE_LINES" 'BEGIN {
    for (i = 1; i <= n; i++) {
        if (i % 10 == 1) printf "/// trace:TASK-%d | ai:claude\n", i
        else printf "pub fn f%d() {}\n", i
    }
}' >src/big.rs
git add -A
git commit --no-verify -qm "chore: seed huge file"

# Move: delete big.rs and re-add every line indented inside a module, so no
# line is byte-identical and git's rename detection cannot collapse the diff;
# every line is a real `-`/`+` pair (~500k diff lines in total).
{
    echo "pub mod moved {"
    sed 's/^/    /' src/big.rs
    echo "}"
} >src/moved.rs
rm src/big.rs
git add -A

started=$SECONDS
rc=$(commit "refactor: move huge file")
elapsed=$((SECONDS - started))
assert_exit "250k-line move of /// trace debt is allowed" 0 "$rc"
if [ "$elapsed" -lt "$BUDGET_SECONDS" ]; then
    echo "ok   - 250k-line move hook finished in ${elapsed}s (< ${BUDGET_SECONDS}s)"
else
    echo "FAIL - 250k-line move hook took ${elapsed}s (budget ${BUDGET_SECONDS}s)"
    fail=1
fi
assert_stderr_contains "large scan reports progress on stderr" "provenance scan:"

# The same move plus one genuinely new debt line is still refused, quickly,
# and names only the new line.
git rm -q src/moved.rs
mkdir -p src
awk -v n="$MOVE_LINES" 'BEGIN {
    for (i = 1; i <= n; i++) {
        if (i % 10 == 1) printf "/// trace:TASK-%d | ai:claude\n", i
        else printf "pub fn f%d() {}\n", i
    }
}' >src/big.rs
git add -A
git commit --no-verify -qm "chore: restore huge file"
{
    echo "pub mod moved {"
    sed 's/^/    /' src/big.rs
    echo "    /// trace:BUG-4242 | ai:claude"
    echo "    pub fn brand_new() {}"
    echo "}"
} >src/moved.rs
rm src/big.rs
git add -A
started=$SECONDS
rc=$(commit "feat: move plus new debt")
elapsed=$((SECONDS - started))
assert_exit "250k-line move plus one new debt line is refused" 1 "$rc"
assert_stderr_contains "refusal names the new debt line" "BUG-4242"
assert_stderr_lacks "refusal does NOT name a moved line" "TASK-1 |"
if [ "$elapsed" -lt "$BUDGET_SECONDS" ]; then
    echo "ok   - refusing 250k-line move finished in ${elapsed}s (< ${BUDGET_SECONDS}s)"
else
    echo "FAIL - refusing 250k-line move took ${elapsed}s (budget ${BUDGET_SECONDS}s)"
    fail=1
fi
git reset -q --hard
git rm -q src/big.rs
git commit --no-verify -qm "chore: drop huge file"
mkdir -p src

# ---------------------------------------------------------------------------
# 2. MERGE DELTA.
# ---------------------------------------------------------------------------
cat >src/shared.rs <<'EOF'
pub fn shared() -> u32 {
    1
}
EOF
git add -A
git commit --no-verify -qm "chore: seed shared"
git branch feature

# main lands debt (with --no-verify, as pre-existing debt does) in a new file
# and in a line of shared.rs that will conflict with the feature branch.
cat >src/main_side.rs <<'EOF'
/// trace:TASK-900 | ai:claude
pub fn from_main() {}
EOF
cat >src/shared.rs <<'EOF'
/// trace:TASK-902 | ai:claude
pub fn shared() -> u32 {
    2
}
EOF
git add -A
git commit --no-verify -qm "feat(main): land debt"

git checkout -q feature
cat >src/feature_side.rs <<'EOF'
pub fn from_feature() {}
EOF
git add -A
assert_exit "feature commit without debt is allowed" 0 "$(commit "feat: feature work")"

# 2a. A clean merge of main: everything main brought in already landed there,
#     so it is not re-checked, even though it is provenance debt.
git merge -q --no-ff --no-commit main >/dev/null 2>&1
assert_exit "clean merge does not re-check debt landed on the merged parent" 0 "$(commit "merge main")"
assert_stderr_contains "merge mode is announced on stderr" "merge commit"
assert_stderr_lacks "debt from the merged parent is not flagged" "TASK-900"

# 2b. A conflicted merge: the resolution keeps main's side verbatim (its debt
#     line is not new) and ALSO types a new debt line, which is refused.
git checkout -q main
cat >src/shared.rs <<'EOF'
/// trace:TASK-902 | ai:claude
pub fn shared() -> u32 {
    3
}
EOF
git add -A
git commit --no-verify -qm "feat(main): change shared again"
git checkout -q feature
cat >src/shared.rs <<'EOF'
pub fn shared() -> u32 {
    40
}
EOF
git add -A
assert_exit "feature edit to shared is allowed" 0 "$(commit "feat: feature edits shared")"

set +e
git merge -q main >/dev/null 2>&1
merge_rc=$?
set -e
if [ "$merge_rc" -eq 0 ]; then
    echo "FAIL - setup: expected the merge of main to conflict"
    fail=1
fi
cat >src/shared.rs <<'EOF'
/// trace:TASK-902 | ai:claude
/// trace:BUG-901 | ai:claude
pub fn shared() -> u32 {
    43
}
EOF
git add -A
assert_exit "new debt typed into a conflict resolution is refused" 1 "$(commit "merge main (conflict)")"
assert_stderr_contains "refusal names the resolution's new debt" "BUG-901"
assert_stderr_lacks "refusal does NOT name main's side kept verbatim" "TASK-902"
assert_stderr_lacks "refusal does NOT name debt landed on main" "TASK-900"

# Fixing the resolution lets the merge commit through.
cat >src/shared.rs <<'EOF'
/// trace:TASK-902 | ai:claude
pub fn shared() -> u32 {
    43
}
EOF
git add -A
assert_exit "a clean conflict resolution is allowed" 0 "$(commit "merge main (resolved)")"

if [ "$fail" -ne 0 ]; then
    echo "PRE-COMMIT PROVENANCE PERF + MERGE DELTA (BUG-1925): FAILURES"
    exit 1
fi
echo "PRE-COMMIT PROVENANCE PERF + MERGE DELTA (BUG-1925): all checks passed"
