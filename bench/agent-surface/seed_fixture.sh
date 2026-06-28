#!/usr/bin/env bash
# Seed a throwaway AIDA project as the benchmark fixture.
#
# The harness (run_bench.py) runs `claude -p` inside this fixture for every
# condition x task x run. The fixture must therefore be a real, initialized
# AIDA project that holds known data so each task has a checkable answer:
#   - a queued item            -> "next queue item" task
#   - a blocked-by chain        -> "show spec + blocked-by graph" task
#   - a spread of statuses      -> "project status snapshot" task
#   - an open advisor finding   -> "find a finding" task
#   - (file-spec writes a new one at run time)
#
# Idempotent-ish: refuses to clobber an already-initialized fixture unless
# --force is passed. Pass the target dir as $1 (default: ./fixture-project).
set -euo pipefail

FIXTURE_DIR="${1:-$(cd "$(dirname "$0")" && pwd)/fixture-project}"
FORCE="${2:-}"

if [ -f "$FIXTURE_DIR/.aida/config.toml" ] && [ "$FORCE" != "--force" ]; then
  echo "Fixture already initialized at $FIXTURE_DIR (pass --force to re-seed)."
  exit 0
fi

rm -rf "$FIXTURE_DIR"
mkdir -p "$FIXTURE_DIR"
cd "$FIXTURE_DIR"

git init -q
git config user.email "bench@aida.local"
git config user.name "aida-bench"
printf '# AIDA benchmark fixture\n\nThrowaway project seeded by seed_fixture.sh.\n' > README.md
git add -A
git commit -qm "init fixture"

# Initialize AIDA (distributed git-canonical default), skipping the interactive
# and machine-global bits so this runs unattended.
aida init --no-skills --no-hooks --no-agent-config >/dev/null 2>&1

# Seed specs. The advisor role is required to file/queue approved specs from a
# non-TTY shell (the post-TASK-647 gate), so prefix every write with it.
export AIDA_SESSION_ROLE=advisor

aida add --title "Set up auth backend"   --type task  --status approved >/dev/null 2>&1   # -> TASK-1
aida add --title "Add password reset flow" --type story --status approved --blocked-by TASK-1 >/dev/null 2>&1  # -> STORY-2
aida add --title "Wire the login form"    --type task  --status approved >/dev/null 2>&1   # -> TASK-3
aida add --title "Render the dashboard"   --type task  --status draft >/dev/null 2>&1      # -> TASK-4

# Queue TASK-3 for the implementer (the benchmarked agent's default role).
aida queue add TASK-3 --for implementer --note "ready to implement" >/dev/null 2>&1

# File an advisor finding linked to TASK-3.
aida findings add \
  --note "Login form submits with empty email; needs validation" \
  --title "Empty-email login bug" \
  --severity major \
  --linked-specs TASK-3 >/dev/null 2>&1

echo "Seeded fixture at $FIXTURE_DIR"
echo "  specs: TASK-1, STORY-2 (blocked-by TASK-1), TASK-3 (queued), TASK-4 (draft)"
echo "  finding: 'Empty-email login bug' linked to TASK-3"
