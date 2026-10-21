#!/usr/bin/env bash
# scripts/release.sh — bump workspace version through a release PR, tag the
# merged commit, push the tag.
#
# This is a developer-only script (the AIDA repo's release process). End
# users of AIDA never need to run this. Triggered by:
#
#   ./scripts/release.sh patch       # 0.4.0 -> 0.4.1
#   ./scripts/release.sh minor       # 0.4.0 -> 0.5.0
#   ./scripts/release.sh major       # 0.4.0 -> 1.0.0
#   ./scripts/release.sh 0.5.0       # explicit version
#
# Publishing never pushes main directly (TASK-1721): the version bump is
# committed on `release/vX.Y.Z`, pushed, opened as a PR, merged with
# `gh pr merge --squash --match-head-commit` once its checks pass, and the
# squash-merged commit on main is what gets tagged. Only the tag is pushed, so
# the release works with "Require a pull request before merging" on main.
# `--dry-run` prints that planned sequence and changes nothing.
#
# Pushing the tag triggers .github/workflows/release.yml, which builds
# binaries for linux-{x86_64,arm64} (AIDA is Linux-only, ADR-72) and attaches
# them to the GitHub release.
#
# Conventions enforced by this script:
#   - workspace.package.version uses full form ("0.5.0")
#   - aida-crate package version uses full form ("0.5.0")
#   - intra-workspace path-dep version constraints use short form ("0.5") —
#     cargo treats this and "0.5.0" identically for matching, but short
#     form survives patch bumps without churn. The full set of pins is
#     discovered generically from [workspace.members] (BUG-111), so a new
#     workspace crate is bumped automatically — no hardcoded crate list.
#
# trace:EPIC-1-001 | ai:claude

set -euo pipefail

# Load the intra-workspace dependency-pin discovery helpers. trace:BUG-111
. "$(dirname "$0")/workspace-deps.sh"

# Parse args first so --help works in any repo state. Support `--yes` /
# `-y` to skip the interactive prompt (also satisfied by env
# `AIDA_RELEASE_YES=1`). Non-tty invocations (CI, `make ... | tee`,
# captured shells) require one of these because bash's `read` returns
# EOF immediately on a closed stdin and the prompt falls through to
# "cancelled" without ever pausing. trace:TASK-79 | ai:claude
auto_yes=${AIDA_RELEASE_YES:-0}
prepare_only=0
dry_run=0
# ADR-72: releases no longer require any cross-platform CI run. The former
# --skip-xplat-check flag / AIDA_SKIP_XPLAT_CHECK env are still accepted (and
# ignored) so existing callers such as `aida release` keep working.
# trace:TASK-1723 | ai:claude
# TASK-1149: gate documentation currency at RELEASE time (not per-PR).
# Verifies the regenerated CHANGELOG is current + deterministic before the
# tag. --skip-docs-check / AIDA_SKIP_DOCS_CHECK=1 bypasses (mirrors the
# cross-platform gate's opt-out). trace:TASK-1149 | ai:claude
skip_docs=${AIDA_SKIP_DOCS_CHECK:-0}
bump=
for arg in "$@"; do
    case "$arg" in
        --yes|-y) auto_yes=1 ;;
        --prepare-only) prepare_only=1 ;;
        --dry-run) dry_run=1 ;;  # trace:TASK-1721 | ai:claude
        --skip-xplat-check) ;;  # accepted, no-op (ADR-72)
        --skip-docs-check) skip_docs=1 ;;
        -h|--help)
            echo "usage: $0 [--prepare-only] [--dry-run] [--yes] [--skip-xplat-check] [--skip-docs-check] {major|minor|patch|<explicit-version>}"
            exit 0
            ;;
        -*)
            echo "error: unknown flag '$arg'" >&2
            exit 1
            ;;
        *)
            if [ -n "$bump" ]; then
                echo "error: multiple positional args; pass exactly one bump or version" >&2
                exit 1
            fi
            bump=$arg
            ;;
    esac
done

if [ -z "$bump" ]; then
    echo "usage: $0 [--prepare-only] [--dry-run] [--yes] [--skip-xplat-check] [--skip-docs-check] {major|minor|patch|<explicit-version>}" >&2
    exit 1
fi

# Refuse to run anywhere but the aida repo itself.
if ! grep -q 'repository = "https://github.com/joemooney/aida"' Cargo.toml 2>/dev/null; then
    echo "error: must be run from the root of the joemooney/aida repo" >&2
    exit 1
fi

# Refuse if the working tree is dirty.
if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "error: working tree has uncommitted changes" >&2
    git status --short
    exit 1
fi

current=$(awk '/^\[workspace\.package\]/{f=1; next} f && /^version/{gsub(/[" ]/,""); split($0, a, "="); print a[2]; exit}' Cargo.toml)
if [ -z "$current" ]; then
    echo "error: could not parse current workspace.package version" >&2
    exit 1
fi
case "$bump" in
    major|minor|patch)
        IFS='.' read -r maj min pat <<<"$current"
        case "$bump" in
            major) new="$((maj + 1)).0.0" ;;
            minor) new="${maj}.$((min + 1)).0" ;;
            patch) new="${maj}.${min}.$((pat + 1))" ;;
        esac
        ;;
    *)
        # Explicit version like "0.5.0" or "1.2.3-beta.1".
        new=$bump
        ;;
esac

if [ "$new" = "$current" ]; then
    echo "error: new version $new equals current version" >&2
    exit 1
fi

# ─── Release PR plumbing (TASK-1721) ─────────────────────────────────────
# The remote and default branch the release PR targets. Overridable only so the
# bare-remote fixture (tests/test_release_pr_flow.sh) can point elsewhere.
# trace:TASK-1721 | ai:claude
release_remote=${AIDA_RELEASE_REMOTE:-origin}
release_base=${AIDA_RELEASE_BASE:-main}
release_branch="release/v$new"

# BUG-2193 (folded into TASK-1721): `aida pr ship` holds the base branch's
# merge-lease (aida-cli-lib/src/merge_lock.rs, STORY-1171) while it checks,
# merges and fast-forwards the primary checkout. A release that starts, or
# merges, under a live ship races it, so release.sh refuses to start while the
# lease is live and takes the same lease (same file format) for the local
# mutation and merge phases. trace:TASK-1721 | ai:claude
common_dir=$(git rev-parse --path-format=absolute --git-common-dir)
main_root=$(cd "$common_dir/.." && pwd)
lease_file="$main_root/.aida/merge-locks/${release_base//\//_}"
lease_host=${AIDA_HOST_OVERRIDE:-$(uname -n)}
lease_ttl=${AIDA_RELEASE_LEASE_TTL_SECS:-3600}
lease_held=0

# Field 22 (starttime) of /proc/<pid>/stat as `linux-starttime:<ticks>`, the
# identity merge_lock.rs records; empty when unreadable.
lease_pid_start() {
    local stat
    stat=$(cat "/proc/$1/stat" 2>/dev/null) || return 0
    stat=${stat##*)}
    # shellcheck disable=SC2086
    set -- $stat
    [ -n "${20:-}" ] && printf 'linux-starttime:%s' "${20}"
}

lease_field() {
    sed -n "s/^$1=//p" "$lease_file" 2>/dev/null | head -n1
}

# Exit 0 when the lease file names a live holder, mirroring merge_lock.rs's
# stale_reason: a same-host holder whose pid is dead (or recycled, or a zombie)
# is stale, and so is any holder aged past its TTL.
lease_is_live() {
    [ -f "$lease_file" ] || return 1
    local host pid start ttl acquired age live_start state
    host=$(lease_field host)
    pid=$(lease_field pid)
    start=$(lease_field pid_start)
    ttl=$(lease_field ttl_secs)
    acquired=$(lease_field acquired_at)
    case "$ttl" in ''|*[!0-9]*|0) ttl=600 ;; esac
    case "$acquired" in ''|*[!0-9]*) acquired=0 ;; esac
    age=$(( $(date +%s) - acquired ))
    [ "$age" -gt "$ttl" ] && return 1
    if [ -n "$host" ] && [ "$host" = "$lease_host" ]; then
        case "$pid" in ''|*[!0-9]*|0) return 1 ;; esac
        kill -0 "$pid" 2>/dev/null || [ -d "/proc/$pid" ] || return 1
        state=$(sed -n 's/.*) \([A-Z]\).*/\1/p' "/proc/$pid/stat" 2>/dev/null)
        [ "$state" = "Z" ] && return 1
        case "$start" in
            linux-starttime:*)
                live_start=$(lease_pid_start "$pid")
                [ -n "$live_start" ] && [ "$live_start" != "$start" ] && return 1
                ;;
        esac
    fi
    return 0
}

lease_describe() {
    local pr note
    pr=$(lease_field pr)
    note=$(lease_field note)
    printf '%s (pid %s on %s%s)' "${note:-a merger}" "$(lease_field pid)" "$(lease_field host)" "${pr:+, PR #$pr}"
}

lease_acquire() {
    local wait_secs=${1:-0} deadline
    deadline=$(( $(date +%s) + wait_secs ))
    mkdir -p "$(dirname "$lease_file")"
    while :; do
        # Write the full record to a temp file in the same directory, then
        # hard-link it into place: link(2) fails if the lease exists, so the
        # claim is atomic and a reader never sees a partial or empty lease.
        local tmp
        tmp=$(mktemp "$lease_file.XXXXXX")
        printf 'host=%s\npid=%s\npid_start=%s\npr=%s\nnote=%s\nacquired_at=%s\nttl_secs=%s\n' \
            "$lease_host" "$$" "$(lease_pid_start "$$")" "${2:-}" \
            "scripts/release.sh v$new" "$(date +%s)" "$lease_ttl" > "$tmp"
        if ln "$tmp" "$lease_file" 2>/dev/null; then
            rm -f "$tmp"
            lease_held=1
            return 0
        fi
        rm -f "$tmp"
        if ! lease_is_live; then
            rm -f "$lease_file"
            continue
        fi
        [ "$(date +%s)" -ge "$deadline" ] && return 1
        sleep 2
    done
}

lease_release() {
    if [ "$lease_held" = "1" ] && [ "$(lease_field pid)" = "$$" ]; then
        rm -f "$lease_file"
    fi
    lease_held=0
}
trap lease_release EXIT

if lease_is_live && [ "$dry_run" = "1" ]; then
    # A preview changes nothing, so it warns instead of refusing.
    echo "warning: a real release would refuse to start now: $(lease_describe) holds the '$release_base' merge-lease (check with \`aida merge-lock\`)." >&2
elif lease_is_live; then
    cat <<EOM >&2
error: an \`aida pr ship\` (or other merger) holds the '$release_base' merge-lease:
  $(lease_describe)
Releasing now would race that merge. Wait for it to finish (check with
\`aida merge-lock\`), then re-run. Nothing was changed.
EOM
    exit 1
fi

# The release-PR publish sequence, printed by --dry-run and by the manual
# fallbacks so an operator never pushes main directly. trace:TASK-1721 | ai:claude
print_publish_steps() {
    local indent=$1 notes=${2:-<tag-notes-file>}
    cat <<EOM
${indent}git switch -c $release_branch
${indent}git add ${manifest_paths[*]:-Cargo.toml Cargo.lock <member Cargo.toml files> CHANGELOG.md}
${indent}AIDA_RELEASE=1 git commit -m "chore: release v$new"
${indent}git push $release_remote $release_branch
${indent}gh pr create --base $release_base --head $release_branch --title "chore: release v$new"
${indent}gh pr checks <PR> --watch                      # wait for checks
${indent}gh pr merge <PR> --squash --match-head-commit <release-branch-sha> --delete-branch
${indent}git fetch $release_remote $release_base       # merged SHA: gh pr view <PR> --json mergeCommit
${indent}git tag -a v$new <merged-sha> -F $notes
${indent}git push $release_remote v$new
EOM
}

if [ "$dry_run" = "1" ]; then
    echo "dry run: v$current -> v$new. Nothing will be changed, committed, or pushed."
    echo
    echo "Planned release sequence:"
    echo "  1. take the '$release_base' merge-lease ($lease_file) after any ecosystem-watch prompt"
    echo "  2. bump the workspace version, dependency pins, Cargo.lock and CHANGELOG.md; run cargo check and the docs gate"
    echo "  3. release the lease, then ask for confirmation"
    echo "  4. retake the lease; commit the bump on branch $release_branch (AIDA_RELEASE=1)"
    echo "  5. push branch: git push $release_remote $release_branch; release the lease"
    echo "  6. open the PR: gh pr create --base $release_base --head $release_branch --title \"chore: release v$new\""
    echo "  7. wait for checks: gh pr checks <PR> --watch"
    echo "  8. retake the lease; squash-merge: gh pr merge <PR> --squash --match-head-commit <release-branch-sha>"
    echo "  9. tag the merged SHA: git tag -a v$new <merged-sha>"
    echo " 10. push the tag: git push $release_remote v$new"
    echo
    echo "Main is never pushed directly; only the tag is pushed."
    exit 0
fi

# Publish prerequisites, checked before anything is modified: the release PR
# is cut from an up-to-date base branch, and needs gh. --prepare-only stops
# before publishing, so it skips them. trace:TASK-1721 | ai:claude
if [ "$prepare_only" != "1" ]; then
    on_branch=$(git symbolic-ref --short -q HEAD || true)
    if [ "$on_branch" != "$release_base" ]; then
        echo "error: run the release from '$release_base' (currently on '${on_branch:-detached HEAD}')" >&2
        exit 1
    fi
    if ! command -v gh >/dev/null 2>&1; then
        echo "error: gh (GitHub CLI) is required to open and merge the release PR" >&2
        exit 1
    fi
    git fetch -q "$release_remote" "$release_base"
    if [ "$(git rev-parse HEAD)" != "$(git rev-parse FETCH_HEAD)" ]; then
        echo "error: local $release_base is not $release_remote/$release_base; sync it (git pull --ff-only) and re-run" >&2
        exit 1
    fi
    if git rev-parse -q --verify "refs/heads/$release_branch" >/dev/null \
        || git ls-remote --exit-code --heads "$release_remote" "$release_branch" >/dev/null 2>&1; then
        cat <<EOM >&2
error: branch $release_branch already exists (locally or on $release_remote), probably from
an earlier release run that stopped partway. Either finish that release PR:
  gh pr create --base $release_base --head $release_branch --title "chore: release v$new"   # if no PR is open yet
  gh pr view $release_branch                                                                # if one is
or delete the branch and re-run:
  git push $release_remote --delete $release_branch; git branch -D $release_branch
EOM
        exit 1
    fi
    if git rev-parse -q --verify "refs/tags/v$new" >/dev/null; then
        echo "error: tag v$new already exists" >&2
        exit 1
    fi
fi

# Verify the ecosystem review has been performed recently for major/minor releases.
IFS='.' read -r current_maj current_min current_pat <<<"$current"
IFS='.' read -r new_maj new_min new_pat <<<"$new"
is_major_or_minor=0
if [ "$bump" = "major" ] || [ "$bump" = "minor" ]; then
    is_major_or_minor=1
elif [ "$new_maj" != "$current_maj" ] || [ "$new_min" != "$current_min" ]; then
    is_major_or_minor=1
fi

# TASK-562: replace the opaque y/N ecosystem-watch prompt with an
# age-thresholded auto-decision + an actionable refresh hint. The marker date
# in ecosystem-watch.md is compared against today: fresh auto-passes (no
# prompt), middling warns-then-prompts, stale (or unparseable) requires action.
# --yes and non-TTY still bypass, so unattended releases are unchanged.
if [ "$is_major_or_minor" = "1" ]; then
    echo
    echo "─── Ecosystem Watch Verification ───"
    eco_file="docs/competitive-analysis/ecosystem-watch.md"
    echo "  File: $eco_file"

    last_updated=""
    if [ -f "$eco_file" ]; then
        last_updated=$(grep -m1 -E "Last [Uu]pdated" "$eco_file" | sed -E 's/.*: *//' | tr -d '[:space:]')
    else
        echo "  ✗ not found — run an ecosystem scan and create it before a major/minor release."
    fi

    # Marker age in days (GNU date; age_days stays empty if it can't parse).
    age_days=""
    if [ -n "$last_updated" ]; then
        marker_epoch=$(date -d "$last_updated" +%s 2>/dev/null || true)
        if [ -n "$marker_epoch" ]; then
            age_days=$(( ( $(date +%s) - marker_epoch ) / 86400 ))
            echo "  Last updated: $last_updated (${age_days} days ago)"
        else
            echo "  Last updated: $last_updated (could not parse date)"
        fi
    fi
    last_commit=$(git log -1 --format='%ad' --date=short -- "$eco_file" 2>/dev/null || true)
    [ -n "$last_commit" ] && echo "  Last commit:  $last_commit"
    echo "  Refresh: review $eco_file + docs/competitive-analysis/ snapshots, then bump its 'Last updated:' line."
    echo "  If marketplace/package metadata changed, also walk docs/security/marketplace-publication-checklist.md."

    # Thresholds: <14d auto-pass · 14–30d warn+prompt · 30+d (or unknown) require.
    fresh_days=14
    stale_days=30
    decision="require"   # strict default when age can't be determined
    if [ -n "$age_days" ]; then
        if [ "$age_days" -lt "$fresh_days" ]; then
            decision="pass"
        elif [ "$age_days" -lt "$stale_days" ]; then
            decision="warn"
        else
            decision="require"
        fi
    fi

    case "$decision" in
        pass)
            echo "  [auto-pass] ✓ Fresh (< ${fresh_days}d) — continuing release."
            ;;
        warn)
            echo "  [warn] ⚠ Stale (${fresh_days}–${stale_days}d) — refresh recommended."
            if [ "$auto_yes" = "1" ]; then
                echo "  auto-confirm: --yes (or AIDA_RELEASE_YES=1) — continuing despite staleness."
            elif [ ! -t 0 ]; then
                echo "  warning: no TTY — continuing despite staleness."
            else
                read -r -p "  Continue anyway? [y/N]: " eco_answer
                case "${eco_answer,,}" in
                    y|yes) echo "  ok — continuing." ;;
                    *) echo "error: refresh the ecosystem watch, then re-run." >&2; exit 1 ;;
                esac
            fi
            ;;
        require)
            if [ -z "$age_days" ]; then
                echo "  [require] ✗ Freshness unknown — refresh + set a 'Last updated:' date before releasing."
            else
                echo "  [require] ✗ Too stale (≥ ${stale_days}d) — refresh required before a major/minor release."
            fi
            if [ "$auto_yes" = "1" ]; then
                echo "  auto-confirm: --yes (or AIDA_RELEASE_YES=1) — overriding the require-action gate."
            elif [ ! -t 0 ]; then
                echo "  warning: no TTY — overriding the require-action gate (review the ecosystem watch soon)."
            else
                read -r -p "  Override and release anyway? [y/N]: " eco_answer
                case "${eco_answer,,}" in
                    y|yes) echo "  overridden — release continuing." ;;
                    *) echo "error: Ecosystem review required before this release." >&2; exit 1 ;;
                esac
            fi
            ;;
    esac
fi

# Hold the lease while the working tree is being bumped, so a concurrent ship
# cannot fast-forward this checkout mid-bump. It is taken after the ecosystem
# prompt and released before the confirmation prompt, so no prompt waits on a
# human while holding it. trace:TASK-1721 | ai:claude
if ! lease_acquire 0; then
    echo "error: the '$release_base' merge-lease was taken by $(lease_describe); re-run once it finishes" >&2
    exit 1
fi

echo "bumping workspace from v$current -> v$new"

# Bump workspace.package.version (full form).
sed -i.bak -E "/^\[workspace\.package\]/,/^\[/ s/^version = \"$current\"/version = \"$new\"/" Cargo.toml
rm -f Cargo.toml.bak

# Bump aida-crate package version (full form).
sed -i.bak -E "0,/^version = \"$current\"/ s/^version = \"$current\"/version = \"$new\"/" aida-crate/Cargo.toml
rm -f aida-crate/Cargo.toml.bak

# Bump every intra-workspace path-dependency version pin to the new
# version. The set of pins is discovered generically from
# [workspace.members] (see scripts/workspace-deps.sh) rather than from a
# hardcoded crate list, so a newly-added workspace crate is picked up
# automatically — the trap BUG-111 was filed for. trace:BUG-111 | ai:claude
short_new=$(echo "$new" | awk -F. '{print $1"."$2}')
echo
echo "─── Bumping intra-workspace dependency pins -> $short_new ───"
ws_bump_intra_pins "." "$short_new"

# Manifests a version bump can touch: the workspace root, Cargo.lock, and
# every member Cargo.toml (any of which may carry a bumped dep pin). Used
# for the diff preview and the release commit so a new crate's manifest is
# never left unstaged. trace:BUG-111
manifest_paths=("Cargo.toml" "Cargo.lock")
while IFS= read -r _m; do
    [ -n "$_m" ] && [ -f "$_m/Cargo.toml" ] && manifest_paths+=("$_m/Cargo.toml")
done < <(ws_discover_members ".")

# Verify the bump before committing. (1) A self-check that names any
# intra-workspace pin the discovery left stale; (2) cargo check --workspace,
# which resolves the whole graph authoritatively (a missed pin surfaces as
# "failed to select a version for the requirement") and refreshes
# Cargo.lock. trace:BUG-111 | ai:claude
echo
echo "─── Verifying intra-workspace dependency pins ───"
if ! ws_verify_intra_pins "." "$short_new"; then
    cat <<EOM >&2

error: an intra-workspace dependency pin was not bumped to $short_new.
scripts/workspace-deps.sh discovery missed a crate — fix it before releasing.

The version bump is in your working tree but not committed. Discard with:
  git restore ${manifest_paths[*]}
EOM
    exit 1
fi
echo "  ok — all intra-workspace pins point at $short_new"

echo
echo "─── cargo check --workspace ───"
if ! cargo check --workspace; then
    cat <<EOM >&2

error: cargo check --workspace failed after the v$new bump.
A "failed to select a version for the requirement" error means a workspace
crate's path-dependency pin is still at the old version.

The version bump is in your working tree but not committed. Discard with:
  git restore ${manifest_paths[*]}
EOM
    exit 1
fi

echo
echo "─── Regenerating CHANGELOG.md ───"
# Regenerate CHANGELOG.md so it lands with the version-bump commit. Use
# `cargo run` rather than the PATH `aida` to guarantee the *current
# branch's* code runs (a pre-feature binary on PATH would not know the
# `changelog` subcommand). cargo's incremental graph is warm from the
# preceding `cargo check --workspace`, so this rebuilds only the final
# `aida` binary. A failure is non-fatal — the release still proceeds with
# whatever CHANGELOG.md is already tracked. trace:TASK-299 | ai:claude
if cargo run -q -p aida-cli -- changelog refresh --released-as "v$new"; then
    manifest_paths+=("CHANGELOG.md")
    echo "  ok — CHANGELOG.md regenerated for v$new"
else
    echo "  warning: 'aida changelog' unavailable (pre-feature binary) — skipping" >&2
fi

# TASK-1149: documentation gate — the cherry-picked EPIC-25 idea (gate docs
# at RELEASE, not per-PR). Verify the just-regenerated CHANGELOG is current
# and deterministic (blocking), and surface any drifted plan refs touched
# since the previous tag (advisory). Runs before the diff/confirmation
# preview so the releaser sees the verdict before deciding. Opt out with
# --skip-docs-check / AIDA_SKIP_DOCS_CHECK=1. trace:TASK-1149 | ai:claude
if [ "$skip_docs" = "1" ]; then
    echo
    echo "skipping documentation gate (--skip-docs-check / AIDA_SKIP_DOCS_CHECK=1)."
else
    echo
    if ! "$(dirname "$0")/docs-gate.sh" "$new"; then
        cat <<EOM >&2

error: documentation gate failed — refusing to tag v$new.

The version bump (and regenerated CHANGELOG.md) is in your working tree but
not committed. Fix the reported doc issue and re-run, or bypass the gate
(not recommended for a published release) with --skip-docs-check. Discard
the bump with:
  git restore ${manifest_paths[*]}
EOM
        exit 1
    fi
fi

# Generate tag notes from `git log <prev_tag>..HEAD`. Saved to a temp file
# so we can both display them and feed them to `git tag -a -F`. The temp
# file is preserved if the user cancels — they can use it to tag manually.
prev_tag=$(git describe --tags --abbrev=0 2>/dev/null || true)
notes_file=$(mktemp -t aida-release-notes-XXXXXX)
{
    echo "v$new"
    echo
    if [ -n "$prev_tag" ]; then
        echo "Changes since $prev_tag:"
        echo
        git log "${prev_tag}..HEAD" --pretty=format:"- %s" --no-merges
        echo
    else
        echo "Initial release."
    fi
} > "$notes_file"

# Show the version-bump diff and the proposed tag notes.
echo
echo "─── Version bump diff ───"
git --no-pager diff "${manifest_paths[@]}"
echo
echo "─── Tag notes (will be used for v$new) ───"
cat "$notes_file"
echo
echo "─── End tag notes ───"
echo

# STORY-1125: queueable release prep stops at the publish boundary. This mode
# produces the mechanical prep (version bump, Cargo.lock, CHANGELOG, docs gate,
# tag notes), then exits before commit,
# tag, or push. The operator can inspect the dirty tree and then publish
# manually if they approve the prepared release. trace:STORY-1125 | ai:codex
if [ "$prepare_only" = "1" ]; then
    cat <<EOM
prepared v$new. No commit, tag, or push was performed.

Next operator step:
  inspect the version bump and tag notes, then publish through a release PR
  if approved (main is never pushed directly):
$(print_publish_steps "    " "$notes_file")

Tag notes saved at: $notes_file
EOM
    exit 0
fi

lease_release  # trace:TASK-1721 | ai:claude

if [ "$auto_yes" = "1" ]; then
    echo "auto-confirm: --yes (or AIDA_RELEASE_YES=1) — proceeding without prompt."
    answer=y
elif [ ! -t 0 ]; then
    # Non-interactive stdin (piped, captured by `make … | tee`, CI without
    # TTY allocation) and no explicit consent — refuse rather than silently
    # treating EOF as "no" and leaving a half-applied version bump on disk.
    cat <<EOM >&2

error: release script invoked without a TTY and without explicit consent.

The version bump has been applied to the working tree but the commit/tag/push
step requires confirmation. Pick one:

  - rerun interactively (gives you the diff + tag-notes preview), or
  - pass --yes on the command line:    $0 $bump --yes
  - set the env:                       AIDA_RELEASE_YES=1 $0 $bump

The version bump is still in your working tree. Either commit it manually,
or run \`git restore ${manifest_paths[*]}\` to discard.

Tag notes saved at: $notes_file
EOM
    exit 1
else
    read -r -p "Open release PR for v$new, merge it, then tag + push the tag? [y/N]: " answer
fi
case "${answer,,}" in
    y|yes)
        ;;
    *)
        cat <<EOM

cancelled. The version bump is in your working tree but not committed.
Pick one:

  1. Proceed manually through a release PR (main is never pushed directly;
     uses the auto-generated tag notes):
$(print_publish_steps "       " "$notes_file")

  2. Discard the bump (stay on v$current):
       git restore ${manifest_paths[*]}

  3. Re-run the script after deciding (a clean working tree is required).

Tag notes saved at: $notes_file
EOM
        exit 1
        ;;
esac

# AIDA_RELEASE=1 signals the AIDA commit-msg hook (templates/hooks/aida-commit-msg)
# to suppress AI-tag and trace-reference warnings on this mechanical version-bump
# commit. The substrate-as-bouncer gitignored-file gate (templates/hooks/aida-pre-commit.sh)
# is unaffected — release commits still can't sneak intermediate build products in.
# trace:TASK-488 | ai:claude
export AIDA_RELEASE=1
# Retake the lease for the commit and branch push. trace:TASK-1721 | ai:claude
if ! lease_acquire "${AIDA_RELEASE_LEASE_WAIT_SECS:-180}"; then
    cat <<EOM >&2
error: the '$release_base' merge-lease is held by $(lease_describe).
The version bump is still in your working tree; re-run the publish steps once it
finishes, or discard with: git restore ${manifest_paths[*]}
Tag notes saved at: $notes_file
EOM
    exit 1
fi
# TASK-1721: the bump is committed on release/vX, not on main, so the
# STORY-1647 primary-checkout gate only warns and no override is needed.
# trace:TASK-1721 | ai:claude
git switch -q -c "$release_branch"
git add "${manifest_paths[@]}"
git commit -m "chore: release v$new"
release_head=$(git rev-parse HEAD)
if ! git push "$release_remote" "$release_branch"; then
    git switch -q "$release_base"
    cat <<EOM >&2
error: pushing $release_branch failed; nothing was opened, merged, or tagged.
The bump commit is on local branch $release_branch ($release_head). Retry with
  git push $release_remote $release_branch
and finish by hand, or discard it: git branch -D $release_branch
Tag notes saved at: $notes_file
EOM
    exit 1
fi
# Back on the base branch, whose tree is clean again: the bump only exists on
# the release branch until the PR merges.
git switch -q "$release_base"
lease_release

# Open the PR and wait for its checks. The lease is not held while CI runs.
# trace:TASK-1721 | ai:claude
if ! pr_url=$(gh pr create --base "$release_base" --head "$release_branch" \
    --title "chore: release v$new" --body-file "$notes_file"); then
    cat <<EOM >&2
error: gh pr create failed; $release_branch ($release_head) is pushed but no PR is open.
Nothing was merged or tagged. Either open the PR by hand and finish the release:
  gh pr create --base $release_base --head $release_branch --title "chore: release v$new" --body-file $notes_file
  gh pr checks <PR> --watch
  gh pr merge <PR> --squash --match-head-commit $release_head --delete-branch
  git fetch $release_remote $release_base && git tag -a v$new <merged-sha> -F $notes_file
  git push $release_remote v$new
or delete the branch and re-run:
  git push $release_remote --delete $release_branch; git branch -D $release_branch

Tag notes saved at: $notes_file
EOM
    exit 1
fi
pr_url=$(printf '%s\n' "$pr_url" | tail -n1)
pr_number=${pr_url##*/}
echo "opened release PR #$pr_number: $pr_url"

checks_appear_secs=${AIDA_RELEASE_CHECKS_APPEAR_SECS:-300}
checks_timeout_secs=${AIDA_RELEASE_CHECKS_TIMEOUT_SECS:-3600}
checks_interval=${AIDA_RELEASE_CHECKS_INTERVAL_SECS:-10}
release_pr_stuck() {
    cat <<EOM >&2

error: $1
Release PR #$pr_number ($pr_url) is open on $release_branch; nothing was tagged.
Fix the cause, then finish by hand:
  gh pr merge $pr_number --squash --match-head-commit $release_head --delete-branch
  git fetch $release_remote $release_base && git tag -a v$new <merged-sha> -F $notes_file
  git push $release_remote v$new
Or abandon it: gh pr close $pr_number --delete-branch

Tag notes saved at: $notes_file
EOM
    exit 1
}
# Checks register a few seconds after the PR opens; until then `gh pr checks`
# reports none and exits non-zero.
appear_deadline=$(( $(date +%s) + checks_appear_secs ))
until checks_out=$(gh pr checks "$pr_number" 2>&1) || ! printf '%s' "$checks_out" | grep -qi 'no checks reported'; do
    [ "$(date +%s)" -ge "$appear_deadline" ] && release_pr_stuck "no checks appeared on the release PR within ${checks_appear_secs}s"
    sleep "$checks_interval"
done
echo "waiting for release PR #$pr_number checks (timeout ${checks_timeout_secs}s)..."
if ! timeout "$checks_timeout_secs" gh pr checks "$pr_number" --watch --interval "$checks_interval"; then
    release_pr_stuck "release PR #$pr_number checks failed or did not finish within ${checks_timeout_secs}s"
fi

# Merge pinned to the head the checks ran on, under the merge-lease.
# trace:TASK-1721 | ai:claude
if ! lease_acquire "${AIDA_RELEASE_LEASE_WAIT_SECS:-180}" "$pr_number"; then
    release_pr_stuck "the '$release_base' merge-lease is held by $(lease_describe)"
fi
if ! gh pr merge "$pr_number" --squash --match-head-commit "$release_head" --delete-branch; then
    release_pr_stuck "gh pr merge failed"
fi
merged_sha=$(gh pr view "$pr_number" --json mergeCommit -q .mergeCommit.oid)
git fetch -q "$release_remote" "$release_base"
merged_version=$(git show "$merged_sha:Cargo.toml" 2>/dev/null \
    | awk '/^\[workspace\.package\]/{f=1; next} f && /^version/{gsub(/[" ]/,""); split($0, a, "="); print a[2]; exit}')
if [ -z "$merged_sha" ] || ! git merge-base --is-ancestor "$merged_sha" FETCH_HEAD 2>/dev/null \
    || [ "$merged_version" != "$new" ]; then
    cat <<EOM >&2
error: release PR #$pr_number merged, but its merge commit '${merged_sha:-?}' is not on
$release_remote/$release_base at v$new (found '${merged_version:-?}'); not tagging.
Find the squash-merged commit on $release_base, then tag it by hand:
  git fetch $release_remote $release_base && git tag -a v$new <merged-sha> -F $notes_file
  git push $release_remote v$new
EOM
    exit 1
fi

# Tag only the squash-merged commit main contains. A failed tag push removes
# the local tag so no tag is left that the remote does not have.
git tag -a "v$new" "$merged_sha" -F "$notes_file"
if ! git push "$release_remote" "v$new"; then
    git tag -d "v$new" >/dev/null
    echo "error: pushing tag v$new failed; the local tag was removed. Retry with:" >&2
    echo "  git tag -a v$new $merged_sha -F $notes_file && git push $release_remote v$new" >&2
    exit 1
fi
# Bring the local base branch up to the merged commit (best-effort).
if ! git merge -q --ff-only FETCH_HEAD; then
    echo "  ⚠ could not fast-forward local $release_base; run: git pull --ff-only" >&2
fi
git branch -q -D "$release_branch" 2>/dev/null || true
lease_release
if [ -n "${AIDA_RELEASE_TAG_FILE:-}" ]; then
    printf 'v%s\n' "$new" > "$AIDA_RELEASE_TAG_FILE"
fi

# STORY-1048: the GitHub release remains the source of record, but release tags
# must be visible on every configured hub so GitLab-triggered release jobs can
# publish their own assets. Mirror legs are best-effort: warn, never fail.
mirror_remotes=""
if [ -f .aida/config.toml ]; then
    mirror_remotes=$(
        awk '
            /^\[.*\]/ { in_store_sync = ($0 ~ /^\[store\.sync\]/) }
            in_store_sync && /^[[:space:]]*mirror_remotes[[:space:]]*=/ {
                sub(/[[:space:]]+#.*/, "")
                sub(/^[^=]*=/, "")
                gsub(/[\[\]",]/, " ")
                for (i = 1; i <= NF; i++) print $i
            }
        ' .aida/config.toml
    )
fi
for mirror in $mirror_remotes; do
    [ "$mirror" = "origin" ] && continue
    if ! git remote get-url "$mirror" >/dev/null 2>&1; then
        echo "  ⚠ release mirror remote '$mirror' not configured — skipping" >&2
        continue
    fi
    if git push "$mirror" "v$new"; then
        echo "  mirrored tag v$new → $mirror"
    else
        echo "  ⚠ release mirror '$mirror' tag push failed — skipped; check drift with 'aida remote status'" >&2
    fi
done

# Tag now lives in the repo; safe to clean the temp file.
rm -f "$notes_file"

echo
echo "✓ release PR #$pr_number merged; tag v$new pushed at $merged_sha."
echo "  Watch the release workflow: gh run watch --workflow=release.yml"
echo "  Once it completes, release artifacts will be at:"
echo "    https://github.com/joemooney/aida/releases/tag/v$new"
