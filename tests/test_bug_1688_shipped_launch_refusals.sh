#!/usr/bin/env bash
# trace:BUG-1688 | ai:codex
# Verify the five named shipped scripts refuse direct full-access launches.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bug-1688-launch-fixture-XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
calls="$tmp/vendor-calls.log"
recorder="$tmp/vendor-recorder"
cat > "$recorder" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$0 $*" >> "$AIDA_VENDOR_CALL_LOG"
SH
chmod +x "$recorder"
export AIDA_VENDOR_CALL_LOG="$calls"
export AIDA_ABLATION_CLAUDE="$recorder"
export AIDA_ABLATION_CODEX="$recorder"

assert_refused() {
    local script="$1" expected="$2"; shift 2
    local output="$tmp/output" rc=0
    bash "$script" "$@" >"$output" 2>&1 || rc=$?
    if [ "$rc" -ne 78 ]; then
        cat "$output" >&2
        printf 'expected exit 78 from %s, got %s\n' "$script" "$rc" >&2
        exit 1
    fi
    if ! grep -Fq 'ERROR[AIDA_VENDOR_LAUNCH_REFUSED]' "$output" || ! grep -Fq "$expected" "$output"; then
        cat "$output" >&2
        printf 'missing typed refusal from %s\n' "$script" >&2
        exit 1
    fi
}

assert_refused_py() {
    local script="$1" expected="$2"; shift 2
    local output="$tmp/output" rc=0
    python3 "$script" "$@" >"$output" 2>&1 || rc=$?
    if [ "$rc" -ne 78 ]; then
        cat "$output" >&2
        printf 'expected exit 78 from python %s, got %s\n' "$script" "$rc" >&2
        exit 1
    fi
    if ! grep -Fq 'ERROR[AIDA_VENDOR_LAUNCH_REFUSED]' "$output" || ! grep -Fq "$expected" "$output"; then
        cat "$output" >&2
        printf 'missing typed refusal from python %s\n' "$script" >&2
        exit 1
    fi
}

for script in \
    "$ROOT/scripts/ablations/gate-vs-rule.sh" \
    "$ROOT/scripts/ablations/gate-vs-rule-i2.sh" \
    "$ROOT/scripts/ablations/gate-vs-rule-i3.sh" \
    "$ROOT/scripts/ablations/gate-vs-rule-i4.sh"; do
    assert_refused "$script" "disabled because it launches a full-access vendor"
done

# trace:BUG-1784 | ai:antigravity
claude_shim_dir="$tmp/claude-shim"
mkdir -p "$claude_shim_dir"
cat > "$claude_shim_dir/claude" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$0 $*" >> "$AIDA_VENDOR_CALL_LOG"
SH
chmod +x "$claude_shim_dir/claude"
PATH="$claude_shim_dir:$PATH"
export PATH
assert_refused_py "$ROOT/bench/agent-surface/run_bench.py" "disabled because it launches a full-access vendor directly" matrix
assert_refused_py "$ROOT/bench/agent-surface/run_bench.py" "disabled because it launches a full-access vendor directly" run --condition cli --task foo


# Acceptance audit: these are the only shipped shell scripts containing either
# full-access flag. Every match is one of the four refused targets above.
mapfile -t full_access_routes < <(rg -l 'bypassPermissions|dangerously-bypass-approvals-and-sandbox' --glob '*.sh' "$ROOT/scripts" | sort)
expected_routes=(
    "$ROOT/scripts/ablations/gate-vs-rule-i2.sh"
    "$ROOT/scripts/ablations/gate-vs-rule-i3.sh"
    "$ROOT/scripts/ablations/gate-vs-rule-i4.sh"
    "$ROOT/scripts/ablations/gate-vs-rule.sh"
)
if [ "${full_access_routes[*]}" != "${expected_routes[*]}" ]; then
    printf 'full-access route audit differs from expected list:\n%s\n' "${full_access_routes[*]}" >&2
    exit 1
fi

demo_output="$tmp/demo-output"
bash "$ROOT/scripts/aida-demo.sh" --vendor-launch-fixture >"$demo_output" 2>&1
for surface in "queue walkthrough" "Claude self-test"; do
    grep -Fq "ERROR[AIDA_VENDOR_LAUNCH_REFUSED]: $surface" "$demo_output"
done

if [ -s "$calls" ]; then
    cat "$calls" >&2
    echo 'recording vendor binary was launched' >&2
    exit 1
fi

echo 'Disposition: scripts/aida-demo.sh = manual queue walkthrough + refused Claude self-test.'
echo 'Disposition: gate-vs-rule.sh and gate-vs-rule-i2.sh through i4.sh = typed exit-78 refusal.'
echo 'Audit: no other shipped shell script contains either full-access launch flag.'
echo 'BUG-1688: all seven refusal surfaces refused; vendor launch count = 0'
