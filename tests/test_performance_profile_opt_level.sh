#!/usr/bin/env bash
# trace:TASK-1551 | ai:antigravity
# A named check confirms the performance budget still measures an opt-level-3 binary.

set -euo pipefail

# Ensure Cargo.toml does not degrade the release profile.
# It should either omit [profile.release] (meaning defaults apply) or explicitly set opt-level = 3.

CARGO_TOML="Cargo.toml"

echo "Checking release profile opt-level..."
# Check if [profile.release] exists
if grep -q "\[profile\.release\]" "$CARGO_TOML"; then
    echo "[profile.release] exists, verifying opt-level is 3 or omitted (defaulting to 3)..."
    # A bit simplified, but checks if opt-level = 1 or 2 or something else was set under release.
    # We will just strictly forbid setting opt-level to anything other than 3 under release.
    if grep -A 5 "\[profile\.release\]" "$CARGO_TOML" | grep -Eq "opt-level\s*=\s*[0-2zsq]"; then
        echo "ERROR: [profile.release] opt-level cannot be reduced. It must remain 3 for performance budget measures."
        exit 1
    fi
else
    echo "[profile.release] is omitted, using cargo default (opt-level 3). OK."
fi

echo "Checking agent profile..."
if ! grep -q "\[profile\.agent\]" "$CARGO_TOML"; then
    echo "ERROR: [profile.agent] must exist for fast agent iteration."
    exit 1
fi

if ! grep -A 5 "\[profile\.agent\]" "$CARGO_TOML" | grep -Eq "opt-level\s*=\s*1"; then
    echo "ERROR: [profile.agent] must have opt-level = 1."
    exit 1
fi

echo "Performance profile gate passed."
