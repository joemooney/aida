#!/usr/bin/env bash
# trace:TASK-1538 | ai:claude
# Per-PR verification gate for the STORY-1488 incremental split.
#   usage: verify_pr.sh <base-ref> [module-name]
# With a module name: V2 multiset + V7 size cap for that module's PR.
# Without: visibility PR (commit A) — V1 is run separately via check_v1_tokens.sh.
# Always: fmt check, build, workspace tests w/ count, golden outputs vs base.
set -uo pipefail
BASE="$1"; MODULE="${2:-}"
cd "$(git rev-parse --show-toplevel)"
LIB=aida-cli-lib/src/lib.rs
FAIL=0
note() { printf '%s\n' "$*"; }
gate() { # gate <label> <cmd...>
  local label="$1"; shift
  if "$@"; then note "PASS: $label"; else note "FAIL: $label"; FAIL=1; fi
}

T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
git show "$BASE:$LIB" > "$T/lib.old.rs"

if [[ -n "$MODULE" ]]; then
  # V2 pure move
  gate "V2 multiset ($MODULE)" python3 scripts/split-lib/check_v2_multiset.py \
      "$T/lib.old.rs" "$LIB" "aida-cli-lib/src/$MODULE.rs" "$MODULE"
  # V7 size caps: new file <= 6000 lines
  n=$(wc -l < "aida-cli-lib/src/$MODULE.rs")
  if (( n <= 6000 )); then note "PASS: V7 size cap ($MODULE.rs = $n lines)"; else note "FAIL: V7 size cap ($MODULE.rs = $n lines > 6000)"; FAIL=1; fi
  note "--- color-moved diff summary (review manually for stray non-move hunks):"
  git diff --color-moved=zebra --color-moved-ws=allow-indentation-change -M --stat "$BASE" -- aida-cli-lib/src/ | tail -5
fi

# V3 fmt (the repo must already be fmt-clean; a pure move keeps column 0)
gate "V3 cargo fmt --check" cargo fmt --all -- --check
gate "V3 rustfmt --check lib_part*.rs" rustfmt --check aida-cli-lib/src/lib_part*.rs

# Build + V4 tests with count parity
gate "build aida-cli" cargo build -p aida-cli
TESTLOG="$T/tests.log"
if cargo test --workspace 2>&1 | tee "$TESTLOG" | tail -2; then
  COUNT=$(grep -Eo '([0-9]+) passed' "$TESTLOG" | awk '{s+=$1} END {print s}')
  note "PASS: V4 workspace tests green (total passed: $COUNT)"
  echo "$COUNT" > "$T/count"
else
  note "FAIL: V4 workspace tests"; FAIL=1
fi

# V5 golden outputs vs base binary outputs captured earlier (if present)
GOLD=scripts/split-lib/golden
if [[ -d "$GOLD" ]]; then
  BIN=target/debug/aida
  for g in help help-all list-toon; do
    case $g in
      help) "$BIN" --help > "$T/$g.out" 2>&1;;
      help-all) "$BIN" help-all > "$T/$g.out" 2>&1;;
      list-toon) "$BIN" list --format toon > "$T/$g.out" 2>&1;;
    esac
    gate "V5 golden $g" diff -q "$GOLD/$g.out" "$T/$g.out"
  done
else
  note "SKIP: V5 golden outputs — capture them on the BASE commit first (finding 5f):"
  note "  git stash && cargo build -p aida-cli && mkdir -p $GOLD && \\"
  note "  target/debug/aida --help > $GOLD/help.out && target/debug/aida help-all > $GOLD/help-all.out && \\"
  note "  target/debug/aida list --format toon > $GOLD/list-toon.out && git stash pop && cargo build -p aida-cli"
  note "  ($GOLD is per-PR scratch — do not commit it; outputs embed store state)"
fi

if (( FAIL == 0 )); then note "=== ALL GATES PASS ==="; else note "=== GATE FAILURES — do not merge ==="; exit 1; fi
