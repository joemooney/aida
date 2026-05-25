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
dim "Running: aida init"

aida init 2>&1 | tail -20
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

git add .
git commit -m "chore(aida): scaffold AIDA into demo project" --quiet || dim "nothing to commit"
git push origin main --quiet
ok "main pushed"

git push origin aida-store --quiet 2>&1 | tail -3 || true
ok "aida-store orphan branch pushed (substrate now lives on GitHub)"

# -----------------------------------------------------------------------------
# Initial state inspection
# -----------------------------------------------------------------------------

heading "Initial substrate state"
note "Run: aida status"
aida status
pause

heading "What's in the substrate after init"
note "Run: aida list"
aida list
echo
note "The 5 META-* specs are AI prompt templates AIDA uses for self-customization"
note "TASK-N is the auto-enqueued onboarding task (file scaffolding into git)"
pause

# -----------------------------------------------------------------------------
# File the first real spec
# -----------------------------------------------------------------------------

heading "Filing the first real spec — STORY for hello.sh"

aida add --type story --status approved --priority medium \
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

cat > hello.sh <<EOF
#!/usr/bin/env bash
# trace:$HELLO_SPEC | ai:demo-operator
echo "Hello, World!"
EOF
chmod +x hello.sh
note "Created hello.sh with trace:$HELLO_SPEC | ai:demo-operator comment"
./hello.sh

git add hello.sh
git commit -m "feat: hello world script ($HELLO_SPEC)" --quiet
git push origin main --quiet
ok "Committed + pushed with trailer ($HELLO_SPEC) — auto-bump will pick this up"
pause

# -----------------------------------------------------------------------------
# Auto-bump via aida pull
# -----------------------------------------------------------------------------

heading "Auto-bump via aida pull"
note "AIDA's pull scans recent commits for (SPEC-ID) trailers and bumps Done → Completed"
aida pull
echo
note "Now verify $HELLO_SPEC's status:"
aida show "$HELLO_SPEC" 2>/dev/null | head -10 || aida show "STORY-1" 2>/dev/null | head -10
pause

# -----------------------------------------------------------------------------
# Final state
# -----------------------------------------------------------------------------

heading "Final state"
aida status
echo
note "Notice the substrate tracks:"
note "  - Your spec ($HELLO_SPEC) with status, priority, type, tags"
note "  - The trace link from hello.sh to $HELLO_SPEC"
note "  - The commit lineage via git log + (SPEC-ID) trailers"
note "  - All of this is queryable via 'aida show / list / search / history --events'"
pause

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
