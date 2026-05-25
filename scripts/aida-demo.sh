#!/usr/bin/env bash
# aida-demo.sh — throwaway hello-world walkthrough for first-user testing of AIDA
#
# Creates a temporary GitHub repo, runs `aida init`, walks through the
# first-user experience step-by-step (pause-on-Enter between sections),
# then prompts for cleanup at the end.
#
# Prerequisites:
#   - `aida` on PATH (run `aida-on` first if using dev build)
#   - `gh` CLI authenticated (`gh auth status` to verify)
#   - `git` configured with user.name + user.email
#
# Usage:
#   bash scripts/aida-demo.sh
#
# Known limitations (per BUG-386, filed 2026-05-25):
#   `aida init` currently scaffolds only ~20 of ~38 .claude/skills/ templates,
#   so /aida-pickup and /aida-pr won't be available in the demo. This script
#   uses the manual commit-trailer + auto-bump path instead. When BUG-386
#   ships, this script can showcase the full /aida-pickup → /aida-pr flow.
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

# box_title TITLE [SUBTITLE] — print a boxed heading. Used for the top-of-
# demo intro screen.
box_title() {
    local title="$1" subtitle="${2:-}"
    local pad=2
    local title_len=${#title} sub_len=${#subtitle}
    local content_width=$title_len
    [ "$sub_len" -gt "$content_width" ] && content_width=$sub_len
    local total_width=$((content_width + pad * 2))

    printf "${CYAN}╔"; repeat "═" "$total_width"; printf "╗${NC}\n"
    printf "${CYAN}║${NC}"; repeat " " "$total_width"; printf "${CYAN}║${NC}\n"
    printf "${CYAN}║${NC}  ${BOLD}%-${content_width}s${NC}${CYAN}║${NC}\n" "$title"
    if [ -n "$subtitle" ]; then
        printf "${CYAN}║${NC}  ${DIM}%-${content_width}s${NC}${CYAN}║${NC}\n" "$subtitle"
    fi
    printf "${CYAN}║${NC}"; repeat " " "$total_width"; printf "${CYAN}║${NC}\n"
    printf "${CYAN}╚"; repeat "═" "$total_width"; printf "╝${NC}\n"
}

# step_header TITLE — major section header with step counter, clears screen
# first, and prints a boxed "[N of M] TITLE" line.
step_header() {
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
    local width=70
    # Top border (with optional title pill)
    if [ -n "$title" ]; then
        local title_len=${#title}
        local left=2
        local right=$((width - title_len - left - 2))
        [ "$right" -lt 0 ] && right=0
        printf "${YELLOW}╭"; repeat "─" "$left"; printf "┤ ${BOLD}%s${NC}${YELLOW} ├" "$title"; repeat "─" "$right"; printf "╮${NC}\n"
    else
        printf "${YELLOW}╭"; repeat "─" "$width"; printf "╮${NC}\n"
    fi
    # Body lines
    for line in "$@"; do
        printf "${YELLOW}│${NC} %s\n" "$line"
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

# Glossary surface — paginates docs/aida/discipline/glossary.yaml so the
# operator can browse definitions of AIDA's vocabulary (substrate, lease,
# auto-bump, etc.) and commands (aida pull, aida queue work, etc.) without
# leaving the demo. The YAML is the structured single source of truth —
# scaffolded into every aida-init'd project, embedded in the binary,
# editable separately from this script. trace:TASK-1-100 | ai:claude
run_glossary() {
    local glossary="docs/aida/discipline/glossary.yaml"
    if [ ! -f "$glossary" ]; then
        fail "glossary file not found at $glossary (run 'aida init' first or upgrade aida)"
        return 1
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

command -v aida >/dev/null 2>&1 || { fail "aida not on PATH. Run 'aida-on' first if using the dev build."; exit 1; }
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
note "This creates a PUBLIC repo at https://github.com/$GH_USER/$DEMO_REPO_NAME"
note "It will be cleaned up at the end (with confirmation) or you can keep it for exploration."
pause

gh repo create "$GH_USER/$DEMO_REPO_NAME" \
    --public \
    --description "Throwaway AIDA demo (created by scripts/aida-demo.sh — safe to delete)" \
    --add-readme >/dev/null
ok "GitHub repo created"
dim "   https://github.com/$GH_USER/$DEMO_REPO_NAME"

gh repo clone "$GH_USER/$DEMO_REPO_NAME" "$DEMO_LOCAL_DIR" -- --quiet
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
pause

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
show_cmd "demo$" git commit -m "[AI:claude] chore(aida): scaffold AIDA into demo project" --quiet || dim "nothing to commit"
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
pause

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
pause

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
pause

# -----------------------------------------------------------------------------
# File the first real spec
# -----------------------------------------------------------------------------

step_header "File the first real spec — STORY for hello.sh"

show_cmd "demo$" aida add --type story --status approved --priority medium \
    --title "Print 'Hello, World!' from hello.sh" \
    --description "Add hello.sh that prints 'Hello, World!' to stdout. Acceptance: ./hello.sh prints the literal string and exits 0."

# Find what ID it got assigned
HELLO_SPEC=$(aida list --type story 2>/dev/null | awk '/Hello, World/ {print $1; exit}')
[ -z "$HELLO_SPEC" ] && HELLO_SPEC="STORY-1"
ok "Filed as $HELLO_SPEC"
pause

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
pause

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
pause

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
note_box --title "Verify: did the auto-bump fire?" \
  "Run 'aida show $HELLO_SPEC' and look for:" \
  "  • Status: Completed  (auto-bumped from Approved)" \
  "  • Git linkage: hello.sh listed — the trace:$HELLO_SPEC comment" \
  "    was scanned automatically and bound back to the spec" \
  "  • Recent commits: the 'feat: hello world script ($HELLO_SPEC)'" \
  "    commit appears here"
show_cmd "demo$" aida show "$HELLO_SPEC"
pause

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
note_box --title "Substrate surfaces you can now query" \
  "  aida show $HELLO_SPEC            # spec body + git linkage" \
  "  aida history --events            # chronological substrate ledger" \
  "  aida list                        # backlog view" \
  "  aida search 'Hello'              # full-text spec search"
note "  aida doctor                      # multi-agent state drift detect + heal"
pause

# -----------------------------------------------------------------------------
# Explore menu — let the operator pick what to demonstrate next
# -----------------------------------------------------------------------------

heading "Optional — explore more substrate surfaces"

while true; do
    echo
    note "Pick a surface to demonstrate (or skip to cleanup):"
    note "  [1] aida queue work — the queue → /aida-pickup → ship lifecycle"
    note "  [2] aida history --events — the substrate event ledger"
    note "  [3] aida doctor — multi-agent state drift detect + heal"
    note "  [4] aida search — full-text query across specs"
    note "  [5] aida findings add — advisor observation capture"
    note "  [6] aida queue list / next / done — queue manipulation primitives"
    note "  [7] aida queue work --zen --auto-complete — autonomous drain (self-test)"
    note "  [g] Glossary — definitions of AIDA terms + commands"
    note "  [s] Skip to cleanup"
    echo
    if [ ! -t 0 ]; then
        # Non-interactive (CI / piped): skip the menu entirely.
        dim "(non-interactive shell — skipping explore menu)"
        break
    fi
    printf "${DIM}Choice: ${NC}"
    read -r choice
    case "${choice,,}" in
        1)
            heading "aida queue work — the queue → session → ship lifecycle (substrate-level demo)"

            note "TASK-007 (the onboarding task) is still on the queue from 'aida init'."
            note "TASK-007's work (committing the scaffolding) was already done in step 4."
            note "Here we'll demonstrate the substrate-level lifecycle commands that"
            note "'aida queue work' orchestrates — without spawning an interactive"
            note "Claude Code session (which would halt the scripted demo)."
            echo
            show_cmd "demo$" aida queue list

            echo
            note "Step 1: 'aida session start --owns TASK-007 --force-claim' creates"
            note "a sibling worktree + lease + bumps status Approved → In Progress."
            note "(In a normal flow, this is what 'aida queue work TASK-007' invokes"
            note "internally before launching claude code.)"
            show_cmd "demo$" aida session start --owns TASK-007 --force-claim

            echo
            note "Step 2: check the lease + status transition:"
            show_cmd "demo$" aida session leases

            echo
            note "Step 3: in a real workflow operator would 'cd' into the new sibling"
            note "worktree, do the work in a Claude Code session driven by /aida-pickup,"
            note "then 'aida pr ship' to open a PR. We'll skip those interactive steps"
            note "and just close the lifecycle — TASK-007's work is already in main."
            note "'aida queue done TASK-007' atomically marks complete + removes from queue:"
            show_cmd "demo$" aida queue done TASK-007

            echo
            note "Closed lifecycle:"
            show_cmd "demo$" aida queue list

            echo
            note "End-state verification — TASK-007 now Completed:"
            show_cmd "demo$" aida show TASK-007
            ;;
        2)
            heading "aida history --events — the substrate ledger"
            note "Every status transition, comment, tag edit shows up as an event:"
            show_cmd "demo$" aida history --events --limit 10
            ;;
        3)
            heading "aida doctor — multi-agent state drift detect + heal"
            note "Read-only diagnostic by default; --heal applies safe fixes per category."
            show_cmd "demo$" aida doctor
            ;;
        4)
            heading "aida search — full-text search across specs"
            show_cmd "demo$" aida search Hello
            echo
            note "Substrate-grounded search — finds specs by description content, not just title."
            ;;
        5)
            heading "aida findings add — advisor observation capture"
            note "Capture a pattern you've spotted without making it a full BUG yet."
            note "Recurrence ≥ 3 promotes to a substrate-actionable spec (STORY-467)."
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
        s|skip|"")
            break
            ;;
        *)
            dim "(unknown choice: $choice)"
            ;;
    esac
    echo
    note "Pick another, or 's' to skip to cleanup."
done

# -----------------------------------------------------------------------------
# Cleanup
# -----------------------------------------------------------------------------

heading "Demo complete"
echo "Local dir: $DEMO_LOCAL_DIR"
echo "GitHub repo: https://github.com/$GH_USER/$DEMO_REPO_NAME"
echo

if [ "$AUTO_CLEANUP" = "1" ]; then
    confirm="y"
else
    note "Cleanup will:"
    note "  - Delete the local directory ($DEMO_LOCAL_DIR)"
    note "  - Delete the GitHub repo ($GH_USER/$DEMO_REPO_NAME)"
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

heading "Notes"
note "Tonight's known gap (BUG-386): aida init scaffolds only ~20/38 .claude/skills/."
note "/aida-pickup and /aida-pr aren't available yet in fresh projects."
note "Once BUG-386 ships, this demo can showcase the full /aida-pickup → /aida-pr flow."
echo
ok "Demo done. Run again any time: bash scripts/aida-demo.sh"
