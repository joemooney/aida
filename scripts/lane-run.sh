#!/usr/bin/env bash
# trace:BUG-2648 | ai:codex
# Run from the primary checkout. AIDA owns admission, child exit and publication.
# Never follow this command with an unconditional ship --commit-pending.
set -euo pipefail
# trace:TASK-3812 | ai:claude
# Refuse to start a lane on a binary too far behind origin/main: a stale host
# runs none of the recent lane fixes or admission rules. `aida dev check-fresh`
# exits 10 only when stale; any other failure (3 = cannot judge, 2 = a binary
# that predates the command, 1 = a generic error) warns and continues. AIDA_LANE_ALLOW_STALE=1
# overrides; AIDA_LANE_FRESH_ARGS passes limits such as "--max-hours 12".
if [ "${AIDA_LANE_ALLOW_STALE:-0}" != 1 ]; then
  rc=0
  # shellcheck disable=SC2086
  aida dev check-fresh ${AIDA_LANE_FRESH_ARGS:-} || rc=$?
  if [ "$rc" = 10 ]; then
    echo "lane-run: REFUSED: the aida binary is stale (above). Rebuild it, or set AIDA_LANE_ALLOW_STALE=1 to launch anyway." >&2
    exit 1
  elif [ "$rc" != 0 ]; then
    echo "lane-run: warning: binary freshness check unavailable (exit $rc); continuing" >&2
  fi
fi
exec aida queue work "$@"
