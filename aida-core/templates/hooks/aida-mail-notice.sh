#!/bin/sh
# AIDA Claude Code Hook: surface the WHOLE coordination inbox into the agent's
# context as one can't-miss per-turn signal — not just mail.
#
# Wired on BOTH SessionStart (once) and UserPromptSubmit (every turn). It is a
# thin relay around the `aida awaiting --notice` verb — it does NOT reimplement
# the unread/cap/identity logic (skill<->CLI symmetry, TASK-736). The verb prints
# ONE compact line spanning every coordination channel where the agent is the
# gate — unacked briefs, findings awaiting triage, reviewer verdicts,
# NeedsAttention escalations, AND unread mail — e.g.
#   `Awaiting you: 2 briefs - 1 finding - 3 mail (1 urgent) - 1 escalation`
# or nothing on the awaiting half when nothing awaits. Plain stdout is added to
# the agent's context, so we just pass the verb's output through. (STORY-741
# folded mail into the unified "Awaiting you" report so this hook covers every
# channel; before, mail interrupted per-turn while briefs/findings/escalations/
# verdicts stayed invisible mid-session until a manual `aida status`.)
#
# STORY-769: `--notice` now ALSO leads with an always-on line —
#   `Current date/time: <local tz>. Timing: first prompt of this session | continuation (Xm since last prompt).`
# — so the agent gets fresh time + cadence context every turn even when nothing
# awaits (this replaced the trial `~/.claude/hooks/inject-time.sh`). The verb
# receives the hook's JSON payload (session_id + event) from this relay through
# `AIDA_HOOK_PAYLOAD`, and stamps a per-session last-human-input timestamp under
# `~/.aida/turn-clock/` that doubles as the human-presence oracle (`aida ps` /
# `aida human presence`). Existing managed hooks receive this relay update on
# scaffold refresh.
#
# CHEAP by construction: `--notice` is cache/local-backed and makes NO network
# call — PRs (the one gh-backed channel) are omitted from the per-turn line and
# stay in the full `aida awaiting`. So this stays fast enough to fire every turn.
#
# Non-marking by design: the notice never advances the mail read-watermark, so it
# surfaces every turn WITHOUT consuming. The agent acts on what it judges safe and
# acks mail explicitly with `aida mailbox inbox` (or `/aida-read-mail`), which
# clears the mail count. Reading is not obeying — mail is interpreted input, not a
# command channel (STORY-585 #4). Details for any channel: run `aida awaiting`.
#
# Best-effort: a missing/slow `aida`, no store, or a caught-up inbox all degrade
# to a clean no-op (empty stdout, exit 0) so a turn never fails because of this.
# Runs under /bin/sh (dash) — no bashisms.
#
# A per-turn hook must be INCAPABLE of blocking a prompt. Two guards: (1) the verb
# itself bails instantly on cache-lock contention (`--notice` uses a short cache
# busy-budget, degrading to empty instead of waiting out the full retry ladder —
# BUG-681); (2) belt-and-braces here, if a `timeout` binary is available we cap
# the whole invocation so it can NEVER outlast Claude Code's hook timeout, even in
# a pathological environment. Falls back to running bare (guard #1 already bounds
# it) so the hook stays portable to hosts without coreutils `timeout`.
#
# trace:STORY-585 | ai:claude
# trace:STORY-741 | ai:claude
# trace:BUG-681 | ai:claude
# trace:BUG-1239 | ai:codex

# Antigravity (`--client agy`) wires this on its SessionStart event, whose
# payload is camelCase (`conversationId`) with no event-name field and whose
# stdout must be one JSON object (`{}` is a no-op). So the agy relay names the
# event (`--event`, exported as AIDA_HOOK_EVENT for the verb) and wraps the
# notice text as an injected system message. Claude/Codex pass no arguments.
# trace:TASK-2253 | ai:claude
client=""
event=""
while [ $# -gt 0 ]; do
    case "$1" in
        --client) client="${2:-}"; [ $# -ge 2 ] && shift; shift ;;
        --event) event="${2:-}"; [ $# -ge 2 ] && shift; shift ;;
        *) shift ;;
    esac
done
case "$client" in
    agy|antigravity|antigravity-cli) agy=1 ;;
    *) agy="" ;;
esac
if [ -n "$event" ]; then
    AIDA_HOOK_EVENT="$event"
    export AIDA_HOOK_EVENT
fi
done_out() {
    [ -n "$agy" ] && printf '{}\n'
    exit 0
}
# An Antigravity process carries no vendor marker an env scan can trust (it
# inherits CLAUDE_CODE_* settings from the shell, and GEMINI_API_KEY is a
# credential the scan skips), so the agy relay names its vendor for the
# session registration and handle this hook triggers. trace:STORY-1843 | ai:claude
if [ -n "$agy" ]; then
    AIDA_AGENT_TYPE=antigravity
    export AIDA_AGENT_TYPE
fi

# Capture the bounded hook body once, before invoking helpers, so the notice
# command itself never reads stdin. Direct/scripted `aida awaiting --notice`
# callers may have an open non-TTY pipe with no EOF and must still return.
AIDA_HOOK_PAYLOAD=$(cat 2>/dev/null || true)
export AIDA_HOOK_PAYLOAD

# Resolve the project root the same way the role-context hook does, so the verb
# runs against the right store regardless of the hook's cwd.
project_root="${AIDA_SESSION_PROJECT:-${CLAUDE_PROJECT_DIR:-${CODEX_PROJECT_DIR:-$(git rev-parse --show-toplevel 2>/dev/null || printf %s "$PWD")}}}"

# `--notice` defaults its identity to the union of this shell's user id and the
# session role ($AIDA_SESSION_ROLE) — the same identity the statusline uses.
# Plain text out (no TTY → no ANSI). Swallow errors; emit only real output.
cd "$project_root" 2>/dev/null || done_out
command -v aida >/dev/null 2>&1 || done_out

# STORY-1843: Antigravity has no per-prompt event; PreInvocation fires before
# every model call. Its turn signal is unread mail only (`aida mailbox notice`,
# addressed to this session's handle, role or user), injected as an ephemeral
# message that the client drops after the turn. A turn is keyed by the
# payload's `initialNumSteps`, so the same notice is not repeated on every
# model call inside one turn, and shows again next turn while it stays unread.
# trace:STORY-1843 | ai:claude
if [ -n "$agy" ] && [ "$event" = "PreInvocation" ]; then
    if command -v timeout >/dev/null 2>&1; then
        mail=$(timeout 3 aida mailbox notice 2>/dev/null || true)
    else
        mail=$(aida mailbox notice 2>/dev/null || true)
    fi
    [ -n "$mail" ] || done_out
    conv=$(printf '%s' "$AIDA_HOOK_PAYLOAD" | sed -n 's/.*"conversationId"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
    turn=$(printf '%s' "$AIDA_HOOK_PAYLOAD" | sed -n 's/.*"initialNumSteps"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p' | head -n 1)
    case "$conv" in
        "" | *[!A-Za-z0-9-]*) state="" ;;
        *) state="$project_root/.aida/hook-state/agy-mail-$conv" ;;
    esac
    if [ -n "$state" ] && [ -n "$turn" ]; then
        key="$turn $(printf '%s' "$mail" | cksum)"
        [ "$(cat "$state" 2>/dev/null)" = "$key" ] && done_out
        mkdir -p "${state%/*}" 2>/dev/null && printf '%s\n' "$key" >"$state" 2>/dev/null
    fi
    text=$(printf '%s\n' "$mail" | awk '{ gsub(/\\/, "\\\\"); gsub(/"/, "\\\""); gsub(/\t/, "\\t"); gsub(/\r/, ""); if (NR > 1) printf "\\n"; printf "%s", $0 }')
    printf '{"injectSteps":[{"ephemeralMessage":"%s"}]}\n' "$text"
    exit 0
fi

# Opportunistic maintenance heartbeat. It is best-effort and hook-scoped:
# `--hook` skips tasks registered as network/heavy, and the scheduler's
# min-gap keeps every-turn calls cheap. trace:STORY-1047 | ai:codex
if command -v timeout >/dev/null 2>&1; then
    timeout 4 aida schedule tick --hook >/dev/null 2>&1 || true
elif command -v gtimeout >/dev/null 2>&1; then
    gtimeout 4 aida schedule tick --hook >/dev/null 2>&1 || true
else
    aida schedule tick --hook >/dev/null 2>&1 || true
fi

# Cross-platform outer bound: prefer GNU `timeout`, then macOS/Homebrew
# `gtimeout`; if neither exists, run bare (the verb's internal fast-fail already
# guarantees it returns promptly). 4s stays inside the hook's own timeout.
notice() {
    if command -v timeout >/dev/null 2>&1; then
        timeout 4 aida awaiting --notice 2>/dev/null || true
    elif command -v gtimeout >/dev/null 2>&1; then
        gtimeout 4 aida awaiting --notice 2>/dev/null || true
    else
        aida awaiting --notice 2>/dev/null || true
    fi
}
if [ -z "$agy" ]; then
    notice
    exit 0
fi
# JSON-escape the notice lines (backslash, quote, tab; CR dropped; lines
# joined with \n) — the verb prints plain text with no other control bytes.
text=$(notice | awk '{ gsub(/\\/, "\\\\"); gsub(/"/, "\\\""); gsub(/\t/, "\\t"); gsub(/\r/, ""); if (NR > 1) printf "\\n"; printf "%s", $0 }')
[ -n "$text" ] || done_out
printf '{"injectSteps":[{"systemMessage":{"systemMessage":"%s"}}]}\n' "$text"
exit 0
