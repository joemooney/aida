#!/usr/bin/env bash
# scripts/publish.sh — publish AIDA crates to crates.io.
#
# This is a developer-only script. End users of AIDA never need to run this.
#
# Status: AIDA is not yet published to crates.io. Before this script can
# work end-to-end, the maintainer needs to:
#   1. cargo login                       (one-time, with crates.io API token)
#   2. Verify aida-core/Cargo.toml has all required metadata fields
#   3. Verify aida-cli/Cargo.toml has all required metadata fields
#   4. Verify aida-crate/Cargo.toml (the published `aida` umbrella crate)
#   5. Decide on the publishing order (aida-core before aida-cli-lib before
#      aida-cli before the umbrella `aida` crate, since they depend on each other)
#
# Until then, this script is a stub that prints what *would* happen.
#
# trace:EPIC-1-001 | ai:claude

set -euo pipefail

# Refuse to run anywhere but the aida repo itself.
if ! grep -q 'repository = "https://github.com/joemooney/aida"' Cargo.toml 2>/dev/null; then
    echo "error: must be run from the root of the joemooney/aida repo" >&2
    exit 1
fi

cat <<'EOF'
scripts/publish.sh — STUB

AIDA is not yet published to crates.io. To wire this up:

1. Create a crates.io account if you don't have one.
2. Run `cargo login` with your API token.
3. Publish in dependency order:

    cd aida-core   && cargo publish --features postgres,gitlab,github,jira
    cd ../aida-cli-lib && cargo publish --features remote
    cd ../aida-cli && cargo publish --features remote
    cd ../aida-crate && cargo publish

4. Update README.md install section to include:

    cargo install aida-cli                       # from crates.io

5. (Optional) Add `cargo binstall` metadata to aida-cli/Cargo.toml so
   `cargo binstall aida-cli` fetches the GitHub release tarball.

When this is done, replace the body of this script with the actual
publish commands. For now, exiting without action.
EOF

exit 0
