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
#   - aida-core path-dep version constraints use short form ("0.5") —
#     cargo treats this and "0.5.0" identically for matching, but short
#     form survives patch bumps without churn
#
# trace:EPIC-1-001 | ai:claude

set -euo pipefail

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

if [ $# -ne 1 ]; then
    echo "usage: $0 {major|minor|patch|<explicit-version>}" >&2
    exit 1
fi

current=$(awk '/^\[workspace\.package\]/{f=1; next} f && /^version/{gsub(/[" ]/,""); split($0, a, "="); print a[2]; exit}' Cargo.toml)
if [ -z "$current" ]; then
    echo "error: could not parse current workspace.package version" >&2
    exit 1
fi

bump=$1
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

# Bump aida-core path-dep version constraints in dependents (short form).
# Match either short ("0.4") or any full ("0.4.x") form on the way in;
# always emit short ("0.5") on the way out.
short_current=$(echo "$current" | awk -F. '{print $1"."$2}')
short_new=$(echo "$new" | awk -F. '{print $1"."$2}')
sed -i.bak -E "s/aida-core = \\{ version = \"$short_current(\\.[0-9]+)?\"/aida-core = { version = \"$short_new\"/" \
    aida-cli/Cargo.toml aida-crate/Cargo.toml
rm -f aida-cli/Cargo.toml.bak aida-crate/Cargo.toml.bak

# Refresh Cargo.lock.
cargo build --workspace --offline >/dev/null 2>&1 || cargo build --workspace

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
git --no-pager diff Cargo.toml Cargo.lock aida-cli/Cargo.toml aida-crate/Cargo.toml
echo
echo "─── Tag notes (will be used for v$new) ───"
cat "$notes_file"
echo
echo "─── End tag notes ───"
echo

read -r -p "Commit + tag v$new + push? [y/N]: " answer
case "${answer,,}" in
    y|yes)
        ;;
    *)
        cat <<EOM

cancelled. The version bump is in your working tree but not committed.
Pick one:

  1. Proceed manually (use the auto-generated tag notes):
       git add Cargo.toml Cargo.lock aida-cli/Cargo.toml aida-crate/Cargo.toml
       git commit -m "chore: release v$new"
       git tag -a v$new -F $notes_file
       git push origin main
       git push origin v$new

  2. Discard the bump (stay on v$current):
       git restore Cargo.toml Cargo.lock aida-cli/Cargo.toml aida-crate/Cargo.toml

  3. Re-run the script after deciding (a clean working tree is required).

Tag notes saved at: $notes_file
EOM
        exit 1
        ;;
esac

git add Cargo.toml Cargo.lock aida-cli/Cargo.toml aida-crate/Cargo.toml
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
