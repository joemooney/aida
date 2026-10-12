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
#   - solo mode is active within its TTL, as resolved by `aida internal solo-active` —
#     operator-sanctioned coding here
#   - the target file is not code (specs/plans/docs/config are advisor work)
#   - the target lies outside every worktree of the session's repository
#     (temp dirs, the session scratchpad)
#   - this session already fired once (per-session marker)
#
# Install: .claude/settings.json hooks.PreToolUse matcher "Write|Edit|MultiEdit".
# Hooks run under their shebang (bash) when invoked as an executable path.
# trace:STORY-670

set -Eeuo pipefail

# Unexpected hook failures must be visible to the agent runtime. Claude/Codex
# treat a silent non-zero hook as a block with no useful reason, so only the
# deliberate `exit 2` path below should ever produce a non-zero status.
# trace:BUG-1092 | ai:codex
__aida_hook_unexpected_error() {
    local status=$?
    [ "$status" -eq 0 ] && return
    printf 'AIDA hook internal error: aida-advisor-code-guard.sh aborted unexpectedly at line %s (exit %s). This is a hook bug, not a policy block.\n' "${BASH_LINENO[0]:-unknown}" "$status" >&2
    exit "$status"
}
trap __aida_hook_unexpected_error ERR

INPUT=$(cat)

# Deliberately duplicated in aida-git-guardrails.sh: scaffolded hooks are
# self-contained, so keep this parser byte-identical in both templates.
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

# ── Resolve the target file (Write/Edit/MultiEdit all carry file_path) ──
FILE_PATH=$(printf '%s' "$INPUT" | __aida_json_field file_path)
if [ -z "$FILE_PATH" ]; then
    exit 0
fi

# ── Advisor work legitimately edits specs/plans/docs/config — only CODE is the
# violation. Suppress doc-ish files and the AIDA/Claude metadata trees. ──
case "$FILE_PATH" in
    *.md | *.txt | *.json | *.toml | *.yaml | *.yml | *.lock) exit 0 ;;
    # Agent + substrate config, not product code. `.codex/` is the Codex
    # equivalent of `.claude/` and must be treated identically, or the
    # guard fires on the agent's own configuration.
    */docs/* | */.aida/* | */.claude/* | */.codex/* | */.antigravity/*) exit 0 ;;
esac

# ── Fire only on recognized code extensions ──
case "$FILE_PATH" in
    *.rs | *.ts | *.tsx | *.js | *.jsx | *.py | *.go | *.java | *.rb | *.c | *.h | *.hpp | *.cc | *.cpp | *.cs | *.kt | *.swift | *.php | *.sh) ;;
    *) exit 0 ;;
esac

# trace:BUG-1976 | ai:claude
# ── Guard only the session's repository ──
# A path outside every worktree of this repository (a temp dir, the Claude
# session scratchpad) is not the code this guard protects: pass silently and
# leave the fire-once marker untouched. Sibling linked worktrees share the
# repository, so they stay guarded. The repository is the project's
# (CLAUDE_PROJECT_DIR) and the cwd's: a session that cd'd into another
# checkout still guards both. When the answer cannot be determined (git
# missing, neither in a repository, the target's directory does not exist,
# or the target is itself a symlink) the guard applies as before rather
# than failing open.
__aida_guard_target_outside_repo() {
    local target="$1" parent canon_parent root canon_root roots="" anchor listed
    case "$target" in
        /*) ;;
        *) target="$PWD/$target" ;;
    esac
    [ -L "$target" ] && return 1
    parent=$(dirname -- "$target")
    canon_parent=$(cd -- "$parent" 2>/dev/null && pwd -P) || return 1
    [ -n "$canon_parent" ] || return 1
    target="${canon_parent%/}/$(basename -- "$target")"
    command -v git >/dev/null 2>&1 || return 1
    for anchor in "${CLAUDE_PROJECT_DIR:-}" "$PWD"; do
        [ -n "$anchor" ] || continue
        listed=$(env -u GIT_DIR -u GIT_WORK_TREE -u GIT_COMMON_DIR \
            git -C "$anchor" worktree list --porcelain 2>/dev/null) || continue
        roots="$roots
$listed"
    done
    case "$roots" in
        *"worktree "*) ;;
        *) return 1 ;;
    esac
    while IFS= read -r root; do
        case "$root" in
            "worktree "*) root="${root#worktree }" ;;
            *) continue ;;
        esac
        canon_root=$(cd -- "$root" 2>/dev/null && pwd -P) || continue
        canon_root="${canon_root%/}"
        case "$target" in
            "$canon_root" | "$canon_root"/*) return 1 ;;
        esac
    done <<EOF
$roots
EOF
    return 0
}
if __aida_guard_target_outside_repo "$FILE_PATH"; then
    exit 0
fi

# ── Per-session fire-once marker, keyed by the Claude session id ──
SESSION_ID=$(printf '%s' "$INPUT" | __aida_json_field session_id)
SESSION_ID=${SESSION_ID:-nosession}
MARKER="${TMPDIR:-/tmp}/aida-advisor-guard-${SESSION_ID}"
if [ -f "$MARKER" ]; then
    exit 0
fi

# trace:BUG-1748 | ai:codex
# Ask the same TTL-aware resolver used by the commit-boundary gate. Resolve
# only on a code edit that would otherwise fire; missing/stale binaries fail
# safe by treating solo as off.
__aida_guard_bin=$(command -v aida 2>/dev/null || true)
if [ -n "$__aida_guard_bin" ] && "$__aida_guard_bin" internal solo-active --help >/dev/null 2>&1; then
    if "$__aida_guard_bin" internal solo-active >/dev/null 2>&1; then
        exit 0
    fi
else
    printf 'aida-advisor-code-guard: could not consult `aida internal solo-active`; treating solo as off.\n' >&2
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
