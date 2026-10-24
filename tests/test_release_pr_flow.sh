#!/usr/bin/env bash
# tests/test_release_pr_flow.sh — TASK-1721 (with BUG-2193 folded in).
#
# scripts/release.sh used to commit `chore: release vX` on main and push it
# straight to main, the one path that kept GitHub from requiring a pull request
# on main. It now commits the bump on release/vX, opens a PR, waits for checks,
# squash-merges with --match-head-commit, tags the merged commit and pushes only
# the tag. It also refuses to start while an `aida pr ship` holds the main
# merge-lease.
#
# Everything runs against a local bare remote whose pre-receive hook rejects
# every push to main (the "Require a pull request" stand-in), with a fake `gh`
# that performs the squash-merge server-side and a fake `cargo`. No network, no
# real release, no real tag.
#
# trace:TASK-1721 | ai:claude
set -euo pipefail
# Fixture git runs with automatic maintenance off: a detached repack can
# race the next object write (BUG-2191). `env -i` sites pass the -c pair.
# trace:TASK-1955 | ai:claude (TASK-2010)
export GIT_CONFIG_COUNT=2 GIT_CONFIG_KEY_0=maintenance.auto GIT_CONFIG_VALUE_0=false GIT_CONFIG_KEY_1=gc.auto GIT_CONFIG_VALUE_1=0

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
RELEASE="$PROJECT_ROOT/scripts/release.sh"
TEST_DIR=$(mktemp -d)

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'
pass() { echo -e "${GREEN}PASS${NC} $1"; }
fail() { echo -e "${RED}FAIL${NC} $1"; exit 1; }
info() { echo -e "${YELLOW}----${NC} $1"; }

cleanup() { rm -rf "$TEST_DIR"; }
trap cleanup EXIT

export GIT_AUTHOR_NAME="TASK-1721 test" GIT_AUTHOR_EMAIL="test@example.com"
export GIT_COMMITTER_NAME="TASK-1721 test" GIT_COMMITTER_EMAIL="test@example.com"
export GIT_CONFIG_NOSYSTEM=1 HOME="$TEST_DIR/home"
mkdir -p "$HOME"
git config --global init.defaultBranch main
git config --global advice.detachedHead false

# ─── fake gh / cargo ──────────────────────────────────────────────────────
FAKE_BIN="$TEST_DIR/bin"
mkdir -p "$FAKE_BIN"
cat > "$FAKE_BIN/cargo" <<'EOF'
#!/usr/bin/env bash
# `cargo check` passes; `cargo run … changelog` is unavailable (non-fatal).
[ "${1:-}" = "check" ] && exit 0
exit 1
EOF
cat > "$FAKE_BIN/gh" <<'EOF'
#!/usr/bin/env bash
# Minimal gh for the release PR flow, backed by $FAKE_GH_REMOTE (a bare repo)
# and $FAKE_GH_STATE. Records every invocation in $FAKE_GH_STATE/calls.
set -euo pipefail
state=$FAKE_GH_STATE
echo "$*" >> "$state/calls"
case "$1 $2" in
    "pr create")
        [ "${FAKE_GH_CREATE_FAIL:-0}" = "1" ] && { echo "fake gh: pr create failed" >&2; exit 1; }
        shift 2
        while [ $# -gt 0 ]; do
            case "$1" in --head) echo "$2" > "$state/head"; shift ;; --base) echo "$2" > "$state/base"; shift ;; esac
            shift
        done
        echo "https://github.com/joemooney/aida/pull/7"
        ;;
    "pr checks")
        if [ ! -f "$state/checks_seen" ]; then
            touch "$state/checks_seen"
            echo "no checks reported on the '$(cat "$state/head")' branch"
            exit 1
        fi
        if [ "${FAKE_GH_CHECKS_FAIL:-0}" = "1" ]; then
            echo "Build	fail"
            exit 1
        fi
        echo "Build	pass"
        ;;
    "pr merge")
        shift 3
        squash=0 match=""
        while [ $# -gt 0 ]; do
            case "$1" in --squash) squash=1 ;; --match-head-commit) match=$2; shift ;; esac
            shift
        done
        [ "$squash" = "1" ] || { echo "fake gh: only --squash is allowed" >&2; exit 1; }
        head=$(cat "$state/head") base=$(cat "$state/base")
        tip=$(git --git-dir="$FAKE_GH_REMOTE" rev-parse "refs/heads/$head")
        [ -n "$match" ] && [ "$tip" = "$match" ] || { echo "fake gh: head $tip != --match-head-commit '$match'" >&2; exit 1; }
        parent=$(git --git-dir="$FAKE_GH_REMOTE" rev-parse "refs/heads/$base")
        merged=$(git --git-dir="$FAKE_GH_REMOTE" commit-tree "$tip^{tree}" -p "$parent" -m "chore: release (#7)")
        # update-ref runs no hooks: this is GitHub merging the PR server-side.
        git --git-dir="$FAKE_GH_REMOTE" update-ref "refs/heads/$base" "$merged" "$parent"
        git --git-dir="$FAKE_GH_REMOTE" update-ref -d "refs/heads/$head"
        echo "$merged" > "$state/merged"
        ;;
    "pr view")
        cat "$state/merged"
        ;;
    *) echo "fake gh: unsupported: $*" >&2; exit 1 ;;
esac
EOF
chmod +x "$FAKE_BIN/cargo" "$FAKE_BIN/gh"

# ─── fixture: workspace clone + bare remote that refuses pushes to main ────
make_fixture() {
    local root=$1
    mkdir -p "$root/state"
    git init -q --bare "$root/remote.git"
    git clone -q "$root/remote.git" "$root/work" 2>/dev/null
    (
        cd "$root/work"
        cat > Cargo.toml <<'TOML'
[workspace]
resolver = "2"
members = [
    "aida-crate",
    "aida-core",
]

[workspace.package]
version = "0.15.0"
repository = "https://github.com/joemooney/aida"
TOML
        mkdir -p aida-crate aida-core
        cat > aida-crate/Cargo.toml <<'TOML'
[package]
name = "aida"
version = "0.15.0"
edition = "2021"

[dependencies]
aida-core = { path = "../aida-core", version = "0.15" }
TOML
        cat > aida-core/Cargo.toml <<'TOML'
[package]
name = "aida-core"
version.workspace = true
edition = "2021"
TOML
        echo "# lock" > Cargo.lock
        git add -A
        git commit -q -m "init"
        git push -q origin main
        # The merge-lease lives under .aida/, which is ignored in a real clone.
        echo "/.aida/" >> .git/info/exclude
        # Records AIDA_RELEASE for each commit, to prove the hook suppression env
        # reaches the bump commit.
        cat > .git/hooks/commit-msg <<HOOK
#!/usr/bin/env bash
echo "AIDA_RELEASE=\${AIDA_RELEASE:-}" >> "$root/state/commit-env"
cp "$root/work/.aida/merge-locks/main" "$root/state/lease-at-commit" 2>/dev/null || true
HOOK
        chmod +x .git/hooks/commit-msg
    )
    cat > "$root/remote.git/hooks/pre-receive" <<'HOOK'
#!/usr/bin/env bash
# "Require a pull request before merging": no direct push may update main.
while read -r old new ref; do
    if [ "$ref" = "refs/heads/main" ]; then
        echo "GH006: Protected branch update failed for refs/heads/main: changes must be made through a pull request." >&2
        exit 1
    fi
done
HOOK
    chmod +x "$root/remote.git/hooks/pre-receive"
}

run_release() {
    local root=$1; shift
    (
        cd "$root/work"
        PATH="$FAKE_BIN:$PATH" FAKE_GH_REMOTE="$root/remote.git" FAKE_GH_STATE="$root/state" \
            AIDA_RELEASE_CHECKS_INTERVAL_SECS=0 AIDA_RELEASE_TAG_FILE="$root/state/tag" \
            bash "$RELEASE" "$@"
    )
}

remote_refs() { git --git-dir="$1/remote.git" for-each-ref --format='%(refname) %(objectname)'; }

echo ""
echo "=== TASK-1721: release goes through a PR, tags the merged commit ==="
echo "Fixture root: $TEST_DIR"
echo ""

# ─── 0. the fixture really refuses a direct push to main ──────────────────
info "Test 0: the bare remote rejects a direct push to main"
# trace:TASK-1721.acd8fb85 | ai:claude
F0="$TEST_DIR/f0"; make_fixture "$F0"
(
    cd "$F0/work"
    git commit -q --allow-empty -m "direct"
    if git push -q origin HEAD:main 2>/dev/null; then exit 1; fi
) || fail "fixture remote accepted a direct push to main"
pass "direct push to main is rejected by the fixture remote"

# ─── 1. --dry-run prints the PR sequence and changes nothing ──────────────
info "Test 1: --dry-run prints the planned PR sequence, pushes nothing"
# trace:TASK-1721.aced1599 | ai:claude
F1="$TEST_DIR/f1"; make_fixture "$F1"
before=$(remote_refs "$F1")
out=$(run_release "$F1" patch --dry-run 2>&1) || fail "dry run exited non-zero: $out"
for want in "release/v0.15.1" "git push origin release/v0.15.1" "gh pr create" \
    "gh pr checks" "--squash --match-head-commit" "git tag -a v0.15.1 <merged-sha>" \
    "git push origin v0.15.1"; do
    grep -qF -- "$want" <<<"$out" || fail "dry run output lacks '$want': $out"
done
if grep -qE 'git push origin (HEAD|main)' <<<"$out"; then
    fail "dry run plans a direct push of main: $out"
fi
[ "$before" = "$(remote_refs "$F1")" ] || fail "dry run changed the remote"
[ -z "$(git -C "$F1/work" status --porcelain)" ] || fail "dry run changed the working tree"
[ "$(git -C "$F1/work" branch --list 'release/*')" = "" ] || fail "dry run created a branch"
[ ! -s "$F1/state/calls" ] || fail "dry run called gh: $(cat "$F1/state/calls")"
pass "--dry-run prints the sequence and leaves remote, tree and branches unchanged"

info "Test 1b: no direct push of main remains in release.sh, including fallback text"
# trace:TASK-1721.aced1599 | ai:claude
if grep -nE 'git push (origin|"\$release_remote"|\$release_remote) (HEAD|main)\b' "$RELEASE"; then
    fail "release.sh still pushes main directly"
fi
pass "release.sh has no 'git push origin HEAD|main'"

# ─── 2. end-to-end release against the PR-required remote ─────────────────
info "Test 2: release completes end to end through the PR"
# trace:TASK-1721.acd8fb85 | ai:claude
# trace:TASK-1721.acfe2a84 | ai:claude
F2="$TEST_DIR/f2"; make_fixture "$F2"
out=$(run_release "$F2" patch --yes --skip-docs-check 2>&1) || fail "release failed: $out"
merged=$(cat "$F2/state/merged")
remote_main=$(git --git-dir="$F2/remote.git" rev-parse refs/heads/main)
[ "$merged" = "$remote_main" ] || fail "remote main $remote_main is not the squash-merged commit $merged"
tag_target=$(git --git-dir="$F2/remote.git" rev-parse 'refs/tags/v0.15.1^{commit}' 2>/dev/null) \
    || fail "tag v0.15.1 was not pushed: $out"
[ "$tag_target" = "$merged" ] || fail "tag points at $tag_target, not the merged commit $merged"
git --git-dir="$F2/remote.git" merge-base --is-ancestor v0.15.1 main \
    || fail "v0.15.1 is not an ancestor of main"
git --git-dir="$F2/remote.git" show main:Cargo.toml | grep -q '^version = "0.15.1"' \
    || fail "Cargo.toml on main is not 0.15.1"
grep -q -- "--match-head-commit" "$F2/state/calls" || fail "merge was not pinned with --match-head-commit"
pass "tag v0.15.1 is the squash-merged commit on main, and main is at 0.15.1"

info "Test 2b: local state after the release"
[ "$(git -C "$F2/work" rev-parse HEAD)" = "$merged" ] || fail "local main was not fast-forwarded"
[ "$(git -C "$F2/work" symbolic-ref --short HEAD)" = "main" ] || fail "release left the checkout off main"
[ -z "$(git -C "$F2/work" branch --list 'release/*')" ] || fail "local release branch left behind"
[ -z "$(git --git-dir="$F2/remote.git" branch --list 'release/*')" ] || fail "remote release branch left behind"
[ "$(cat "$F2/state/tag")" = "v0.15.1" ] || fail "AIDA_RELEASE_TAG_FILE not written"
[ ! -e "$F2/work/.aida/merge-locks/main" ] || fail "merge-lease left behind"
pass "checkout back on fast-forwarded main, branches cleaned up, lease released"

info "Test 2c: the bump commit keeps AIDA_RELEASE=1"
# trace:TASK-1721.ac8f0bcb | ai:claude
grep -qx "AIDA_RELEASE=1" "$F2/state/commit-env" || fail "bump commit ran without AIDA_RELEASE=1: $(cat "$F2/state/commit-env")"
pass "AIDA_RELEASE=1 reached the commit-msg hook"

# ─── 3. failed checks: no merge, no tag ───────────────────────────────────
info "Test 3: failing PR checks stop before merge and tag"
F3="$TEST_DIR/f3"; make_fixture "$F3"
main_before=$(git --git-dir="$F3/remote.git" rev-parse refs/heads/main)
if out=$(FAKE_GH_CHECKS_FAIL=1 run_release "$F3" patch --yes --skip-docs-check 2>&1); then
    fail "release succeeded despite failing checks"
fi
grep -q "nothing was tagged" <<<"$out" || fail "missing stuck-PR guidance: $out"
[ "$(git --git-dir="$F3/remote.git" rev-parse refs/heads/main)" = "$main_before" ] || fail "main moved"
[ -z "$(git --git-dir="$F3/remote.git" tag)" ] || fail "a tag was pushed"
[ -z "$(git -C "$F3/work" tag)" ] || fail "a local tag was created"
if grep -q "pr merge" "$F3/state/calls"; then fail "merge was attempted"; fi
pass "failed checks leave main and tags untouched"

# ─── 4. in-flight `aida pr ship` (BUG-2193) ───────────────────────────────
info "Test 4: refuses to start while a live ship holds the main merge-lease"
F4="$TEST_DIR/f4"; make_fixture "$F4"
mkdir -p "$F4/work/.aida/merge-locks"
write_lease() {
    printf 'host=%s\npid=%s\npid_start=\npr=42\nnote=aida pr ship\nacquired_at=%s\nttl_secs=600\n' \
        "$(uname -n)" "$1" "$2" > "$F4/work/.aida/merge-locks/main"
}
write_lease "$$" "$(date +%s)"
if out=$(run_release "$F4" patch --yes --skip-docs-check 2>&1); then
    fail "release started while a live ship held the lease"
fi
grep -q "merge-lease" <<<"$out" || fail "refusal does not name the merge-lease: $out"
grep -q "PR #42" <<<"$out" || fail "refusal does not name the ship's PR: $out"
[ -z "$(git -C "$F4/work" status --porcelain)" ] || fail "refused release modified the tree"
[ ! -s "$F4/state/calls" ] || fail "refused release called gh"
grep -q "pid=$$" "$F4/work/.aida/merge-locks/main" || fail "refused release disturbed the ship's lease"
pass "live ship lease refuses the release before anything changes"

info "Test 4a: --dry-run warns about a live ship lease instead of refusing"
out=$(run_release "$F4" patch --dry-run 2>&1) || fail "dry run refused under a live lease: $out"
grep -q "would refuse to start" <<<"$out" || fail "dry run did not warn about the lease: $out"
grep -qF "gh pr create" <<<"$out" || fail "dry run under a lease printed no plan: $out"
pass "--dry-run warns and still prints the plan"

info "Test 4b: a dead or expired holder does not block"
sleep 0 & dead_pid=$!; wait "$dead_pid"
write_lease "$dead_pid" "$(date +%s)"
run_release "$F4" patch --dry-run >/dev/null 2>&1 || fail "dead-pid lease blocked the release"
write_lease "$$" "$(( $(date +%s) - 4000 ))"
run_release "$F4" patch --dry-run >/dev/null 2>&1 || fail "expired lease blocked the release"
pass "stale leases (dead pid, past TTL) are not treated as an in-flight ship"

info "Test 4b2: an empty, just-created lease blocks; an old empty one does not"
# trace:TASK-1955 | ai:claude (TASK-1-421)
: > "$F4/work/.aida/merge-locks/main"
if out=$(run_release "$F4" patch --yes --skip-docs-check 2>&1); then
    fail "release started over a fresh empty lease (read as acquired_at=0, TTL-stale)"
fi
[ -e "$F4/work/.aida/merge-locks/main" ] && [ ! -s "$F4/work/.aida/merge-locks/main" ] \
    || fail "release stole or rewrote a fresh empty lease"
touch -d "@$(( $(date +%s) - 120 ))" "$F4/work/.aida/merge-locks/main"
run_release "$F4" patch --dry-run >/dev/null 2>&1 || fail "an empty lease past its grace blocked the release"
pass "a fresh empty lease reads live; past its grace it is stale"
# --dry-run only warns about a live lease, so read its warning.
touch -d "@$(( $(date +%s) + 600 ))" "$F4/work/.aida/merge-locks/main"
out=$(run_release "$F4" patch --dry-run 2>&1) || fail "dry run failed over a far-future empty lease: $out"
if printf '%s' "$out" | grep -q "would refuse to start"; then
    fail "an empty lease with a far-future mtime read live: $out"
fi
pass "an empty lease with a far-future mtime is stale, not live forever"

info "Test 4c: the release holds the lease while it mutates the checkout"
F5="$TEST_DIR/f5"; make_fixture "$F5"
mkdir -p "$TEST_DIR/bin-lease"
cat > "$TEST_DIR/bin-lease/cargo" <<EOF
#!/usr/bin/env bash
if [ "\${1:-}" = "check" ]; then
    cp "$F5/work/.aida/merge-locks/main" "$F5/state/lease-during-check" 2>/dev/null || true
    exit 0
fi
exit 1
EOF
chmod +x "$TEST_DIR/bin-lease/cargo"
(
    cd "$F5/work"
    PATH="$TEST_DIR/bin-lease:$FAKE_BIN:$PATH" FAKE_GH_REMOTE="$F5/remote.git" FAKE_GH_STATE="$F5/state" \
        AIDA_RELEASE_CHECKS_INTERVAL_SECS=0 bash "$RELEASE" patch --yes --skip-docs-check >/dev/null 2>&1
) || fail "release with lease probe failed"
grep -q "note=scripts/release.sh v0.15.1" "$F5/state/lease-during-check" 2>/dev/null \
    || fail "the main merge-lease was not held during cargo check"
pass "release.sh holds the main merge-lease during the bump"
grep -q "note=scripts/release.sh v0.15.1" "$F5/state/lease-at-commit" 2>/dev/null \
    || fail "the main merge-lease was not retaken for the release commit"
pass "release.sh retakes the main merge-lease for the release commit"

# ─── interactive driver: a pty, answering each prompt after a probe ───────
# Runs release.sh under script(1) so `[ -t 0 ]` holds and the real prompts are
# read. For each "<prompt text>|<probe>" pair it waits for the prompt, runs the
# probe (a shell function, given the fixture root), then answers "y".
# trace:TASK-1951 trace:TASK-1691 | ai:claude
pty_release() {
    local root=$1 args=$2; shift 2
    local fifo="$root/state/stdin" log="$root/state/pty.log"
    rm -f "$fifo" "$log"; mkfifo "$fifo"
    (
        cd "$root/work"
        PATH="$FAKE_BIN:$PATH" FAKE_GH_REMOTE="$root/remote.git" FAKE_GH_STATE="$root/state" \
            AIDA_RELEASE_CHECKS_INTERVAL_SECS=0 AIDA_RELEASE_LEASE_WAIT_SECS=0 \
            script -qfec "bash '$RELEASE' $args --skip-docs-check" "$log" <"$fifo" >/dev/null 2>&1
        echo $? > "$root/state/pty.rc"
    ) &
    local runner=$! step seen=0
    exec 9>"$fifo"
    for step in "$@"; do
        local prompt=${step%%|*} probe=${step#*|} i
        for i in $(seq 1 600); do
            [ "$(grep -cF -- "$prompt" "$log" 2>/dev/null || true)" -gt "$seen" ] && break
            kill -0 "$runner" 2>/dev/null || break
            sleep 0.1
        done
        grep -qF -- "$prompt" "$log" 2>/dev/null || { exec 9>&-; wait "$runner"; fail "prompt '$prompt' never appeared: $(cat "$log")"; }
        "$probe" "$root"
        printf 'y\n' >&9
    done
    exec 9>&-
    wait "$runner" || true
    cat "$root/state/pty.rc"
}

lease_absent_at_prompt() {
    if [ -e "$1/work/.aida/merge-locks/main" ]; then
        echo held >> "$1/state/lease-at-prompt"
    else
        echo free >> "$1/state/lease-at-prompt"
    fi
}

info "Test 4d: no prompt runs while the lease is held"
F8="$TEST_DIR/f8"; make_fixture "$F8"
# A minor bump with no ecosystem watch file asks the ecosystem override prompt
# first, then the confirmation prompt.
rc=$(pty_release "$F8" minor \
    "Override and release anyway?|lease_absent_at_prompt" \
    "Open release PR for v0.16.0|lease_absent_at_prompt")
[ "$rc" = "0" ] || fail "interactive release failed (rc=$rc): $(cat "$F8/state/pty.log")"
[ "$(cat "$F8/state/lease-at-prompt")" = "$(printf 'free\nfree')" ] \
    || fail "a prompt waited on a human while the lease was held: $(cat "$F8/state/lease-at-prompt")"
git --git-dir="$F8/remote.git" rev-parse -q --verify 'refs/tags/v0.16.0' >/dev/null \
    || fail "interactive release did not finish"
# trace:TASK-1955 | ai:claude (TASK-2093)
grep -q "note=scripts/release.sh v0.16.0" "$F8/state/lease-at-commit" 2>/dev/null \
    || fail "the lease was not retaken after the confirmation prompt"
[ "$(git --git-dir="$F8/remote.git" rev-parse 'refs/tags/v0.16.0^{commit}')" = "$(cat "$F8/state/merged")" ] \
    || fail "the interactive release did not tag the merged commit"
pass "neither the ecosystem prompt nor the confirmation prompt runs under the lease"

info "Test 4e: the lease is claimed atomically, never visible empty"
F9="$TEST_DIR/f9"; make_fixture "$F9"
mkdir -p "$F9/work/.aida/merge-locks"
(
    lease="$F9/work/.aida/merge-locks/main"
    while [ ! -e "$F9/state/stop-watch" ]; do
        if [ -e "$lease" ] && ! { grep -q '^pid=' "$lease" && grep -q '^ttl_secs=' "$lease"; } 2>/dev/null; then
            [ -e "$lease" ] && echo "partial" >> "$F9/state/partial-lease"
        fi
        sleep 0.01
    done
) & watcher=$!
run_release "$F9" patch --yes --skip-docs-check >/dev/null 2>&1 || fail "release under the lease watcher failed"
touch "$F9/state/stop-watch"; wait "$watcher"
[ ! -e "$F9/state/partial-lease" ] || fail "a reader saw an empty or partial lease"
[ -z "$(ls -A "$F9/work/.aida/merge-locks/" 2>/dev/null)" ] || fail "lease temp files left behind: $(ls -A "$F9/work/.aida/merge-locks/")"
pass "a concurrent reader never sees a partial lease; no temp files remain"

# ─── 4f. the base moves while the confirmation prompt waits (TASK-1688) ───
info "Test 4f: a base that moved during the confirmation prompt aborts before committing"
advance_base() {
    # A ship fast-forwards main on the remote and in this checkout.
    local root=$1 parent merged
    parent=$(git --git-dir="$root/remote.git" rev-parse refs/heads/main)
    merged=$(git --git-dir="$root/remote.git" commit-tree "$parent^{tree}" -p "$parent" -m "feat: shipped meanwhile (#8)")
    git --git-dir="$root/remote.git" update-ref refs/heads/main "$merged" "$parent"
    git -C "$root/work" fetch -q origin main
    git -C "$root/work" merge -q --ff-only FETCH_HEAD
}
advance_remote_only() {
    local root=$1 parent merged
    parent=$(git --git-dir="$root/remote.git" rev-parse refs/heads/main)
    merged=$(git --git-dir="$root/remote.git" commit-tree "$parent^{tree}" -p "$parent" -m "feat: shipped meanwhile (#9)")
    git --git-dir="$root/remote.git" update-ref refs/heads/main "$merged" "$parent"
}
for probe in advance_base advance_remote_only; do
    FM="$TEST_DIR/fm-$probe"; make_fixture "$FM"
    rc=$(pty_release "$FM" patch "Open release PR for v0.15.1|$probe")
    [ "$rc" != "0" ] || fail "$probe: release continued on a moved base"
    grep -q "while the release waited for confirmation" "$FM/state/pty.log" \
        || fail "$probe: no moved-base refusal: $(cat "$FM/state/pty.log")"
    grep -qF "git restore" "$FM/state/pty.log" || fail "$probe: refusal lacks the restore hint"
    [ -z "$(git -C "$FM/work" branch --list 'release/*')" ] || fail "$probe: a release branch was created"
    [ -z "$(git --git-dir="$FM/remote.git" branch --list 'release/*')" ] || fail "$probe: a release branch was pushed"
    [ ! -s "$FM/state/calls" ] || fail "$probe: gh was called: $(cat "$FM/state/calls")"
    [ ! -e "$FM/work/.aida/merge-locks/main" ] || fail "$probe: the lease was left behind"
done
pass "a local or remote base move during the prompt aborts with nothing committed or pushed"

# ─── 6. gh pr create fails after the branch push ──────────────────────────
info "Test 6: a failed gh pr create prints recovery, and a re-run names the leftover branch"
F7="$TEST_DIR/f7"; make_fixture "$F7"
if out=$(FAKE_GH_CREATE_FAIL=1 run_release "$F7" patch --yes --skip-docs-check 2>&1); then
    fail "release succeeded although gh pr create failed"
fi
grep -q "gh pr create failed" <<<"$out" || fail "no gh pr create failure message: $out"
grep -qF "gh pr create --base main --head release/v0.15.1" <<<"$out" || fail "no manual PR command: $out"
grep -qF "git push origin --delete release/v0.15.1" <<<"$out" || fail "no branch-delete command: $out"
[ -z "$(git --git-dir="$F7/remote.git" tag)" ] || fail "a tag was pushed"
[ "$(git -C "$F7/work" symbolic-ref --short HEAD)" = "main" ] || fail "left the checkout off main"
if out=$(run_release "$F7" patch --yes --skip-docs-check 2>&1); then
    fail "re-run succeeded with the release branch still present"
fi
grep -q "already exists" <<<"$out" || fail "re-run did not refuse on the leftover branch: $out"
grep -qF "git push origin --delete release/v0.15.1" <<<"$out" || fail "re-run refusal lacks the delete hint: $out"
pass "gh pr create failure and the re-run both print the exact recovery"

# ─── 5. --prepare-only fallback text routes through a PR ──────────────────
info "Test 5: --prepare-only manual steps open a PR instead of pushing main"
# trace:TASK-1721.aced1599 | ai:claude
F6="$TEST_DIR/f6"; make_fixture "$F6"
out=$(run_release "$F6" patch --prepare-only --skip-docs-check 2>&1) || fail "prepare-only failed: $out"
grep -qF "gh pr create" <<<"$out" || fail "prepare-only steps lack gh pr create: $out"
grep -qF -- "--match-head-commit" <<<"$out" || fail "prepare-only steps lack --match-head-commit: $out"
if grep -qE 'git push origin (HEAD|main)' <<<"$out"; then
    fail "prepare-only tells the operator to push main: $out"
fi
[ ! -s "$F6/state/calls" ] || fail "prepare-only called gh"
[ ! -e "$F6/work/.aida/merge-locks/main" ] || fail "prepare-only left the merge-lease behind"
pass "--prepare-only prints the PR sequence and publishes nothing"

echo ""
echo "All TASK-1721 release PR-flow tests passed."
