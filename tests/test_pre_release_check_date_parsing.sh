#!/usr/bin/env bash
# tests/test_pre_release_check_date_parsing.sh — TASK-284 regression test.
#
# scripts/pre-release-check.sh used `date -d "$created_at" +%s` to parse the
# timestamp of the last cross-platform CI run. `date -d` is a GNU extension;
# on a BSD/macOS host the call would fail silently and `created_epoch`
# would always be 0, so the script ALWAYS dispatches a fresh run instead of
# honouring a recent green one — defeating the freshness gate.
#
# This test sources the script (the BASH_SOURCE guard suppresses its main
# flow) and exercises `parse_iso_to_epoch` against:
#
#   1. The real GNU date on this host — sanity-check the happy path.
#   2. A stubbed BSD date — assert the BSD branch is reached and its
#      argument form (`date -j -u -f "%Y-%m-%dT%H:%M:%SZ" <str> +%s`) is
#      what we actually emit.
#   3. A stub that rejects every form — assert the fallback echoes 0
#      rather than blowing up.
#
# trace:TASK-284 | ai:claude
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

STUB_DIR=$(mktemp -d)
cleanup() { rm -rf "$STUB_DIR"; }
trap cleanup EXIT

# Source the script under test. The BASH_SOURCE guard inside
# pre-release-check.sh suppresses the main flow when sourced, so
# parse_iso_to_epoch becomes available without the script trying to
# contact GitHub.
# shellcheck source=scripts/pre-release-check.sh
. "$PROJECT_ROOT/scripts/pre-release-check.sh"

echo ""
echo "=== TASK-284: pre-release-check.sh portable date parsing ==="
echo ""

# ---------------------------------------------------------------------------
# Case 1 — real GNU date on this host (Linux runs GNU coreutils).
# ---------------------------------------------------------------------------
info "Case 1: real GNU date — known timestamp parses to expected epoch"
# 2026-05-19T12:34:56Z = 1779194096 in Unix epoch seconds
# (verified locally via `date -d "2026-05-19T12:34:56Z" +%s`).
expected_epoch=1779194096
got=$(parse_iso_to_epoch "2026-05-19T12:34:56Z")
if [ "$got" = "$expected_epoch" ]; then
    pass "real GNU date parsed 2026-05-19T12:34:56Z to $got"
else
    fail "expected $expected_epoch, got '$got'"
fi

# ---------------------------------------------------------------------------
# Case 2 — simulated BSD date. The stub rejects `-d` (the GNU form), and
# accepts only the BSD form `-j -u -f <fmt> <str> +%s`. Putting the stub
# first on PATH shadows /bin/date for the duration of this case.
# ---------------------------------------------------------------------------
info "Case 2: stubbed BSD date — only -j -u -f succeeds"
cat > "$STUB_DIR/date" <<'STUB'
#!/usr/bin/env bash
# Fake BSD-style date. Rejects -d (the GNU form). Accepts:
#   date -j -u -f "%Y-%m-%dT%H:%M:%SZ" <str> +%s
# and emits a known constant so the test can assert the BSD branch was hit.
if [ "$1" = "-d" ]; then
    echo "date: illegal option -- d" >&2
    exit 1
fi
if [ "$1" = "-j" ] && [ "$2" = "-u" ] && [ "$3" = "-f" ] \
   && [ "$4" = "%Y-%m-%dT%H:%M:%SZ" ] && [ -n "$5" ] && [ "$6" = "+%s" ]; then
    echo 1779194096
    exit 0
fi
# Anything else: also fail, so an unexpected call shape surfaces.
echo "stub-date: unexpected argv: $*" >&2
exit 2
STUB
chmod +x "$STUB_DIR/date"

got=$(PATH="$STUB_DIR:$PATH" parse_iso_to_epoch "2026-05-19T12:34:56Z")
if [ "$got" = "1779194096" ]; then
    pass "BSD-style stub matched -j -u -f form; parse_iso_to_epoch echoed $got"
else
    fail "expected 1779194096 from BSD branch, got '$got'"
fi

# ---------------------------------------------------------------------------
# Case 3 — both forms fail. Helper must echo 0 (the "unknown age" sentinel),
# not crash under `set -e` in the caller.
# ---------------------------------------------------------------------------
info "Case 3: stubbed date that rejects everything — falls back to 0"
cat > "$STUB_DIR/date" <<'STUB'
#!/usr/bin/env bash
echo "stub-date: refusing argv: $*" >&2
exit 1
STUB
chmod +x "$STUB_DIR/date"

got=$(PATH="$STUB_DIR:$PATH" parse_iso_to_epoch "2026-05-19T12:34:56Z")
if [ "$got" = "0" ]; then
    pass "both branches failing → echoed 0 as documented"
else
    fail "expected 0 fallback, got '$got'"
fi

# ---------------------------------------------------------------------------
# Case 4 — malformed input under real GNU date. Should also yield 0.
# ---------------------------------------------------------------------------
info "Case 4: real GNU date with garbage input — falls back to 0"
got=$(parse_iso_to_epoch "not-a-timestamp")
# GNU date is lenient and may parse some odd strings. We only assert that
# either it parsed (non-zero numeric) OR it fell back to 0 — never crashed.
if [ "$got" = "0" ] || [[ "$got" =~ ^[0-9]+$ ]]; then
    pass "garbage input handled gracefully (echoed '$got')"
else
    fail "expected '0' or a numeric, got '$got'"
fi

echo ""
echo -e "${GREEN}=== All TASK-284 portable-date-parsing tests passed ===${NC}"
