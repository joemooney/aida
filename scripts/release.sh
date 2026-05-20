#!/usr/bin/env bash
# scripts/release.sh — bump workspace version, tag, push tag.
#
# This is a developer-only script (the AIDA repo's release process). End
# users of AIDA never need to run this. Triggered by:
#
#   ./scripts/release.sh patch       # 0.4.0 -> 0.4.1
#   ./scripts/release.sh minor       # 0.4.0 -> 0.5.0
#   ./scripts/release.sh major       # 0.4.0 -> 1.0.0
#   ./scripts/release.sh 0.5.0       # explicit version
#
# Pushing the tag triggers .github/workflows/release.yml, which builds
# binaries for linux-{x86_64,arm64} and darwin-{x86_64,arm64} and attaches
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
# TASK-257: by default the release is gated on a recent, green cross-platform
# CI run — PR CI is Linux-only during the alpha, so Windows + macOS are only
# validated by the nightly cross-platform.yml workflow. --skip-xplat-check /
# AIDA_SKIP_XPLAT_CHECK=1 bypasses the gate (not recommended for a published
# release). trace:TASK-257 | ai:claude
skip_xplat=${AIDA_SKIP_XPLAT_CHECK:-0}
bump=
for arg in "$@"; do
    case "$arg" in
        --yes|-y) auto_yes=1 ;;
        --skip-xplat-check) skip_xplat=1 ;;
        -h|--help)
            echo "usage: $0 [--yes] [--skip-xplat-check] {major|minor|patch|<explicit-version>}"
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
    echo "usage: $0 [--yes] [--skip-xplat-check] {major|minor|patch|<explicit-version>}" >&2
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
    read -r -p "Commit + tag v$new + push? [y/N]: " answer
fi
case "${answer,,}" in
    y|yes)
        ;;
    *)
        cat <<EOM

cancelled. The version bump is in your working tree but not committed.
Pick one:

  1. Proceed manually (use the auto-generated tag notes):
       git add ${manifest_paths[*]}
       git commit -m "chore: release v$new"
       git tag -a v$new -F $notes_file
       git push origin main
       git push origin v$new

  2. Discard the bump (stay on v$current):
       git restore ${manifest_paths[*]}

  3. Re-run the script after deciding (a clean working tree is required).

Tag notes saved at: $notes_file
EOM
        exit 1
        ;;
esac

# TASK-257: gate the tag on a recent, green cross-platform CI run. PR CI is
# Linux-only during the alpha, so Windows + macOS are only validated by the
# nightly cross-platform.yml workflow — block the release until that's green
# (pre-release-check.sh reuses a <24h green run or dispatches a fresh one and
# blocks on it). Runs after the confirmation prompt so the user is not made to
# wait on CI before deciding to release. trace:TASK-257 | ai:claude
if [ "$skip_xplat" = "1" ]; then
    echo "skipping cross-platform pre-release check (--skip-xplat-check / AIDA_SKIP_XPLAT_CHECK=1)."
else
    echo
    echo "─── Cross-platform pre-release check ───"
    if ! "$(dirname "$0")/pre-release-check.sh"; then
        cat <<EOM >&2

error: cross-platform CI is not green — refusing to tag v$new.

The version bump is in your working tree but not committed. Once
cross-platform CI is green, re-run this script. To tag without the check
(not recommended for a published release), pass --skip-xplat-check.

Tag notes saved at: $notes_file
EOM
        exit 1
    fi
fi

git add "${manifest_paths[@]}"
git commit -m "chore: release v$new"
git tag -a "v$new" -F "$notes_file"
git push origin HEAD
git push origin "v$new"

# Tag now lives in the repo; safe to clean the temp file.
rm -f "$notes_file"

echo
echo "✓ tag v$new pushed."
echo "  Watch the release workflow: gh run watch --workflow=release.yml"
echo "  Once it completes, release artifacts will be at:"
echo "    https://github.com/joemooney/aida/releases/tag/v$new"
