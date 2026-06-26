#!/bin/bash
# Proof that the scaffolded pre-commit hook's section-5 gate blocks a bare
# SPEC-ID (or `trace:`) on a `///` doc comment in a staged cli.rs, while letting
# a clean line — and an `e.g. TASK-N` usage example — through. Mirrors the cli.rs
# CI tests source_doc_comments_carry_no_trace_token +
# source_doc_comments_carry_no_spec_id_provenance, but runs in milliseconds with
# grep only (no cargo compile). The CI tests are the ~7-min-later net; this gate
# is the moment-of-writing fast path that kills the recurring red cycle.
#
# Drives the REAL template hook (aida-core/templates/hooks/aida-pre-commit.sh)
# against a throwaway git repo with files actually staged in the index — exactly
# what `git commit` invokes — so what we exercise is what ships.
#
# trace:TASK-903
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOK="$PROJECT_ROOT/aida-core/templates/hooks/aida-pre-commit.sh"

if [ ! -f "$HOOK" ]; then
    echo "FAIL: hook template not found at $HOOK" >&2
    exit 1
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
fail=0

cd "$TMP"
git init -q -b main
git config user.email t@t.t
git config user.name t
# Don't let any installed aida binary's advisor-code-gate interfere: this repo
# has no store, and the gate falls open without one, but keep the env clean.
unset AIDA_SESSION_ROLE 2>/dev/null || true

# run_hook <expected_exit> <label> — stage src/cli.rs as currently on disk,
# run the hook, assert its exit code. Uses a fresh index each call.
run_hook() {
    local expected="$1" label="$2"
    git rm --cached -r --quiet . >/dev/null 2>&1 || true
    git add src/cli.rs
    set +e
    bash "$HOOK" >/dev/null 2>&1
    local rc=$?
    set -e
    if [ "$rc" -eq "$expected" ]; then
        echo "PASS: $label (exit $rc)"
    else
        echo "FAIL: $label — expected exit $expected, got $rc" >&2
        fail=1
    fi
}

mkdir -p src

# Case 1: bare SPEC-ID on a `///` doc comment in cli.rs → BLOCKED (exit 1).
cat >src/cli.rs <<'EOF'
/// Drain the queue (STORY-249 introduced this flag).
pub struct Args {}
EOF
run_hook 1 "bare SPEC-ID on /// in cli.rs is blocked"

# Case 2: `trace:` on a `///` doc comment → BLOCKED (exit 1).
cat >src/cli.rs <<'EOF'
/// Drain the queue. trace:STORY-249
pub struct Args {}
EOF
run_hook 1 "trace: marker on /// in cli.rs is blocked"

# Case 3: clean `///` doc comment, no SPEC-ID, no trace → ALLOWED (exit 0).
cat >src/cli.rs <<'EOF'
/// Drain the queue until it is empty.
pub struct Args {}
EOF
run_hook 0 "clean /// doc comment passes"

# Case 4: `e.g. TASK-N` usage example on `///` → ALLOWED (exit 0, exempt).
cat >src/cli.rs <<'EOF'
/// Spec id to operate on, e.g. TASK-249.
pub struct Args {}
EOF
run_hook 0 "e.g. usage example on /// passes (exempt)"

# Case 5: SPEC-ID on a plain `//` comment (not `///`) → ALLOWED (exit 0).
cat >src/cli.rs <<'EOF'
// trace:STORY-249 | ai:claude
/// Drain the queue.
pub struct Args {}
EOF
run_hook 0 "SPEC-ID on plain // comment passes"

if [ "$fail" -ne 0 ]; then
    echo "RESULT: FAIL" >&2
    exit 1
fi
echo "RESULT: PASS"
