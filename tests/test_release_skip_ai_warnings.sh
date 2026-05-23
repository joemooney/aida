#!/usr/bin/env bash
# tests/test_release_skip_ai_warnings.sh — regression test for TASK-488.
#
# scripts/release.sh produces a mechanical version-bump commit. Before
# TASK-488, the commit-msg hook emitted noisy AI-tag and trace-reference
# warnings on every release commit because incidentally-touched files
# (Cargo.toml, CHANGELOG.md, etc.) carry `trace:` comments. The fix:
# scripts/release.sh exports AIDA_RELEASE=1, and the commit-msg hook
# suppresses validations 3 (AI-tag) and 4 (trace-reference) under that
# env var. OTHER validations (conventional format, feat/fix REQ-ID) and
# the pre-commit substrate-as-bouncer gate still apply.
#
# This test invokes the commit-msg hook directly with a mock commit
# message file and a mock staged-files setup, then asserts the warnings
# are emitted or suppressed correctly.
#
# trace:TASK-488 | ai:claude
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOK="$PROJECT_ROOT/aida-core/templates/hooks/aida-commit-msg"
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

echo ""
echo "=== TASK-488: release commit suppresses AI-tag/trace warnings ==="
echo "Test repo: $TEST_DIR"
echo ""

# Build a throwaway git repo with a staged file that carries a trace
# comment. The hook reads staged files via `git diff --cached --name-only`,
# so we need a real git index — a flat tmpdir without `git init` won't
# exercise the trace-detection path.
cd "$TEST_DIR"
git init -q -b main
git config user.email "test@example.com"
git config user.name "TASK-488 test"

cat > Cargo.toml <<'EOF'
# Mock workspace manifest with an incidental trace comment, the way the
# real one carries traces in CHANGELOG.md / nearby files.
# trace:STORY-137 | ai:claude
[workspace]
EOF
git add Cargo.toml

# ----------------------------------------------------------------------------
# Test 1: non-release commit (AIDA_RELEASE unset) emits both warnings.
# ----------------------------------------------------------------------------
info "Test 1: non-release commit emits AI-tag + trace warnings"
msg_file="$TEST_DIR/.git/COMMIT_EDITMSG"
echo "chore: release v0.9.0" > "$msg_file"

unset AIDA_RELEASE
output_default=$(bash "$HOOK" "$msg_file" 2>&1 || true)

echo "$output_default" | grep -q "consider adding \[AI:tool\] tag" \
    || fail "expected AI-tag warning in non-release output, got: $output_default"
echo "$output_default" | grep -q "Staged files trace to:" \
    || fail "expected trace-reference warning in non-release output, got: $output_default"
pass "non-release commit emits both warnings"

# ----------------------------------------------------------------------------
# Test 2: release commit (AIDA_RELEASE=1) suppresses both warnings.
# ----------------------------------------------------------------------------
info "Test 2: AIDA_RELEASE=1 suppresses both warnings"
output_release=$(AIDA_RELEASE=1 bash "$HOOK" "$msg_file" 2>&1 || true)

if echo "$output_release" | grep -q "consider adding \[AI:tool\] tag"; then
    fail "AI-tag warning NOT suppressed under AIDA_RELEASE=1: $output_release"
fi
if echo "$output_release" | grep -q "Staged files trace to:"; then
    fail "trace-reference warning NOT suppressed under AIDA_RELEASE=1: $output_release"
fi
pass "AIDA_RELEASE=1 suppresses both warnings"

# ----------------------------------------------------------------------------
# Test 3: AIDA_RELEASE=1 still flags conventional-format errors. The
# release-flag is narrow — it must not turn the hook into a no-op.
# ----------------------------------------------------------------------------
info "Test 3: AIDA_RELEASE=1 does not bypass conventional-format check"
bad_msg_file="$TEST_DIR/.git/BAD_MSG"
echo "not a conventional commit" > "$bad_msg_file"

output_bad=$(AIDA_RELEASE=1 bash "$HOOK" "$bad_msg_file" 2>&1 || true)
echo "$output_bad" | grep -q "conventional format" \
    || fail "AIDA_RELEASE=1 unexpectedly suppressed conventional-format error: $output_bad"
pass "conventional-format check still runs under AIDA_RELEASE=1"

# ----------------------------------------------------------------------------
# Test 4: scripts/release.sh actually exports AIDA_RELEASE=1 before its
# commit step. Guards against the export being removed in a refactor.
# ----------------------------------------------------------------------------
info "Test 4: scripts/release.sh exports AIDA_RELEASE=1 before git commit"
release_sh="$PROJECT_ROOT/scripts/release.sh"
# Find the line numbers of the export and the release commit. The export
# must come before the commit (export-after-commit would be a no-op).
export_line=$(grep -nE '^export AIDA_RELEASE=1' "$release_sh" | head -n1 | cut -d: -f1 || true)
commit_line=$(grep -nE '^git commit -m "chore: release v' "$release_sh" | head -n1 | cut -d: -f1 || true)
[ -n "$export_line" ] || fail "scripts/release.sh does not export AIDA_RELEASE=1"
[ -n "$commit_line" ] || fail "could not locate release-commit line in scripts/release.sh"
[ "$export_line" -lt "$commit_line" ] \
    || fail "export AIDA_RELEASE=1 (line $export_line) is not before git commit (line $commit_line)"
pass "scripts/release.sh exports AIDA_RELEASE=1 on line $export_line, before commit on line $commit_line"

echo ""
echo -e "${GREEN}All tests passed.${NC}"
