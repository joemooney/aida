#!/usr/bin/env bash
# Matrix test for aida-git-guardrails.sh force-push TARGET detection (BUG-661).
#
# The guardrails hook must block a force-push only when the ACTUAL push target
# is a protected branch (main / the repo default / aida-store / master / develop)
# — and must keep allowing legitimate feature-branch force-pushes, including the
# post-rebase push of a feature branch that lives in a worktree while the agent's
# CWD sits on `main`. Unparseable targets fail CLOSED (block).
#
# Run: bash scripts/test-git-guardrails.sh
set -uo pipefail

HOOK="$(cd "$(dirname "$0")/.." && pwd)/aida-core/templates/hooks/aida-git-guardrails.sh"
if [ ! -f "$HOOK" ]; then
    echo "hook not found: $HOOK" >&2
    exit 1
fi

PASS=0
FAIL=0

# check LABEL EXPECTED(allow|block) DIR COMMAND
check() {
    local label="$1" expected="$2" dir="$3" cmd="$4"
    local rc verdict
    rc=$( cd "$dir" && printf '{"tool_input":{"command":"%s"}}' "$cmd" | bash "$HOOK" >/dev/null 2>&1; echo $? )
    case "$rc" in
        0) verdict=allow ;;
        2) verdict=block ;;
        *) verdict="error(rc=$rc)" ;;
    esac
    if [ "$verdict" = "$expected" ]; then
        printf 'PASS [%-5s] %s\n' "$expected" "$label"
        PASS=$((PASS + 1))
    else
        printf 'FAIL [exp:%s got:%s] %s\n        cmd: %s\n' "$expected" "$verdict" "$label" "$cmd"
        FAIL=$((FAIL + 1))
    fi
}

SANDBOX=$(mktemp -d)
cleanup() { rm -rf "$SANDBOX"; }
trap cleanup EXIT

# --- Primary repo: default branch main, plus aida-store + feature branches ---
git init -q -b main "$SANDBOX/repo"
REPO="$SANDBOX/repo"
git -C "$REPO" config user.email t@t.test
git -C "$REPO" config user.name tester
git -C "$REPO" commit -q --allow-empty -m init
git -C "$REPO" branch aida-store
git -C "$REPO" branch feature-x
# A feature branch checked out in a SEPARATE worktree (CWD will stay on main).
git -C "$REPO" worktree add -q -b feature-y "$SANDBOX/wt-feature" >/dev/null 2>&1
WT="$SANDBOX/wt-feature"
# A detached-HEAD worktree, to exercise the fail-closed path.
git -C "$REPO" worktree add -q --detach "$SANDBOX/wt-detached" >/dev/null 2>&1
WTD="$SANDBOX/wt-detached"

# --- Secondary repo: default branch is 'trunk' (not main), via origin/HEAD ---
git init -q -b trunk "$SANDBOX/dflt"
DFLT="$SANDBOX/dflt"
git -C "$DFLT" config user.email t@t.test
git -C "$DFLT" config user.name tester
git -C "$DFLT" commit -q --allow-empty -m init
git -C "$DFLT" branch feature-z
git init -q --bare "$SANDBOX/dflt-origin.git"
git -C "$DFLT" remote add origin "$SANDBOX/dflt-origin.git"
git -C "$DFLT" push -q origin trunk >/dev/null 2>&1
git -C "$DFLT" remote set-head origin trunk >/dev/null 2>&1

echo "== MUST BLOCK: force-push to a protected branch =="
check "force-push main"                         block "$REPO" "git push --force origin main"
check "force-push main (--force-with-lease)"    block "$REPO" "git push --force-with-lease origin main"
check "force-push main (-f short flag)"         block "$REPO" "git push -f origin main"
check "force-push main (trailing -f)"           block "$REPO" "git push origin main -f"
check "force-push HEAD:main refspec"            block "$REPO" "git push --force origin HEAD:main"
check "force-push feat:main (dst protected)"    block "$REPO" "git push --force origin feature-x:main"
check "force-push +main (refspec + marker)"     block "$REPO" "git push origin +main"
check "force-push aida-store"                   block "$REPO" "git push --force origin aida-store"
check "force-push master"                       block "$REPO" "git push --force origin master"
check "force-push develop"                      block "$REPO" "git push --force origin develop"
check "force-push bare from main checkout"      block "$REPO" "git push --force-with-lease"
check "chained cd && force-push main"           block "$REPO" "cd /x && git push --force origin main"
check "default-branch trunk (dynamic default)"  block "$DFLT" "git push --force origin trunk"
check "default-branch trunk bare push"          block "$DFLT" "git push --force-with-lease"

echo
echo "== MUST ALLOW: legitimate feature-branch force-pushes =="
check "force-push feature (explicit refspec)"   allow "$REPO" "git push --force-with-lease origin feature-x"
check "plain --force feature → lease nudge"     block "$REPO" "git push --force origin feature-x"
check "force-push feature from WORKTREE (-C bare)" allow "$REPO" "git -C $WT push --force-with-lease"
check "force-push feature from worktree (-C +refspec)" allow "$REPO" "git -C $WT push --force-with-lease origin feature-y"
check "lease ref names main, target is feature" allow "$REPO" "git push --force-with-lease=main origin feature-x"
check "force-push main:feat (dst is feature)"   allow "$REPO" "git push --force-with-lease origin main:feature-x"
check "force-push substring story-610-main-fix" allow "$REPO" "git push --force-with-lease origin story-610-main-fix"
check "chained status && force-push feature"    allow "$REPO" "git status && git push --force-with-lease origin feature-x"
check "force-push feature-z in trunk repo"      allow "$DFLT" "git push --force-with-lease origin feature-z"

echo
echo "== MUST ALLOW: non-force pushes =="
check "normal push main (no force)"             allow "$REPO" "git push origin main"
check "normal push feature (no force)"          allow "$REPO" "git push origin feature-x"
check "normal bare push from main"              allow "$REPO" "git push"

echo
echo "== MUST BLOCK: unparseable target fails CLOSED =="
check "bare force-push from DETACHED worktree"  block "$REPO" "git -C $WTD push --force-with-lease"

# --- Branch deletion (BUG-662) ---------------------------------------------
echo
echo "== MUST ALLOW: deleting a feature branch =="
check "delete feature -D"                       allow "$REPO" "git branch -D feature-x"
check "delete feature -d"                        allow "$REPO" "git branch -d feature-x"
check "delete feature --delete"                  allow "$REPO" "git branch --delete feature-x"
check "delete feature --delete --force"          allow "$REPO" "git branch --delete --force feature-x"
check "delete two feature branches"              allow "$REPO" "git branch -D feature-x feature-y"
check "delete feature from WORKTREE (-C)"        allow "$REPO" "git -C $WT branch -D feature-y"
check "delete substring story-610-main-fix"      allow "$REPO" "git branch -D story-610-main-fix"
check "chained status && delete feature"         allow "$REPO" "git status && git branch -D feature-x"
check "delete feature-z in trunk repo"           allow "$DFLT" "git branch -D feature-z"

echo
echo "== MUST ALLOW: non-delete branch commands =="
check "create branch (no flag)"                  allow "$REPO" "git branch new-feature"
check "rename branch (-m)"                        allow "$REPO" "git branch -m old-name new-name"
check "list branches (-a)"                        allow "$REPO" "git branch -a"

echo
echo "== MUST BLOCK: deleting a protected branch =="
check "delete main (-D)"                          block "$REPO" "git branch -D main"
check "delete main (-d)"                          block "$REPO" "git branch -d main"
check "delete main (--delete)"                    block "$REPO" "git branch --delete main"
check "delete aida-store"                         block "$REPO" "git branch -D aida-store"
check "delete master"                             block "$REPO" "git branch -D master"
check "delete develop"                            block "$REPO" "git branch -D develop"
check "delete feature + main (one protected)"    block "$REPO" "git branch -D feature-x main"
check "delete default branch trunk (dynamic)"    block "$DFLT" "git branch -D trunk"
check "chained delete feature && delete main"    block "$REPO" "git branch -D feature-x && git branch -D main"

echo
echo "== MUST BLOCK: unparseable delete fails CLOSED =="
check "delete with no branch name (-D)"           block "$REPO" "git branch -D"
check "delete with no branch name (--delete)"     block "$REPO" "git branch --delete"

echo
echo "----------------------------------------"
echo "PASS=$PASS FAIL=$FAIL"
if [ "$FAIL" -ne 0 ]; then
    exit 1
fi
echo "ALL GREEN"
