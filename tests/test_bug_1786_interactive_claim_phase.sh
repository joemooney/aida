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
printf '\n\n\n\n\n' | "$AIDA" init --force
ADD_OUT=$("$AIDA" add --title "test spec" --type bug --status approved 2>&1)
SPEC=$(echo "$ADD_OUT" | grep -oE 'BUG-[0-9]+' | head -1)
git add .
git commit -m "init" >/dev/null

# Mock session role
export AIDA_SESSION_ROLE=advisor
export AIDA_SESSION_ID=test1234

# Run session start to claim interactive
"$AIDA" session start --owns "$SPEC" --role advisor --base main

# Assert the PhaseEntered event has slug: implementer
EVENTS=$(cat .aida/events.jsonl)
if ! echo "$EVENTS" | grep -q '"slug":"implementer"'; then
  echo -e "\033[0;31mFAIL\033[0m: expected slug: implementer"
  echo "$EVENTS"
  exit 1
fi

echo -e "\033[0;32mPASS\033[0m"
