#!/bin/bash
# AIDA cargo time-budget hint — PreToolUse hook for Claude Code and Codex.
#
# Warns (never blocks) when a shell command wraps cargo or make in an outer
# GNU `timeout <N>`. That clock starts when the command is issued, so time
# spent waiting for a cargo build slot on a busy host counts against it, and
# the run is killed mid-build and usually retried, re-queueing behind itself.
# The suggested replacement hands the budget to the cargo wrapper, which
# (once aida-host STORY-100 ships) starts the clock when the slot is acquired,
# and runs long work in the tool's background mode, where no foreground tool
# cap applies. Until then the variable is a no-op.
#
# Non-cargo timeouts (`timeout 540 gh pr checks ...`) are left alone. Prose
# that only mentions the pattern (a quoted argument, heredoc body or comment)
# is not a command and is not flagged. A `timeout <N> bash -c "..."` (or sh,
# zsh, dash) is flagged when the quoted body runs cargo or make. A make target
# is passed over only when the Makefile in the hook's directory shows that it
# and every prerequisite run nothing that could reach cargo; anything it cannot
# resolve still gets the hint.
#
# trace:STORY-1854 | ai:claude
# trace:TASK-1967 | ai:claude

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
TIMEOUT_PREFIX="(^|[[:space:];&|(\`])(${WORD}*/)?g?timeout[[:space:]]+(${OPT}[[:space:]]+)*([0-9]+(\.[0-9]+)?[smhd]?)[[:space:]]+(${ASSIGN}[[:space:]]+)*"
TOOL_TAIL="(${WORD}*/)?(cargo|g?make)([[:space:];&|)]|$)"
PATTERN="${TIMEOUT_PREFIX}${TOOL_TAIL}"
# `timeout <N> bash -c "<body>"`: the body is a quoted literal, so the
# stripped scan sees only its quote. trace:TASK-1967 | ai:claude
SHELL_C="(${WORD}*/)?(ba|da|z|k)?sh[[:space:]]+(-[A-Za-z]+[[:space:]]+)*-[A-Za-z]*c[[:space:]]*"
SHELL_PATTERN="${TIMEOUT_PREFIX}${SHELL_C}[\"']"
DQ_BODY='"(([^"\\]|\\.)*)"'
SQ_BODY="'([^']*)'"
# Inside the body: cargo or make at a command position, after optional
# env assignments and plain wrappers such as `nice -n 10`.
WRAPPER="((exec|time|command|nohup|nice)[[:space:]]+(-n[[:space:]]*-?[0-9]+[[:space:]]+)?)"
BODY_PATTERN="(^|[;&|(\`])[[:space:]]*${WRAPPER}*(${ASSIGN}[[:space:]]+)*${TOOL_TAIL}"
CD_PATTERN='(^|[;&|(`])[[:space:]]*(cd|pushd)([[:space:]]|$)'

# True (0) only when every named make target, resolved through the Makefile
# in the current directory, runs nothing that could reach cargo: no cargo or
# rustc, no recursive make, no `aida dev`, no script file and no make variable
# (which could expand to any of those). An unknown target, a prerequisite
# that may come from an include or pattern rule, or no target at all (the
# default goal) is not shown safe. trace:TASK-1967 | ai:claude
make_targets_skip_cargo() {
    local makefile=""
    local f
    for f in GNUmakefile makefile Makefile; do
        if [ -f "$f" ]; then makefile=$f; break; fi
    done
    [ -n "$makefile" ] && [ "$#" -gt 0 ] || return 1
    awk -v wanted="$*" '
    function risky(cmd,   c) {
        c = tolower(cmd)
        gsub(/\$\$/, "", c)
        if (c ~ /cargo|rustc/) return 1
        if (c ~ /\$[({]/) return 1
        if (c ~ /(^|[^a-z0-9_.-])g?make([ \t;&|)]|$)/) return 1
        if (c ~ /aida[ \t]+dev/) return 1
        if (c ~ /\.(sh|py|pl|rb|bash)([^a-z0-9]|$)/) return 1
        if (c ~ /(^|[ \t;&|(@-])(\.\.?\/|scripts\/|tests\/)/) return 1
        return 0
    }
    BEGIN { owner = ""; opaque = 0; incont = 0; indef = 0 }
    indef { if ($0 ~ /^[ \t]*endef/) indef = 0; next }
    /^\t/ || incont {
        # A recipe line, or the continuation of one, whatever its indent.
        incont = ($0 ~ /\\$/)
        if (owner != "") {
            n = split(owner, os, " ")
            for (k = 1; k <= n; k++) if (risky($0)) bad[os[k]] = 1
        }
        next
    }
    /^[ \t]*(#|$)/ { next }
    /^[ \t]*(override[ \t]+)?define[ \t]/ { indef = 1; owner = ""; next }
    /^[ \t]*-?(include|sinclude)[ \t]/ { opaque = 1; owner = ""; next }
    {
        line = $0
        while (line ~ /\\$/ && (getline nxt) > 0) line = substr(line, 1, length(line) - 1) " " nxt
        sub(/[ \t]#.*$/, "", line)
        c = index(line, ":")
        head = substr(line, 1, c - 1)
        if (c == 0 || head ~ /[=$]/ || substr(line, c + 1, 1) == "=") { owner = ""; next }
        body = substr(line, c + 1)
        sub(/^:/, "", body)
        if (body ~ /=/) { owner = ""; next }
        if (head ~ /%/) opaque = 1
        inline = ""
        s = index(body, ";")
        if (s > 0) { inline = substr(body, s + 1); body = substr(body, 1, s - 1) }
        gsub(/\|/, " ", body)
        owner = ""
        n = split(head, ts, /[ \t]+/)
        for (k = 1; k <= n; k++) {
            if (ts[k] == "") continue
            known[ts[k]] = 1
            deps[ts[k]] = deps[ts[k]] " " body
            if (inline != "" && risky(inline)) bad[ts[k]] = 1
            owner = owner " " ts[k]
        }
    }
    END {
        n = split(wanted, queue, " ")
        for (i = 1; i <= n; i++) {
            t = queue[i]
            if (t == "" || seen[t]++) continue
            if (!(t in known)) {
                # A plain file prerequisite is fine; a top-level target, or a
                # prerequisite an include or pattern rule may define, is not.
                if (i <= top || opaque) exit 1
                continue
            }
            if (t in bad) exit 1
            m = split(deps[t], ds, /[ \t]+/)
            for (k = 1; k <= m; k++) if (ds[k] != "") queue[++n] = ds[k]
        }
        exit 0
    }' top="$#" "$makefile"
}

# The make targets named after the matched `make` word, up to the end of that
# simple command. Prints nothing, and fails, when the arguments cannot be
# resolved here (a -C/-f directory or file switch, a quoted target).
make_targets_from() {
    local args=${1%%[;&|)]*}
    local -a words targets=()
    local w
    read -r -a words <<< "$args"
    for w in "${words[@]}"; do
        case "$w" in
            -C*|-f*|-I*|-o*|-W*|--directory*|--file*|--makefile*|--include-dir*|--old-file*|--what-if*|--new-file*|--assume*) return 1 ;;
            *[\"\'\\\$\`]*) return 1 ;;
            -*|*=*) ;;
            *) [[ "$w" =~ ^[0-9.]+$ ]] || targets+=("$w") ;;
        esac
    done
    [ "${#targets[@]}" -gt 0 ] || return 1
    printf '%s\n' "${targets[*]}"
}

duration=""
tool=""
rest=""
shown=""
while IFS= read -r line; do
    if [[ "$line" =~ $PATTERN ]]; then
        duration=${BASH_REMATCH[7]}
        tool=${BASH_REMATCH[12]}
        rest=${line#*"${BASH_REMATCH[0]}"}
        [[ "${BASH_REMATCH[0]}" =~ [[:space:]]$ ]] || rest=""
        shown="timeout ${duration} ${tool}"
        break
    fi
    if [[ "$line" =~ $SHELL_PATTERN ]]; then
        sh_duration=${BASH_REMATCH[7]}
        # Read the body from the raw command: the scan blanked it out.
        body=""
        if [[ "$COMMAND" =~ ${TIMEOUT_PREFIX}${SHELL_C}${DQ_BODY} ]]; then
            body=${BASH_REMATCH[14]}
        elif [[ "$COMMAND" =~ ${TIMEOUT_PREFIX}${SHELL_C}${SQ_BODY} ]]; then
            body=${BASH_REMATCH[14]}
        fi
        body_scan=$(strip_shell_literals "$body") || body_scan=$body
        while IFS= read -r body_line; do
            if [[ "$body_line" =~ $BODY_PATTERN ]]; then
                tool=${BASH_REMATCH[8]}
                rest=${body_line#*"${BASH_REMATCH[0]}"}
                [[ "${BASH_REMATCH[0]}" =~ [[:space:]]$ ]] || rest=""
                break
            fi
        done <<< "$body_scan"
        if [ -n "$tool" ]; then
            duration=$sh_duration
            shown="timeout ${duration} sh -c \"${tool} ...\""
            [[ "$body_scan" =~ $CD_PATTERN ]] && rest=""
            break
        fi
    fi
done <<< "$SCAN"
[ -n "$duration" ] || exit 0

# A make target shown not to reach cargo needs no hint. A `cd` anywhere
# means the Makefile here may not be the one that runs. trace:TASK-1967 | ai:claude
if [ "$tool" != cargo ] && ! [[ "$SCAN" =~ $CD_PATTERN ]]; then
    if targets=$(make_targets_from "$rest") && make_targets_skip_cargo $targets; then
        exit 0
    fi
fi

case "$tool" in
    cargo) replacement="AIDA_CARGO_TIMEOUT=${duration} cargo ..." ;;
    *) replacement="AIDA_CARGO_TIMEOUT=${duration} ${tool} <target>" ;;
esac

MESSAGE="Cargo time budget: \`${shown}\` starts its clock before the cargo slot queue, so a long wait for a build slot can kill the run partway through. Write \`${replacement}\` instead, and run anything that can take more than about 10 minutes in the tool's background mode. Once the host cargo wrapper supports AIDA_CARGO_TIMEOUT (aida-host STORY-100), it starts that budget when the slot is acquired; until then it ignores the variable, and background mode is the only protection. After a timeout or KILLED, check \`cargo ps\` before retrying; never retry blindly."

# JSON-escape the message (it holds backticks and quotes, no newlines).
escaped=${MESSAGE//\\/\\\\}
escaped=${escaped//\"/\\\"}
printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"%s"}}\n' "$escaped"
exit 0
