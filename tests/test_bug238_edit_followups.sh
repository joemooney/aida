#!/bin/bash
# Regression test for BUG-238: `aida edit <ID> --status completed` (and `--status done`)
# must trigger the same plan `## Followups` parse that `aida queue done` and the
# STORY-86 Done→Completed auto-bump already do. The `/aida-review` skill marks
# specs Completed directly via this path; without the fix, plan followups are
# silently lost.
#
# trace:BUG-238 | ai:claude

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Honor CARGO_TARGET_DIR (the shared aida workspace target dir).
TARGET_DIR="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
AIDA="$TARGET_DIR/debug/aida"

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

info "Building aida..."
cd "$PROJECT_ROOT"
cargo build -p aida-cli --quiet 2>/dev/null || cargo build -p aida-cli

[ -x "$AIDA" ] || fail "aida binary not found at $AIDA"

echo ""
echo "=== BUG-238: aida edit --status completed triggers ## Followups parse ==="
echo "Test directory: $TEST_DIR"
echo ""

# Init a fresh project inside TEST_DIR (the `init` command needs a git repo).
cd "$TEST_DIR"
git init -q
git config user.email "test@example.com"
git config user.name "BUG-238 Test"
git commit -q --allow-empty -m "init"
"$AIDA" init --force >/dev/null 2>&1

# ----------------------------------------------------------------------------
# Test 1: edit --status completed on a spec whose plan has ## Followups
#         → followup TASKs are filed as children (non-TTY → file all).
# ----------------------------------------------------------------------------
info "Test 1: edit --status completed triggers the parse"

ADD_OUT=$("$AIDA" add --title "Test 1 parent" --description "x" --type task --status approved 2>&1)
SPEC1=$(echo "$ADD_OUT" | grep -oE 'TASK-[0-9]+' | head -1)
[ -n "$SPEC1" ] || fail "Could not extract spec id from add output"

mkdir -p docs/plans
cat > "docs/plans/2026-05-20-test1.md" <<EOF
# Plan: $SPEC1

Specs: $SPEC1

## Followups

- First followup from the plan.
- Second followup from the plan.

## Related

- nothing
EOF

# stdin from /dev/null → non-TTY → file all (mirrors /aida-review skill context).
"$AIDA" edit "$SPEC1" --status completed </dev/null >/dev/null 2>&1

CHILDREN=$("$AIDA" list --parent "$SPEC1" 2>/dev/null | grep -cE "^TASK-" || true)
[ "$CHILDREN" -eq 2 ] || fail "Expected 2 followup children, found $CHILDREN"
pass "edit --status completed filed 2 followups as child TASKs"

# ----------------------------------------------------------------------------
# Test 2: --status done also triggers the parse (BUG-238 acceptance covers
#         both terminal-completed transitions).
# ----------------------------------------------------------------------------
info "Test 2: edit --status done triggers the parse"

ADD_OUT=$("$AIDA" add --title "Test 2 parent" --description "x" --type task --status approved 2>&1)
SPEC2=$(echo "$ADD_OUT" | grep -oE 'TASK-[0-9]+' | head -1)
[ -n "$SPEC2" ] || fail "Could not extract spec id (test 2)"

cat > "docs/plans/2026-05-20-test2.md" <<EOF
# Plan: $SPEC2

Specs: $SPEC2

## Followups

- Only one followup for test 2.
EOF

"$AIDA" edit "$SPEC2" --status done </dev/null >/dev/null 2>&1

CHILDREN=$("$AIDA" list --parent "$SPEC2" 2>/dev/null | grep -cE "^TASK-" || true)
[ "$CHILDREN" -eq 1 ] || fail "Expected 1 followup child for $SPEC2, found $CHILDREN"
pass "edit --status done filed the followup as a child TASK"

# ----------------------------------------------------------------------------
# Test 3: idempotency — re-completing a spec does NOT double-file. The
#         FOLLOWUPS_MARKER comment is the guard; whichever extraction path
#         runs first wins.
# ----------------------------------------------------------------------------
info "Test 3: idempotency — re-completing does not double-file"

# Re-open the spec from Test 1 (Completed → Approved needs --force per the
# TASK-47 terminal-status guard), then re-complete it.
"$AIDA" edit "$SPEC1" --status approved --force </dev/null >/dev/null 2>&1
"$AIDA" edit "$SPEC1" --status completed </dev/null >/dev/null 2>&1

CHILDREN=$("$AIDA" list --parent "$SPEC1" 2>/dev/null | grep -cE "^TASK-" || true)
[ "$CHILDREN" -eq 2 ] || fail "Expected still 2 children after re-completion (idempotency), found $CHILDREN"
pass "Re-completion did not double-file (marker guard worked)"

# ----------------------------------------------------------------------------
# Test 4: AIDA_AUTO_FOLLOWUPS=false opt-out — the parse is skipped.
# ----------------------------------------------------------------------------
info "Test 4: AIDA_AUTO_FOLLOWUPS=false skips the parse"

ADD_OUT=$("$AIDA" add --title "Test 4 parent" --description "x" --type task --status approved 2>&1)
SPEC4=$(echo "$ADD_OUT" | grep -oE 'TASK-[0-9]+' | head -1)
[ -n "$SPEC4" ] || fail "Could not extract spec id (test 4)"

cat > "docs/plans/2026-05-20-test4.md" <<EOF
# Plan: $SPEC4

Specs: $SPEC4

## Followups

- A followup that should NOT be filed (opt-out).
EOF

AIDA_AUTO_FOLLOWUPS=false "$AIDA" edit "$SPEC4" --status completed </dev/null >/dev/null 2>&1

CHILDREN=$("$AIDA" list --parent "$SPEC4" 2>/dev/null | grep -cE "^TASK-" || true)
[ "$CHILDREN" -eq 0 ] || fail "Expected 0 children with AIDA_AUTO_FOLLOWUPS=false, found $CHILDREN"
pass "AIDA_AUTO_FOLLOWUPS=false correctly skipped the parse"

# ----------------------------------------------------------------------------
# Test 5: a non-terminal status flip (Approved → Planned) does NOT trigger
#         the parse — followups are only filed on transitions into Done or
#         Completed.
# ----------------------------------------------------------------------------
info "Test 5: non-terminal status flip does not trigger the parse"

ADD_OUT=$("$AIDA" add --title "Test 5 parent" --description "x" --type task --status approved 2>&1)
SPEC5=$(echo "$ADD_OUT" | grep -oE 'TASK-[0-9]+' | head -1)
[ -n "$SPEC5" ] || fail "Could not extract spec id (test 5)"

cat > "docs/plans/2026-05-20-test5.md" <<EOF
# Plan: $SPEC5

Specs: $SPEC5

## Followups

- A followup that should NOT be filed (non-terminal flip).
EOF

"$AIDA" edit "$SPEC5" --status planned </dev/null >/dev/null 2>&1

CHILDREN=$("$AIDA" list --parent "$SPEC5" 2>/dev/null | grep -cE "^TASK-" || true)
[ "$CHILDREN" -eq 0 ] || fail "Expected 0 children for non-terminal flip, found $CHILDREN"
pass "Non-terminal status flip (Approved → Planned) did not trigger the parse"

# ============================================================================
# Summary
# ============================================================================
echo ""
echo "=== All BUG-238 tests passed ==="
echo ""
