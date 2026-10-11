#!/bin/bash
# STORY-1647: the pre-commit hook refuses commits made in the PRIMARY checkout
# on main when the project opts in with `[hooks] block_primary_commits = true`
# in .aida/config.toml. Merges, rebase replays, the aida-store branch and
# linked worktrees stay exempt; AIDA_ALLOW_PRIMARY_COMMIT=1 is the operator
# override; a primary checkout on another branch is allowed with a warning.
#
# Drives the REAL template (aida-core/templates/hooks/aida-pre-commit.sh)
# installed as .git/hooks/pre-commit in throwaway repos, via real `git commit`.
#
# trace:STORY-1647 | ai:claude
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOK_TEMPLATE="$PROJECT_ROOT/aida-core/templates/hooks/aida-pre-commit.sh"

SCRATCH=$(mktemp -d)
FAKEHOME="$SCRATCH/home"
STDERR_LOG="$SCRATCH/stderr.txt"
STDOUT_LOG="$SCRATCH/stdout.txt"
mkdir -p "$FAKEHOME"
trap 'rm -rf "$SCRATCH"' EXIT
fail=0

# PATH without any aida binary, so the binary-backed gates (advisor-code-gate,
# locking-gate, scope guard) stay out of the picture; only git + coreutils.
HOOK_PATH="/usr/local/bin:/usr/bin:/bin"

# new_repo <dir> <opt-in: yes|no> — a primary checkout on main with one seed
# commit (made with --no-verify so the seed itself is not gated).
new_repo() {
    local dir="$1" opt_in="$2"
    mkdir -p "$dir"
    (
        cd "$dir"
        git init -q -b main
        git config user.email t@t.t
        git config user.name t
        mkdir -p .git/hooks .aida
        cp "$HOOK_TEMPLATE" .git/hooks/pre-commit
        chmod +x .git/hooks/pre-commit
        if [ "$opt_in" = "yes" ]; then
            printf '[deployment]\nmode = "distributed"\n\n[hooks]\nblock_primary_commits = true # opt in\n' >.aida/config.toml
        else
            printf '[deployment]\nmode = "distributed"\n' >.aida/config.toml
        fi
        printf '.aida/\n' >.gitignore
        echo seed >seed.txt
        git add .gitignore seed.txt
        git commit --no-verify -qm "seed"
    )
}

# commit_in <dir> <msg> [VAR=value...] — hermetic `git commit` of whatever is
# staged; echoes the exit code. Extra VAR=value args go into the env.
commit_in() {
    local dir="$1" msg="$2"
    shift 2
    set +e
    (cd "$dir" && env -i PATH="$HOOK_PATH" HOME="$FAKEHOME" "$@" \
        git -c user.email=t@t.t -c user.name=t commit -qm "$msg" \
        >"$STDOUT_LOG" 2>"$STDERR_LOG")
    local rc=$?
    set -e
    echo "$rc"
}

stage_change() {
    local dir="$1" file="$2"
    (cd "$dir" && echo "$RANDOM $file" >>"$file" && git add "$file")
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
    if grep -qF -- "$needle" "$STDERR_LOG"; then
        echo "ok   - $desc"
    else
        echo "FAIL - $desc (stderr lacks '$needle')"
        sed 's/^/       | /' "$STDERR_LOG"
        fail=1
    fi
}

assert_stderr_lacks() {
    local desc="$1" needle="$2"
    if grep -qF -- "$needle" "$STDERR_LOG"; then
        echo "FAIL - $desc (stderr unexpectedly contains '$needle')"
        sed 's/^/       | /' "$STDERR_LOG"
        fail=1
    else
        echo "ok   - $desc"
    fi
}

# --- refused_on_main_primary -------------------------------------------------
R="$SCRATCH/primary"
new_repo "$R" yes
stage_change "$R" a.txt
rc=$(commit_in "$R" "feat: straight onto main")
assert_exit "refused_on_main_primary: primary checkout on main is refused" 1 "$rc"
assert_stderr_contains "refused_on_main_primary: names the worktree command" "aida worktree add <SPEC>"
assert_stderr_contains "refused_on_main_primary: names the override" "AIDA_ALLOW_PRIMARY_COMMIT=1"
if [ "$(git -C "$R" rev-list --count HEAD)" = "1" ]; then
    echo "ok   - refused_on_main_primary: no commit was created"
else
    echo "FAIL - refused_on_main_primary: a commit landed despite the refusal"
    fail=1
fi

# --allow-intermediate's env bypass must NOT lift this gate.
rc=$(commit_in "$R" "feat: intermediate bypass" AIDA_ALLOW_INTERMEDIATE=1)
assert_exit "refused_on_main_primary: AIDA_ALLOW_INTERMEDIATE does not bypass it" 1 "$rc"

# --- override_allows_and_writes_sentinel -------------------------------------
rm -f "$R/.git/aida-precommit-sentinel"
rc=$(commit_in "$R" "chore: operator commit" AIDA_ALLOW_PRIMARY_COMMIT=1)
assert_exit "override_allows_and_writes_sentinel: override commits" 0 "$rc"
assert_stderr_contains "override_allows_and_writes_sentinel: says the override applied" "allowed by AIDA_ALLOW_PRIMARY_COMMIT=1"
if [ -f "$R/.git/aida-precommit-sentinel" ] \
    && [ "$(cat "$R/.git/aida-precommit-sentinel")" = "$(git -C "$R" rev-parse 'HEAD^{tree}')" ]; then
    echo "ok   - override_allows_and_writes_sentinel: TASK-917 sentinel matches the committed tree"
else
    echo "FAIL - override_allows_and_writes_sentinel: sentinel missing or stale (would read as --no-verify)"
    fail=1
fi

# --- merge_commit_allowed ----------------------------------------------------
git -C "$R" checkout -q -b side
stage_change "$R" side.txt
git -C "$R" commit --no-verify -qm "side work"
git -C "$R" checkout -q main
stage_change "$R" mainline.txt
git -C "$R" commit --no-verify -qm "main work"
git -C "$R" merge --no-ff --no-commit -q side >/dev/null 2>&1 || true
if git -C "$R" rev-parse -q --verify MERGE_HEAD >/dev/null; then
    rc=$(commit_in "$R" "Merge branch side")
    assert_exit "merge_commit_allowed: merge commit on main in the primary checkout" 0 "$rc"
    assert_stderr_lacks "merge_commit_allowed: no refusal text" "refusing commit in the primary checkout"
else
    echo "FAIL - merge_commit_allowed: fixture did not leave MERGE_HEAD"
    fail=1
fi

# --- rebase_replay_allowed ---------------------------------------------------
# A rebase replay running the hook on main: simulate the in-progress state
# (rebase-merge dir present) with HEAD still on main, the strictest shape.
mkdir -p "$R/.git/rebase-merge"
stage_change "$R" rebased.txt
rc=$(commit_in "$R" "replayed commit")
assert_exit "rebase_replay_allowed: rebase-merge in progress" 0 "$rc"
rmdir "$R/.git/rebase-merge"
mkdir -p "$R/.git/rebase-apply"
stage_change "$R" applied.txt
rc=$(commit_in "$R" "applied commit")
assert_exit "rebase_replay_allowed: rebase-apply in progress" 0 "$rc"
rmdir "$R/.git/rebase-apply"
# And a real `git rebase` of main (the replay runs with a detached HEAD and the
# rebase-merge state dir present) must complete with the gate on.
git -C "$R" checkout -q side
stage_change "$R" side2.txt
git -C "$R" commit --no-verify -qm "more side"
git -C "$R" checkout -q main
stage_change "$R" main2.txt
git -C "$R" commit --no-verify -qm "more main"
set +e
(cd "$R" && env -i PATH="$HOOK_PATH" HOME="$FAKEHOME" \
    git -c user.email=t@t.t -c user.name=t rebase -q --no-ff side \
    >"$STDOUT_LOG" 2>"$STDERR_LOG")
rc=$?
set -e
assert_exit "rebase_replay_allowed: real git rebase of main completes" 0 "$rc"

# --- aida_store_branch_allowed -----------------------------------------------
git -C "$R" checkout -q -b aida-store
stage_change "$R" store.txt
rc=$(commit_in "$R" "store: write")
assert_exit "aida_store_branch_allowed: aida-store branch in the primary checkout" 0 "$rc"
assert_stderr_lacks "aida_store_branch_allowed: no warning" "warning: committing in the primary checkout"
git -C "$R" checkout -q main

# .aida-store linked worktree on its own branch.
git -C "$R" worktree add -q -b store-wt "$R/.aida-store" >/dev/null 2>&1
stage_change "$R/.aida-store" obj.yaml
rc=$(commit_in "$R/.aida-store" "store: object")
assert_exit "aida_store_branch_allowed: .aida-store worktree commit" 0 "$rc"

# --- worktree_feature_branch_allowed_silently --------------------------------
W="$SCRATCH/primary-story1"
git -C "$R" worktree add -q -b story-1-work "$W" >/dev/null 2>&1
mkdir -p "$W/.aida"
cp "$R/.aida/config.toml" "$W/.aida/config.toml"
stage_change "$W" feature.txt
rc=$(commit_in "$W" "feat: in a worktree")
assert_exit "worktree_feature_branch_allowed_silently: sibling worktree commit" 0 "$rc"
assert_stderr_lacks "worktree_feature_branch_allowed_silently: no refusal" "refusing commit"
assert_stderr_lacks "worktree_feature_branch_allowed_silently: no warning" "primary checkout"

# --- primary_non_main_warns --------------------------------------------------
git -C "$R" checkout -q -b hotfix
stage_change "$R" hotfix.txt
rc=$(commit_in "$R" "fix: on a branch in the primary checkout")
assert_exit "primary_non_main_warns: non-main branch in the primary checkout is allowed" 0 "$rc"
assert_stderr_contains "primary_non_main_warns: prints the warning" "warning: committing in the primary checkout on branch hotfix"
if [ "$(grep -c 'primary checkout' "$STDERR_LOG")" = "1" ]; then
    echo "ok   - primary_non_main_warns: warning is one line"
else
    echo "FAIL - primary_non_main_warns: expected exactly one warning line"
    fail=1
fi
git -C "$R" checkout -q main

# --- origin_default_branch_protected -----------------------------------------
# A project whose default branch is not `main` (origin/HEAD -> trunk).
T="$SCRATCH/trunk"
new_repo "$T" yes
git -C "$T" branch -q -m main trunk
git -C "$T" update-ref refs/remotes/origin/trunk HEAD
git -C "$T" symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/trunk
stage_change "$T" t.txt
rc=$(commit_in "$T" "feat: onto trunk")
assert_exit "origin_default_branch_protected: origin/HEAD branch is refused" 1 "$rc"

# --- gate_inert_without_opt_in -----------------------------------------------
N="$SCRATCH/consumer"
new_repo "$N" no
stage_change "$N" n.txt
rc=$(commit_in "$N" "feat: consumer project on main")
assert_exit "gate_inert_without_opt_in: no [hooks] opt-in, commit on main allowed" 0 "$rc"
assert_stderr_lacks "gate_inert_without_opt_in: silent" "primary checkout"

# Opt-in set to false is also inert; a `block_primary_commits` key in another
# section does not count.
printf '[hooks]\nblock_primary_commits = false\n[other]\nblock_primary_commits = true\n' >"$N/.aida/config.toml"
stage_change "$N" n.txt
rc=$(commit_in "$N" "feat: consumer, explicit false")
assert_exit "gate_inert_without_opt_in: false / wrong-section key stays inert" 0 "$rc"

# No .aida/config.toml at all (plain repo with the hook) is inert.
rm -f "$N/.aida/config.toml"
stage_change "$N" n.txt
rc=$(commit_in "$N" "feat: no config")
assert_exit "gate_inert_without_opt_in: no config file stays inert" 0 "$rc"

if [ "$fail" -ne 0 ]; then
    echo "test_precommit_primary_checkout: FAILED"
    exit 1
fi
echo "test_precommit_primary_checkout: all checks passed"
