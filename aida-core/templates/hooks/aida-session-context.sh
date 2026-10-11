#!/bin/bash
# AIDA Session Context Hook
# Hook type: SessionStart (runs once when a Claude Code session begins).
#
# Emits a COMPACT, token-minimal ambient-context digest so every session opens
# already knowing the active role, how deep the queue is, how much is in flight,
# and the top-N queued items — the AXI "ambient context" principle.
#
# CHANNEL: writes to STDOUT. Claude Code injects a SessionStart hook's stdout
# into the session context; stderr is NOT injected. (The earlier version wrote
# to stderr and was never wired, so it was a dead asset — TASK-971.)
#
# BUDGET: this loads into EVERY session, so the digest is deliberately terse —
# one header line (role / queue depth / in-flight count) plus up to N queued
# spec ids with trimmed titles. No verbose dashboard.
#
# Best-effort: a missing/slow `aida`, no store, or an empty queue all degrade to
# a clean no-op (empty stdout, exit 0) so a session never fails on this hook.
#
# trace:TASK-971 | ai:claude

set -u

# Number of queued items to surface and the title trim width.
TOP_N=5
TITLE_WIDTH=56

command -v aida >/dev/null 2>&1 || exit 0

# Run against the right store regardless of the hook's cwd, mirroring the other
# SessionStart hooks (role-context / mail-notice).
project_root="${AIDA_SESSION_PROJECT:-${CLAUDE_PROJECT_DIR:-${CODEX_PROJECT_DIR:-$(git rev-parse --show-toplevel 2>/dev/null || printf %s "$PWD")}}}"
cd "$project_root" 2>/dev/null || exit 0

# The role shown is the seat this session actually holds. `aida role current`
# prints the seat of the validated grant (empty when there is none), while
# AIDA_SESSION_ROLE is only a display/routing label: a label naming a seat with
# no valid grant (missing, expired, revoked, or minted in another shell) is
# shown as "<role> (no grant)" so the session can tell it lacks that authority.
# The implementer baseline and the guest/requester personas need no grant.
# trace:BUG-1924 | ai:claude
label="${AIDA_SESSION_ROLE:-}"
[ "$label" = "dialog" ] && label=advisor
seat=$(aida role current 2>/dev/null | head -n 1)
ungranted=""
if [ -n "$seat" ]; then
    role="$seat"
elif [ -z "$label" ] || [ "$label" = "none" ]; then
    role=none
else
    case "$label" in
        implementer | guest | requester) role="$label" ;;
        *)
            role="$label (no grant)"
            ungranted="$label"
            ;;
    esac
fi

# In-flight count: dependency-free line count of in-progress spec ids.
in_flight=$(aida list --status in-progress --format ids 2>/dev/null | grep -c .)
case "$in_flight" in '' | *[!0-9]*) in_flight=0 ;; esac

# Queue depth + top-N. `aida queue list --json` is the cache-fast machine read;
# it can include terminal (Completed/Rejected) entries, so we filter those out.
# Prefer jq, fall back to python3, and degrade to a header-only digest if
# neither is available.
queue_json=$(aida queue list --json 2>/dev/null)
queue_depth=0
queue_lines=""
if [ -n "$queue_json" ]; then
    if command -v jq >/dev/null 2>&1; then
        queue_depth=$(printf '%s' "$queue_json" | jq -r '
            [.[] | select(.status != "Completed" and .status != "Rejected")] | length' 2>/dev/null)
        queue_lines=$(printf '%s' "$queue_json" | jq -r --argjson n "$TOP_N" --argjson w "$TITLE_WIDTH" '
            [.[] | select(.status != "Completed" and .status != "Rejected")][:$n][]
            | "  " + .spec_id + "  " + ((.title // "") | if length > $w then .[:$w] + "..." else . end)' 2>/dev/null)
    elif command -v python3 >/dev/null 2>&1; then
        queue_depth=$(printf '%s' "$queue_json" | python3 -c '
import json, sys
try:
    rows = [r for r in json.load(sys.stdin)
            if r.get("status") not in ("Completed", "Rejected")]
except Exception:
    print(0); sys.exit(0)
print(len(rows))' 2>/dev/null)
        queue_lines=$(printf '%s' "$queue_json" | python3 -c '
import json, sys
try:
    rows = [r for r in json.load(sys.stdin)
            if r.get("status") not in ("Completed", "Rejected")]
except Exception:
    sys.exit(0)
n, w = '"$TOP_N"', '"$TITLE_WIDTH"'
for r in rows[:n]:
    t = (r.get("title") or "")
    if len(t) > w:
        t = t[:w] + "..."
    print("  " + str(r.get("spec_id", "")) + "  " + t)' 2>/dev/null)
    fi
fi
case "$queue_depth" in '' | *[!0-9]*) queue_depth=0 ;; esac

# Emit the compact digest to STDOUT.
printf 'AIDA · role: %s · queue: %s · in-flight: %s\n' "$role" "$queue_depth" "$in_flight"
if [ -n "$ungranted" ]; then
    printf 'role %s is a label only: this shell holds no valid grant for it, so commands gated on that role will refuse. A human enters it at a terminal with: eval "$(aida role enter %s)"\n' "$ungranted" "$ungranted"
fi
if [ -n "$queue_lines" ]; then
    printf '%s\n' "$queue_lines"
fi

# Primary-checkout drift: one extra line when the PRIMARY checkout (the main
# worktree, located via --git-common-dir so a session in a linked worktree
# still sees it) has uncommitted tracked edits, untracked files, an off-main
# HEAD, or has diverged from its upstream. Silent when clean. Cost: one
# rev-parse plus one `git status` that takes no index lock. Read-only.
# Opt out with AIDA_MAIN_CHECKOUT_DRIFT=off. The wording mirrors
# `summary_line` in aida-cli-lib/src/main_checkout_drift.rs.
# trace:STORY-1648 | ai:claude
main_checkout_drift_line() {
    [ "${AIDA_MAIN_CHECKOUT_DRIFT:-on}" = "off" ] && return 0
    common_dir=$(git rev-parse --git-common-dir 2>/dev/null) || return 0
    # Only a normal `<checkout>/.git` layout has a primary checkout to report
    # (a bare repo or a submodule's modules/ dir does not).
    case "$common_dir" in
        .git | */.git) ;;
        *) return 0 ;;
    esac
    primary=$(cd "$common_dir/.." 2>/dev/null && pwd -P) || return 0
    git --no-optional-locks -C "$primary" status --porcelain=v2 --branch 2>/dev/null |
        awk '
            /^# branch\.head / { head = substr($0, 15) }
            /^# branch\.upstream / { up = substr($0, 19) }
            /^# branch\.ab / { a = substr($3, 2) + 0; b = substr($4, 2) + 0 }
            /^[12u] / { t++ }
            /^\? / { u++ }
            END {
                if (head == "") exit
                t += 0; u += 0; a += 0; b += 0
                ondef = (head == "main" || head == "master")
                if (t == 0 && u == 0 && ondef && a == 0 && b == 0) exit
                label = (head == "(detached)") ? "detached HEAD" : head
                if (up == "") {
                    sync = "no upstream"
                } else {
                    remote = up; sub(/\/.*/, "", remote)
                    sync = ""
                    if (a > 0) sync = a " ahead"
                    if (b > 0) sync = (sync == "" ? "" : sync ", ") b " behind"
                    sync = (sync == "") ? "up to date with " remote : sync " " remote
                }
                printf "main checkout: %d tracked change%s, %d untracked · on %s · %s\n", \
                    t, (t == 1 ? "" : "s"), u, label, sync
            }'
    return 0
}
main_checkout_drift_line

exit 0
