#!/bin/bash
set -euo pipefail

# // trace:BUG-1802 | ai:antigravity

echo "Setting up isolated test environment..."
TEST_DIR=$(mktemp -d)
trap 'rm -rf "$TEST_DIR"' EXIT

cd "$TEST_DIR"
git init main-repo
cd main-repo
git config user.name "Test User"
git config user.email "test@example.com"

# Setup basic aida store
mkdir -p .aida-store/objects .aida/sessions .aida/review-verdicts
touch .aida/config.toml

# We will test using cargo run --bin aida
AIDA="cargo run --manifest-path=/home/joe/ai/aida-worktrees/aida-bug-1802-3/Cargo.toml --bin aida --"

# Create a spec
$AIDA add --type task --title "Test task" --status approved
SPEC=$($AIDA list --status approved | head -n 1 | awk '{print $1}')

# Check that implementer seat cannot review
AIDA_SESSION_ROLE=implementer $AIDA review record $SPEC --verdict approved --sha HEAD --pr 1 2> error.log || true

if ! grep -q "refused: implementer seat cannot record a review verdict" error.log; then
    echo "FAIL: implementer was allowed to review!"
    cat error.log
    exit 1
fi

# Check that operator cannot review without TTY (which is simulated by running from script)
$AIDA review record $SPEC --verdict approved --sha HEAD --pr 1 2> error2.log || true

if ! grep -q "role grants require a human at an interactive TTY" error2.log; then
    echo "FAIL: operator was allowed to review without TTY!"
    cat error2.log
    exit 1
fi

echo "PASS: BUG-1802 requirements met"
