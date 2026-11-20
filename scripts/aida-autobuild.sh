#!/usr/bin/env bash
# Operator-installed host cron, every 30 minutes. trace:TASK-3812 | ai:codex
# Promotes the newest commit on origin/main's first-parent line whose CI is
# green and that is newer than the live binary (bounded to 20 commits), not
# only a green tip: merges land faster than CI finishes. trace:BUG-2687 | ai:claude
set -euo pipefail
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
export GIT_TERMINAL_PROMPT=0
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

repo=${AIDA_REPO_DIR:-"$HOME/ai/aida"}
gh_repo=${AIDA_GH_REPO:-joemooney/aida}
log_file=${AIDA_AUTOBUILD_LOG:-"$HOME/.aida/autobuild.log"}
config=.aida/config.toml
dry=0
case "${1:-}:$#" in
    :0) ;;
    --dry-run:1) dry=1 ;;
    *) echo "usage: aida-autobuild.sh [--dry-run]" >&2; exit 2 ;;
esac

log() {
    printf '%s %s\n' "$(date '+%F %T')" "$*" >> "$log_file"
    if (( dry )); then printf '%s\n' "$*"; fi
}
skip() { log "skip: $*"; exit 0; }
fail() { log "FAILED: $*"; exit 1; }

cd "$repo" || fail "cannot enter $repo"
primary=$(git rev-parse --show-toplevel) || fail "cannot resolve checkout"
[[ "$PWD" == "$primary" && -d .git ]] || fail "run from the primary checkout root"
# Shared with aida dev build and dev-autopromote. Pass the held descriptor
# to the build so there is no unlocked interval after fast-forwarding.
exec 9<.git || fail "cannot open checkout build lock"
if flock -n -E 75 9; then
    :
elif [[ $? == 75 ]]; then
    skip "build in progress, skipped"
else
    fail "cannot acquire checkout build lock"
fi

clean_main() {
    local status branch hidden
    branch=$(git symbolic-ref -q HEAD) || return 1
    status=$(git status --porcelain --untracked-files=no) || return 1
    hidden=$(git ls-files -v) || return 1
    [[ "$branch" == refs/heads/main && -z "$status" ]] || return 1
    [[ ! -e .git/MERGE_HEAD && ! -e .git/CHERRY_PICK_HEAD &&
       ! -e .git/REVERT_HEAD && ! -d .git/rebase-merge && ! -d .git/rebase-apply &&
       ! -d .git/sequencer ]] || return 1
    # Refuse additional skip-worktree or assume-unchanged entries.
    printf '%s\n' "$hidden" | awk '
        /^[a-zS] / && $0 != "S .aida/config.toml" { found = 1 }
        END { exit found }
    '
}
fetch() {
    timeout -k 5 60 git fetch -q origin refs/heads/main:refs/remotes/origin/main >>"$log_file" 2>&1
}
live_sha() {
    local banner stamped resolved
    banner=$(timeout -k 5 15 aida --version) || return 1
    [[ "$banner" =~ ,\ sha\ ([[:xdigit:]]{7,64})(\+dirty)?\)$ ]] || return 1
    stamped=${BASH_REMATCH[1]}
    resolved=$(git rev-parse --verify "$stamped^{commit}" 2>/dev/null) || return 1
    [[ "$resolved" == "${stamped,,}"* ]] || return 1
    printf '%s\n' "$resolved"
}

short() { printf '%s' "${1:0:12}"; }

clean_main || skip "primary checkout must be on main with no visible tracked changes or other hidden entries"
fetch || fail "fetch failed"
tip=$(git rev-parse --verify refs/remotes/origin/main)
head=$(git rev-parse --verify HEAD)
git merge-base --is-ancestor "$head" "$tip" || skip "main cannot fast-forward to $tip"
live=$(live_sha) || live=
# Newest first: stop at the live commit (nothing older is newer than live)
# and after HEAD (nothing older can be fast-forwarded to). A red, pending or
# missing CI run is passed over, never promoted.
target=
passed=()
revs=$(git rev-list --first-parent --max-count=20 "$tip" --) || fail "cannot list origin/main history"
mapfile -t line <<<"$revs"
for commit in "${line[@]}"; do
    [[ "$commit" =~ ^[[:xdigit:]]{40}$ ]] || fail "unexpected rev-list output: $commit"
    if [[ -n "$live" && "$commit" == "$live" && "$head" == "$live" ]]; then
        # Nothing newer than live is green. When live is origin/main itself,
        # still verify the launcher below.
        if (( ${#passed[@]} == 0 )); then target=$live; fi
        break
    fi
    # An unreadable CI result (query error or timeout) is unknown: passed
    # over like a red one, and the walk goes on to the next older commit.
    # Only a successful query reporting success selects a commit.
    # trace:BUG-2687 | ai:claude
    if ci=$(timeout -k 5 30 gh run list -R "$gh_repo" --branch main --workflow CI --commit "$commit" \
        --limit 1 --json status,conclusion --jq '.[0] // empty | [.status,.conclusion] | join(" ")' \
        2>>"$log_file"); then
        :
    else
        rc=$?
        ci="CI unknown (query failed, exit $rc)"
    fi
    if [[ "$ci" == 'completed success' ]]; then
        target=$commit
        break
    fi
    passed+=("$(short "$commit") ${ci:-no CI run}")
    [[ "$commit" != "$head" && "$commit" != "$live" ]] || break
done
if (( ${#passed[@]} )); then
    log "newer commits passed over: $(IFS=';'; printf '%s' "${passed[*]}")"
fi
[[ -n "$target" ]] || skip "no green commit newer than live ${live:-unknown} on origin/main $(short "$tip")"
if [[ -n "$live" && "$target" != "$live" ]]; then
    git merge-base --is-ancestor "$live" "$target" ||
        skip "newest green $target does not contain live $live"
fi
git merge-base --is-ancestor "$head" "$target" || skip "main cannot fast-forward to newest green $target"
log "promotion target: newest green $target (origin/main $(short "$tip"))"

# origin/main may move on meanwhile; the chosen commit stays valid while it
# remains an ancestor of origin/main.
on_main() {
    git merge-base --is-ancestor "$target" refs/remotes/origin/main
}
clean_main || skip "checkout changed while checking CI"
[[ "$(git rev-parse HEAD)" == "$head" ]] && on_main || skip "HEAD moved or origin/main no longer contains $target"

backup=
restore_override() {
    local current candidate
    trap - EXIT
    if [[ -n "$backup" ]]; then
        current=$(git rev-parse HEAD) || current=
        case "$current" in
            "$target") candidate="$backup/rebased" ;;
            "$head") candidate="$backup/original" ;;
            *) fail "checkout moved during fast-forward; recover override from $backup" ;;
        esac
        # A failed merge should leave the old checkout at the base. If an
        # unexpected writer edited config in the meantime, keep both copies.
        [[ -d .aida && ! -L .aida && -f "$config" && ! -L "$config" ]] ||
            fail "config path changed during fast-forward; recover override from $backup"
        if ! cmp -s "$config" "$backup/base" &&
           ! cmp -s "$config" "$backup/target" &&
           ! cmp -s "$config" "$candidate"; then
            fail "config changed during fast-forward; recover override from $backup"
        fi
        cp -p -- "$candidate" "$config" && git update-index --skip-worktree -- "$config" ||
            fail "could not restore config override from $backup"
        rm -rf -- "$backup"
    fi
}

entry=$(git ls-files -v -- "$config")
if [[ "$entry" == "S $config" ]]; then
    [[ -d .aida && ! -L .aida && -f "$config" && ! -L "$config" ]] || fail "hidden config must be a regular file"
    old_entry=$(git ls-tree "$head" -- "$config")
    new_entry=$(git ls-tree "$target" -- "$config")
    [[ "${old_entry%% *}" == 100644 && "${new_entry%% *}" == 100644 ]] ||
        skip "hidden config changed type or mode"
    if [[ "$(git rev-parse "$head:$config")" != "$(git rev-parse "$target:$config")" ]]; then
        scratch=$(mktemp -d "${TMPDIR:-/tmp}/aida-autobuild.XXXXXXXX")
        trap 'rm -rf -- "$scratch"' EXIT
        cp -p -- "$config" "$scratch/original"
        git show "$head:$config" > "$scratch/base"
        git show "$target:$config" > "$scratch/target"
        cp -p -- "$scratch/original" "$scratch/rebased"
        git merge-file "$scratch/rebased" "$scratch/base" "$scratch/target" >>"$log_file" 2>&1 ||
            skip "incoming config conflicts with the host override"
        python3 -c 'import sys, tomllib; tomllib.load(open(sys.argv[1], "rb"))' \
            "$scratch/rebased" >>"$log_file" 2>&1 || fail "rebased host config is not valid TOML"
        (( !dry )) || skip "dry run would fast-forward to $target with host override"
        cmp -s "$config" "$scratch/original" || skip "host override changed during preparation"
        backup=$scratch
        log "config override recovery copy: $backup/original"
        trap 'rc=$?; restore_override; exit "$rc"' EXIT
        trap 'exit 130' INT
        trap 'exit 143' TERM
        git update-index --no-skip-worktree -- "$config"
        git checkout-index -f -- "$config"
    fi
fi
(( !dry )) || skip "dry run would check live binary at $target"

if [[ "$head" != "$target" ]]; then
    git -c core.hooksPath=/dev/null merge --ff-only --no-overwrite-ignore -q "$target" >>"$log_file" 2>&1 ||
        fail "main could not fast-forward to $target"
fi
if [[ -n "$backup" ]]; then
    # Restore immediately, including on a failed merge via the EXIT trap.
    restore_override
    backup=
    trap - EXIT INT TERM
fi

clean_main || fail "checkout is no longer clean main"
[[ "$(git rev-parse HEAD)" == "$target" ]] && on_main ||
    fail "main is no longer the verified green commit $target on origin/main"
live=$(live_sha) || live=
built=0
if [[ -z "$live" || "$target" != "$live" ]]; then
    log "building $target (live ${live:-unknown})"
    AIDA_AUTOBUILD_LOCK_FD=9 AIDA_AUTOBUILD_TARGET=$target aida dev build >>"$log_file" 2>&1 ||
        fail "aida dev build failed for $target"
    live=$(live_sha) || fail "cannot read promoted binary SHA"
    [[ "$target" == "$live" ]] || fail "live binary $live does not match $target"
    built=1
fi
timeout -k 5 30 "$HOME/lane-start.sh" --check >>"$log_file" 2>&1 || fail "lane-start --check failed for $live"
if (( built )); then log "promoted and verified: $target"; fi
