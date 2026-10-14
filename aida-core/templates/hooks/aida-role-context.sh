#!/bin/bash
# AIDA Claude Code Hook: per-role system-prompt addendum
# SessionStart hook (runs once when a Claude Code session begins).
#
# Reads the active role from $AIDA_SESSION_ROLE, locates the role's TOML
# file (project-local first, then global ~/.aida/roles/), extracts the
# role's purpose and system_prompt fields, and emits them as
# additionalContext so Claude sees them in its context window.
#
# Best-effort: any error or missing role results in a silent no-op so
# the session start never fails because of this hook.
#
# trace:TASK-1-022 | ai:claude

set -u

# BUG-2007: a Codex session attached to the shared app-server daemon runs its
# hooks with the DAEMON's environment, i.e. the role and grant of whichever
# shell first started the daemon, not this session's. Detect that ancestry
# (Linux /proc; elsewhere this is a no-op) so the role is reported as
# unresolved instead of borrowed. trace:BUG-2007 | ai:claude
under_shared_codex_daemon() {
    # Builtins only (no subshell per ancestor): this runs on every session start.
    local pid="$PPID" depth=0 a0 a1 a2 stat rest
    while [ "$depth" -lt 16 ] && [ "${pid:-0}" -gt 1 ] 2>/dev/null; do
        a0="" a1="" a2=""
        {
            IFS= read -r -d '' a0
            IFS= read -r -d '' a1
            IFS= read -r -d '' a2
        } <"/proc/$pid/cmdline" 2>/dev/null
        # The daemon is `codex app-server ...`; an interpreter-run stand-in
        # shifts it one slot (`bash .../codex app-server`).
        case "${a0##*/} $a1 ${a1##*/} $a2" in
            "codex app-server "* | *" codex app-server") return 0 ;;
        esac
        IFS= read -r stat <"/proc/$pid/stat" 2>/dev/null || return 1
        # Field 4 is the ppid; skip past "(comm) " since comm may hold spaces.
        rest=${stat##*) }
        # shellcheck disable=SC2086
        set -- $rest
        pid="${2:-0}"
        depth=$((depth + 1))
    done
    return 1
}
# shellcheck disable=SC2016 # backticks are literal prose
DAEMON_NOTE='this session runs under the shared Codex app-server daemon, whose environment (role and grant) belongs to the shell that started it, not to this session. Relaunch with `codex --no-daemon` (`aida agent new codex` does this) to get this session'"'"'s own role.'

# Emit JSON envelope so Claude Code injects body as additionalContext.
# jq if available (handles all the escaping), Python as fallback, raw
# heredoc as last resort (only safe for plain ASCII).
emit_context() {
    if command -v jq >/dev/null 2>&1; then
        jq -n --arg ctx "$1" \
            '{hookSpecificOutput: {hookEventName: "SessionStart", additionalContext: $ctx}}'
    elif command -v python3 >/dev/null 2>&1; then
        python3 -c '
import json, os, sys
body = sys.stdin.read()
print(json.dumps({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": body}}))
' <<<"$1"
    fi
    # Bare-bones fallback: no escaping. Skip emission rather than emit
    # malformed JSON.
    return 0
}

if under_shared_codex_daemon; then
    emit_context "# AIDA active role: unresolved

Role unresolved: ${DAEMON_NOTE}"
    exit 0
fi

role="${AIDA_SESSION_ROLE:-}"
if [ -z "$role" ]; then
    exit 0
fi

# Locate the role file. Honor AIDA_SESSION_PROJECT (set by `aida role enter`)
# so the project copy takes precedence even when the hook fires from a
# different cwd; fall back to the vendor project var, then git, then pwd.
project_root="${AIDA_SESSION_PROJECT:-${CLAUDE_PROJECT_DIR:-${CODEX_PROJECT_DIR:-$(git rev-parse --show-toplevel 2>/dev/null || printf %s "$PWD")}}}"

# AIDA_SESSION_ROLE is a display/routing label, not authority. Check it against
# the validated seat grant (`aida role current` prints the granted seat, empty
# when none) so the header never claims a role the session cannot act as. The
# explanatory note lives in the session-context digest; this addendum only tags
# its header. The implementer baseline and guest/requester personas need no
# grant. With no `aida` on PATH, or a project root we cannot enter, there is
# nothing reliable to validate against, so the label stands.
# trace:BUG-1924 | ai:claude
canonical_label=$(printf '%s' "$role" | tr '[:upper:]' '[:lower:]')
[ "$canonical_label" = "dialog" ] && canonical_label=advisor
shown_role="$canonical_label"
case "$canonical_label" in
    implementer | guest | requester) ;;
    *)
        # `aida role current` exits 0 (empty line) when no grant is held; a
        # nonzero status (timeout kill, failure) leaves the label unvalidated.
        if command -v aida >/dev/null 2>&1 && seat_out=$(
            cd "$project_root" 2>/dev/null || exit 1
            if command -v timeout >/dev/null 2>&1; then
                timeout 2 aida role current 2>/dev/null
            else
                aida role current 2>/dev/null
            fi
        ); then
            seat=$(printf '%s\n' "$seat_out" | head -n 1)
            if [ -z "$seat" ]; then
                shown_role="${canonical_label} (no grant)"
            elif [ "$seat" != "$canonical_label" ]; then
                shown_role="$seat"
            fi
        fi
        ;;
esac

# Locate the role file (project copy first, then the global one).
# Try the canonical (lowercase) name first, then the label as written.
role_file=""
for name in "$canonical_label" "$role"; do
    if [ -f "$project_root/.aida/roles/$name.toml" ]; then
        role_file="$project_root/.aida/roles/$name.toml"
    elif [ -f "$HOME/.aida/roles/$name.toml" ]; then
        role_file="$HOME/.aida/roles/$name.toml"
    fi
    [ -n "$role_file" ] && break
done
if [ -n "$role_file" ] && [ ! -r "$role_file" ]; then
    role_file=""
fi
if [ -z "$role_file" ]; then
    exit 0
fi

# Pull purpose + system_prompt out of the TOML. We avoid a hard `tomlq`
# dependency and parse the two specific top-level keys ourselves.
# Fields are written by `aida role` as either single-quoted multi-line
# (toml's """...""") OR escaped single-line strings — handle both.
extract_field() {
    local key="$1"
    local file="$2"
    # serde-toml writes multi-line strings as:
    #   key = """
    #   line1
    #   line2"""
    # i.e. opener on its own line, closer at the END of the last content line.
    # Single-line strings are: key = "escaped\ntext"
    awk -v k="$key" '
        BEGIN { in_block=0; collected="" }
        in_block {
            if ($0 ~ /"""[[:space:]]*$/) {
                trimmed = $0
                sub(/"""[[:space:]]*$/, "", trimmed)
                if (collected != "") collected = collected "\n"
                collected = collected trimmed
                print collected
                exit
            }
            if (collected != "") collected = collected "\n"
            collected = collected $0
            next
        }
        # Single-line """body""" form (rare, but handle it).
        $0 ~ "^" k "[[:space:]]*=[[:space:]]*\"\"\".*\"\"\"[[:space:]]*$" {
            v=$0; sub("^" k "[[:space:]]*=[[:space:]]*\"\"\"", "", v); sub("\"\"\"[[:space:]]*$", "", v); print v; exit
        }
        # Multi-line opener: key = """ alone on a line.
        $0 ~ "^" k "[[:space:]]*=[[:space:]]*\"\"\"[[:space:]]*$" { in_block=1; next }
        # Single-line escaped form.
        $0 ~ "^" k "[[:space:]]*=[[:space:]]*\"" {
            v=$0; sub("^" k "[[:space:]]*=[[:space:]]*\"", "", v); sub("\"[[:space:]]*$", "", v);
            gsub("\\\\\"", "\"", v); gsub("\\\\n", "\n", v); gsub("\\\\\\\\", "\\", v);
            print v; exit
        }
    ' "$file"
}

purpose=$(extract_field purpose "$role_file")
system_prompt=$(extract_field system_prompt "$role_file")

# STORY-278/STORY-285: the advisor seat triages findings the headless drain
# files as draft TASKs — the reviewer (from-review:) and the implementer
# (from-implementer:). Surface a pending count at session start so they aren't
# missed. Non-zero count only — silent when clean. Best-effort: a missing/slow
# `aida` degrades to no line.
# TASK-586: `canonical_label` already folds the deprecated `dialog` alias (and
# any casing) into `advisor`.
findings_line=""
if [ "$canonical_label" = "advisor" ]; then
    n=$(aida findings list --count 2>/dev/null || echo 0)
    case "$n" in
        '' | *[!0-9]*) n=0 ;;
    esac
    if [ "$n" -gt 0 ]; then
        findings_line="${n} findings awaiting triage (use \`aida findings list\` to review)"
    fi
fi

# Build the addendum body. If the role contributes no purpose/system_prompt
# AND there is no findings line, there's nothing useful — exit silently.
if [ -z "$purpose" ] && [ -z "$system_prompt" ] && [ -z "$findings_line" ]; then
    exit 0
fi

body="# AIDA active role: ${shown_role}"
if [ -n "$purpose" ]; then
    body="${body}

Purpose: ${purpose}"
fi
if [ -n "$system_prompt" ]; then
    body="${body}

${system_prompt}"
fi
if [ -n "$findings_line" ]; then
    body="${body}

${findings_line}"
fi

emit_context "$body"

exit 0
