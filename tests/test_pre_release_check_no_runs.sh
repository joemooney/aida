#!/usr/bin/env bash
# tests/test_pre_release_check_no_runs.sh — regression test for TASK-389.
#
# scripts/pre-release-check.sh inspects the most recent cross-platform.yml
# run on main to decide whether to reuse it or dispatch a fresh one. When
# there are zero matching runs, `gh run list` emits `[]`. The pre-fix jq
# expression `.[0] | [...] | @tsv` rendered that as a tab-separated row of
# `null`s — a non-empty string — so the downstream `[ -z "$line" ]` "no
# prior run" branch never triggered and the script propagated `null`
# fields into `gh run watch`. The fix switches to `.[] | [...] | @tsv`,
# which iterates the array and emits *zero* rows on `[]`.
#
# This test mocks `gh` with two fixtures and asserts the routing:
#   1. empty-array fixture → script reaches the "no prior cross-platform
#      run found." branch (verifies the bug is fixed).
#   2. runs-exist fixture → script consumes the row normally (verifies the
#      happy path didn't regress).
#
# trace:TASK-389 | ai:claude
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SCRIPT="$PROJECT_ROOT/scripts/pre-release-check.sh"
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

# ---------------------------------------------------------------- mock gh
# The mock dispatches by argv shape:
#   `gh run list ...`   → emits the fixture pointed at by $GH_FIXTURE_FILE
#                         (when -q is present, applies it via jq, matching
#                         real gh behavior).
#   `gh workflow run …` → records the dispatch into $GH_DISPATCH_LOG and
#                         exits 0 (so we don't actually dispatch CI).
#   `gh run watch …`    → records into $GH_WATCH_LOG and exits 1 (so the
#                         script doesn't claim success on a fake run).
#   any other gh call   → exit 0 with no output (we don't care for routing).
mkdir -p "$TEST_DIR/bin"
cat >"$TEST_DIR/bin/gh" <<'GH_EOF'
#!/usr/bin/env bash
# Mock gh for pre-release-check.sh tests.
case "$1" in
    run)
        case "${2:-}" in
            list)
                # Parse out -q '<filter>' from the args.
                filter=""
                next_is_q=0
                for arg in "$@"; do
                    if [ "$next_is_q" = "1" ]; then
                        filter="$arg"
                        next_is_q=0
                        continue
                    fi
                    case "$arg" in
                        -q|--jq) next_is_q=1 ;;
                    esac
                done
                if [ -n "$filter" ]; then
                    jq -r "$filter" <"${GH_FIXTURE_FILE:-/dev/null}"
                else
                    cat "${GH_FIXTURE_FILE:-/dev/null}"
                fi
                ;;
            watch)
                echo "$*" >>"${GH_WATCH_LOG:-/dev/null}"
                exit 1
                ;;
            *) ;;
        esac
        ;;
    workflow)
        if [ "${2:-}" = "run" ]; then
            echo "$*" >>"${GH_DISPATCH_LOG:-/dev/null}"
        fi
        ;;
esac
GH_EOF
chmod +x "$TEST_DIR/bin/gh"
export PATH="$TEST_DIR/bin:$PATH"

# Sanity-check: our mock is on PATH ahead of any real gh.
[ "$(command -v gh)" = "$TEST_DIR/bin/gh" ] || fail "mock gh not on PATH"

# ---------------------------------------------------------------- test 1
info "empty-array fixture routes through the no-prior-run branch"

EMPTY_FIXTURE="$TEST_DIR/empty.json"
echo '[]' >"$EMPTY_FIXTURE"
export GH_FIXTURE_FILE="$EMPTY_FIXTURE"
export GH_DISPATCH_LOG="$TEST_DIR/dispatch_empty.log"
export GH_WATCH_LOG="$TEST_DIR/watch_empty.log"
: >"$GH_DISPATCH_LOG"
: >"$GH_WATCH_LOG"

out=$("$SCRIPT" 2>&1 || true)

if ! echo "$out" | grep -qF "no prior cross-platform run found"; then
    echo "--- script output ---"
    echo "$out"
    echo "---"
    fail "expected 'no prior cross-platform run found' message; got the populated-row path instead"
fi
pass "empty array → no-prior-run branch"

if ! [ -s "$GH_DISPATCH_LOG" ]; then
    fail "expected dispatch_and_watch to call 'gh workflow run' on the empty-array path"
fi
pass "empty array → dispatch_and_watch invoked 'gh workflow run'"

# ---------------------------------------------------------------- test 2
info "runs-exist fixture consumes the row normally (no regression)"

# Recent green run (created 1h ago) → script should reuse it and exit 0.
created_at=$(date -u -d '1 hour ago' +%Y-%m-%dT%H:%M:%SZ 2>/dev/null \
            || date -u -v-1H +%Y-%m-%dT%H:%M:%SZ)
RUNS_FIXTURE="$TEST_DIR/runs.json"
cat >"$RUNS_FIXTURE" <<EOF
[{"status":"completed","conclusion":"success","createdAt":"$created_at","databaseId":4242,"url":"https://example.invalid/runs/4242"}]
EOF
export GH_FIXTURE_FILE="$RUNS_FIXTURE"
export GH_DISPATCH_LOG="$TEST_DIR/dispatch_runs.log"
export GH_WATCH_LOG="$TEST_DIR/watch_runs.log"
: >"$GH_DISPATCH_LOG"
: >"$GH_WATCH_LOG"

if ! out=$("$SCRIPT" 2>&1); then
    echo "--- script output ---"
    echo "$out"
    echo "---"
    fail "script exited non-zero on a fresh green run"
fi

if ! echo "$out" | grep -qF "cross-platform CI is green and recent"; then
    echo "--- script output ---"
    echo "$out"
    echo "---"
    fail "expected 'cross-platform CI is green and recent' on the happy path"
fi
pass "populated array → reused-green-run branch"

if [ -s "$GH_DISPATCH_LOG" ]; then
    fail "expected NO dispatch on the reused-green-run path (got $(wc -l <"$GH_DISPATCH_LOG") dispatches)"
fi
pass "populated array → no dispatch_and_watch on the reuse path"

echo
echo -e "${GREEN}All tests passed.${NC}"
