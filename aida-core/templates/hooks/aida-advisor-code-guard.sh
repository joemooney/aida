#!/bin/bash
# AIDA Advisor Code Guard — PreToolUse hook for Write|Edit|MultiEdit (STORY-670)
#
# Makes the advisor role boundary a TRIPWIRE instead of fine print. When a
# session running as role=advisor tries to edit CODE (not specs/plans/docs),
# soft-blocks the FIRST such edit and hands the decision back to the agent:
# switch role, route the work, or have the operator enable solo mode. Fires
# ONCE per session (a marker), so after the agent acknowledges/routes — or just
# repeats the edit — work proceeds. It's a nudge, not a wall.
#
# Why a gate, not a CLAUDE.md rule: prose does not bind a confident LLM under
# load (the gate-vs-rule finding — controlled ablations could not reproduce
# rule-dropping, yet it happens in the field). This is substrate-as-bouncer for
# the advisor=specs-not-code invariant, symmetric to the TASK-647 queue-add gate
# (which already refuses queue writes from a non-advisor).
#
# Suppressed (exit 0, silent) when ANY of:
#   - the session role is not advisor
#   - AIDA_AUTO_COMPLETE is set (orchestrator / drain implementer child)
#   - solo mode is active (~/.aida/solo.toml) — operator sanctioned coding here
#   - the target file is not code (specs/plans/docs/config are advisor work)
#   - this session already fired once (per-session marker)
#
# Install: .claude/settings.json hooks.PreToolUse matcher "Write|Edit|MultiEdit".
# Hooks run under their shebang (bash) when invoked as an executable path.
# trace:STORY-670

set -euo pipefail

INPUT=$(cat)

# ── Gate only advisor sessions ──
if [ "${AIDA_SESSION_ROLE:-}" != "advisor" ]; then
    exit 0
fi

# ── Sanctioned-coding contexts → allow silently ──
# An orchestrator/drain implementer child carries AIDA_AUTO_COMPLETE; solo mode
# is the operator's explicit opt-in for advisor+integrator coding.
if [ -n "${AIDA_AUTO_COMPLETE:-}" ]; then
    exit 0
fi
if [ -f "${HOME}/.aida/solo.toml" ]; then
    exit 0
fi

# ── Resolve the target file (Write/Edit/MultiEdit all carry file_path) ──
FILE_PATH=$(echo "$INPUT" | grep -oP '"file_path"\s*:\s*"\K[^"]*' 2>/dev/null || echo "")
if [ -z "$FILE_PATH" ]; then
    exit 0
fi

# ── Advisor work legitimately edits specs/plans/docs/config — only CODE is the
# violation. Suppress doc-ish files and the AIDA/Claude metadata trees. ──
case "$FILE_PATH" in
    *.md | *.txt | *.json | *.toml | *.yaml | *.yml | *.lock) exit 0 ;;
    */docs/* | */.aida/* | */.claude/*) exit 0 ;;
esac

# ── Fire only on recognized code extensions ──
case "$FILE_PATH" in
    *.rs | *.ts | *.tsx | *.js | *.jsx | *.py | *.go | *.java | *.rb | *.c | *.h | *.hpp | *.cc | *.cpp | *.cs | *.kt | *.swift | *.php | *.sh) ;;
    *) exit 0 ;;
esac

# ── Per-session fire-once marker, keyed by the Claude session id ──
SESSION_ID=$(echo "$INPUT" | grep -oP '"session_id"\s*:\s*"\K[^"]*' 2>/dev/null || echo "nosession")
MARKER="${TMPDIR:-/tmp}/aida-advisor-guard-${SESSION_ID}"
if [ -f "$MARKER" ]; then
    exit 0
fi
touch "$MARKER" 2>/dev/null || true

# ── Soft-block (exit 2): stderr is fed back to the agent so it re-evaluates ──
cat >&2 <<'EOF'
⛔ You are in the ADVISOR seat — specs, routing, and review, NOT implementation.
This is a code edit. Before writing source, pick one:
  • aida role enter implementer              — switch hats and implement it yourself
  • aida queue add <SPEC> --for implementer  — route the work to an implementer
  • aida solo                                — operator-sanctioned: code + integrate from this seat
(This fires once per session. If you intend to proceed anyway, repeat the edit.)
EOF
exit 2
