#!/usr/bin/env bash
# demo-spec-kit-composition.sh — the AIDA <-> GitHub Spec Kit composition seam,
# SHOWN end-to-end.
#
# Thesis (composable, NOT competing): scaffold a feature with Spec Kit; keep it
# queryable / traced / lifecycle-tracked with AIDA. Spec Kit's specs are
# per-feature, frozen artifacts after `/implement`. AIDA holds the cross-feature
# graph + stable IDs + code<->spec traces + lifecycle that Spec Kit structurally
# drops once a feature ships. This script demonstrates the difference with a
# concrete BEFORE (bare Spec Kit dir) / AFTER (same features in AIDA's graph).
#
# It is FACTUAL, not advocacy: it also shows where Spec Kit alone is enough.
#
# The Spec Kit feature dir under ./speckit-feature/ is a faithful,
# hand-authored representative of `/speckit.specify`+`/plan`+`/tasks` output
# (the `specify` CLI is not assumed installed). Three features:
#   001-user-accounts   (implemented)
#   002-session-tokens  (implemented; depends on 001)
#   003-password-reset  (in progress; blocked by BOTH 001 and 002)
#
# Prerequisites:
#   - `aida` on PATH (run `aida-on` first if using the dev build)
#   - `git` configured with user.name + user.email
#   (No network, no GitHub repo, no Spec Kit CLI required.)
#
# Usage:
#   bash docs/demos/spec-kit-composition/demo-spec-kit-composition.sh
#   bash docs/demos/spec-kit-composition/demo-spec-kit-composition.sh --no-pause
#
# Cleanup is automatic: everything happens in a throwaway temp dir.
#
# trace:TASK-875 | ai:claude

set -uo pipefail

PAUSE=1
[ "${1:-}" = "--no-pause" ] && PAUSE=0

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)
SPECKIT_SRC="$SCRIPT_DIR/speckit-feature"

say()  { printf '\n\033[1;36m%s\033[0m\n' "$*"; }
note() { printf '\033[0;90m%s\033[0m\n' "$*"; }
cmd()  { printf '\033[1;33m$ %s\033[0m\n' "$*"; }
pause(){ [ "$PAUSE" = 1 ] && { printf '\n\033[0;90m(Enter to continue)\033[0m'; read -r _; }; }

if ! command -v aida >/dev/null 2>&1; then
  echo "ERROR: 'aida' not on PATH. Run 'aida-on' (dev build) or install aida first." >&2
  exit 1
fi

WORK=$(mktemp -d "${TMPDIR:-/tmp}/aida-speckit-demo.XXXXXX")
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

cd "$WORK"

# ---------------------------------------------------------------------------
say "BEFORE — the bare GitHub Spec Kit feature directory"
# ---------------------------------------------------------------------------
cp -r "$SPECKIT_SRC" "$WORK/project"
cd "$WORK/project"
git init -q -b main
git add -A && git -c user.name=demo -c user.email=demo@demo commit -qm "scaffold with Spec Kit" >/dev/null

note "Spec Kit produced a per-feature tree. Three features, each its own island:"
cmd "find specs -name spec.md | sort"
find specs -name spec.md | sort
echo
note "Each feature has its OWN FR-### ids — 'FR-001' means three different things:"
cmd "grep -h 'FR-001' specs/*/spec.md"
grep -rH '\*\*FR-001\*\*' specs/*/spec.md | sed 's#specs/##'
echo
say "The question that matters six features in: 'What is 003-password-reset blocked by, and is that blocker done?'"
note "Spec Kit's answer lives ONLY in prose. There is no graph to walk:"
cmd "grep -A3 '## Dependencies' specs/003-password-reset/spec.md"
grep -A3 '## Dependencies' specs/003-password-reset/spec.md || true
echo
note "You can grep the word 'block', but you get TEXT, not a queryable relationship."
note "'aida graph --blocked-by' has no equivalent here — the dependency is not a record."
pause

# ---------------------------------------------------------------------------
say "THE SEAM — file the SAME Spec Kit features into AIDA's graph"
# ---------------------------------------------------------------------------
note "Init AIDA over the same project (git-canonical store + cache)."
cmd "aida init --no-skills --no-hooks --no-agent-config"
aida init --no-skills --no-hooks --no-agent-config >/dev/null 2>&1 || {
  echo "aida init failed; aborting demo." >&2; exit 1; }

note "File each Spec Kit feature as an AIDA spec under one epic. The CROSS-FEATURE"
note "dependencies that were prose in Spec Kit become TYPED, queryable edges here."
echo

# Helper: `aida add` prints "Added: <SPEC-ID> - <title>". Parse that.
addspec() { aida add "$@" 2>/dev/null | grep -oE 'Added: [A-Z]+-[0-9-]+' | awk '{print $2}'; }

# NOTE (real gap, see README "Honest findings"): `aida add --parent X --blocked-by Y`
# in ONE shot silently DROPS the --blocked-by edge (the parent-link write clobbers
# it). Until that's fixed, file with --parent, then add blocked-by via `aida edit`.
# This is the honest workaround — the demo does not pretend the one-shot form works.

cmd "aida add --type epic --title 'Auth service' --status approved"
EPIC=$(addspec --type epic --title "Auth service" --status approved)
note "  -> epic $EPIC"

cmd "aida add --type story --title 'User accounts (speckit 001)' --parent $EPIC --status approved   # then -> completed"
S1=$(addspec --type story --title "User accounts (speckit 001)" --parent "$EPIC" --status approved)
aida edit "$S1" --status completed >/dev/null 2>&1
note "  -> $S1 (completed — 001 shipped)"

S2=$(addspec --type story --title "Session tokens (speckit 002)" --parent "$EPIC" --status approved)
cmd "aida add --type story --title 'Session tokens (speckit 002)' --parent $EPIC --status approved"
cmd "aida edit  $S2 --blocked-by $S1            # typed edge + inverse Blocks, atomic"
aida edit "$S2" --blocked-by "$S1" >/dev/null 2>&1
aida edit "$S2" --status in-progress >/dev/null 2>&1
note "  -> $S2 (in-progress; BlockedBy $S1)"

S3=$(addspec --type story --title "Password reset (speckit 003)" --parent "$EPIC" --status approved)
cmd "aida add --type story --title 'Password reset (speckit 003)' --parent $EPIC --status approved"
cmd "aida edit  $S3 --blocked-by $S1 --blocked-by $S2   # blocked by BOTH"
aida edit "$S3" --blocked-by "$S1" --blocked-by "$S2" >/dev/null 2>&1
note "  -> $S3 (approved; BlockedBy BOTH $S1 (done) and $S2 (open) — so it is genuinely blocked)"
pause

# ---------------------------------------------------------------------------
say "TRACE — link the implemented code back to the AIDA spec"
# ---------------------------------------------------------------------------
note "Spec Kit drops the code<->spec link after /implement. AIDA keeps it."
mkdir -p src/accounts
cat > src/accounts/mod.rs <<EOF
// trace:$S1 | ai:human
// Implements speckit 001-user-accounts FR-004: the lookup downstream auth depends on.
pub fn find_account_by_email(_email: &str) -> Option<()> { None }
EOF
cmd "aida trace scan src/   # discover the inline // trace: comment"
aida trace scan src/ 2>/dev/null | head -8 || note "(trace scan output)"
git add -A && git -c user.name=demo -c user.email=demo@demo commit -qm "implement accounts ($S1)" >/dev/null
note "The commit (subject trailer ($S1)) + the inline trace are now both linkage AIDA records."
pause

# ---------------------------------------------------------------------------
say "AFTER — the cross-feature questions AIDA can now answer (Spec Kit can't)"
# ---------------------------------------------------------------------------
say "Q1: 'What is 003-password-reset blocked by?'  (unanswerable from the Spec Kit dir)"
cmd "aida graph $S3 --blocked-by"
aida graph "$S3" --blocked-by 2>/dev/null || note "(graph output)"
echo
say "Q2: 'What is at risk across the whole epic if 001 slips?'  (reverse impact)"
cmd "aida graph $S1 --impact"
aida graph "$S1" --impact 2>/dev/null || note "(graph output)"
echo
say "Q3: 'What's the status of every feature in this epic?'  (lifecycle rollup)"
cmd "aida graph $EPIC --tree"
aida graph "$EPIC" --tree 2>/dev/null || note "(graph output)"
echo
say "Q4: 'Is the code still traced to its spec?'"
cmd "aida show $S1   # git linkage section now lists the commit + trace"
aida show "$S1" 2>/dev/null | sed -n '/[Gg]it linkage/,/^$/p' | head -6 || note "(show output)"
pause

# ---------------------------------------------------------------------------
say "THE OTHER HALF OF THE SEAM — 'aida plan scan' grounds the NEXT feature"
# ---------------------------------------------------------------------------
note "Before you hand the next feature to Spec Kit's /specify, ground it: a"
note "read-only pass that summarizes the current API surface from the trace"
note "graph and flags code paths the spec text names that no longer exist."
note "You feed THAT summary to Spec Kit as context, then --attach the provenance."
cmd "aida plan scan $S3"
aida plan scan "$S3" 2>/dev/null | head -20 || note "(plan scan output)"
pause

# ---------------------------------------------------------------------------
say "Bottom line (honest):"
# ---------------------------------------------------------------------------
note "- Spec Kit produced the THREE feature scaffolds. That work is real and good."
note "  If you only ever ship one feature at a time, loosely cross-referenced,"
note "  Spec Kit ALONE is enough and AIDA's machinery would not earn its keep."
note "- The moment 'what's blocked across this epic?' / 'what breaks if 001 slips?'"
note "  / 'is this code still traced?' became live questions, the per-feature dirs"
note "  could not answer them. AIDA's graph + traces + lifecycle could."
note "- The seam: scaffold THERE, keep the graph HERE. 'aida plan scan <SPEC>'"
note "  grounds the NEXT feature's plan in what the tree actually looks like now,"
note "  then you hand that summary to Spec Kit as context. They compose."
echo
note "(Throwaway dir $WORK is removed on exit.)"
