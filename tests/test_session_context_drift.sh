#!/bin/bash
# Primary-checkout drift line in the session-context SessionStart hook.
# Runs the template master against throwaway git fixtures with a stub `aida`
# on PATH; no cargo build, no network. trace:STORY-1648 | ai:claude
set -u

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOK="$ROOT/aida-core/templates/hooks/aida-session-context.sh"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
fail=0

# Isolate from the host's git config (global excludes, signing, identity).
export HOME="$TMP/home"
mkdir -p "$HOME"
export GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.com
export GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.com

# Stub `aida`: the hook's queue/in-flight reads come back empty.
mkdir -p "$TMP/bin"
printf '#!/bin/sh\nexit 0\n' >"$TMP/bin/aida"
chmod +x "$TMP/bin/aida"

# fixture <name>: a primary checkout on main, pushed to a bare origin, clean.
fixture() {
    local d="$TMP/$1"
    git init -q --bare -b main "$d/origin.git"
    git init -q -b main "$d/primary"
    printf 'one\n' >"$d/primary/tracked.txt"
    git -C "$d/primary" add tracked.txt
    git -C "$d/primary" -c commit.gpgsign=false commit -q -m init
    git -C "$d/primary" remote add origin "$d/origin.git"
    git -C "$d/primary" push -q -u origin main 2>/dev/null
    printf '%s\n' "$d"
}

# run_hook <project dir> [extra env...]: the hook's stdout.
run_hook() {
    local dir="$1"
    shift
    (cd "$dir" && env -u AIDA_SESSION_PROJECT -u CODEX_PROJECT_DIR \
        CLAUDE_PROJECT_DIR="$dir" PATH="$TMP/bin:$PATH" "$@" bash "$HOOK")
}

drift_line() { run_hook "$@" | grep '^main checkout:' || true; }

assert_eq() {
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

# 1. Clean: no extra line, and the digest header is still printed.
d=$(fixture clean)
assert_eq "clean checkout prints no drift line" "" "$(drift_line "$d/primary")"
assert_eq "clean checkout still prints the digest header" \
    "AIDA · role: none · queue: 0 · in-flight: 0" \
    "$(run_hook "$d/primary" AIDA_SESSION_ROLE=none | head -n 1)"

# 2. Tracked-dirty.
d=$(fixture tracked)
printf 'two\n' >"$d/primary/tracked.txt"
assert_eq "tracked-dirty" \
    "main checkout: 1 tracked change, 0 untracked · on main · up to date with origin" \
    "$(drift_line "$d/primary")"

# 3. Untracked-only.
d=$(fixture untracked)
printf 'x\n' >"$d/primary/fix_1.py"
printf 'x\n' >"$d/primary/run.log"
assert_eq "untracked-only" \
    "main checkout: 0 tracked changes, 2 untracked · on main · up to date with origin" \
    "$(drift_line "$d/primary")"

# 4. Off main (a fresh local branch with no upstream).
d=$(fixture offmain)
git -C "$d/primary" checkout -q -b feature-x
assert_eq "off-main" \
    "main checkout: 0 tracked changes, 0 untracked · on feature-x · no upstream" \
    "$(drift_line "$d/primary")"

# 5. Behind origin: another clone pushes, the primary fetches.
d=$(fixture behind)
git clone -q "$d/origin.git" "$d/other" 2>/dev/null
printf 'later\n' >"$d/other/later.txt"
git -C "$d/other" add later.txt
git -C "$d/other" -c commit.gpgsign=false commit -q -m later
git -C "$d/other" push -q origin main 2>/dev/null
git -C "$d/primary" fetch -q origin
assert_eq "behind origin" \
    "main checkout: 0 tracked changes, 0 untracked · on main · 1 behind origin" \
    "$(drift_line "$d/primary")"

# 6. Run from a linked worktree: still reports the PRIMARY checkout.
d=$(fixture worktree)
git -C "$d/primary" worktree add -q -b wt-branch "$d/wt" 2>/dev/null
printf 'x\n' >"$d/primary/scratch.py"
assert_eq "linked worktree reports the primary checkout" \
    "main checkout: 0 tracked changes, 1 untracked · on main · up to date with origin" \
    "$(drift_line "$d/wt")"
assert_eq "opt-out silences the line" "" \
    "$(drift_line "$d/primary" AIDA_MAIN_CHECKOUT_DRIFT=off)"

# 7. Cost: the drift probe adds under 50 ms (mean over several runs,
# compared against the same hook with the probe switched off).
d=$(fixture timing)
printf 'two\n' >"$d/primary/tracked.txt"
printf 'x\n' >"$d/primary/scratch.py"
mean_ms() {
    local runs=7 total=0 i start end
    for i in $(seq "$runs"); do
        start=$(date +%s%N)
        run_hook "$d/primary" "$@" >/dev/null
        end=$(date +%s%N)
        total=$((total + (end - start) / 1000000))
    done
    echo $((total / runs))
}
mean_ms >/dev/null # warm the filesystem cache
with_ms=$(mean_ms)
without_ms=$(mean_ms AIDA_MAIN_CHECKOUT_DRIFT=off)
added=$((with_ms - without_ms))
if [ "$added" -lt 50 ]; then
    echo "ok   - drift probe adds ${added}ms (< 50ms; ${with_ms}ms vs ${without_ms}ms)"
else
    echo "FAIL - drift probe adds ${added}ms (>= 50ms; ${with_ms}ms vs ${without_ms}ms)"
    fail=1
fi

[ "$fail" -eq 0 ] || exit 1
echo "SESSION CONTEXT DRIFT: all checks passed"
