#!/usr/bin/env bash
# demo-lifecycle-authority.sh — show AIDA's distinctive AUTHORITY gates
#
# The neighbor tools (Spec Kit, Kiro, Gas Town's Refinery, Beads, ...) gate
# WHAT MERGES — a CI/review queue in front of the default branch. AIDA also has
# that, but its rarer, uncontested enforcement point is the AUTHORITY TO CHANGE
# INTENT STATE:
#
#   1. Draft -> Approved is ADVISOR-ONLY. A non-advisor identity (role unset /
#      a non-advisor role, non-interactive) is REFUSED — the gate is enforced in
#      code (status_advance_requires_advisor_authority), not a convention.
#   2. Completion is MERGE-DRIVEN. A spec reaches `completed` because a commit
#      referencing its SPEC-ID landed on the default branch (the `aida pull`
#      Done -> Completed auto-bump). "Done" is a property of git ancestry, not a
#      manual flag someone flips.
#
# Contrast: Beads has 3 lifecycle states set BY CONVENTION (nothing enforces who
# may set them); the family at large gates only merge-time verification, not the
# authority to advance intent.
#
# This demo runs entirely against a THROWAWAY sandbox store
# (`aida sandbox create`) — it never touches your project's real store.
#
# Prerequisites:
#   - `aida` on PATH (run `aida-on` first if using the dev build)
#
# Usage:
#   bash scripts/demo-lifecycle-authority.sh                # walkthrough
#   bash scripts/demo-lifecycle-authority.sh --auto-cleanup # destroy sandbox at end, no prompt
#
# Cleanup is OPT-IN — the script defaults to keeping the throwaway sandbox so you
# can poke around. Re-run with --auto-cleanup to destroy it automatically.
#
# trace:TASK-873

set -uo pipefail

# -----------------------------------------------------------------------------
# Config + flags
# -----------------------------------------------------------------------------

AUTO_CLEANUP=0
DEMO_COMPLETE=0
# A dedicated sandbox path so we never collide with the user's default sandbox
# (`aida sandbox create` with no --path) or the project's real store.
SANDBOX_PATH="${TMPDIR:-/tmp}/aida-authority-demo-$$"

for arg in "$@"; do
    case "$arg" in
        --auto-cleanup) AUTO_CLEANUP=1 ;;
        -h|--help)
            sed -n '2,/^# trace/p' "$0" | sed 's/^# \?//'
            exit 0
            ;;
        *) echo "unknown flag: $arg (see --help)" >&2; exit 1 ;;
    esac
done

# -----------------------------------------------------------------------------
# Colors (degrade gracefully on non-TTY)
# -----------------------------------------------------------------------------
if [ -t 1 ]; then
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    RED='\033[0;31m'
    BLUE='\033[1;34m'
    CYAN='\033[1;36m'
    MAGENTA='\033[1;35m'
    DIM='\033[2m'
    BOLD='\033[1m'
    NC='\033[0m'
else
    GREEN=''; YELLOW=''; RED=''; BLUE=''; CYAN=''; MAGENTA=''; DIM=''; BOLD=''; NC=''
fi

DEMO_STEP=0
DEMO_TOTAL_STEPS=6

do_clear() { if [ -t 1 ]; then clear || true; fi; }

repeat() { local c="$1" n="$2" i=0; while [ "$i" -lt "$n" ]; do printf '%s' "$c"; i=$((i + 1)); done; }

note()  { printf "${YELLOW}i %s${NC}\n" "$*"; }
ok()    { printf "${GREEN}OK %s${NC}\n" "$*"; }
fail()  { printf "${RED}xx %s${NC}\n" "$*" >&2; }
dim()   { printf "${DIM}%s${NC}\n" "$*"; }

# box_title TITLE [SUBTITLE]
box_title() {
    local title="$1" subtitle="${2:-}"
    local min_width=80
    local content_width=$((min_width - 4))
    [ "${#title}" -gt "$content_width" ] && content_width=${#title}
    [ "${#subtitle}" -gt "$content_width" ] && content_width=${#subtitle}
    local total_width=$((content_width + 4))
    printf "${CYAN}+"; repeat "=" "$total_width"; printf "+${NC}\n"
    local tpad=$((content_width - ${#title}))
    printf "${CYAN}|${NC}  ${BOLD}%s%*s${NC}  ${CYAN}|${NC}\n" "$title" "$tpad" ""
    if [ -n "$subtitle" ]; then
        local spad=$((content_width - ${#subtitle}))
        printf "${CYAN}|${NC}  ${DIM}%s%*s${NC}  ${CYAN}|${NC}\n" "$subtitle" "$spad" ""
    fi
    printf "${CYAN}+"; repeat "=" "$total_width"; printf "+${NC}\n"
}

# note_box [--title TEXT] LINE ...
note_box() {
    local title=""
    if [ "$1" = "--title" ]; then title="$2"; shift 2; fi
    local min_width=80
    local width=$min_width
    local line
    for line in "$@"; do
        local need=$((${#line} + 2))
        [ "$need" -gt "$width" ] && width="$need"
    done
    if [ -n "$title" ]; then
        local title_need=$((${#title} + 6))
        [ "$title_need" -gt "$width" ] && width="$title_need"
    fi
    if [ -n "$title" ]; then
        local left=2
        local right=$((width - left - ${#title} - 4))
        [ "$right" -lt 0 ] && right=0
        printf "${YELLOW}+"; repeat "-" "$left"; printf "[ ${BOLD}%s${NC}${YELLOW} ]" "$title"
        repeat "-" "$right"; printf "+${NC}\n"
    else
        printf "${YELLOW}+"; repeat "-" "$width"; printf "+${NC}\n"
    fi
    for line in "$@"; do
        local pad=$((width - ${#line} - 2))
        [ "$pad" -lt 0 ] && pad=0
        printf "${YELLOW}|${NC} %s%*s ${YELLOW}|${NC}\n" "$line" "$pad" ""
    done
    printf "${YELLOW}+"; repeat "-" "$width"; printf "+${NC}\n"
}

step_header() {
    if [ "$DEMO_STEP" -gt 0 ] && [ -t 0 ]; then
        echo
        printf "${DIM}---- Step %d/%d complete - Press Enter to advance - Ctrl+C to abort ----${NC}" \
            "$DEMO_STEP" "$DEMO_TOTAL_STEPS"
        read -r _
    fi
    DEMO_STEP=$((DEMO_STEP + 1))
    do_clear
    local title="$1"
    local stamp="STEP $DEMO_STEP of $DEMO_TOTAL_STEPS"
    local line="$stamp  -  $title"
    local box_width=$(( ${#line} + 4 ))
    printf "${BLUE}+"; repeat "-" "$box_width"; printf "+${NC}\n"
    printf "${BLUE}|${NC}  ${MAGENTA}%s${NC}  ${DIM}-${NC}  ${BOLD}%s${NC}  ${BLUE}|${NC}\n" "$stamp" "$title"
    printf "${BLUE}+"; repeat "-" "$box_width"; printf "+${NC}\n"
    echo
}

step_pause() {
    if [ -t 0 ]; then
        local prompt="${1:-Press Enter to continue}"
        printf "${DIM}---- ${prompt} ----${NC}"
        read -r _
        echo
    fi
}

# show_cmd PROMPT CMD... — echo a shell-prompt-like line then run it.
show_cmd() {
    local prefix="$1"; shift
    printf "${DIM}%s${NC} ${GREEN}\$ %s${NC}\n" "$prefix" "$*"
    "$@"
}

# -----------------------------------------------------------------------------
# Cleanup (normal + abort trap)
# -----------------------------------------------------------------------------
destroy_sandbox() {
    [ -d "$SANDBOX_PATH" ] || [ -f "$SANDBOX_PATH.cache.db" ] || return 0
    AIDA_STORE="$SANDBOX_PATH" aida sandbox destroy --path "$SANDBOX_PATH" >/dev/null 2>&1 \
        || rm -rf "$SANDBOX_PATH" 2>/dev/null
    # `aida sandbox destroy` removes the store dir but leaves the sibling
    # cache projection; sweep it so the throwaway leaves nothing behind.
    rm -f "$SANDBOX_PATH.cache.db" 2>/dev/null
}

cleanup_on_abort() {
    local code=$?
    [ "$DEMO_COMPLETE" = "1" ] && exit "$code"
    trap 'exit 130' INT
    echo
    printf "\n${YELLOW}xx Demo aborted${NC} (exit %d)\n" "$code"
    if [ -d "$SANDBOX_PATH" ]; then
        printf "Throwaway sandbox: %s\n" "$SANDBOX_PATH"
        local resp="y"
        if [ -t 0 ]; then
            printf "Destroy it? [Y/n] (Enter = yes): "
            read -r resp
        fi
        if [ -z "$resp" ] || [ "${resp,,}" = "y" ] || [ "${resp,,}" = "yes" ]; then
            destroy_sandbox
            printf "${GREEN}OK Sandbox destroyed.${NC}\n"
        else
            printf "${YELLOW}Kept sandbox. Manual: aida sandbox destroy --path %s${NC}\n" "$SANDBOX_PATH"
        fi
    fi
    exit "$code"
}
trap cleanup_on_abort INT TERM

# -----------------------------------------------------------------------------
# Intro + pre-flight
# -----------------------------------------------------------------------------
do_clear
echo
box_title "AIDA - authority gates demo" "throwaway sandbox - safe to abort with Ctrl+C"
echo
note_box --title "What this demonstrates" \
  "Most spec/issue tools gate WHAT MERGES (a CI/review queue). AIDA also" \
  "gates the AUTHORITY TO CHANGE INTENT STATE - a rarer enforcement point:" \
  "" \
  "  1. Draft -> Approved is ADVISOR-ONLY (refused for a non-advisor)." \
  "  2. 'completed' is MERGE-DRIVEN - a property of git ancestry, not a" \
  "     manual flag." \
  "" \
  "Contrast: Beads has 3 lifecycle states set BY CONVENTION (unenforced);" \
  "the family gates merge-time verification, not intent-state authority."
echo
note "${DEMO_TOTAL_STEPS} steps. Press Enter to advance; Ctrl+C aborts. Runs in a throwaway store."
echo
if [ -t 0 ]; then
    printf "${DIM}---- Press Enter to begin ----${NC}"
    read -r _
fi

# Pre-flight
step_header "Pre-flight - confirm aida + create a throwaway store"
command -v aida >/dev/null 2>&1 || { fail "aida not on PATH. Run 'aida-on' first if using the dev build."; exit 1; }
ok "aida found: $(command -v aida)"
dim "   version: $(aida --version 2>&1 | head -1)"
echo

note_box --title "Why a throwaway store" \
  "'aida sandbox create' makes a discardable git-canonical store under a" \
  "temp dir. We point aida at it with AIDA_STORE so every command below" \
  "operates on the sandbox - your project's real store is never touched."
echo
show_cmd "demo" aida sandbox create --path "$SANDBOX_PATH" --force >/dev/null
# Activate the sandbox for the rest of the script.
export AIDA_STORE="$SANDBOX_PATH"
ok "Sandbox active: AIDA_STORE=$SANDBOX_PATH"

# -----------------------------------------------------------------------------
# Step: file a Draft spec as a non-advisor
# -----------------------------------------------------------------------------
step_header "File a Draft spec (as a non-advisor identity)"
note_box --title "Filing is open; APPROVING is gated" \
  "Anyone may FILE a spec (capture is cheap and should be frictionless)." \
  "We file as a non-advisor identity: AIDA_SESSION_ROLE unset, non-TTY" \
  "(stdin redirected). The spec lands at status Draft."
echo
show_cmd "demo" env -u AIDA_SESSION_ROLE aida add --type task --status draft \
    --title "Authority-gate demo spec" \
    --description "Filed by a non-advisor to demonstrate the Draft->Approved authority gate." < /dev/null

SPEC=$(aida list --all --format human < /dev/null 2>/dev/null | awk '/Authority-gate demo/ {print $1; exit}')
[ -z "$SPEC" ] && SPEC="TASK-1-001"
echo
ok "Filed as $SPEC (status: Draft)"
echo
show_cmd "demo" aida show "$SPEC" --no-git < /dev/null 2>/dev/null

# -----------------------------------------------------------------------------
# Step: the AUTHORITY GATE — non-advisor approval is REFUSED
# -----------------------------------------------------------------------------
step_header "Authority gate - a NON-ADVISOR cannot approve"
note_box --title "The distinctive gate" \
  "Draft -> Approved promotes a spec INTO the execution pipeline. That is" \
  "the advisor's triage decision. A non-advisor identity attempting it is" \
  "REFUSED in code (status_advance_requires_advisor_authority), not by a" \
  "convention a confident agent can ignore." \
  "" \
  "We run with AIDA_SESSION_ROLE UNSET and stdin redirected (non-TTY), so" \
  "neither the advisor-role path nor the interactive-TTY carve-out applies."
echo
printf "${DIM}demo${NC} ${GREEN}\$ env -u AIDA_SESSION_ROLE aida edit %s --status approved${NC}\n" "$SPEC"
set +e
REFUSAL=$(env -u AIDA_SESSION_ROLE aida edit "$SPEC" --status approved < /dev/null 2>&1)
REFUSAL_EXIT=$?
set -e 2>/dev/null || true
printf "%s\n" "$REFUSAL"
echo
if [ "$REFUSAL_EXIT" -ne 0 ] && printf '%s' "$REFUSAL" | grep -qi "advisor authority"; then
    ok "REFUSED (exit $REFUSAL_EXIT) - the authority gate held."
else
    fail "Expected a refusal but the edit was NOT blocked (exit $REFUSAL_EXIT)."
    fail "The demo's premise (advisor-only Draft->Approved) may have changed - investigate before trusting this script."
fi
echo
STATUS_AFTER=$(aida show "$SPEC" --no-git --format human < /dev/null 2>/dev/null | awk -F'  *' '/^Status/{print $0; exit}')
note "Status unchanged: $STATUS_AFTER"

# -----------------------------------------------------------------------------
# Step: the advisor approves — SUCCESS
# -----------------------------------------------------------------------------
step_header "Authority gate - the ADVISOR approves"
note_box --title "Same command, advisor authority" \
  "Now the EXACT SAME edit, but as the advisor (AIDA_SESSION_ROLE=advisor)." \
  "The advisor holds the authority to advance intent state, so the" \
  "promotion succeeds. WHO ran the command - not WHAT the command is - is" \
  "the difference."
echo
show_cmd "demo" env AIDA_SESSION_ROLE=advisor aida edit "$SPEC" --status approved < /dev/null
echo
APPROVED_STATUS=$(aida show "$SPEC" --no-git --format human < /dev/null 2>/dev/null | awk '/^Status/{print; exit}')
ok "Approved by the advisor. $APPROVED_STATUS"

# -----------------------------------------------------------------------------
# Step: merge-driven completion
# -----------------------------------------------------------------------------
step_header "Merge-driven completion - 'done' is git ancestry, not a flag"
note_box --title "The second authority property" \
  "An implementer marks work finished on a branch with 'aida queue done'" \
  "-> status Done. But Done is NOT Completed." \
  "" \
  "A spec reaches 'completed' only when a commit referencing its SPEC-ID" \
  "lands on the DEFAULT BRANCH. 'aida pull' runs the auto-bump scan over" \
  "the commits it brings in, finds the (SPEC-ID) trailer, and flips" \
  "Done -> Completed. Completion is therefore a PROPERTY OF GIT ANCESTRY -" \
  "no one types 'aida edit --status completed'."
echo
# Queue it (advisor authority) then mark Done as the implementer.
show_cmd "demo" env AIDA_SESSION_ROLE=advisor aida queue add "$SPEC" --for implementer < /dev/null 2>&1 | head -2
echo
show_cmd "demo" aida queue done "$SPEC" -y --force < /dev/null 2>&1 | grep -iE "marked done|done and removed" | head -1
echo
DONE_STATUS=$(aida show "$SPEC" --no-git --format human < /dev/null 2>/dev/null | awk '/^Status/{print; exit}')
ok "Implementer marked it Done. $DONE_STATUS"
echo
note_box --title "Why we stop at Done in this script" \
  "The Done -> Completed bump needs a real merged commit carrying the" \
  "(SPEC-ID) trailer on the default branch. That is a full git round-trip" \
  "(branch -> commit -> push -> merge -> 'aida pull'), heavier than a" \
  "single-store demo should fake. We show the MECHANISM instead - the" \
  "lifecycle's declared trigger and the scan that reads git ancestry." \
  "(scripts/aida-demo.sh runs the full round-trip end-to-end.)"
echo
note "The lifecycle's declared trigger for Done -> Completed:"
dim "    Done --> Completed: merge auto-bump (aida pull)"
echo
note "The scan that reads git ancestry (over the default branch's commits):"
show_cmd "demo" aida db reconcile-status --help < /dev/null 2>&1 | sed -n '1,4p'
echo
note_box --title "Contrast - what the neighbors do" \
  "  Beads        : 3 lifecycle states, set BY CONVENTION. Nothing enforces" \
  "                 WHO may set them or that completion follows a merge." \
  "  Spec Kit /   : gate WHAT MERGES (CI + review in front of main). The" \
  "  Kiro / Gas     authority to advance INTENT state is not a gated concept." \
  "  Town Refinery" \
  "" \
  "  AIDA         : gates the AUTHORITY TO CHANGE INTENT STATE -" \
  "                 advisor-only approval + merge-driven completion - on TOP" \
  "                 of the usual merge-time gates."

# -----------------------------------------------------------------------------
# Step: recap + cleanup
# -----------------------------------------------------------------------------
step_header "Recap + cleanup"
note_box --title "What you just saw" \
  "  1. A non-advisor filing a Draft: allowed (capture is cheap)." \
  "  2. A non-advisor promoting Draft -> Approved: REFUSED in code." \
  "  3. The advisor running the same promotion: succeeded." \
  "  4. An implementer marking Done; Completed reserved for a merge." \
  "" \
  "AIDA gates the authority to change intent state - not just what merges." \
  "That is the genuinely distinctive, uncontested enforcement point."
echo

do_clear 2>/dev/null || true
box_title "Demo complete" "decide what happens to the throwaway sandbox"
echo
note_box --title "Throwaway artifact" \
  "  Sandbox store: $SANDBOX_PATH"
echo

if [ "$AUTO_CLEANUP" = "1" ]; then
    confirm="y"
else
    note_box --title "Cleanup will" \
      "  - Destroy the throwaway sandbox store ($SANDBOX_PATH)" \
      "" \
      "Default is N - keep it so you can poke around" \
      "(aida sandbox path --path $SANDBOX_PATH to inspect it)."
    echo
    if [ -t 0 ]; then
        printf "Destroy the sandbox now? [y/N]: "
        read -r confirm
    else
        confirm="n"
    fi
fi

if [ "${confirm,,}" = "y" ] || [ "${confirm,,}" = "yes" ]; then
    destroy_sandbox
    ok "Sandbox destroyed."
else
    note "Kept the sandbox. Manual cleanup:"
    dim "  aida sandbox destroy --path $SANDBOX_PATH"
fi
DEMO_COMPLETE=1
echo
ok "Demo done."
