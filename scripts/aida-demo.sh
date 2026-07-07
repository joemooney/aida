#!/usr/bin/env bash
# aida-demo.sh — throwaway hello-world walkthrough for first-user testing of AIDA
#
# Creates a temporary GitHub repo, runs `aida init`, walks through the
# first-user experience step-by-step (pause-on-Enter between sections),
# then prompts for cleanup at the end.
#
# Prerequisites:
#   - `aida` on PATH (run `aida dev activate` first if using dev build)
#   - `gh` CLI authenticated (`gh auth status` to verify)
#   - `git` configured with user.name + user.email
#
# Usage:
#   bash scripts/aida-demo.sh
#
# Design note:
#   This script uses the manual commit-trailer + auto-bump path for the
#   main flow (steps 1-12) to keep the demo scripted-friendly (interactive
#   Claude Code sessions would halt a scripted demo). Option [1] of the
#   explore menu shows the equivalent flow using `claude -p` directly,
#   which is what `aida queue work --no-human=both` invokes for phase 1.
#   BUG-386 (full 38-skill scaffolding) shipped in this demo's authoring
#   session, so /aida-pickup and /aida-pr ARE available in fresh projects
#   for the interactive path.
#
# Cleanup is OPT-IN — script defaults to 'no, keep the demo state' so you
# can poke around afterwards. Re-run with --auto-cleanup to skip the prompt.
#
# trace:TASK-563 | ai:claude

set -uo pipefail

# -----------------------------------------------------------------------------
# Config + helpers
# -----------------------------------------------------------------------------

TIMESTAMP=$(date +%Y%m%d-%H%M%S)
DEMO_REPO_NAME="aida-demo-$TIMESTAMP"
DEMO_LOCAL_DIR="$HOME/ai/$DEMO_REPO_NAME"
AUTO_CLEANUP=0
# DEMO_COMPLETE flips to 1 after the normal cleanup-or-keep section runs,
# so the abort trap knows not to re-fire when the script exits normally.
DEMO_COMPLETE=0

# Resolve script's absolute path BEFORE any later 'cd' into the demo
# project, so fallback paths (e.g. the glossary template under
# aida-core/templates/) stay reachable after the cwd changes. Old
# code used $(cd "$(dirname BASH_SOURCE)" && pwd) at call time, which
# failed when called from inside the demo dir with no scripts/ child.
case "${BASH_SOURCE[0]:-$0}" in
    /*) AIDA_DEMO_SCRIPT_PATH="${BASH_SOURCE[0]:-$0}" ;;
    *)  AIDA_DEMO_SCRIPT_PATH="$PWD/${BASH_SOURCE[0]:-$0}" ;;
esac
AIDA_DEMO_SCRIPT_DIR="$(cd "$(dirname "$AIDA_DEMO_SCRIPT_PATH")" 2>/dev/null && pwd || echo "")"

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

# Abort trap — fires on Ctrl-C (INT) or kill (TERM). Inventories any
# partial demo state and offers to clean it up before exiting, so an
# aborted run doesn't leave a stray GitHub repo + sibling dir lying
# around. A second Ctrl-C during the prompt exits immediately without
# acting. Skipped after DEMO_COMPLETE=1 (normal cleanup already ran).
cleanup_on_abort() {
    local exit_code=$?
    # Already past normal cleanup? Trap should not re-act.
    [ "$DEMO_COMPLETE" = "1" ] && exit "$exit_code"

    # Re-trap INT so a second Ctrl-C during the prompt exits fast
    # rather than re-entering this handler.
    trap 'exit 130' INT

    echo
    echo
    printf "\033[0;33m✗ Demo aborted\033[0m (exit %d)\n" "$exit_code"

    local dir_exists=0 repo_exists=0
    [ -n "${DEMO_LOCAL_DIR:-}" ] && [ -d "$DEMO_LOCAL_DIR" ] && dir_exists=1
    if [ -n "${GH_USER:-}" ] && [ -n "${DEMO_REPO_NAME:-}" ]; then
        gh repo view "$GH_USER/$DEMO_REPO_NAME" >/dev/null 2>&1 && repo_exists=1
    fi

    if [ "$dir_exists" = "0" ] && [ "$repo_exists" = "0" ]; then
        printf "\033[2m(no demo state created yet — nothing to clean up)\033[0m\n"
        exit "$exit_code"
    fi

    printf "\nPartial demo state:\n"
    [ "$dir_exists" = "1" ] && printf "  local dir   : %s\n" "$DEMO_LOCAL_DIR"
    [ "$repo_exists" = "1" ] && printf "  GitHub repo : https://github.com/%s/%s\n" "$GH_USER" "$DEMO_REPO_NAME"
    echo

    local resp="y"
    if [ -t 0 ]; then
        printf "Clean it up? [Y/n] (Enter = yes): "
        read -r resp
    fi

    if [ -z "$resp" ] || [ "${resp,,}" = "y" ] || [ "${resp,,}" = "yes" ]; then
        # cd out of the dir we're about to delete so rm doesn't fail.
        cd "$HOME" 2>/dev/null
        [ "$dir_exists" = "1" ] && rm -rf "$DEMO_LOCAL_DIR" 2>/dev/null
        [ "$repo_exists" = "1" ] && gh repo delete "$GH_USER/$DEMO_REPO_NAME" --yes >/dev/null 2>&1
        printf "\033[0;32m✓ Cleaned up.\033[0m\n"
    else
        printf "\033[0;33mKept partial state. Manual cleanup:\033[0m\n"
        [ "$dir_exists" = "1" ] && printf "  rm -rf %s\n" "$DEMO_LOCAL_DIR"
        [ "$repo_exists" = "1" ] && printf "  gh repo delete %s/%s --yes\n" "$GH_USER" "$DEMO_REPO_NAME"
    fi
    exit "$exit_code"
}
trap cleanup_on_abort INT TERM

# Colors (degrade gracefully on non-TTY)
if [ -t 1 ]; then
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    BLUE='\033[1;34m'
    CYAN='\033[1;36m'
    MAGENTA='\033[1;35m'
    DIM='\033[2m'
    BOLD='\033[1m'
    NC='\033[0m'
else
    GREEN=''; YELLOW=''; BLUE=''; CYAN=''; MAGENTA=''; DIM=''; BOLD=''; NC=''
fi

# TUI step tracking. Each major section bumps DEMO_STEP; DEMO_TOTAL_STEPS
# defines the denominator shown in step headers and pause prompts.
DEMO_STEP=0
DEMO_TOTAL_STEPS=12

# do_clear — clear the screen between sections for the TUI feel. Skipped
# on non-TTY (CI / piped) so output captures stay clean.
do_clear() {
    if [ -t 1 ]; then
        clear || true
    fi
}

# repeat CHAR COUNT — print a character N times. Used to build box borders.
repeat() {
    local c="$1" n="$2" i=0
    while [ "$i" -lt "$n" ]; do printf '%s' "$c"; i=$((i + 1)); done
}

# box_title TITLE [SUBTITLE] — print a boxed heading. Uses the same
# min_width as note_box (80 fill chars between corners) so a box_title
# stacked above a note_box has matching rails. Keeps the heavy ╔══╗║
# borders and CYAN+BOLD styling for visual distinction from the lighter
# YELLOW ╭──╮│ note_box callouts.
box_title() {
    local title="$1" subtitle="${2:-}"
    # Match note_box's default width so stacked boxes align column-for-
    # column. content_width grows past min if title/subtitle exceed.
    local min_width=80
    local title_len=${#title} sub_len=${#subtitle}
    local content_width=$((min_width - 4))   # 4 = 2 left pad + 2 right pad
    [ "$title_len" -gt "$content_width" ] && content_width=$title_len
    [ "$sub_len"   -gt "$content_width" ] && content_width=$sub_len
    local total_width=$((content_width + 4))

    printf "${CYAN}╔"; repeat "═" "$total_width"; printf "╗${NC}\n"
    printf "${CYAN}║${NC}"; repeat " " "$total_width"; printf "${CYAN}║${NC}\n"
    # Body lines: use explicit char-counted pad (bash ${#var} is char-
    # aware in UTF-8 locale) so Unicode em-dash etc. don't push the
    # right rail. printf %-Ns pads to BYTES — wrong for multi-byte glyphs.
    local tpad=$((content_width - ${#title}))
    printf "${CYAN}║${NC}  ${BOLD}%s%*s${NC}  ${CYAN}║${NC}\n" "$title" "$tpad" ""
    if [ -n "$subtitle" ]; then
        local spad=$((content_width - ${#subtitle}))
        printf "${CYAN}║${NC}  ${DIM}%s%*s${NC}  ${CYAN}║${NC}\n" "$subtitle" "$spad" ""
    fi
    printf "${CYAN}║${NC}"; repeat " " "$total_width"; printf "${CYAN}║${NC}\n"
    printf "${CYAN}╚"; repeat "═" "$total_width"; printf "╝${NC}\n"
}

# step_header TITLE — major section header with step counter. Auto-pauses
# at the END of the previous step (so the operator reads it before the
# screen clears) for steps 2..N. Skips the pause for step 1 — the intro
# screen already prompted "Press Enter to begin".
step_header() {
    if [ "$DEMO_STEP" -gt 0 ] && [ -t 0 ]; then
        echo
        printf "${DIM}──── Step %d/%d complete · Press Enter to advance to step %d · Ctrl+C to abort ────${NC}" \
            "$DEMO_STEP" "$DEMO_TOTAL_STEPS" "$((DEMO_STEP + 1))"
        read -r _
    fi
    DEMO_STEP=$((DEMO_STEP + 1))
    do_clear
    local title="$1"
    local stamp="STEP $DEMO_STEP of $DEMO_TOTAL_STEPS"
    local line="$stamp  ·  $title"
    local width=${#line}
    local box_width=$((width + 4))
    printf "${BLUE}┌"; repeat "─" "$box_width"; printf "┐${NC}\n"
    printf "${BLUE}│${NC}  ${MAGENTA}%s${NC}  ${DIM}·${NC}  ${BOLD}%s${NC}  ${BLUE}│${NC}\n" "$stamp" "$title"
    printf "${BLUE}└"; repeat "─" "$box_width"; printf "┘${NC}\n"
    echo
}

# Lightweight inline alternatives — used inside a step, not as a section heading.
heading() { printf "\n${BLUE}═══ %s ═══${NC}\n\n" "$*"; }
note() { printf "${YELLOW}ℹ %s${NC}\n" "$*"; }
ok() { printf "${GREEN}✓ %s${NC}\n" "$*"; }
fail() { printf "${YELLOW}✗ %s${NC}\n" "$*" >&2; }
dim() { printf "${DIM}%s${NC}\n" "$*"; }

# note_box [--title TEXT] LINE [LINE ...] — render a multi-line educational
# callout inside Unicode box-drawing characters. Used for the biggest
# narration moments (trace-comment explainer, META-as-housekeeping framing,
# aida pull lifecycle breakdown, etc.) — keeps multi-paragraph content
# visually demarcated from the surrounding command-output flow.
note_box() {
    local title=""
    if [ "$1" = "--title" ]; then
        title="$2"
        shift 2
    fi
    # Auto-sized true box: width grows to fit the longest body line (or the
    # title-pill row), so the box is a perfect rectangle even when one line
    # is longer than the default. Total visible columns per line = width + 2
    # (the corners). Body lines: │ <content padded to width-2 cols> │.
    # Bash ${#var} counts characters (not bytes) under a UTF-8 locale, so
    # Unicode glyphs (→ ← ✓ etc.) pad correctly without leaking the right
    # edge. trace:demo-tui-polish | ai:claude
    local min_width=80
    local width=$min_width
    # Grow to fit body lines: each needs width >= ${#line} + 2 (the two
    # padding spaces around content).
    for line in "$@"; do
        local need=$((${#line} + 2))
        [ "$need" -gt "$width" ] && width="$need"
    done
    # Grow to fit title pill: pill consumes title_len + 4 (┤ + 2 spaces + ├)
    # plus a 2-char left-fill margin → minimum width = title_len + 6.
    if [ -n "$title" ]; then
        local title_need=$((${#title} + 6))
        [ "$title_need" -gt "$width" ] && width="$title_need"
    fi
    # Top border (with optional title pill)
    if [ -n "$title" ]; then
        local left=2
        local right=$((width - left - ${#title} - 4))
        [ "$right" -lt 0 ] && right=0
        printf "${YELLOW}╭"
        repeat "─" "$left"
        printf "┤ ${BOLD}%s${NC}${YELLOW} ├" "$title"
        repeat "─" "$right"
        printf "╮${NC}\n"
    else
        printf "${YELLOW}╭"; repeat "─" "$width"; printf "╮${NC}\n"
    fi
    # Body lines — pad each to (width - 2) interior columns, close with │
    for line in "$@"; do
        local pad=$((width - ${#line} - 2))
        [ "$pad" -lt 0 ] && pad=0
        printf "${YELLOW}│${NC} %s%*s ${YELLOW}│${NC}\n" "$line" "$pad" ""
    done
    # Bottom border
    printf "${YELLOW}╰"; repeat "─" "$width"; printf "╯${NC}\n"
}

# show_cmd PROMPT_PREFIX CMD [ARGS...] — echo a command in a shell-prompt-like
# form then execute it. Operator sees both the command AND its output, which
# matters for a walkthrough demo (otherwise the output looks like magic).
show_cmd() {
    local prefix="$1"
    shift
    printf "${DIM}%s${NC} ${GREEN}\$ %s${NC}\n" "$prefix" "$*"
    "$@"
}

# show_file PATH — display a file's contents in a labelled box. For showing
# what implementation work was done in scripted demos.
show_file() {
    local path="$1"
    local width=70
    printf "${DIM}┌─ %s " "$path"
    repeat "─" $((width - ${#path} - 3))
    printf "┐${NC}\n"
    while IFS= read -r line || [ -n "$line" ]; do
        printf "${DIM}│${NC} %s\n" "$line"
    done < "$path"
    printf "${DIM}└"
    repeat "─" "$width"
    printf "┘${NC}\n"
}

pause() {
    if [ -t 0 ]; then
        echo
        printf "${DIM}──── Step %d/%d complete · Press Enter to continue · Ctrl+C to abort ────${NC}" \
            "$DEMO_STEP" "$DEMO_TOTAL_STEPS"
        read -r _
    fi
}

# step_pause [PROMPT] — pause within a step, between a command's output and
# its explanation (so the operator reads the output before the explainer
# scrolls in). Lighter prompt than `pause` (which is for step boundaries).
step_pause() {
    if [ -t 0 ]; then
        local prompt="${1:-Press Enter to continue}"
        printf "${DIM}──── ${prompt} ────${NC}"
        read -r _
        echo
    fi
}

# Glossary surface — paginates docs/aida/discipline/glossary.yaml so the
# operator can browse definitions of AIDA's vocabulary (substrate, lease,
# auto-bump, etc.) and commands (aida pull, aida queue work, etc.) without
# leaving the demo. The YAML is the structured single source of truth —
# scaffolded into every aida-init'd project, embedded in the binary,
# editable separately from this script. trace:TASK-1-100 | ai:claude
run_glossary() {
    # Primary path: the scaffolded copy in the current project. Present when
    # 'aida init' ran from a binary whose templates include glossary.yaml.
    local glossary="docs/aida/discipline/glossary.yaml"
    if [ ! -f "$glossary" ]; then
        # Fallback: the script's source-tree templates. AIDA_DEMO_SCRIPT_DIR
        # was resolved to an absolute path at script start (before the cd
        # into the demo project) so it's still valid here.
        local fallback="${AIDA_DEMO_SCRIPT_DIR}/../aida-core/templates/docs/aida/discipline/glossary.yaml"
        if [ -n "$AIDA_DEMO_SCRIPT_DIR" ] && [ -f "$fallback" ]; then
            mkdir -p docs/aida/discipline
            cp "$fallback" "$glossary"
            ok "glossary scaffolded from source tree (rebuild aida to embed it in init)"
            echo
        else
            fail "glossary file not found at $glossary"
            dim "   try: 'aida init' (with an up-to-date aida binary), or"
            dim "        cp <aida-src>/aida-core/templates/docs/aida/discipline/glossary.yaml \\"
            dim "           $(pwd)/$glossary"
            return 1
        fi
    fi

    # Extract top-level keys (one per term) in document order.
    mapfile -t TERMS < <(awk '/^[a-z][a-z0-9_-]*: \|[[:space:]]*$/ {sub(/: \|.*$/,""); print}' "$glossary")
    local total=${#TERMS[@]}
    if [ "$total" -eq 0 ]; then
        fail "no terms parsed from $glossary"
        return 1
    fi

    local idx=0
    local view="index"   # "index" or "term"

    while true; do
        do_clear
        if [ "$view" = "index" ]; then
            box_title "AIDA glossary — index" "${total} terms · source: ${glossary}"
            echo
            local col=0
            local i
            for i in "${!TERMS[@]}"; do
                printf "  ${DIM}%2d${NC} ${CYAN}%-32s${NC}" $((i+1)) "${TERMS[$i]}"
                col=$((col + 1))
                if [ $((col % 2)) -eq 0 ]; then echo; fi
            done
            [ $((col % 2)) -ne 0 ] && echo
            echo
            note "Pick a term by number, or [r] to return to the explore menu."
            printf "${DIM}Choice: ${NC}"
            read -r nav
            case "${nav,,}" in
                r|return|q|quit|"") return 0 ;;
                *[!0-9]*) dim "(invalid: $nav)"; sleep 1 ;;
                *)
                    if [ "$nav" -ge 1 ] 2>/dev/null && [ "$nav" -le "$total" ]; then
                        idx=$((nav - 1)); view="term"
                    else
                        dim "(out of range: $nav)"; sleep 1
                    fi
                    ;;
            esac
        else
            local term="${TERMS[$idx]}"
            box_title "Glossary — ${term}" "term $((idx+1)) of ${total}"
            echo
            # Print this term's block-literal body (2-space-indented lines
            # following the `key: |` header, until the next top-level key).
            awk -v t="$term" '
                $0 ~ "^"t": \\|[[:space:]]*$" { in_body=1; next }
                in_body && /^[a-z][a-z0-9_-]*:/ { in_body=0 }
                in_body && /^[[:space:]]/ { sub(/^[[:space:]]{2}/, ""); print "  "$0 }
            ' "$glossary"
            echo
            printf "${DIM}[n]ext · [p]rev · [i]ndex · [r]eturn · Enter=next · Choice: ${NC}"
            read -r nav
            case "${nav,,}" in
                ""|n|next)         idx=$(( (idx + 1) % total )) ;;
                p|prev|previous)   idx=$(( (idx - 1 + total) % total )) ;;
                i|index|l|list)    view="index" ;;
                r|return|q|quit)   return 0 ;;
                *[0-9]*)
                    if [ "$nav" -ge 1 ] 2>/dev/null && [ "$nav" -le "$total" ]; then
                        idx=$((nav - 1))
                    else
                        dim "(out of range: $nav)"; sleep 1
                    fi
                    ;;
                *) dim "(unknown: $nav)"; sleep 1 ;;
            esac
        fi
    done
}

# -----------------------------------------------------------------------------
# Intro screen
# -----------------------------------------------------------------------------

do_clear
echo
box_title "AIDA — first-user demo" "throwaway hello-world walkthrough · safe to abort with Ctrl+C"
echo
note_box --title "What this walkthrough does" \
  "Creates a temporary PUBLIC GitHub repo, runs 'aida init' to scaffold" \
  "a fresh project, walks through the substrate-grounded spec→code→" \
  "commit→auto-bump loop, and prompts for cleanup at the end."
echo
note_box --title "What you'll see" \
  "Every command echoed inline, file contents shown when created, and" \
  "an optional 'explore' menu at the end to demonstrate additional" \
  "substrate surfaces (queue work, doctor, search, findings)."
echo
note "${DEMO_TOTAL_STEPS} steps total. Press Enter to advance; Ctrl+C aborts at any time."
echo
if [ -t 0 ]; then
    printf "${DIM}──── Press Enter to begin ────${NC}"
    read -r _
fi

# -----------------------------------------------------------------------------
# Pre-flight checks
# -----------------------------------------------------------------------------

step_header "Pre-flight checks"

command -v aida >/dev/null 2>&1 || { fail "aida not on PATH. Run 'aida dev activate' first if using the dev build."; exit 1; }
ok "aida found: $(command -v aida)"
dim "   version: $(aida --version 2>&1 | head -1)"

command -v gh >/dev/null 2>&1 || { fail "gh CLI not installed. https://cli.github.com/"; exit 1; }
ok "gh CLI: $(gh --version | head -1)"

gh auth status >/dev/null 2>&1 || { fail "gh not authenticated. Run 'gh auth login' first."; exit 1; }
GH_USER=$(gh api user --jq .login 2>/dev/null)
ok "gh authenticated as: $GH_USER"

[ -z "$(git config --global user.name 2>/dev/null)" ] && { fail "git user.name not set. Run 'git config --global user.name \"Your Name\"' first."; exit 1; }
[ -z "$(git config --global user.email 2>/dev/null)" ] && { fail "git user.email not set."; exit 1; }
ok "git configured: $(git config --global user.name) <$(git config --global user.email)>"

[ -d "$DEMO_LOCAL_DIR" ] && { fail "$DEMO_LOCAL_DIR already exists. Remove it first or wait one second + re-run."; exit 1; }

# -----------------------------------------------------------------------------
# Create the throwaway GitHub repo
# -----------------------------------------------------------------------------

step_header "Create a throwaway GitHub repo for the walkthrough"

note_box --title "About to do — read before pressing Enter" \
  "This creates a PUBLIC repo at:" \
  "    https://github.com/$GH_USER/$DEMO_REPO_NAME" \
  "" \
  "It's tagged in its description as a safe-to-delete demo artifact." \
  "Cleanup is OPT-IN at the end — you can keep it for exploration."
echo
note "Commands about to run:"
dim "    gh repo create $GH_USER/$DEMO_REPO_NAME --public --add-readme"
dim "    gh repo clone  $GH_USER/$DEMO_REPO_NAME $DEMO_LOCAL_DIR"
step_pause "Press Enter to create the repo (Ctrl+C to abort)"

show_cmd "demo$" gh repo create "$GH_USER/$DEMO_REPO_NAME" \
    --public \
    --description "Throwaway AIDA demo (created by scripts/aida-demo.sh — safe to delete)" \
    --add-readme
ok "GitHub repo created"
dim "   https://github.com/$GH_USER/$DEMO_REPO_NAME"

echo
show_cmd "demo$" gh repo clone "$GH_USER/$DEMO_REPO_NAME" "$DEMO_LOCAL_DIR" -- --quiet
ok "Cloned to $DEMO_LOCAL_DIR"

cd "$DEMO_LOCAL_DIR"

# -----------------------------------------------------------------------------
# aida init
# -----------------------------------------------------------------------------

step_header "Initialize AIDA — the one-command setup"

show_cmd "demo$" aida init
ok "aida init complete"
echo
note_box --title "What was just scaffolded" \
  "  .aida/config.toml + orphan branch 'aida-store' + worktree .aida-store/" \
  "  .claude/skills/ + commands/ + hooks/ (Claude Code integration)" \
  "  .codex/skills/ (Codex integration)" \
  "  CLAUDE.md + AGENTS.md + .mcp.json" \
  "  docs/aida/discipline/ + docs/plans/" \
  "  META requirements + auto-enqueued onboarding task"

# -----------------------------------------------------------------------------
# Push initial scaffolding + orphan store to remote
# -----------------------------------------------------------------------------

step_header "Push the scaffolding + orphan substrate to GitHub"

note_box --title "Why both branches push together" \
  "AIDA's substrate lives in git: the spec graph is one YAML per spec" \
  "under the orphan branch 'aida-store', the conventions doc is in" \
  ".claude/AIDA.md, etc. Both code-side AND substrate-side push" \
  "together so a collaborator's 'git clone' + 'aida pull' rehydrates" \
  "the full substrate."
echo
show_cmd "demo$" git add .
note_box --title "Why the [AI:claude] prefix on this commit" \
  "Scaffolded files include trace:SPEC-ID comments, so the pre-commit" \
  "hook flags any commit containing trace-bearing changes that lacks" \
  "the AI-tool attribution. Convention forces honest authorship" \
  "labelling on every trace-touching commit."
echo
note_box --title "Why we pass AIDA_RELEASE=1 here" \
  "The scaffolded files carry trace:SPEC-ID comments inherited from" \
  "AIDA's master templates (BUG-73, EPIC-21, etc. — specs in AIDA's" \
  "OWN repo, not this demo project's substrate). Without the env var," \
  "the hook would warn 'consider including one' and suggest specs" \
  "that don't exist locally. AIDA_RELEASE=1 is the existing escape" \
  "hatch for 'mechanical commit, inherited traces don't apply' — the" \
  "release script uses it too. (Filed as a finding: a substrate-aware" \
  "hook should distinguish local vs foreign trace IDs and only warn" \
  "on the local ones.)"
show_cmd "demo$" env AIDA_RELEASE=1 git commit -m "[AI:claude] chore(aida): scaffold AIDA into demo project" --quiet || dim "nothing to commit"
show_cmd "demo$" git push origin main --quiet
ok "main pushed"

show_cmd "demo$" git push origin aida-store --quiet
ok "aida-store orphan branch pushed (substrate now lives on GitHub)"

# -----------------------------------------------------------------------------
# Initial state inspection
# -----------------------------------------------------------------------------

step_header "Inspect the substrate state with 'aida status'"
note_box --title "What 'aida status' tells you" \
  "Single-pane summary of session / branch / PR-CI / queue / cache /" \
  "scaffolding state. Use this as your default 'where am I, what's" \
  "open' check at the start of every working session."
echo
show_cmd "demo$" aida status

step_header "View the work backlog with 'aida list'"
note_box --title "What 'aida list' shows" \
  "Lists every spec AIDA tracks as actionable work. Just-init'd" \
  "projects have a single auto-enqueued onboarding task (TASK-007)." \
  "As you file stories/tasks/bugs, they show up here, sorted," \
  "filterable, and queryable."
echo
show_cmd "demo$" aida list
echo
note_box --title "About TASK-007 (auto-enqueued onboarding)" \
  "TASK-007 tells you to commit the scaffolded files into git. We'll" \
  "skip running it because the demo already committed the scaffolding" \
  "— but in a real flow you'd pick it up via 'aida queue work TASK-007'."

step_header "View housekeeping specs with 'aida list --type meta'"

note_box --title "Why 'aida list' hides Meta-type" \
  "META-* specs are HOUSEKEEPING / configuration — not work-to-do." \
  "They're the AI prompt templates AIDA uses for its own self-" \
  "customization (e.g., the prompt that runs when you 'aida evaluate'" \
  "a spec, or 'aida suggest relationships'). Edit META-002's" \
  "description to customize how AIDA evaluates your specs; it stays" \
  "editable like any other spec."
echo
show_cmd "demo$" aida list --type meta
echo
note_box --title "What's actually in this fresh project" \
  "Real work-specs (what the operator cares about): 1 — TASK-007." \
  "Housekeeping specs (META-001..006 AI prompt templates): 6." \
  "Total: 7. Default 'aida list' only shows work-specs (the 1)."

# -----------------------------------------------------------------------------
# File the first real spec
# -----------------------------------------------------------------------------

step_header "File the first real spec — STORY for hello.sh"

show_cmd "demo$" aida add --type story --status approved --priority medium \
    --title "Print 'Hello, World!' from hello.sh" \
    --description "Add hello.sh that prints 'Hello, World!' to stdout. Acceptance: ./hello.sh prints the literal string and exits 0."

# Find what ID it got assigned
# trace:BUG-707 | ai:claude — $1 must survive both the human table and the
# TOON agent format `aida list` emits when stdout is not a TTY (id,"title",…)
HELLO_SPEC=$(aida list --type story 2>/dev/null | awk '/Hello, World/ {sub(/,.*/, "", $1); print $1; exit}')
[ -z "$HELLO_SPEC" ] && HELLO_SPEC="STORY-1"
ok "Filed as $HELLO_SPEC"
echo
step_pause "Press Enter — see the new spec in the backlog"

note_box --title "Verify: $HELLO_SPEC is now in the work backlog" \
  "'aida list' (default view) shows actionable work-specs. The new" \
  "STORY should appear at status 'Approved', ready to implement next."
echo
show_cmd "demo$" aida list

# -----------------------------------------------------------------------------
# Implement
# -----------------------------------------------------------------------------

step_header "Implement the spec — write hello.sh with a trace comment"

note_box --title "AIDA's superpower: bidirectional code↔spec linking" \
  "Every implementation file gets an inline trace comment:" \
  "" \
  "    // trace:<SPEC-ID> | ai:<tool>[:confidence]" \
  "" \
  "Format breakdown:" \
  "  trace:STORY-1   ← the spec ID this code implements" \
  "  ai:claude       ← who wrote it (claude / codex / human / antigravity)" \
  "  [:high|med|low] ← optional confidence (high implied)" \
  "" \
  "Below: writing hello.sh with the trace comment included from the start."
cat > hello.sh <<EOF
#!/usr/bin/env bash
# trace:$HELLO_SPEC | ai:claude
echo "Hello, World!"
EOF
chmod +x hello.sh
show_file hello.sh
echo
note "Run it to verify it works:"
show_cmd "demo$" ./hello.sh
echo
note_box --title "Why the trace comment matters" \
  "• 'aida show $HELLO_SPEC' later will list hello.sh under 'Git" \
  "  linkage' — substrate sees the code-side reference automatically" \
  "• 'aida search Hello' finds the spec via the trace web — not" \
  "  just title match" \
  "• Code reviewer / future-you can grep 'trace:$HELLO_SPEC' to find" \
  "  every file that implements this spec" \
  "• Refactoring is safe: rename the function, the trace stays bound" \
  "  to the SPEC-ID"

step_header "Commit + push with the SPEC-ID trailer convention"

note_box --title "AIDA's commit-message convention" \
  "Format:  [AI:tool] type(scope): subject (SPEC-ID)" \
  "" \
  "The (SPEC-ID) at end of subject is the auto-bump scanner's read" \
  "target — that's how the substrate knows which spec this commit" \
  "satisfies, and triggers the Approved/In-Progress → Done transition" \
  "automatically when 'aida pull' runs."
echo
show_cmd "demo$" git add hello.sh
show_cmd "demo$" git commit -m "[AI:claude] feat: hello world script ($HELLO_SPEC)" --quiet
show_cmd "demo$" git push origin main --quiet
ok "Committed + pushed with trailer ($HELLO_SPEC)"

# -----------------------------------------------------------------------------
# Auto-bump via aida pull
# -----------------------------------------------------------------------------

step_header "Sync the substrate + auto-bump spec status via 'aida pull'"

note_box --title "Why 'aida pull' (vs git pull alone)" \
  "1. Pulls code branch (main) from origin — same as 'git pull'" \
  "2. Pulls the orphan store (aida-store) from origin — keeps the" \
  "   substrate's spec graph in sync across collaborators + machines" \
  "3. Runs the AUTO-BUMP SCANNER: walks recent commits on the default" \
  "   branch, parses (SPEC-ID) trailers, transitions each referenced" \
  "   spec's status:" \
  "" \
  "      Approved / In Progress → Done       (commit references it)" \
  "      Done → Completed                    (that commit lands on main)" \
  "" \
  "Our 'feat: hello world script ($HELLO_SPEC)' commit is on main now," \
  "so the scanner auto-bumps $HELLO_SPEC. This is how spec lifecycle" \
  "closes without manual 'aida edit --status' commands."
echo
show_cmd "demo$" aida pull
echo
step_pause "Press Enter — read the pull output, then we'll verify the auto-bump"
do_clear

note_box --title "Verify: did the auto-bump fire?" \
  "Run 'aida show $HELLO_SPEC' and look for:" \
  "  • Status: Completed  (auto-bumped from Approved)" \
  "  • Git linkage: hello.sh listed — the trace:$HELLO_SPEC comment" \
  "    was scanned automatically and bound back to the spec" \
  "  • Recent commits: the 'feat: hello world script ($HELLO_SPEC)'" \
  "    commit appears here"
echo
show_cmd "demo$" aida show "$HELLO_SPEC"

# -----------------------------------------------------------------------------
# Final state
# -----------------------------------------------------------------------------

step_header "Final state — what the substrate tracks after the loop"

show_cmd "demo$" aida status
echo

note_box --title "Inspect the code-side of the bidirectional link" \
  "The trace-link is queryable both directions: from code (grep below)" \
  "and from substrate (the 'Git linkage' section of 'aida show'" \
  "$HELLO_SPEC). That's the bidirectional code↔spec link AIDA" \
  "maintains automatically."
echo
show_cmd "demo$" grep -n "trace:" hello.sh
echo
step_pause "Press Enter — see the substrate surfaces you can now query"

note_box --title "Substrate surfaces you can now query" \
  "  aida show $HELLO_SPEC            # spec body + git linkage" \
  "  aida history --events            # chronological substrate ledger" \
  "  aida list                        # backlog view" \
  "  aida search 'Hello'              # full-text spec search" \
  "  aida doctor                      # multi-agent state drift detect + heal"
echo
step_pause "Press Enter — open the explore menu"

# -----------------------------------------------------------------------------
# Explore menu — let the operator pick what to demonstrate next
# -----------------------------------------------------------------------------

while true; do
    do_clear
    box_title "Optional — explore more substrate surfaces" \
              "pick a topic · [s] skips to cleanup"
    echo
    note_box --title "Pick a surface to demonstrate" \
      "  [1] Anatomy of 'aida queue work' — dissect what it does internally" \
      "  [2] aida history --events       — the substrate event ledger" \
      "  [3] aida doctor                 — multi-agent state drift detect + heal" \
      "  [4] aida search                 — full-text query across specs" \
      "  [5] aida findings add           — advisor observation capture" \
      "  [6] aida queue list / next / done — queue manipulation primitives" \
      "  [7] aida queue work --zen --auto-complete — autonomous drain (self-test)" \
      "  [g] Glossary                    — definitions of AIDA terms + commands" \
      "  [s] Skip to cleanup"
    echo
    if [ ! -t 0 ]; then
        # Non-interactive (CI / piped): skip the menu entirely.
        dim "(non-interactive shell — skipping explore menu)"
        break
    fi
    printf "${DIM}Choice: ${NC}"
    read -r choice
    # 's' / empty: operator wants out. Break BEFORE the do_clear so the
    # cleanup flow renders against a meaningful state.
    case "${choice,,}" in
        s|skip|"") break ;;
    esac
    # Clear the menu before the picked option's output, so the operator
    # sees the option's content on a fresh screen instead of underneath
    # the menu. Then dispatch the option.
    do_clear
    case "${choice,,}" in
        1)
            box_title "See aida queue work happen" "claude -p does the implementer phase"
            echo
            note_box --title "What this walkthrough does" \
              "Files a tiny task, queues it, then invokes 'claude -p' (the" \
              "same headless claude that 'aida queue work --no-human=both'" \
              "calls internally for the implementer phase). Claude reads" \
              "the spec, makes the edit, commits with the (SPEC-ID) trailer." \
              "We then close the loop with 'aida pull' (auto-bump scanner)" \
              "and verify the queue is empty + spec Completed." \
              "" \
              "Real work, real claude API call, real auto-bump."
            echo

            # Prereq: is claude on PATH?
            if ! command -v claude >/dev/null 2>&1; then
                fail "claude CLI not on PATH — option [1] needs it for the implementer phase"
                dim "   install: https://docs.claude.com/claude-code"
                dim "   or pick option [6] (queue primitives, no claude needed)"
                echo
                step_pause "Press Enter to return to the menu"
                continue
            fi
            ok "claude CLI found: $(command -v claude)"
            echo
            step_pause "Press Enter to begin substep (1) — file the task"

            # ── Substep 1: file the task ───────────────────────────────────
            do_clear
            note_box --title "Substep (1) of 5 — file a tiny task" \
              "We're filing a one-line spec: 'Add Goodbye, World! to" \
              "README.md'. Approved status means it's queue-ready."
            echo
            show_cmd "demo$" aida add --type task --status approved --priority low \
                --title "Add 'Goodbye, World!' to README.md" \
                --description "Append the literal text 'Goodbye, World!' as a new line to README.md."

            # trace:BUG-707 | ai:claude — same TOON-vs-table tolerance as HELLO_SPEC
            GOODBYE_SPEC=$(aida list --type task --status approved 2>/dev/null | \
                awk '/Goodbye/ {sub(/,.*/, "", $1); print $1; exit}')
            [ -z "$GOODBYE_SPEC" ] && GOODBYE_SPEC="TASK-2"
            ok "Filed as $GOODBYE_SPEC"
            echo
            step_pause "Press Enter to see substep (2) — show the queue"

            # ── Substep 2: show the queue ──────────────────────────────────
            do_clear
            note_box --title "Substep (2) of 5 — show the queue head" \
              "'aida queue list' shows everything routed to your role." \
              "$GOODBYE_SPEC should appear (with TASK-007 still there from" \
              "the original onboarding queue)."
            echo
            show_cmd "demo$" aida queue list
            echo
            step_pause "Press Enter to run substep (3) — claude -p does the work"

            # ── Substep 3: claude -p does the implementer phase ────────────
            do_clear
            note_box --title "Substep (3) of 5 — claude -p does the implementer phase" \
              "Calling 'claude -p' with --permission-mode bypassPermissions" \
              "and a tight prompt to: append the line, stage, commit with" \
              "the (SPEC-ID) trailer. This is exactly what 'aida queue work" \
              "--no-human=both' calls for phase 1 (we skip --auto-complete" \
              "because the demo project has no CI/reviewer workflow yet)." \
              "" \
              "Expect ~30-60s while claude thinks and acts."
            echo

            # trace:BUG-707 | ai:claude — top-level code, `local` is illegal here
            claude_prompt="You are the implementer in headless mode for spec $GOODBYE_SPEC of an AIDA demo. Do EXACTLY this and nothing more:

1. Run: echo 'Goodbye, World!' >> README.md
2. Run: git add README.md
3. Run: git commit -m '[AI:claude] feat: add goodbye message ($GOODBYE_SPEC)'

When done, print the single line: DONE — committed $GOODBYE_SPEC"

            show_cmd "demo$" claude -p --permission-mode bypassPermissions "$claude_prompt"
            claude_exit=$?

            if [ "$claude_exit" -ne 0 ] || ! git log -1 --pretty=%s | grep -qF "($GOODBYE_SPEC)"; then
                fail "claude -p didn't land the expected commit (exit $claude_exit)"
                note "Falling back to manual implementation so the demo can proceed:"
                show_cmd "demo$" sh -c "echo 'Goodbye, World!' >> README.md"
                show_cmd "demo$" git add README.md
                show_cmd "demo$" env AIDA_RELEASE=1 git commit -m "[AI:claude] feat: add goodbye message ($GOODBYE_SPEC)"
            fi
            echo
            step_pause "Press Enter to see substep (4) — verify what claude did"

            # ── Substep 4: see what claude produced ────────────────────────
            do_clear
            note_box --title "Substep (4) of 5 — verify what claude did" \
              "The commit log + the README content show the implementer's" \
              "output. The (SPEC-ID) trailer is the auto-bump signal the" \
              "next 'aida pull' will pick up."
            echo
            show_cmd "demo$" git log --oneline -3
            echo
            show_cmd "demo$" tail -2 README.md
            echo
            step_pause "Press Enter to run substep (5) — close the loop"

            # ── Substep 5: close the lifecycle ─────────────────────────────
            do_clear
            note_box --title "Substep (5) of 5 — close the loop" \
              "Three commands to wrap up:" \
              "  1. git push  — origin gets the commit" \
              "  2. aida pull — scanner finds ($GOODBYE_SPEC) trailer," \
              "                 auto-bumps Approved → Done → Completed" \
              "  3. aida queue list — confirm the task is gone from queue" \
              "  4. aida show $GOODBYE_SPEC — confirm Completed + linkage"
            echo
            show_cmd "demo$" git push origin main --quiet || dim "(push may fail if you're not on a branch with upstream — okay for demo)"
            echo
            show_cmd "demo$" aida pull
            echo
            show_cmd "demo$" aida queue list
            echo
            show_cmd "demo$" aida show "$GOODBYE_SPEC"
            echo
            note_box --title "End state — full lifecycle visible" \
              "$GOODBYE_SPEC went: Approved → (commit lands) → Completed" \
              "without any manual 'aida edit --status' steps. The (SPEC-ID)" \
              "trailer + 'aida pull' = automatic substrate-side closure." \
              "" \
              "That's the full 'aida queue work' contract: pick → claim →" \
              "claude implements → commit with trailer → pull auto-bumps."
            ;;
        2)
            heading "aida history --events — the substrate ledger"
            note "Every status transition, comment, tag edit shows up as an event:"
            show_cmd "demo$" aida history --events --limit 10
            ;;
        3)
            box_title "aida doctor — multi-agent state drift detect + heal" \
                      "11 categories scanned, safe heal per category"
            echo
            note_box --title "What 'aida doctor' is for" \
              "Read-only diagnostic by default; --heal applies safe fixes" \
              "per category." \
              "" \
              "Scans 11 categories of drift that accumulate when multiple" \
              "agents (Claude, Codex, Antigravity) share the same backlog:" \
              "uncommitted WIP at risk, sticky In-Progress specs without" \
              "a lease, branches ahead of main without a PR, missed auto-" \
              "bumps, open PRs, dormant leases, stale reviewer leases," \
              "orphan worktrees, etc." \
              "" \
              "Designed to be safe to run frequently — read-only by" \
              "default surfaces what's drifted; --heal applies the" \
              "boring, mechanical fixes (close lease, prune worktree)" \
              "without touching anything that needs operator judgement."
            echo
            show_cmd "demo$" aida doctor
            ;;
        4)
            box_title "aida search — full-text search across specs" \
                      "FTS5-indexed, cache-backed, sub-millisecond"
            echo
            note_box --title "What 'aida search' queries" \
              "Cache-backed FTS5 (SQLite full-text search) across every" \
              "spec's title + description + tags + comments. Results" \
              "ranked by relevance, not lexical match." \
              "" \
              "Substrate-grounded search — finds specs by description" \
              "content, not just title. Useful when you remember 'something" \
              "about lease cleanup' but don't recall the SPEC-ID or exact" \
              "title. The cache rebuilds on stale-detection so newly-filed" \
              "specs surface within one read of the cache."
            echo
            show_cmd "demo$" aida search Hello
            ;;
        5)
            box_title "aida findings add — advisor observation capture" \
                      "the feedback-loop primitive that makes AIDA learn"
            echo
            note_box --title "What 'aida findings' is for" \
              "Capture a pattern you've spotted without making it a full" \
              "BUG yet — a friction point, a confusing flow, a recurring" \
              "papercut. Cheap to file, no decision required at capture" \
              "time about whether it warrants engineering effort." \
              "" \
              "Recurrence ≥ 3 promotes the finding to a substrate-" \
              "actionable spec (STORY-467). Auto-decay after 30 days no" \
              "recurrence keeps the backlog honest."
            echo
            note_box --title "Why this matters — the AIDA feedback loop" \
              "An AI agent (Claude, Codex, Antigravity, this demo's" \
              "advisor) will TYPICALLY use 'aida findings add' to file" \
              "the patterns it notices during work: 'I had to ask the" \
              "operator three times what they meant by X', 'the queue" \
              "head misled me about Y', 'I tried flag Z which doesn't" \
              "exist'. Each filed finding becomes substrate." \
              "" \
              "That substrate flows two directions:" \
              "" \
              "  • Forward — the next session sees prior findings and" \
              "    avoids the same mistake (substrate as nightclub-" \
              "    bouncer, not a rule in a CLAUDE.md a confident LLM" \
              "    can ignore)." \
              "  • Backward — patterns that recur enough get promoted" \
              "    to specs and FIXED in code (BUG-386 began as an" \
              "    observation, became a spec, shipped a fix)." \
              "" \
              "Result: AIDA gets better the more it's used. The agent" \
              "captures friction, the substrate retains it, future" \
              "operations either route around it or close it. That's" \
              "the feedback loop that compounds over weeks."
            echo
            step_pause "Press Enter to file a demo finding"

            show_cmd "demo$" aida findings add --kind observation --severity minor \
                --note "Demo finding — captured during aida-demo.sh walkthrough. Safe to dismiss." \
                --tags demo,from-aida-demo
            echo
            show_cmd "demo$" aida findings list
            ;;
        6)
            heading "aida queue list / next / done — queue primitives"
            show_cmd "demo$" aida queue list
            echo
            show_cmd "demo$" aida queue next
            ;;
        7)
            heading "aida queue work --zen --auto-complete — autonomous drain"

            note_box --title "What --zen + --auto-complete does" \
              "Three autonomy modes (orthogonal axes — human present? what to ask?):" \
              "" \
              "  default        — pause for design forks + small ambiguities" \
              "  --zen          — advisor-on-standby; pause ONLY for design forks," \
              "                   roll through CI/review/merge unattended" \
              "  --no-human     — fully headless; advisor tier resolves forks via" \
              "                   /aida-advise, escalates non-defensible ones" \
              "" \
              "--auto-complete  — drive the full lifecycle: impl → CI → review →" \
              "                   merge → pull → bump (each phase headless via" \
              "                   'claude -p'). Composes with any autonomy mode."

            echo
            note "Step 1: Self-test — can we invoke 'claude -p' headless on this machine?"
            note "        (the autonomous-drain primitive — runs each lifecycle phase"
            note "        via a non-interactive 'claude -p' invocation)"
            echo
            if ! command -v claude >/dev/null 2>&1; then
                fail "claude CLI not on PATH — skip this option, or install Claude Code first"
                note "       Install: https://docs.claude.com/claude-code"
                echo
            else
                ok "claude CLI found: $(command -v claude)"
                dim "    version: $(claude --version 2>&1 | head -1)"
                echo
                note "Probing: claude -p 'reply with the single word OK'"
                echo
                probe_out=$(timeout 60 claude -p --permission-mode bypassPermissions \
                    "reply with the single word OK" 2>&1 | tr -d '\r' | head -5)
                probe_exit=$?
                if [ $probe_exit -eq 0 ] && printf '%s' "$probe_out" | grep -qiE '\bOK\b'; then
                    ok "claude -p self-test PASSED — autonomous drain is workable on this machine"
                    dim "    probe response: $(printf '%s' "$probe_out" | head -1)"
                    echo
                    note_box --title "In your real project — try the drain" \
                      "  # Tag a few low-risk specs as a batch:" \
                      "  aida edit STORY-X --tags batch:overnight" \
                      "  aida edit TASK-Y  --tags batch:overnight" \
                      "" \
                      "  # Drain the batch with advisor on standby:" \
                      "  aida queue work --batch overnight --zen --auto-complete" \
                      "" \
                      "  # Or unattended overnight (advisor tier resolves forks):" \
                      "  aida queue work --batch overnight --no-human --auto-complete" \
                      "" \
                      "Per-spec cost is real (CI runs + review tokens). Start with" \
                      "1-2 small specs to calibrate before larger batches."
                else
                    fail "claude -p self-test FAILED (exit $probe_exit)"
                    dim "    output: $(printf '%s' "$probe_out" | head -2)"
                    echo
                    note "       Common causes:"
                    note "         • 'claude' not authenticated — run 'claude' once interactively"
                    note "         • API key not set / network blocked"
                    note "         • timeout (>60s) — model may be cold or rate-limited"
                fi
            fi
            echo
            note "Contract surface — what's actually behind --zen + --auto-complete:"
            show_cmd "demo$" aida queue work --help 2>&1 | grep -E '^[[:space:]]*--(zen|auto-complete|no-human|max|batch)' | head -10
            ;;
        g|glossary)
            run_glossary
            ;;
        *)
            dim "(unknown choice: $choice — pick a number 1-7, 'g' for glossary, or 's' to skip)"
            sleep 1
            ;;
    esac
    # Pause AFTER the picked option completes so the operator can read
    # its output. Next loop iteration clears the screen + re-shows menu.
    echo
    step_pause "Press Enter to return to the explore menu"
done

# -----------------------------------------------------------------------------
# Cleanup
# -----------------------------------------------------------------------------

do_clear
box_title "Demo complete" "decide what happens to the demo state"
echo
note_box --title "Demo artifacts" \
  "  Local dir   : $DEMO_LOCAL_DIR" \
  "  GitHub repo : https://github.com/$GH_USER/$DEMO_REPO_NAME"
echo

if [ "$AUTO_CLEANUP" = "1" ]; then
    confirm="y"
else
    note_box --title "Cleanup will" \
      "  - Delete the local directory ($DEMO_LOCAL_DIR)" \
      "  - Delete the GitHub repo ($GH_USER/$DEMO_REPO_NAME)" \
      "" \
      "Default is N — keep the demo state so you can poke around" \
      "(re-run 'bash scripts/aida-demo.sh' creates a fresh repo)."
    echo
    printf "Clean up now? [y/N]: "
    read -r confirm
fi

if [ "${confirm,,}" = "y" ] || [ "${confirm,,}" = "yes" ]; then
    cd "$HOME/ai"
    rm -rf "$DEMO_LOCAL_DIR"
    gh repo delete "$GH_USER/$DEMO_REPO_NAME" --yes >/dev/null 2>&1
    ok "Cleaned up — local dir + GitHub repo both gone."
else
    note "Skipped cleanup. Manual:"
    dim "  rm -rf $DEMO_LOCAL_DIR"
    dim "  gh repo delete $GH_USER/$DEMO_REPO_NAME --yes"
fi
# Past this point, the abort trap should NOT re-offer cleanup.
DEMO_COMPLETE=1

echo
note_box --title "Where to go from here" \
  "BUG-386 shipped during this demo's authoring session — fresh" \
  "'aida init' projects now scaffold ALL 38 .claude/skills/ + 37" \
  "/.claude/commands/ templates. /aida-pickup and /aida-pr are" \
  "available out of the box." \
  "" \
  "To see the interactive implementer experience the demo's option" \
  "[1] simulates with 'claude -p':" \
  "" \
  "  cd $DEMO_LOCAL_DIR     # or your real AIDA project" \
  "  aida queue work TASK-N           # claims lease, launches claude" \
  "  # claude opens with active scope; /aida-pickup reads the spec" \
  "  # implement, commit, then 'aida pr ship' opens a PR" \
  "" \
  "Re-run this demo any time: bash scripts/aida-demo.sh"
echo
ok "Demo done."
