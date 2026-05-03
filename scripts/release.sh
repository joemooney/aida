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

# Bump workspace.package.version
sed -i.bak -E "/^\[workspace\.package\]/,/^\[/ s/^version = \"$current\"/version = \"$new\"/" Cargo.toml
rm -f Cargo.toml.bak

# Bump aida-crate package version
sed -i.bak -E "0,/^version = \"$current\"/ s/^version = \"$current\"/version = \"$new\"/" aida-crate/Cargo.toml
rm -f aida-crate/Cargo.toml.bak

# Bump aida-core path-dep version constraints in dependents.
# (Workspace path deps still need a `version =` field for `cargo publish` to work.)
short_current=$(echo "$current" | awk -F. '{print $1"."$2}')
short_new=$(echo "$new" | awk -F. '{print $1"."$2}')
sed -i.bak -E "s/aida-core = \\{ version = \"$short_current(\\.0)?\"/aida-core = { version = \"$short_new\"/" \
    aida-cli/Cargo.toml aida-crate/Cargo.toml
rm -f aida-cli/Cargo.toml.bak aida-crate/Cargo.toml.bak

# Refresh Cargo.lock.
cargo build --workspace --offline >/dev/null 2>&1 || cargo build --workspace

# Show the diff so the human can sanity-check before the commit lands.
echo
echo "─── Diff ───"
git --no-pager diff Cargo.toml Cargo.lock aida-cli/Cargo.toml aida-crate/Cargo.toml
echo

read -r -p "Commit, tag v$new, and push tag? [y/N]: " answer
case "${answer,,}" in
    y|yes) ;;
    *) echo "cancelled. Re-run after manual fixes."; exit 1 ;;
esac

git add Cargo.toml Cargo.lock aida-cli/Cargo.toml aida-crate/Cargo.toml
git commit -m "chore: release v$new"
git tag -a "v$new" -m "v$new"
git push origin HEAD
git push origin "v$new"

echo
echo "✓ tag v$new pushed."
echo "  Watch the release workflow: gh run watch --workflow=release.yml"
echo "  Once it completes, release artifacts will be at:"
echo "    https://github.com/joemooney/aida/releases/tag/v$new"
