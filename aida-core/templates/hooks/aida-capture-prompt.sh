#!/bin/bash
# AIDA Claude Code Hook: verbal-tic safety net via /aida-capture awareness
# Stop hook (runs when Claude's response completes).
#
# Scans the recent transcript for verbal-tic patterns ('worth noting',
# 'should file', etc.) that indicate intent-to-file without matching
# `aida add` or `aida findings` invocations. When such a gap is
# detected, emits a systemMessage reminding the operator to run
# /aida-capture as the safety-net sweep.
#
# Silent (no output) when no triggers detected — keeps Stop quiet in
# routine completions. Best-effort: any error results in silent no-op
# so Stop never fails because of this hook.
#
# Composes with feedback_worth_noting_means_note_it memory (proactive
# instruction layer) — this hook is the reactive backstop.
#
# trace:TASK-496 | ai:claude

set -u

# Read JSON input from stdin (silently no-op on error)
input="$(cat 2>/dev/null || echo '{}')"

# Need jq for parsing; if unavailable, no-op
if ! command -v jq >/dev/null 2>&1; then
    exit 0
fi

session_id="$(echo "$input" | jq -r '.session_id // empty' 2>/dev/null)"
if [ -z "$session_id" ]; then
    exit 0
fi

# Locate the transcript file. Encode cwd by replacing '/' with '-' per
# Claude Code's project-dir convention.
project_dir="${CLAUDE_PROJECT_DIR:-$PWD}"
encoded_cwd="${project_dir//\//-}"
transcript="$HOME/.claude/projects/${encoded_cwd}/${session_id}.jsonl"

if [ ! -f "$transcript" ]; then
    exit 0
fi

# Look at last ~60 entries for verbal-tic patterns vs filing activity.
recent="$(tail -60 "$transcript" 2>/dev/null || true)"
if [ -z "$recent" ]; then
    exit 0
fi

# Verbal-tic triggers: phrases that suggest intent-to-file
tic_count=$(printf '%s' "$recent" | grep -Eic 'worth (noting|capturing|flagging|filing|a follow-up|a memory|a task)|should file|we should track|worth a follow-up|flag (this|it) for later|file (this|that) later' || true)

# Recent filing activity that would close the loop. Also treat /aida-capture
# invocations as filing activity since they're the sweep this hook reminds about.
filing_count=$(printf '%s' "$recent" | grep -Eic 'aida add[^|]*--type|aida findings (promote|dismiss)|<command-name>/aida-capture</command-name>|/aida-capture' || true)

# Only emit reminder when verbal-tic triggers exceed actual filing activity
# (a single filing in a long tic-heavy stretch still warrants the safety-net sweep)
if [ "${tic_count:-0}" -gt "${filing_count:-0}" ]; then
    cat <<'EOF'
{"systemMessage": "Verbal-tic detected: 'worth noting' / 'should file' / similar phrases in recent transcript without a matching `aida add` or `aida findings` invocation. Consider running `/aida-capture` to sweep for missed filings (substrate-side safety net per feedback_worth_noting_means_note_it). Silent when no triggers detected; this fired because triggers were present."}
EOF
fi

exit 0
