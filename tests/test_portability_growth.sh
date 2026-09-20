#!/usr/bin/env bash
set -euo pipefail

# trace:BUG-1301 | ai:codex
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHECK="$ROOT/scripts/check-portability-growth.py"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat >"$TMP/base-rules.json" <<'EOF'
[{"id":"old","label":"old","regex":"old"}]
EOF
printf 'old\told.rs:old\n' >"$TMP/base-allowlist"

expect_failure() {
  local expected="$1"
  shift
  if "$@" >"$TMP/stdout" 2>"$TMP/stderr"; then
    echo "expected command to fail: $*" >&2
    exit 1
  fi
  grep -F "$expected" "$TMP/stderr" >/dev/null
}

# An unchanged rule set cannot excuse new debt.
printf 'old\told.rs:old\nold\tnew.rs:old\n' >"$TMP/current-allowlist"
cp "$TMP/base-rules.json" "$TMP/current-rules.json"
expect_failure "rules are unchanged" python3 "$CHECK" \
  "$TMP/base-allowlist" "$TMP/base-rules.json" \
  "$TMP/current-allowlist" "$TMP/current-rules.json"

# A new rule may baseline its findings, but an old rule still may not.
cat >"$TMP/current-rules.json" <<'EOF'
[
  {"id":"old","label":"old","regex":"old"},
  {"id":"new","label":"new","regex":"new"}
]
EOF
printf 'old\told.rs:old\nnew\tlegacy.rs:new\n' >"$TMP/current-allowlist"
python3 "$CHECK" "$TMP/base-allowlist" "$TMP/base-rules.json" \
  "$TMP/current-allowlist" "$TMP/current-rules.json"
printf 'old\told.rs:old\nnew\tlegacy.rs:new\nold\tnew.rs:old\n' >"$TMP/current-allowlist"
expect_failure "pre-existing rules" python3 "$CHECK" \
  "$TMP/base-allowlist" "$TMP/base-rules.json" \
  "$TMP/current-allowlist" "$TMP/current-rules.json"

# Entries for a removed rule remain tolerated.
printf '[]\n' >"$TMP/current-rules.json"
cp "$TMP/base-allowlist" "$TMP/current-allowlist"
python3 "$CHECK" "$TMP/base-allowlist" "$TMP/base-rules.json" \
  "$TMP/current-allowlist" "$TMP/current-rules.json"

# CI executes the base verifier. A weakened current verifier cannot approve
# an old-rule addition when the trusted base verifier still rejects it.
printf '#!/usr/bin/env python3\nraise SystemExit(0)\n' >"$TMP/weakened-check.py"
chmod +x "$TMP/weakened-check.py"
cp "$TMP/base-rules.json" "$TMP/current-rules.json"
printf 'old\told.rs:old\nold\tbypass.rs:old\n' >"$TMP/current-allowlist"
python3 "$TMP/weakened-check.py" "$TMP/base-allowlist" "$TMP/base-rules.json" \
  "$TMP/current-allowlist" "$TMP/current-rules.json"
expect_failure "rules are unchanged" python3 "$CHECK" \
  "$TMP/base-allowlist" "$TMP/base-rules.json" \
  "$TMP/current-allowlist" "$TMP/current-rules.json"

echo "portability growth tests passed"
