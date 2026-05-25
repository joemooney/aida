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

# -----------------------------------------------------------------------------
# Intro screen
# -----------------------------------------------------------------------------

do_clear
echo
box_title "AIDA — first-user demo" "throwaway hello-world walkthrough · safe to abort with Ctrl+C"
echo
note "This walkthrough creates a temporary public GitHub repo, runs 'aida"
note "init' to scaffold a fresh project, walks through the substrate-grounded"
note "spec→code→commit→auto-bump loop, and prompts for cleanup at the end."
echo
note "What you'll see: every command echoed inline, file contents shown when"
note "created, and an optional 'explore' menu at the end to demonstrate"
note "additional substrate surfaces (queue work, doctor, search, findings)."
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
note "What was just scaffolded:"
note "  .aida/config.toml + orphan branch 'aida-store' + worktree .aida-store/"
note "  .claude/skills/ + commands/ + hooks/ (Claude Code integration)"
note "  .codex/skills/ (Codex integration)"
note "  CLAUDE.md + AGENTS.md + .mcp.json"
note "  docs/aida/discipline/ + docs/plans/"
note "  META requirements + auto-enqueued onboarding task"
pause

# -----------------------------------------------------------------------------
# Push initial scaffolding + orphan store to remote
# -----------------------------------------------------------------------------

step_header "Push the scaffolding + orphan substrate to GitHub"

note "AIDA's substrate lives in git: the spec graph is one YAML per spec"
note "under the orphan branch 'aida-store', the conventions doc is in"
note ".claude/AIDA.md, etc. Both code-side AND substrate-side push together:"
echo
show_cmd "demo$" git add .
note "[AI:claude] prefix on the commit subject because the scaffolded files"
note "include trace:SPEC-ID comments (pre-commit hook flags any commit"
note "with trace-bearing changes that lacks the AI-tool attribution)."
show_cmd "demo$" git commit -m "[AI:claude] chore(aida): scaffold AIDA into demo project" --quiet || dim "nothing to commit"
show_cmd "demo$" git push origin main --quiet
ok "main pushed"

show_cmd "demo$" git push origin aida-store --quiet
ok "aida-store orphan branch pushed (substrate now lives on GitHub)"

# -----------------------------------------------------------------------------
# Initial state inspection
# -----------------------------------------------------------------------------

step_header "Inspect the substrate state with 'aida status'"
note "Single-pane summary of session / branch / PR-CI / queue / cache / scaffolding state."
note "Use this as your default 'where am I, what's open' check."
echo
show_cmd "demo$" aida status
pause

step_header "View the work backlog with 'aida list'"
note "Lists every spec AIDA tracks as actionable work. Just-init'd projects"
note "have a single auto-enqueued onboarding task (TASK-007). As you file"
note "stories/tasks/bugs, they show up here, sorted, filterable, queryable."
echo
show_cmd "demo$" aida list
note "TASK-007 is the auto-enqueued onboarding task: it tells you to commit"
note "the scaffolded files into git. We'll skip running it because the demo"
note "already committed the scaffolding — but in a real flow you'd pick it"
note "up via 'aida queue work TASK-007'."
pause

step_header "View housekeeping specs with 'aida list --type meta'"
note "By default 'aida list' hides Meta-type specs because they're"
note "HOUSEKEEPING / configuration, not work-to-do. They're the AI prompt"
note "templates AIDA uses for its own self-customization (e.g., the prompt"
note "that runs when you 'aida evaluate' a spec, or the prompt 'aida suggest"
note "relationships' uses). Edit META-002's description to customize how"
note "AIDA evaluates your specs — it stays editable like any other spec."
echo
show_cmd "demo$" aida list --type meta
note "These five seeded prompts are the AI-customization layer. Project size"
note "the operator cares about (real specs to ship): 1 (TASK-007). Total"
note "specs including housekeeping: 7 (5 META + TASK-007 + 1 admin)."
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

note "AIDA's superpower: bidirectional code↔spec linking. Every"
note "implementation file gets an inline trace comment:"
echo
dim "    // trace:<SPEC-ID> | ai:<tool>[:confidence]"
echo
note "Format breakdown:"
note "  trace:STORY-1         ← the spec ID this code implements"
note "  ai:claude             ← who wrote it (claude / codex / human / antigravity / etc.)"
note "  [:high|med|low]       ← optional confidence (high implied; med = 40-80% AI; low = <40%)"
echo
note "Below: writing hello.sh with the trace comment included from the start."
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
note "Why the trace comment matters:"
note "  • 'aida show $HELLO_SPEC' later will list hello.sh under 'Git linkage'"
note "    (substrate sees the code-side reference automatically)"
note "  • 'aida search Hello' finds the spec via the trace web — not just title match"
note "  • Code reviewer / future-you can grep 'trace:$HELLO_SPEC' to find every"
note "    file that implements this spec"
note "  • Refactoring is safe: rename the function, the trace stays bound to the SPEC-ID"
pause

step_header "Commit + push with the SPEC-ID trailer convention"

note "AIDA commits follow: [AI:tool] type(scope): subject (SPEC-ID)"
note "The (SPEC-ID) at end of subject is the auto-bump scanner's read target."
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

note "Why we run 'aida pull' here:"
note "  1. It pulls the code branch (main) from origin — same as 'git pull'"
note "  2. It pulls the orphan store (aida-store) from origin — keeps the"
note "     substrate's spec graph in sync across collaborators / machines"
note "  3. It runs the AUTO-BUMP SCANNER: walks recent commits on the"
note "     default branch, parses (SPEC-ID) trailers, and transitions"
note "     each referenced spec's status:"
note "        Approved / In Progress → Done (when a commit references it)"
note "        Done → Completed       (when that commit lands on main)"
echo
note "Our 'feat: hello world script ($HELLO_SPEC)' commit lives on main now,"
note "so the scanner should auto-bump $HELLO_SPEC's status. This is how spec"
note "lifecycle closes without manual 'aida edit --status' commands."
echo
show_cmd "demo$" aida pull
echo
note "Verify the auto-bump worked + see the substrate's code-side reference"
note "of $HELLO_SPEC (hello.sh shows up under 'Git linkage' because of the"
note "trace:$HELLO_SPEC comment AIDA scanned automatically):"
show_cmd "demo$" aida show "$HELLO_SPEC"
pause

# -----------------------------------------------------------------------------
# Final state
# -----------------------------------------------------------------------------

step_header "Final state — what the substrate tracks after the loop"

show_cmd "demo$" aida status
echo

note "The trace-link is queryable via the substrate. Look at hello.sh's"
note "trace comment + see the substrate side of the same link:"
echo
show_cmd "demo$" grep -n "trace:" hello.sh
echo
note "And from the spec side, the Git linkage section listed hello.sh"
note "(see 'aida show $HELLO_SPEC' output above). That's the bidirectional"
note "code↔spec link AIDA's substrate maintains."
echo
note "Substrate surfaces you can now query:"
note "  aida show $HELLO_SPEC            # spec body + git linkage"
note "  aida history --events            # chronological substrate ledger"
note "  aida list                        # backlog view"
note "  aida search 'Hello'              # full-text"
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
