#!/usr/bin/env bash
# drain-loop.sh — v1 resilient drain loop (EPIC-30 v1).
#
# A constant-vigilance backstop for the autonomous drain: it never halts on a
# single drain failure, parks failures to NeedsAttention, clears stuck state
# between iterations, and idles when the queue is dry. Run it under the systemd
# --user unit (scripts/aida-drain-loop.service) so it survives crashes + reboots.
#
# This is the SIMPLE v1: no advisor pre-flight stage, no progress watchdog — see
# EPIC-30 for the v2 triage-gated design. v1 stops the *gaps* (drain halting and
# waiting for a human); it does NOT raise the per-spec success rate — that's the
# phase-1/phase-3 reliability work (EPIC-33). Expect it to ship the ~60% that
# works and park the rest to NeedsAttention for triage.
#
# Knobs (env vars):
#   AIDA_SESSION_ROLE   role whose queue to drain        (default: implementer)
#   AIDA_DRAIN_CHUNK    items per drain pass (nextN)      (default: 10)
#   AIDA_DRAIN_MAXFAIL  shelve-and-continue cap; high = never halt on shelving
#                                                         (default: 1000)
#   AIDA_DRAIN_IDLE     sleep seconds when the queue is dry (default: 300)
#   AIDA_DRAIN_NOHUMAN  headless mode flag                (default: --no-human=both)
#
# Hard budget caps (TASK-966) — orthogonal stop conditions so an unattended loop
# can't silently burn a weekly quota on a wedged chunk. Each, when set, is passed
# straight through to `aida queue work` as the matching `--max-*` flag and bounds
# ONE drain pass; when a cap fires, `aida` exits 7 and this loop stops cleanly.
#   AIDA_DRAIN_MAXTOKENS  cumulative reported-token cap   -> --max-tokens     (unset)
#   AIDA_DRAIN_MAXITER    specs-acted-on cap per pass     -> --max-iterations (unset)
#   AIDA_DRAIN_MAXRUNTIME wall-clock cap (min, or 90s/2h) -> --max-runtime    (unset)
#
# WARNING: do not run two drain loops against the same role's queue — they will
# contend on the queue head. One loop per role.

set -uo pipefail

ROLE="${AIDA_SESSION_ROLE:-implementer}"
export AIDA_SESSION_ROLE="$ROLE"        # nextN drains the ACTIVE role's queue
CHUNK="${AIDA_DRAIN_CHUNK:-10}"
MAXF="${AIDA_DRAIN_MAXFAIL:-1000}"
IDLE="${AIDA_DRAIN_IDLE:-300}"
NOHUMAN="${AIDA_DRAIN_NOHUMAN:---no-human=both}"

# TASK-966: translate the budget-cap env knobs into `aida queue work` flags. An
# unset knob adds nothing, so the default (no cap) behaviour is unchanged.
CAPS=()
[ -n "${AIDA_DRAIN_MAXTOKENS:-}" ]  && CAPS+=(--max-tokens "$AIDA_DRAIN_MAXTOKENS")
[ -n "${AIDA_DRAIN_MAXITER:-}" ]    && CAPS+=(--max-iterations "$AIDA_DRAIN_MAXITER")
[ -n "${AIDA_DRAIN_MAXRUNTIME:-}" ] && CAPS+=(--max-runtime "$AIDA_DRAIN_MAXRUNTIME")
DRAIN_CAP_EXIT=7                         # aida's exit code when a budget cap fires

log() { printf '%s drain-loop[%s]: %s\n' "$(date -u +%H:%M:%S)" "$ROLE" "$*"; }
trap 'log "stop (signal)"; exit 0' INT TERM

command -v aida >/dev/null || { echo "FATAL: aida not on PATH"; exit 1; }
log "start (chunk=$CHUNK max-failures=$MAXF idle=${IDLE}s mode=$NOHUMAN caps=${CAPS[*]:-none})"

while true; do
  # 1. stay current with origin (code leg only; background-safe, never blocks)
  aida fetch --code-only --quiet >/dev/null 2>&1 || true

  # 2. clear debris a failed drain leaves behind — dead-process leases that
  #    would block the next pickup (the lease-ownership trap). Best-effort.
  aida doctor heal stale-leases --yes >/dev/null 2>&1 || true

  # 3. anything pickable for this role? if not, idle (don't spin).
  if ! aida queue list 2>/dev/null | grep -qE '^[A-Z]+-[0-9]+'; then
    log "queue dry — idling ${IDLE}s"
    sleep "$IDLE"
    continue
  fi

  # 4. drain a chunk. --max-failures high → shelve-and-continue (park failures
  #    to NeedsAttention, advance the head) instead of halting. Any budget caps
  #    (TASK-966) ride along. Capture the exit code rather than swallowing it so
  #    a budget-cap stop (exit 7) can break the loop cleanly.
  log "draining next${CHUNK}"
  rc=0
  aida queue work "next${CHUNK}" --auto-complete $NOHUMAN --max-failures "$MAXF" "${CAPS[@]}" || rc=$?

  # TASK-966: a hard budget cap fired — the drain stopped itself cleanly and
  # reported why. Don't start another pass; stop the loop so the quota is honored.
  if [ "$rc" -eq "$DRAIN_CAP_EXIT" ]; then
    log "budget cap reached — stopping the loop"
    exit 0
  fi

  # breather so a tight all-failing chunk doesn't spin hot
  sleep 5
done
