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
        if command -v aida >/dev/null 2>&1 && seat=$(
            cd "$project_root" 2>/dev/null || exit 1
            if command -v timeout >/dev/null 2>&1; then
                timeout 2 aida role current 2>/dev/null | head -n 1
            else
                aida role current 2>/dev/null | head -n 1
            fi
        ); then
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

# Emit JSON envelope so Claude Code injects body as additionalContext.
# jq if available (handles all the escaping), Python as fallback, raw
# heredoc as last resort (only safe for plain ASCII).
if command -v jq >/dev/null 2>&1; then
    jq -n --arg ctx "$body" \
        '{hookSpecificOutput: {hookEventName: "SessionStart", additionalContext: $ctx}}'
elif command -v python3 >/dev/null 2>&1; then
    python3 -c '
import json, os, sys
body = sys.stdin.read()
print(json.dumps({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": body}}))
' <<<"$body"
else
    # Bare-bones fallback: no escaping. Skip emission rather than emit
    # malformed JSON.
    exit 0
fi

exit 0
