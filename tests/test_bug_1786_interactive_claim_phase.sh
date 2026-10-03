#!/bin/bash
# trace:BUG-1786 | ai:antigravity

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
source "$SCRIPT_DIR/lib/isolated_home.sh"

TARGET_DIR="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
AIDA="$TARGET_DIR/debug/aida"
if [ ! -x "$AIDA" ]; then
    echo "Requires a built debug binary"
    exit 1
fi

TEST_DIR=$(mktemp -d)
aida_test_cleanup() { rm -rf "$TEST_DIR"; }
aida_isolate_home

cd "$TEST_DIR"
git init -b main >/dev/null
git config user.email "test@example.com"
git config user.name "Test"

# Setup initial requirement
mkdir -p .aida
"$AIDA" init
"$AIDA" add --title "test spec" --type bug --status approved
SPEC=$("$AIDA" list --format json | jq -r '.[0].id')
git add .
git commit -m "init" >/dev/null

# Mock session role
export AIDA_SESSION_ROLE=advisor
export AIDA_SESSION_ID=test1234

# Run session start to claim interactive
"$AIDA" session start --owns "$SPEC" --role advisor --base main --no-context >/dev/null 2>&1 || true

# Assert the PhaseEntered event has slug: implementer
EVENTS=$(cat .aida/events.jsonl)
if ! echo "$EVENTS" | grep -q '"slug":"implementer"'; then
  echo -e "\033[0;31mFAIL\033[0m: expected slug: implementer"
  echo "$EVENTS"
  exit 1
fi

echo -e "\033[0;32mPASS\033[0m"
