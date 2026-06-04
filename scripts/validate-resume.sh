#!/usr/bin/env bash
#
# Controlled validation for `aida queue work --auto-complete --resume-drain`
# (STORY-492). Two parts:
#
#   1. DECISION CHECKS (this script, automatic, no Claude): manufacture
#      crashed-drain `.aida/drain-state.json` scenarios in a throwaway repo and
#      assert the resume DECISION — the PID-liveness double-drive guard, the
#      reconcile-from-reality re-entry, the CI→reviewer safety clamp, the
#      drain-id corroboration, and the no-state case. All via
#      `--resume-dry-run`, so nothing is driven and no Claude session spawns.
#
#   2. LIVE kill-and-resume (printed recipe, your keyboard): the one path the
#      decision checks can't cover — the real re-entry that re-spawns phases.
#      Run it once in a controlled setting before relying on --resume-drain for
#      a genuine crash, so its first live execution isn't a real emergency.
#
# Usage:   AIDA=./target/release/aida bash scripts/validate-resume.sh
#          (AIDA defaults to whatever `aida` is on PATH)
#
# trace:STORY-492 | ai:claude
set -uo pipefail

AIDA="${AIDA:-aida}"
# The harness cd's into a throwaway repo, so a relative AIDA path (e.g.
# ./target/release/aida) must be absolutized first.
case "$AIDA" in
  */*) AIDA="$(cd "$(dirname "$AIDA")" && pwd)/$(basename "$AIDA")" ;;
esac
if ! command -v "$AIDA" >/dev/null 2>&1 && [ ! -x "$AIDA" ]; then
  echo "✗ '$AIDA' not found — set AIDA=path/to/aida" >&2
  exit 1
fi
if ! "$AIDA" queue work --help 2>&1 | grep -q -- '--resume-drain'; then
  echo "✗ this build has no --resume-drain — build from the STORY-492 branch" >&2
  exit 1
fi

REPO="$(mktemp -d)"
HOMEX="$(mktemp -d)"   # isolate ~/.aida so the harness never touches real state
cleanup() { rm -rf "$REPO" "$HOMEX"; }
trap cleanup EXIT

echo "Setting up a throwaway AIDA project…"
(
  cd "$REPO"
  git init -q -b main
  git config user.email t@t.t && git config user.name t
  git commit -q --allow-empty -m init
  HOME="$HOMEX" "$AIDA" init >/dev/null 2>&1
) || { echo "✗ project setup failed"; exit 1; }

FAILED=0
STAMP="2026-01-01T00:00:00Z"

# run_scenario <name> <drain-state-json> <expected-regex> [extra-args...]
run_scenario() {
  local name="$1" json="$2" expect="$3"; shift 3
  printf '%s' "$json" > "$REPO/.aida/drain-state.json"
  local out
  out="$(cd "$REPO" && HOME="$HOMEX" "$AIDA" queue work --auto-complete --resume-drain --resume-dry-run "$@" 2>&1 || true)"
  if echo "$out" | grep -qiE "$expect"; then
    printf '  \033[32mPASS\033[0m  %s\n' "$name"
  else
    printf '  \033[31mFAIL\033[0m  %s — expected /%s/, got:\n' "$name" "$expect"
    echo "$out" | sed 's/^/        /'
    FAILED=1
  fi
}

state() {  # state(orchestrator_pid, member_state, current_phase)
  printf '{"command":"aida queue work --auto-complete","mode":"single","members":[{"spec":"TASK-1","state":"%s","pr":999}],"current":"TASK-1","current_phase":"%s","orchestrator_pid":%s,"started_at":"%s","on_drain_complete":"x","run_uuid":"test-run-uuid"}' \
    "$2" "$3" "$1" "$STAMP"
}

echo
echo "Decision checks (manufactured crashes, dry-run — no Claude):"

# 1. Original orchestrator still alive ($$ = this script) → REFUSE (double-drive guard).
run_scenario "live-PID refuses (double-drive guard)" \
  "$(state "$$" "in-phase-4" "4 (merge)")" \
  "refus|still alive|double-drive"

# 2. Dead orchestrator, crashed in phase 4 with a PR → reconcile to the first
#    unmet phase; CI(2) re-entry is clamped up to the reviewer (phase 3).
run_scenario "dead-PID reconciles + clamps CI→reviewer" \
  "$(state 999999 "in-phase-4" "4 (merge)")" \
  "resuming .*phase 3|reviewer"

# 3. Deliberately shelved member (state=failed) → leave parked, don't resume.
run_scenario "shelved member stays parked" \
  "$(state 999999 "failed" "4 (merge)")" \
  "shelved|parked|findings list"

# 4. --drain-id mismatch → refuse (stale-state guard).
run_scenario "drain-id mismatch refuses" \
  "$(state 999999 "in-phase-4" "4 (merge)")" \
  "does not match|drain-id" \
  --drain-id WRONG-ID

# 5. No drain-state file at all → nothing to resume.
rm -f "$REPO/.aida/drain-state.json"
out5="$(cd "$REPO" && HOME="$HOMEX" "$AIDA" queue work --auto-complete --resume-drain --resume-dry-run 2>&1 || true)"
if echo "$out5" | grep -qiE "no .*drain-state|nothing to resume|no crashed drain"; then
  printf '  \033[32mPASS\033[0m  %s\n' "no drain-state → nothing to resume"
else
  printf '  \033[31mFAIL\033[0m  %s — got:\n' "no drain-state"; echo "$out5" | sed 's/^/        /'; FAILED=1
fi

echo
if [ "$FAILED" -eq 0 ]; then
  printf '\033[32mAll decision checks passed.\033[0m\n'
else
  printf '\033[31mSome decision checks FAILED (see above).\033[0m\n'
fi

cat <<'RECIPE'

────────────────────────────────────────────────────────────────────────
LIVE kill-and-resume (your keyboard — spawns real Claude phases)

The decision checks above cover every path EXCEPT the actual live re-entry
(re-spawning phases). Run this once, in a controlled setting, before trusting
--resume-drain on a real crash:

  # terminal A — start a drain on a small spec and let it begin a phase
  aida queue work <SPEC> --auto-complete --zen

  # terminal B — find the orchestrator pid and kill it mid-phase
  # (drain-state.json is pretty-printed — "orchestrator_pid": N has a space, so
  #  grep the line and pull the digits rather than matching ":N" directly)
  PID=$(grep orchestrator_pid .aida/drain-state.json | grep -oE '[0-9]+'); echo "$PID"
  kill -9 "$PID"

  # preview the reconcile (safe), then do the live re-entry
  aida queue work --auto-complete --resume-drain --resume-dry-run
  aida queue work --auto-complete --resume-drain

Expect: it refuses while the pid is alive; after the kill it re-enters at the
reconciled phase, seeds the existing branch + PR, and never re-opens a PR or
re-merges. If it crashed before/at CI, it re-enters at the reviewer (the clamp).
────────────────────────────────────────────────────────────────────────
RECIPE

exit "$FAILED"
