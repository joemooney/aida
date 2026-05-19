#!/usr/bin/env bash
# tests/test_release_dep_discovery.sh — regression test for BUG-111.
#
# scripts/release.sh used to bump a *hardcoded* list of intra-workspace
# dependency pins. When STORY-132 added the aida-tui crate, its pin was not
# in the list, so `make release-minor` left it stale and cargo failed to
# resolve the workspace ("failed to select a version for the requirement
# `aida-tui = ^0.7`"). The fix (scripts/workspace-deps.sh) discovers the
# pins generically from [workspace.members].
#
# This test simulates "add a new workspace crate, then run a release bump"
# and asserts the pin discovery + bump + verify handle the new crate with
# no manual intervention.
#
# trace:BUG-111 | ai:claude
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
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

# Load the discovery helpers under test.
# shellcheck source=scripts/workspace-deps.sh
. "$PROJECT_ROOT/scripts/workspace-deps.sh"

echo ""
echo "=== BUG-111: release-script dep-discovery regression test ==="
echo "Test workspace: $TEST_DIR"
echo ""

# ---------------------------------------------------------------------------
# Build a fake workspace. All crates inherit version.workspace = true (the
# AIDA convention), so a version bump only needs to touch the workspace
# version + the path-dep pins.
#
#   wsd-core   — leaf, no intra deps
#   wsd-mid    — pins wsd-core
#   wsd-app    — pins wsd-core + wsd-mid
#   wsd-tool   — depends on wsd-core *without* a version (no pin to bump)
# ---------------------------------------------------------------------------
mk_crate() { mkdir -p "$TEST_DIR/$1"; cat > "$TEST_DIR/$1/Cargo.toml"; }

cat > "$TEST_DIR/Cargo.toml" <<'EOF'
[workspace]
resolver = "2"
members = [
    "wsd-core",
    "wsd-mid",
    "wsd-app",
    "wsd-tool",
]

[workspace.package]
version = "0.7.0"
EOF

mk_crate wsd-core <<'EOF'
[package]
name = "wsd-core"
version.workspace = true
edition = "2021"

[dependencies]
EOF

mk_crate wsd-mid <<'EOF'
[package]
name = "wsd-mid"
version.workspace = true
edition = "2021"

[dependencies]
wsd-core = { version = "0.7", path = "../wsd-core" }
EOF

mk_crate wsd-app <<'EOF'
[package]
name = "wsd-app"
version.workspace = true
edition = "2021"

[dependencies]
wsd-core = { version = "0.7", path = "../wsd-core", features = ["x"] }
wsd-mid = { version = "0.7", path = "../wsd-mid", optional = true }
EOF

# wsd-tool deliberately omits the version constraint — a path-only dep has
# no pin to bump and must not be flagged.
mk_crate wsd-tool <<'EOF'
[package]
name = "wsd-tool"
version.workspace = true
edition = "2021"

[dependencies]
wsd-core = { path = "../wsd-core" }
EOF

# ---------------------------------------------------------------------------
# Test 1: member discovery
# ---------------------------------------------------------------------------
info "Test 1: ws_discover_members finds every member"
members=$(ws_discover_members "$TEST_DIR" | tr '\n' ' ')
[ "$members" = "wsd-core wsd-mid wsd-app wsd-tool " ] \
    || fail "discovered members: '$members'"
pass "all 4 members discovered"

# ---------------------------------------------------------------------------
# Test 2: intra-workspace pins are listed (version-bearing path deps only)
# ---------------------------------------------------------------------------
info "Test 2: ws_list_intra_pins lists exactly the version-bearing pins"
pins=$(ws_list_intra_pins "$TEST_DIR" | sort)
expected=$(printf '%s\n' \
    "$TEST_DIR/wsd-app/Cargo.toml	wsd-core	0.7" \
    "$TEST_DIR/wsd-app/Cargo.toml	wsd-mid	0.7" \
    "$TEST_DIR/wsd-mid/Cargo.toml	wsd-core	0.7" | sort)
[ "$pins" = "$expected" ] || fail "listed pins:
$pins
expected:
$expected"
pass "3 version-bearing pins listed; the path-only wsd-tool dep is ignored"

# ---------------------------------------------------------------------------
# Test 3: SIMULATE THE BUG — add a brand-new workspace crate, then bump.
# wsd-plugin is added after the fact (pins wsd-core + wsd-mid) and wsd-app
# is updated to also pin wsd-plugin. The old hardcoded-list release script
# would miss the new crate entirely.
# ---------------------------------------------------------------------------
info "Test 3: add a new crate (wsd-plugin), run the release bump"

# Register the new member.
sed -i.bak 's/    "wsd-tool",/    "wsd-tool",\n    "wsd-plugin",/' "$TEST_DIR/Cargo.toml"
rm -f "$TEST_DIR/Cargo.toml.bak"

mk_crate wsd-plugin <<'EOF'
[package]
name = "wsd-plugin"
version.workspace = true
edition = "2021"

[dependencies]
wsd-core = { version = "0.7", path = "../wsd-core" }
wsd-mid = { version = "0.7", path = "../wsd-mid" }
EOF

# An existing crate now also pins the new crate.
cat >> "$TEST_DIR/wsd-app/Cargo.toml" <<'EOF'
wsd-plugin = { version = "0.7", path = "../wsd-plugin" }
EOF

# This is the release-script bump sequence: bump workspace version, then
# discover-and-bump every intra-workspace pin.
sed -i.bak -E '/^\[workspace\.package\]/,/^\[/ s/^version = "0.7.0"/version = "0.8.0"/' "$TEST_DIR/Cargo.toml"
rm -f "$TEST_DIR/Cargo.toml.bak"
ws_bump_intra_pins "$TEST_DIR" "0.8"

# Every intra-workspace pin — including the brand-new crate's, and the new
# pin *on* the new crate — must now be at 0.8.
if grep -rn 'version = "0.7"' "$TEST_DIR"/*/Cargo.toml; then
    fail "a pin was left at the old version 0.7 (the BUG-111 trap)"
fi
pass "no intra-workspace pin left at the old version"

grep -q 'wsd-plugin = { version = "0.8"' "$TEST_DIR/wsd-app/Cargo.toml" \
    || fail "wsd-app's pin on the new crate wsd-plugin was not bumped"
grep -q 'wsd-core = { version = "0.8"' "$TEST_DIR/wsd-plugin/Cargo.toml" \
    || fail "the new crate wsd-plugin's own pins were not bumped"
pass "the freshly-added crate is bumped in both directions"

# The path-only dep must be untouched (no version field to bump).
grep -q 'wsd-core = { path = "../wsd-core" }' "$TEST_DIR/wsd-tool/Cargo.toml" \
    || fail "the version-less path dep in wsd-tool was wrongly rewritten"
pass "path-only dep (no version) left untouched"

# ---------------------------------------------------------------------------
# Test 4: verification passes on the correctly-bumped workspace
# ---------------------------------------------------------------------------
info "Test 4: ws_verify_intra_pins accepts the fully-bumped workspace"
if ws_verify_intra_pins "$TEST_DIR" "0.8" 2>/dev/null; then
    pass "verification passed — release would proceed without manual intervention"
else
    fail "verification rejected a correctly-bumped workspace"
fi

# ---------------------------------------------------------------------------
# Test 5: verification CATCHES a missed crate (reproduces the BUG-111 fault)
# ---------------------------------------------------------------------------
info "Test 5: ws_verify_intra_pins catches and names a stale pin"
# Revert one pin to the old version — exactly what the old hardcoded list
# did to aida-tui.
sed -i.bak 's/wsd-plugin = { version = "0.8"/wsd-plugin = { version = "0.7"/' \
    "$TEST_DIR/wsd-app/Cargo.toml"
rm -f "$TEST_DIR/wsd-app/Cargo.toml.bak"

verify_out=$(ws_verify_intra_pins "$TEST_DIR" "0.8" 2>&1) && rc=0 || rc=$?
[ "${rc:-0}" -ne 0 ] || fail "verification passed despite a stale pin"
echo "$verify_out" | grep -q "wsd-plugin" \
    || fail "verification error did not name the unbumped crate (wsd-plugin)"
pass "stale pin detected, exit non-zero, error names wsd-plugin"

echo ""
echo -e "${GREEN}=== All BUG-111 regression tests passed ===${NC}"
