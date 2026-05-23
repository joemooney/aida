#!/usr/bin/env bash
# trace:TASK-505 | ai:claude
# Kill running `aida mcp-serve` processes whose binary is OLDER than the
# freshly-built one, so MCP clients respawn with the fresh binary on next request.
#
# Composes with TASK-493 (MCP self-respawn): post-TASK-493 servers self-respawn
# on the next request anyway; pre-TASK-493 zombies have no self-respawn code
# and need an external nudge. Either way, this runs after `make build` so a
# stale server can't survive a build.
#
# The "older binary" filter is load-bearing: it spares the current session's
# MCP server (already on the just-built or just-respawned binary), so running
# `make build` from inside a Claude Code session doesn't cascade-SIGTERM the
# make process itself by killing its controlling MCP.
set -euo pipefail

target_bin=""
for cand in target/debug/aida target/release/aida; do
    if [ -x "$cand" ]; then
        target_bin="$(realpath "$cand")"
        break
    fi
done

if [ -z "$target_bin" ]; then
    echo "ℹ no built aida binary at target/debug or target/release — skipping MCP restart"
    exit 0
fi

# Portable mtime: GNU stat (-c %Y) on Linux, BSD stat (-f %m) on macOS
mtime() {
    stat -c %Y "$1" 2>/dev/null || stat -f %m "$1" 2>/dev/null
}

target_mtime="$(mtime "$target_bin")"
if [ -z "$target_mtime" ]; then
    echo "ℹ stat failed on $target_bin — skipping MCP restart"
    exit 0
fi

killed=""
# [a]ida bracket trick — the regex matches 'aida mcp-serve' but this script's
# own command line contains the literal string '[a]ida mcp-serve' (with brackets)
# which doesn't match the regex. Avoids self-match.
for pid in $(pgrep -f '[a]ida mcp-serve' 2>/dev/null); do
    # /proc/PID/exe symlink is Linux-only; on macOS readlink will fail and we skip.
    # (macOS dev loop will need a separate path — see TASK-505 for follow-up.)
    exe="$(readlink -f "/proc/$pid/exe" 2>/dev/null)" || continue

    # Binary file deleted from disk (path resolves but file gone) — strongest
    # stale signal possible. The classic May-17-zombie shape:
    #   /proc/<pid>/exe -> /home/.../target/debug/aida (deleted)
    if [ ! -f "$exe" ]; then
        kill "$pid" 2>/dev/null || continue
        killed="$killed $pid"
        continue
    fi

    exe_mtime="$(mtime "$exe")"
    [ -z "$exe_mtime" ] && continue
    if [ "$exe_mtime" -lt "$target_mtime" ]; then
        kill "$pid" 2>/dev/null || continue
        killed="$killed $pid"
    fi
done

if [ -n "$killed" ]; then
    echo "⟲ restarted stale MCP servers (PIDs:$killed) — clients respawn on next request"
else
    echo "ℹ no stale MCP servers found (all up-to-date or none running)"
fi
