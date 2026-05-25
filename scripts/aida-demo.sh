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
    GREEN='\033[0;32m'; YELLOW='\033[0;33m'; BLUE='\033[1;34m'; DIM='\033[2m'; NC='\033[0m'
else
    GREEN=''; YELLOW=''; BLUE=''; DIM=''; NC=''
fi
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
    printf "${DIM}--- contents of %s ---${NC}\n" "$path"
    cat "$path"
    printf "${DIM}--- end of %s ---${NC}\n" "$path"
}
pause() {
    if [ -t 0 ]; then
        printf "${DIM}--- Press Enter to continue (or Ctrl+C to abort) ---${NC}"
        read -r _
    fi
}

# -----------------------------------------------------------------------------
# Pre-flight checks
# -----------------------------------------------------------------------------

heading "Pre-flight checks"

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

heading "Creating throwaway GitHub repo: $DEMO_REPO_NAME"
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

heading "Initializing AIDA (the one-command setup)"

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

heading "Pushing scaffolding + orphan substrate to GitHub"

show_cmd "demo$" git add .
show_cmd "demo$" git commit -m "chore(aida): scaffold AIDA into demo project" --quiet || dim "nothing to commit"
show_cmd "demo$" git push origin main --quiet
ok "main pushed"

show_cmd "demo$" git push origin aida-store --quiet
ok "aida-store orphan branch pushed (substrate now lives on GitHub)"

# -----------------------------------------------------------------------------
# Initial state inspection
# -----------------------------------------------------------------------------

heading "Initial substrate state"
show_cmd "demo$" aida status
pause

heading "What's in the substrate after init"
show_cmd "demo$" aida list
note "By default 'aida list' hides Meta-type specs (config/AI-prompt templates,"
note "not work-to-do). Reveal them explicitly:"
echo
show_cmd "demo$" aida list --type meta
note "These META-* are AI prompt templates AIDA uses for self-customization."
note "TASK-007 is the auto-enqueued onboarding task (file scaffolding into git)."
pause

# -----------------------------------------------------------------------------
# File the first real spec
# -----------------------------------------------------------------------------

heading "Filing the first real spec — STORY for hello.sh"

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

heading "Implementing — write + commit hello.sh"

note "Writing hello.sh with a trace:SPEC-ID comment linking the code back to the spec:"
cat > hello.sh <<EOF
#!/usr/bin/env bash
# trace:$HELLO_SPEC | ai:claude
echo "Hello, World!"
EOF
chmod +x hello.sh
show_file hello.sh
echo
show_cmd "demo$" ./hello.sh

show_cmd "demo$" git add hello.sh
note "Commit subject ends with (SPEC-ID) — the auto-bump scanner reads it. Prefix"
note "is [AI:claude] because hello.sh has an 'ai:claude' trace comment."
show_cmd "demo$" git commit -m "[AI:claude] feat: hello world script ($HELLO_SPEC)" --quiet
show_cmd "demo$" git push origin main --quiet
ok "Committed + pushed with trailer ($HELLO_SPEC) — auto-bump will pick this up"
pause

# -----------------------------------------------------------------------------
# Auto-bump via aida pull
# -----------------------------------------------------------------------------

heading "Auto-bump via aida pull"
note "AIDA's pull scans recent commits for (SPEC-ID) trailers and bumps Done → Completed"
show_cmd "demo$" aida pull
echo
note "Verify $HELLO_SPEC's status — should show Done (or Completed if the auto-bump"
note "scanner picked up the commit), plus a 'Git linkage' section listing hello.sh"
note "since the file has a 'trace:$HELLO_SPEC' comment:"
show_cmd "demo$" aida show "$HELLO_SPEC"
pause

# -----------------------------------------------------------------------------
# Final state
# -----------------------------------------------------------------------------

heading "Final state — what the substrate now tracks"

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
            heading "aida queue work — the queue → /aida-pickup → ship lifecycle"
            note "TASK-007 (the onboarding task) is still on the queue from 'aida init':"
            show_cmd "demo$" aida queue list
            echo
            note "'aida queue work TASK-007' would:"
            note "  1. Create a sibling worktree at ~/ai/${DEMO_REPO_NAME}-task-007"
            note "  2. Create a session lease (.aida/sessions/<id>.toml)"
            note "  3. Bump TASK-007 status Approved → In Progress (BUG-379)"
            note "  4. Launch claude code in the worktree with /aida-pickup TASK-007"
            note "  5. The implementer skill loads spec context + drives the work to completion"
            note "  6. 'aida pr ship' opens the PR + queues a reviewer story"
            note "  7. BUG-376's banner signals 'exit now' — Ctrl+D back to operator shell"
            note "  8. agy2 (integrator) merges + auto-bump fires Done → Completed"
            echo
            note "We won't actually launch a Claude Code session in this demo (would"
            note "be disruptive to script flow), but you can see the queue is ready:"
            show_cmd "demo$" aida queue next
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
