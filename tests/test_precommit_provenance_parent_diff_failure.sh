#!/bin/bash
# BUG-1933: in merge mode the pre-commit `///`-provenance gate diffs the staged
# tree against every parent. A parent whose `git diff --cached` fails must
# refuse the commit and name that parent. It must not count as "no offenders"
# from that parent. Two shapes:
#   1. a parent commit is present but one of its blobs is not (a partial clone
#      offline): git can still record the merge, so the old hook let it
#      through unchecked;
#   2. MERGE_HEAD names a commit the clone does not have at all.
# A healthy merge still goes through. Then, outside a merge:
#   3. a `diff.external` driver or `-diff` attribute cannot hide new debt;
#   4. a `git diff` killed mid-line still reports its failure.
#
# Drives the REAL template (aida-core/templates/hooks/aida-pre-commit.sh)
# installed as .git/hooks/pre-commit in a throwaway repo, via real `git commit`.
#
# trace:BUG-1933 | ai:claude
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOK_TEMPLATE="$PROJECT_ROOT/aida-core/templates/hooks/aida-pre-commit.sh"
MISSING=0123456789abcdef0123456789abcdef01234567

TMP=$(mktemp -d)
FAKEHOME=$(mktemp -d)
STDERR_LOG="$FAKEHOME/stderr.txt"
trap 'rm -rf "$TMP" "$FAKEHOME"' EXIT
fail=0

# A no-op `cargo` keeps the hook's rustfmt step out of the way.
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

# commit <msg> — a real hermetic `git commit`; echoes the exit code.
commit() {
    set +e
    env -i PATH="$SHIMS:$PATH" HOME="$FAKEHOME" \
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
        [ -s "$STDERR_LOG" ] && head -n 20 "$STDERR_LOG" | sed 's/^/       | /'
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

echo 'pub fn seed() {}' >src/seed.rs
git add -A
git commit --no-verify -qm "chore: seed"
git branch side
echo 'pub fn from_main() {}' >src/main_side.rs
git add -A
git commit --no-verify -qm "feat(main): main work"
git checkout -q side
echo 'pub fn from_side() {}' >src/side.rs
git add -A
git commit --no-verify -qm "feat(side): side work"
git checkout -q main

# 1. A conflicted merge of src/shared.rs whose side-version blob then goes
#    missing: the diff against `side` cannot read it.
echo 'pub fn shared() -> u32 { 1 }' >src/shared.rs
git add -A
git commit --no-verify -qm "chore: seed shared"
git branch side-blob
echo 'pub fn shared() -> u32 { 2 }' >src/shared.rs
git commit --no-verify -qam "feat(main): shared 2"
git checkout -q side-blob
echo 'pub fn shared() -> u32 { 3 }' >src/shared.rs
git commit --no-verify -qam "feat(side): shared 3"
side_blob=$(git rev-parse side-blob:src/shared.rs)
git checkout -q main
set +e
git merge -q side-blob >/dev/null 2>&1
set -e
echo 'pub fn shared() -> u32 { 5 }' >src/shared.rs
git add -A
rm -f ".git/objects/${side_blob:0:2}/${side_blob:2}"
if git cat-file -e "$side_blob" 2>/dev/null; then
    echo "FAIL - setup: the side blob is still readable"
    fail=1
fi
side_tip=$(git rev-parse side-blob)
assert_exit "a merge parent with a missing blob refuses the commit" 1 "$(commit "merge side-blob")"
assert_stderr_contains "the refusal names the parent whose diff failed" "against $side_tip failed"
assert_stderr_contains "the refusal shows git's own reason for the missing blob" "  git: "
# If the hook regressed and committed, there is no merge to abort: undo the
# commit instead, so the remaining cases still run and report.
git merge --abort 2>/dev/null || git reset -q --hard HEAD~1
git branch -q -D side-blob

# 2. MERGE_HEAD also names a commit this clone does not have.
git merge -q --no-ff --no-commit side >/dev/null 2>&1
echo "$MISSING" >>"$(git rev-parse --git-path MERGE_HEAD)"
assert_exit "a merge parent whose diff fails refuses the commit" 1 "$(commit "merge side")"
assert_stderr_contains "the refusal names the failing parent" "against $MISSING failed"

# The same merge with only real parents goes through.
git rev-parse side >"$(git rev-parse --git-path MERGE_HEAD)"
assert_exit "a merge whose parent diffs all succeed is allowed" 0 "$(commit "merge side")"

# 3. A user's `diff.external` (difftastic and friends) must not replace the
#    unified diff the scan parses: an empty external diff would hide new debt.
printf '#!/bin/sh\nexit 0\n' >"$FAKEHOME/empty-ext-diff"
chmod +x "$FAKEHOME/empty-ext-diff"
git config diff.external "$FAKEHOME/empty-ext-diff"
printf '/// trace:BUG-4251 | ai:claude\npub fn ext() {}\n' >src/ext.rs
git add -A
assert_exit "new debt is refused even with diff.external set" 1 "$(commit "feat: ext")"
assert_stderr_contains "the refusal names the debt the external diff would hide" "BUG-4251"
git config --unset diff.external
git rm -q --cached src/ext.rs
rm src/ext.rs

# 3b. A `-diff` (binary) attribute on `*.rs` would print "Binary files ...
#     differ" with no `+` lines; `--text` keeps the debt visible.
echo '*.rs -diff' >.git/info/attributes
printf '/// trace:BUG-4252 | ai:claude\npub fn bin() {}\n' >src/bin.rs
git add -A
assert_exit "new debt is refused even with a -diff attribute on *.rs" 1 "$(commit "feat: bin")"
assert_stderr_contains "the refusal names the debt the binary diff would hide" "BUG-4252"
rm .git/info/attributes
git rm -q --cached src/bin.rs
rm src/bin.rs

# 4. A `git diff` killed mid-line: its partial last line must not swallow the
#    failure marker, so the scan still sees the failure.
REAL_GIT=$(command -v git)
KILLED="$FAKEHOME/killed"
mkdir -p "$KILLED"
cat >"$KILLED/git" <<SHIM
#!/bin/sh
case " \$* " in
    *" --unified=0 "*) printf '+pub fn partial'; exit 1 ;;
esac
exec "$REAL_GIT" "\$@"
SHIM
chmod +x "$KILLED/git"
echo 'pub fn killed() {}' >src/killed.rs
git add -A
# `git commit` puts git's exec-path ahead of PATH for hooks, which would skip
# the shim, so run the installed hook directly.
set +e
env -i PATH="$KILLED:$SHIMS:$PATH" HOME="$FAKEHOME" \
    bash .git/hooks/pre-commit >/dev/null 2>"$STDERR_LOG"
rc=$?
set -e
assert_exit "a git diff killed mid-line refuses the commit" 1 "$rc"
assert_stderr_contains "the killed diff is reported as failed" "against index failed"

if [ "$fail" -ne 0 ]; then
    echo "PRE-COMMIT PROVENANCE PARENT DIFF FAILURE (BUG-1933): FAILURES"
    exit 1
fi
echo "PRE-COMMIT PROVENANCE PARENT DIFF FAILURE (BUG-1933): all checks passed"
