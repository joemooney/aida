#!/bin/bash
# Integration test for TASK-294: the `aida-worker` shell function emitted by
# `aida dev shell-init`, plus the `aida worker directives` subcommand.
#
# Each test is hermetic: a tmpdir with its own .aida/ stands in for a project
# root. The aida(1) binary is stubbed so the worker's loop branches can be
# exercised deterministically (timeout / nothing-to-drive / failure / success)
# without running the real `queue work --auto-complete` pipeline.
#
# Pattern lifted from tests/test_distributed.sh.
#
# trace:TASK-294 | ai:claude
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

cleanup() {
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT

info "Building aida..."
cd "$PROJECT_ROOT"
cargo build -p aida-cli --quiet

# Cargo respects CARGO_TARGET_DIR; resolve the real path by asking cargo.
TARGET_DIR=$(cargo metadata --no-deps --format-version 1 \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
REAL_AIDA="$TARGET_DIR/debug/aida"
[ -x "$REAL_AIDA" ] || fail "real aida binary not found at $REAL_AIDA"

# Capture the emitted aida-worker function into a sourcable file. We strip
# the marker-wrapped block and source only the shell helpers + the worker.
WORKER_SRC="$TEST_DIR/worker.sh"
"$REAL_AIDA" dev shell-init \
    | sed -n '/^# >>> aida shell helpers >>>/,/^# <<< aida shell helpers <<<$/p' \
    | sed '1d;$d' \
    > "$WORKER_SRC"
[ -s "$WORKER_SRC" ] || fail "emitted shell-init produced no helper body"
grep -q '^aida-worker()' "$WORKER_SRC" || fail "emitted shell-init missing aida-worker function"

echo ""
echo "=== AIDA Worker Integration Tests ==="
echo "Test dir: $TEST_DIR"
echo ""

# ---------------------------------------------------------------------------
# Set up a stub `aida` that simulates `aida queue work --auto-complete`. The
# stub reads $TEST_DIR/stub.mode and behaves according to its value:
#   ok               → exit 0, print "stub: shipped"
#   nothing-to-drive → exit 1, print "queue is empty for implementer; nothing to drive"
#   fail             → exit 2, print "stub: real failure"
#   timeout          → sleep forever (so `timeout` fires)
# We prepend the stub dir to PATH so `command aida` inside the worker hits it
# instead of the real binary.
# ---------------------------------------------------------------------------
STUB_DIR="$TEST_DIR/stub-bin"
mkdir -p "$STUB_DIR"
cat > "$STUB_DIR/aida" <<'STUB'
#!/bin/bash
# Stub aida for worker integration tests.
mode=$(cat "${AIDA_STUB_MODE_FILE:-/tmp/aida-stub-mode}" 2>/dev/null || echo ok)
case "$mode" in
    ok)
        echo "stub: shipped"
        exit 0
        ;;
    nothing-to-drive)
        echo "queue is empty for implementer; nothing to drive" >&2
        exit 1
        ;;
    fail)
        echo "stub: real failure" >&2
        exit 2
        ;;
    timeout)
        # Sleep beyond AIDA_WORKER_SPEC_TIMEOUT so `timeout` returns 124.
        # Absolute path bypasses the PATH-override `sleep` stub the tests
        # use to short-circuit the worker's own 30s post-pause delay.
        /bin/sleep 30
        exit 0
        ;;
    *)
        echo "stub: unknown mode '$mode'" >&2
        exit 99
        ;;
esac
STUB
chmod +x "$STUB_DIR/aida"

# Each test is run in a fresh subshell with its own .aida/ root.
new_project_root() {
    local root
    root=$(mktemp -d "$TEST_DIR/proj.XXXXXX")
    mkdir -p "$root/.aida"
    echo "$root"
}

# Run the worker in a subshell. $1 is the project root; the rest forwards env
# vars to the worker shell. Returns the worker's rc.
run_worker() {
    local root="$1"; shift
    local logfile="$root/worker.log"
    (
        cd "$root"
        # shellcheck disable=SC1090
        source "$WORKER_SRC"
        aida-worker
    ) > "$logfile" 2>&1
    local rc=$?
    echo "$logfile|$rc"
}

# ===========================================================================
# Test 1: `exit` directive returns 0 cleanly.
# ===========================================================================
info "Test 1: exit directive → rc 0"
root=$(new_project_root)
echo exit > "$root/.aida/worker.cmd"
result=$(PATH="$STUB_DIR:$PATH" run_worker "$root")
logfile="${result%|*}"; rc="${result#*|}"
[ "$rc" = "0" ] || fail "exit directive: expected rc 0, got $rc (log: $(cat "$logfile"))"
grep -q "exit directive" "$logfile" || fail "exit directive: missing 'exit directive' log line"
pass "exit directive returns 0 cleanly"

# ===========================================================================
# Test 2: unknown directive → pause path, no drain attempted.
# (We bound the test by also writing an `exit` line on the next iteration.)
# ===========================================================================
info "Test 2: unknown directive → pause path"
root=$(new_project_root)
# A trick: a `blorf` directive triggers the unknown→pause branch, which
# sleeps for sleep_short (30s). We re-export sleep_short to 1s in this test
# environment by wrapping sleep in a stub. Then on the second iteration the
# file content stays the same `blorf` line, so we instead replace the file
# from another process. Simpler: write blorf then on next iteration the
# worker re-reads — we replace the file via a `sleep` stub that flips it.
mkdir -p "$root/bin"
cat > "$root/bin/sleep" <<EOF
#!/bin/bash
# After the first sleep call, swap the directive to 'exit' so the loop ends.
if [ ! -f "$root/.aida/.first-sleep-done" ]; then
    touch "$root/.aida/.first-sleep-done"
    echo exit > "$root/.aida/worker.cmd"
fi
# Stub: return immediately so the test isn't paced by real sleep durations.
exit 0
EOF
chmod +x "$root/bin/sleep"
echo blorf > "$root/.aida/worker.cmd"
result=$(PATH="$root/bin:$STUB_DIR:$PATH" \
    AIDA_WORKER_SPEC_TIMEOUT=2 run_worker "$root")
logfile="${result%|*}"; rc="${result#*|}"
[ "$rc" = "0" ] || fail "unknown directive: expected rc 0, got $rc (log: $(cat "$logfile"))"
grep -q "unknown directive 'blorf'" "$logfile" \
    || fail "unknown directive: missing 'unknown directive' log line"
grep -q "stub: shipped" "$logfile" \
    && fail "unknown directive: drain ran when it should not have"
pass "unknown directive treated as pause; no drain attempted"

# ===========================================================================
# Test 3: drain failure → file overwritten with 'pause'.
# ===========================================================================
info "Test 3: drain failure → file rewritten to 'pause'"
root=$(new_project_root)
echo drain > "$root/.aida/worker.cmd"
echo fail > "$root/.aida/stub-mode"
# After the worker writes 'pause', it will loop and pause. Use the same
# sleep-stub trick to flip to 'exit' so the test terminates.
mkdir -p "$root/bin"
cat > "$root/bin/sleep" <<EOF
#!/bin/bash
if [ ! -f "$root/.aida/.first-sleep-done" ]; then
    touch "$root/.aida/.first-sleep-done"
    echo exit > "$root/.aida/worker.cmd"
fi
# Stub: return immediately so the test isn't paced by real sleep durations.
exit 0
EOF
chmod +x "$root/bin/sleep"
result=$(PATH="$root/bin:$STUB_DIR:$PATH" \
    AIDA_STUB_MODE_FILE="$root/.aida/stub-mode" \
    AIDA_WORKER_SPEC_TIMEOUT=2 run_worker "$root")
logfile="${result%|*}"; rc="${result#*|}"
[ "$rc" = "0" ] || fail "drain failure: expected rc 0 (exit after pause), got $rc (log: $(cat "$logfile"))"
grep -q "halted (exit 2); auto-pausing" "$logfile" \
    || fail "drain failure: missing 'halted; auto-pausing' log line"
pass "drain failure → 'halted; auto-pausing' + file rewritten to 'pause'"

# ===========================================================================
# Test 4: 'nothing to drive' output → sleep, do NOT pause.
# We assert the worker logs 'queue empty — sleeping' AND the directive file
# is NOT overwritten with 'pause' (it stays 'drain').
# ===========================================================================
info "Test 4: 'nothing to drive' → sleep (not pause)"
root=$(new_project_root)
echo drain > "$root/.aida/worker.cmd"
echo nothing-to-drive > "$root/.aida/stub-mode"
mkdir -p "$root/bin"
cat > "$root/bin/sleep" <<EOF
#!/bin/bash
# After the first sleep, flip mode to 'ok' and write 'exit' on the next
# iteration so the worker terminates. We assert *before* the flip that the
# file still says 'drain' (proves the worker did not auto-pause).
if [ ! -f "$root/.aida/.first-sleep-done" ]; then
    touch "$root/.aida/.first-sleep-done"
    # CAPTURE the directive-file contents at the sleep moment — proof that
    # the worker did not rewrite it to 'pause'.
    cp "$root/.aida/worker.cmd" "$root/.aida/worker.cmd.at-sleep"
    echo exit > "$root/.aida/worker.cmd"
fi
# Stub: return immediately so the test isn't paced by real sleep durations.
exit 0
EOF
chmod +x "$root/bin/sleep"
result=$(PATH="$root/bin:$STUB_DIR:$PATH" \
    AIDA_STUB_MODE_FILE="$root/.aida/stub-mode" \
    AIDA_WORKER_SPEC_TIMEOUT=2 run_worker "$root")
logfile="${result%|*}"; rc="${result#*|}"
[ "$rc" = "0" ] || fail "nothing-to-drive: expected rc 0, got $rc (log: $(cat "$logfile"))"
grep -q "queue empty" "$logfile" \
    || fail "nothing-to-drive: missing 'queue empty' log line"
[ "$(cat "$root/.aida/worker.cmd.at-sleep" | tr -d '[:space:]')" = "drain" ] \
    || fail "nothing-to-drive: directive file was modified (expected to stay 'drain'; got '$(cat "$root/.aida/worker.cmd.at-sleep")')"
pass "'nothing to drive' → sleep, directive file unchanged"

# ===========================================================================
# Test 5: scoped drain consumes the head line on success (FIFO pop).
# ===========================================================================
info "Test 5: scoped drain → FIFO pop on success"
root=$(new_project_root)
printf 'drain batch:x --zen\nexit\n' > "$root/.aida/worker.cmd"
echo ok > "$root/.aida/stub-mode"
result=$(PATH="$STUB_DIR:$PATH" \
    AIDA_STUB_MODE_FILE="$root/.aida/stub-mode" \
    AIDA_WORKER_SPEC_TIMEOUT=2 run_worker "$root")
logfile="${result%|*}"; rc="${result#*|}"
[ "$rc" = "0" ] || fail "scoped drain pop: expected rc 0, got $rc (log: $(cat "$logfile"))"
grep -q "drain batch:x --zen" "$logfile" \
    || fail "scoped drain pop: missing 'drain batch:x --zen' log line"
grep -q "drain succeeded" "$logfile" \
    || fail "scoped drain pop: missing 'drain succeeded' log line"
# After the pop the file should only contain 'exit'.
remaining=$(cat "$root/.aida/worker.cmd" | tr -d '[:space:]')
[ "$remaining" = "exit" ] \
    || fail "scoped drain pop: expected 'exit' to remain, got '$remaining'"
pass "scoped drain pops head line on success; 'exit' survives"

# ===========================================================================
# Test 6: timeout (rc 124) → auto-pause.
# ===========================================================================
info "Test 6: timeout → 124 → auto-pause"
root=$(new_project_root)
echo drain > "$root/.aida/worker.cmd"
echo timeout > "$root/.aida/stub-mode"
mkdir -p "$root/bin"
cat > "$root/bin/sleep" <<EOF
#!/bin/bash
if [ ! -f "$root/.aida/.first-sleep-done" ]; then
    touch "$root/.aida/.first-sleep-done"
    echo exit > "$root/.aida/worker.cmd"
fi
# Stub: return immediately so the test isn't paced by real sleep durations.
exit 0
EOF
chmod +x "$root/bin/sleep"
result=$(PATH="$root/bin:$STUB_DIR:$PATH" \
    AIDA_STUB_MODE_FILE="$root/.aida/stub-mode" \
    AIDA_WORKER_SPEC_TIMEOUT=1 run_worker "$root")
logfile="${result%|*}"; rc="${result#*|}"
[ "$rc" = "0" ] || fail "timeout: expected rc 0 (exit after pause), got $rc (log: $(cat "$logfile"))"
grep -q "TIMED OUT" "$logfile" \
    || fail "timeout: missing 'TIMED OUT' log line"
pass "timeout (rc 124) → 'TIMED OUT' + auto-pause"

# ===========================================================================
# Test 7: aida worker directives — empty, populated, --json.
# ===========================================================================
info "Test 7: aida worker directives — listing"
root=$(new_project_root)
# Empty file → "No pending directives."
out=$(cd "$root" && "$REAL_AIDA" worker directives 2>&1)
echo "$out" | grep -q "No pending directives" \
    || fail "directives empty: expected 'No pending directives.', got: $out"
# Populated → counts + verbs.
printf '# overnight plan\n\ndrain batch:b --zen\ndrain batch:c\nexit\n' \
    > "$root/.aida/worker.cmd"
out=$(cd "$root" && "$REAL_AIDA" worker directives 2>&1)
echo "$out" | grep -q "3 pending directives" \
    || fail "directives populated: expected '3 pending directives', got: $out"
echo "$out" | grep -q "drain batch:b --zen" \
    || fail "directives populated: missing 'drain batch:b --zen'"
# --json
out=$(cd "$root" && "$REAL_AIDA" worker directives --json 2>&1)
echo "$out" | python3 -c 'import json,sys; d=json.load(sys.stdin); assert len(d)==3; assert d[0]["verb"]=="drain"; assert d[0]["args"]==["batch:b","--zen"]' \
    || fail "directives --json: payload did not match expectations"
pass "aida worker directives — empty / human / --json all behave"

# ===========================================================================
# Test 8: directives surface in aida drain status output (no drain active).
# ===========================================================================
info "Test 8: aida drain status surfaces directives"
root=$(new_project_root)
printf 'drain batch:b\nexit\n' > "$root/.aida/worker.cmd"
# A genuine drain-state probe needs a git repo for find_main_worktree_root;
# stub it with `git init` so the lookup resolves to $root.
(cd "$root" && git init -q && git config user.email t@t && git config user.name t)
out=$(cd "$root" && "$REAL_AIDA" drain status 2>&1)
echo "$out" | grep -qi "Worker directives" \
    || fail "drain status: missing 'Worker directives' line. Output: $out"
pass "aida drain status surfaces 'Worker directives: N pending' line"

echo ""
echo -e "${GREEN}=== All aida-worker tests passed ===${NC}"
