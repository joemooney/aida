#!/usr/bin/env bash
# tests/test_install_agent_skill_hooks.sh — BUG-1762 fixtures for the
# agent-skill-hook installer (scripts/install-agent-skill-hooks.sh).
#
# BUG-1760 shipped the installer target but nothing invoked it, so the
# regeneration half of the pack drift fix stayed inert in every clone. The fix
# runs the installer as a build prerequisite in --best-effort mode. These
# fixtures pin the behaviours that make that safe:
#   - a fresh main checkout gets the three symlinks (loud and best-effort)
#   - re-running is idempotent: no change, exit 0
#   - a pre-existing foreign hook is never clobbered — loud mode refuses with
#     exit 1, best-effort warns and skips with exit 0; the other hooks still
#     install either way
#   - a linked worktree is refused loudly, and skipped silently in
#     best-effort mode (builds run from worktrees constantly)
#   - an unwritable hooks dir (or a filesystem without symlinks) cannot fail
#     a best-effort run — the reviewer-found set -e escape
#   - outside a git repository, best-effort is a silent no-op
#
# trace:BUG-1762 | ai:claude
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
installer="$repo_root/scripts/install-agent-skill-hooks.sh"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

# A fixture repo with the hook master in place, so the installer's symlinks
# have a real target. Prints the repo path.
make_fixture() {
    local repo="$work/$1"
    git init -q "$repo"
    git -C "$repo" config user.name fixture
    git -C "$repo" config user.email fixture@example.invalid
    mkdir -p "$repo/aida-core/templates/hooks"
    printf '#!/bin/sh\nexit 0\n' >"$repo/aida-core/templates/hooks/aida-sync-agent-skills.sh"
    chmod +x "$repo/aida-core/templates/hooks/aida-sync-agent-skills.sh"
    echo "$repo"
}

assert_installed() {
    local repo="$1" name link
    for name in post-merge post-checkout post-rewrite; do
        link="$repo/.git/hooks/$name"
        [ -L "$link" ] || fail "$name is not a symlink in $repo"
        [ "$(readlink "$link")" = "$repo/aida-core/templates/hooks/aida-sync-agent-skills.sh" ] ||
            fail "$name points at $(readlink "$link"), not the hook master"
    done
}

echo "==> fresh checkout: loud mode installs all three hooks"
repo=$(make_fixture fresh-loud)
(cd "$repo" && bash "$installer") >/dev/null || fail "loud install exited non-zero on a fresh repo"
assert_installed "$repo"

echo "==> fresh checkout: best-effort mode installs all three hooks"
repo=$(make_fixture fresh-quiet)
(cd "$repo" && bash "$installer" --best-effort) >/dev/null ||
    fail "best-effort install exited non-zero on a fresh repo"
assert_installed "$repo"

echo "==> idempotent: a second run changes nothing and exits 0"
before=$(ls -l "$repo/.git/hooks")
(cd "$repo" && bash "$installer" --best-effort) >/dev/null || fail "second best-effort run exited non-zero"
(cd "$repo" && bash "$installer") >/dev/null || fail "second loud run exited non-zero"
after=$(ls -l "$repo/.git/hooks")
[ "$before" = "$after" ] || fail "re-running the installer changed the hooks:
--- before ---
$before
--- after ---
$after"

echo "==> foreign hook: loud mode refuses with exit 1 and leaves it untouched"
repo=$(make_fixture foreign-loud)
printf '#!/bin/sh\necho mine\n' >"$repo/.git/hooks/post-merge"
err=$work/foreign-loud.err
if (cd "$repo" && bash "$installer") >/dev/null 2>"$err"; then
    fail "loud mode exited 0 over a foreign post-merge"
fi
grep -q "Refusing to overwrite existing hook" "$err" ||
    fail "loud refusal did not name the clobber: $(cat "$err")"
[ ! -L "$repo/.git/hooks/post-merge" ] || fail "loud mode replaced the foreign post-merge"
grep -q "echo mine" "$repo/.git/hooks/post-merge" || fail "foreign post-merge content was altered"
for name in post-checkout post-rewrite; do
    [ -L "$repo/.git/hooks/$name" ] || fail "loud mode did not install $name alongside the refusal"
done

echo "==> foreign hook: best-effort warns, skips it, exits 0, installs the rest"
repo=$(make_fixture foreign-quiet)
printf '#!/bin/sh\necho mine\n' >"$repo/.git/hooks/post-merge"
err=$work/foreign-quiet.err
(cd "$repo" && bash "$installer" --best-effort) >/dev/null 2>"$err" ||
    fail "best-effort exited non-zero over a foreign post-merge"
grep -q "skipping post-merge" "$err" || fail "best-effort did not warn about the skip: $(cat "$err")"
[ ! -L "$repo/.git/hooks/post-merge" ] || fail "best-effort replaced the foreign post-merge"
for name in post-checkout post-rewrite; do
    [ -L "$repo/.git/hooks/$name" ] || fail "best-effort did not install $name alongside the skip"
done

echo "==> linked worktree: loud mode refuses, best-effort is a silent no-op"
repo=$(make_fixture worktree-base)
git -C "$repo" add -A
git -C "$repo" commit -qm fixture
git -C "$repo" worktree add -q "$work/linked" -b linked-fixture
err=$work/worktree.err
if (cd "$work/linked" && bash "$installer") >/dev/null 2>"$err"; then
    fail "loud mode exited 0 from a linked worktree"
fi
grep -q "Refusing to install portable-pack hooks from a linked worktree" "$err" ||
    fail "worktree refusal message missing: $(cat "$err")"
out=$work/worktree-quiet.out
(cd "$work/linked" && bash "$installer" --best-effort) >"$out" 2>&1 ||
    fail "best-effort exited non-zero from a linked worktree"
[ ! -s "$out" ] || fail "best-effort was not silent from a linked worktree: $(cat "$out")"
for name in post-merge post-checkout post-rewrite; do
    [ ! -e "$repo/.git/hooks/$name" ] || fail "worktree run installed $name into the main checkout"
done

echo "==> unwritable hooks dir: best-effort warns and exits 0, loud mode fails"
repo=$(make_fixture unwritable)
chmod a-w "$repo/.git/hooks"
err=$work/unwritable.err
(cd "$repo" && bash "$installer" --best-effort) >/dev/null 2>"$err" ||
    { chmod u+w "$repo/.git/hooks"; fail "best-effort exited non-zero over an unwritable hooks dir: $(cat "$err")"; }
grep -q "cannot create a symlink" "$err" ||
    { chmod u+w "$repo/.git/hooks"; fail "best-effort did not warn about the unwritable hooks dir: $(cat "$err")"; }
if (cd "$repo" && bash "$installer") >/dev/null 2>&1; then
    chmod u+w "$repo/.git/hooks"
    fail "loud mode exited 0 over an unwritable hooks dir"
fi
chmod u+w "$repo/.git/hooks"

echo "==> outside a git repository: best-effort is a silent no-op, loud mode fails"
mkdir -p "$work/not-a-repo"
out=$work/not-a-repo.out
(cd "$work/not-a-repo" && GIT_CEILING_DIRECTORIES="$work" bash "$installer" --best-effort) >"$out" 2>&1 ||
    fail "best-effort exited non-zero outside a repo"
[ ! -s "$out" ] || fail "best-effort was not silent outside a repo: $(cat "$out")"
if (cd "$work/not-a-repo" && GIT_CEILING_DIRECTORIES="$work" bash "$installer") >/dev/null 2>&1; then
    fail "loud mode exited 0 outside a repo"
fi

echo "PASS: install-agent-skill-hooks fixtures"
