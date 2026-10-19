#!/bin/bash
# AIDA cargo time-budget hint — PreToolUse hook for Claude Code and Codex.
#
# Warns (never blocks) when a shell command wraps cargo or make in an outer
# GNU `timeout <N>`. That clock starts when the command is issued, so time
# spent waiting for a cargo build slot on a busy host counts against it, and
# the run is killed mid-build and usually retried, re-queueing behind itself.
# The suggested replacement hands the budget to the cargo wrapper, which
# starts the clock when the slot is acquired, and runs long work in the
# tool's background mode, where no foreground tool cap applies.
#
# Non-cargo timeouts (`timeout 540 gh pr checks ...`) are left alone. Prose
# that only mentions the pattern (a quoted argument, heredoc body or comment)
# is not a command and is not flagged.
#
# trace:STORY-1854 | ai:claude

set -Euo pipefail

# Warn-only: no failure inside this hook may block the tool call.
trap 'exit 0' ERR

INPUT=$(cat)

# Deliberately duplicated from aida-git-guardrails.sh: scaffolded hooks are
# self-contained, so keep this parser byte-identical in every hook template.
__aida_json_field() {
    local field="$1"
    awk -v wanted="$field" '
    { text = text (NR > 1 ? "\n" : "") $0 }
    END {
        pattern = "\"" wanted "\"[ \t\r\n]*:[ \t\r\n]*\""
        from = 1
        while (from <= length(text) && match(substr(text, from), pattern)) {
            start = from + RSTART - 1
            before = start - 1
            while (before > 0 && substr(text, before, 1) ~ /[ \t\r\n]/) before--
            # Valid JSON escapes quotes of nested keys, so this branch is unreachable
            # for well-formed input and is deliberately untested; retain malformed-input defense.
            if (before == 0 || (substr(text, before, 1) != "{" && substr(text, before, 1) != ",")) {
                from = start + RLENGTH
                continue
            }
            i = start + RLENGTH
            value = ""
            while (i <= length(text)) {
                c = substr(text, i, 1)
                if (c == "\"") { printf "%s", value; exit }
                if (c == "\\" && i < length(text)) {
                    e = substr(text, i + 1, 1)
                    if (e == "\"" || e == "\\" || e == "/") value = value e
                    else if (e == "n") value = value "\n"
                    else if (e == "r") value = value "\r"
                    else if (e == "t") value = value "\t"
                    else if (e == "u") value = value "\\u" # Leave \uXXXX escapes as-is.
                    else value = value "\\" e
                    i += 2
                } else { value = value c; i++ }
            }
            exit
        }
    }'
}

# Deliberately duplicated from aida-git-guardrails.sh (byte-identical): blank
# out quoted bodies, heredoc bodies and comments so prose is not scanned.
strip_shell_literals() {
    printf '%s' "$1" | awk '
    BEGIN { SQ = "\047"; DQ = "\042"; BS = "\\"; st = "N"; nq = 0; hi = 0; inhd = 0; ar = 0; bad = 0 }
    {
        line = $0
        if (inhd) {
            cmp = line
            if (hstrip[hi]) sub(/^\t+/, "", cmp)
            if (cmp == hdelim[hi]) {
                hi++
                if (hi > nq) { inhd = 0; nq = 0; hi = 0 }
            }
            next
        }
        out = ""; n = length(line); i = 1; cont = 0
        while (i <= n) {
            c = substr(line, i, 1)
            if (st == "S") {
                if (c == SQ) { st = "N"; out = out c }
                i++; continue
            }
            if (st == "A" || st == "D") {
                if (c == BS) { i += 2; continue }
                if ((st == "A" && c == SQ) || (st == "D" && c == DQ)) { st = "N"; out = out c }
                i++; continue
            }
            # Unquoted text.
            if (c == BS) {
                if (i == n) { cont = 1; out = out " "; i++; continue }
                out = out substr(line, i, 2); i += 2; continue
            }
            if (c == SQ) {
                st = (substr(out, length(out), 1) == "$") ? "A" : "S"
                out = out c; i++; continue
            }
            if (c == DQ) { st = "D"; out = out c; i++; continue }
            if (c == "#" && (i == 1 || substr(line, i - 1, 1) ~ /[ \t;&|(]/)) break
            if (substr(line, i, 2) == "((") { ar++; out = out "(("; i += 2; continue }
            if (ar > 0 && substr(line, i, 2) == "))") { ar--; out = out "))"; i += 2; continue }
            if (substr(line, i, 3) == "<<<") { out = out "<<<"; i += 3; continue }
            if (ar == 0 && substr(line, i, 2) == "<<") {
                j = i + 2; strip = 0
                if (substr(line, j, 1) == "-") { strip = 1; j++ }
                while (j <= n && substr(line, j, 1) ~ /[ \t]/) j++
                d = ""
                while (j <= n) {
                    ch = substr(line, j, 1)
                    if (ch ~ /[ \t;&|<>()]/) break
                    if (ch != SQ && ch != DQ && ch != BS) d = d ch
                    j++
                }
                if (d == "") bad = 1
                else { nq++; hdelim[nq] = d; hstrip[nq] = strip }
                out = out substr(line, i, j - i); i = j; continue
            }
            out = out c; i++
        }
        if (st == "N" && !cont) {
            printf "%s\n", out
            if (nq > 0) { inhd = 1; hi = 1 }
        } else {
            printf "%s", out
        }
    }
    END { if (st != "N" || inhd || bad) exit 3 }'
}

COMMAND=$(printf '%s' "$INPUT" | __aida_json_field command)
[ -n "$COMMAND" ] || exit 0
# Cheap pre-filter: the common call never names timeout at all.
case "$COMMAND" in
    *timeout*) ;;
    *) exit 0 ;;
esac

# An unparseable command (unterminated quote or heredoc) is scanned raw: a
# spurious hint is harmless, a missed one is the failure this hook prevents.
SCAN=$(strip_shell_literals "$COMMAND") || SCAN=$COMMAND

# `timeout [options] <duration> [env] [VAR=value ...] cargo|make ...`, at a
# command position, with the duration captured. trace:STORY-1854 | ai:claude
WORD='[^[:space:];&|()]'
OPT="(-[ks][[:space:]]*${WORD}+|--(kill-after|signal)(=|[[:space:]]+)${WORD}+|--foreground|--preserve-status|-v|--verbose)"
ASSIGN="(env|[A-Za-z_][A-Za-z0-9_]*=${WORD}*)"
PATTERN="(^|[[:space:];&|(\`])(${WORD}*/)?g?timeout[[:space:]]+(${OPT}[[:space:]]+)*([0-9]+(\.[0-9]+)?[smhd]?)[[:space:]]+(${ASSIGN}[[:space:]]+)*(${WORD}*/)?(cargo|g?make)([[:space:];&|)]|$)"

duration=""
tool=""
while IFS= read -r line; do
    if [[ "$line" =~ $PATTERN ]]; then
        duration=${BASH_REMATCH[7]}
        tool=${BASH_REMATCH[12]}
        break
    fi
done <<< "$SCAN"
[ -n "$duration" ] || exit 0

case "$tool" in
    cargo) replacement="AIDA_CARGO_TIMEOUT=${duration} cargo ..." ;;
    *) replacement="AIDA_CARGO_TIMEOUT=${duration} ${tool} <target>" ;;
esac

MESSAGE="Cargo time budget: \`timeout ${duration} ${tool}\` starts its clock before the cargo slot queue, so a long wait for a build slot can kill the run partway through. Write \`${replacement}\` instead so the cargo wrapper starts the budget when the slot is acquired, and run anything that can take more than about 10 minutes in the tool's background mode. Until the host cargo wrapper honours AIDA_CARGO_TIMEOUT it ignores the variable, and background mode is the only protection. After a timeout or KILLED, check \`cargo ps\` before retrying; never retry blindly."

# JSON-escape the message (it holds backticks and quotes, no newlines).
escaped=${MESSAGE//\\/\\\\}
escaped=${escaped//\"/\\\"}
printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"%s"}}\n' "$escaped"
exit 0
