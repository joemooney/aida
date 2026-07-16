#!/usr/bin/env bash
# tests/test_docs_gate.sh — TASK-1149 regression test.
#
# scripts/docs-gate.sh is the release-time documentation gate (the one
# load-bearing idea cherry-picked from EPIC-25: gate docs at RELEASE, not
# per-PR). Its blocking check verifies the regenerated CHANGELOG.md (a)
# contains a section for the version being tagged and (b) is idempotent —
# a second `changelog refresh` produces byte-identical output.
#
# This test sources the script (the BASH_SOURCE guard suppresses its main
# flow) and drives `docs_gate_changelog` / `docs_gate_plan_advisory`
# against a stubbed `aida` binary, so it never rebuilds the real CLI or
# touches the real store:
#
#   1. A well-behaved stub (section present, deterministic)  → gate passes.
#   2. A stub that omits the version section                 → gate fails.
#   3. A stub that emits different content each run          → gate fails
#      (non-idempotent — would leave uncommitted drift).
#   4. A stub that fails outright                            → gate fails.
#   5. Plan advisory in a tag-less repo                      → skips, exit 0.
#
# trace:TASK-1149 | ai:claude
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'
pass() { echo -e "${GREEN}PASS${NC} $1"; }
fail() { echo -e "${RED}FAIL${NC} $1"; exit 1; }
info() { echo -e "${YELLOW}----${NC} $1"; }

TEST_DIR=$(mktemp -d)
STUB_DIR="$TEST_DIR/stubs"
mkdir -p "$STUB_DIR"
cleanup() { rm -rf "$TEST_DIR"; }
trap cleanup EXIT

# Source the gate. The BASH_SOURCE guard suppresses its main flow, exposing
# the check functions without running a real release.
# shellcheck source=scripts/docs-gate.sh
. "$PROJECT_ROOT/scripts/docs-gate.sh"

# A throwaway git repo so `git hash-object` / `git describe` work.
cd "$TEST_DIR"
git init -q -b main
git config user.email "test@example.com"
git config user.name "TASK-1149 test"

VERSION="0.15.0"

# make_stub <name> <body> — write an executable stub and point AIDA_BIN at it.
make_stub() {
    local body="$2"
    printf '%s\n' "#!/usr/bin/env bash" "$body" > "$STUB_DIR/$1"
    chmod +x "$STUB_DIR/$1"
    export AIDA_BIN="$STUB_DIR/$1"
    docs_gate_resolve_aida
}

echo ""
echo "=== TASK-1149: release-time documentation gate ==="
echo ""

# ---------------------------------------------------------------------------
# Case 1 — well-behaved changelog: section present + deterministic → pass.
# ---------------------------------------------------------------------------
info "Case 1: current + idempotent changelog → docs_gate_changelog passes"
make_stub aida-good 'printf "%s\n" "## [v0.15.0] — 2026-07-16" "- TASK-1149 — docs gate" > CHANGELOG.md'
if docs_gate_changelog "$VERSION" >/dev/null 2>&1; then
    pass "well-behaved changelog accepted"
else
    fail "expected docs_gate_changelog to pass on a current+idempotent changelog"
fi

# ---------------------------------------------------------------------------
# Case 2 — no section for the release version → fail.
# ---------------------------------------------------------------------------
info "Case 2: changelog missing the '## [v0.15.0]' section → fails"
make_stub aida-nosection 'printf "%s\n" "## [Unreleased]" "- nothing tagged" > CHANGELOG.md'
if docs_gate_changelog "$VERSION" >/dev/null 2>&1; then
    fail "expected failure when the version section is absent"
else
    pass "missing version section rejected"
fi

# ---------------------------------------------------------------------------
# Case 3 — non-idempotent generation (nonce per run) → fail. The header is
# always present so the grep passes; only the idempotency check catches it.
# ---------------------------------------------------------------------------
info "Case 3: non-deterministic changelog (drift on re-run) → fails"
make_stub aida-drift '
n=$(cat "'"$TEST_DIR"'/nonce" 2>/dev/null || echo 0)
n=$((n + 1)); echo "$n" > "'"$TEST_DIR"'/nonce"
printf "%s\n" "## [v0.15.0] — 2026-07-16" "- run $n" > CHANGELOG.md'
if docs_gate_changelog "$VERSION" >/dev/null 2>&1; then
    fail "expected failure when regeneration is non-idempotent"
else
    pass "non-idempotent changelog rejected"
fi

# ---------------------------------------------------------------------------
# Case 4 — changelog generation fails outright → fail (blocking, not a
# warning as the pre-gate release.sh path treated it).
# ---------------------------------------------------------------------------
info "Case 4: 'aida changelog refresh' exits non-zero → fails"
make_stub aida-fail 'echo "boom" >&2; exit 1'
if docs_gate_changelog "$VERSION" >/dev/null 2>&1; then
    fail "expected failure when changelog generation errors"
else
    pass "failed changelog generation rejected"
fi

# ---------------------------------------------------------------------------
# Case 5 — plan advisory is non-blocking: a repo with no tags must return 0
# and never abort the caller under set -e.
# ---------------------------------------------------------------------------
info "Case 5: plan-ref advisory with no prior tag → skips, returns 0"
make_stub aida-noop 'exit 0'
if docs_gate_plan_advisory >/dev/null 2>&1; then
    pass "advisory returned 0 with no previous tag"
else
    fail "advisory must never return non-zero (it is informational only)"
fi

echo ""
echo -e "${GREEN}=== All TASK-1149 documentation-gate tests passed ===${NC}"
