#!/bin/sh
# Best-effort local activity observation. Forward stdin without interpreting it.
# trace:STORY-2748 | ai:codex
command -v aida >/dev/null 2>&1 || exit 0
timeout 1.5s aida internal activity-heartbeat "$1" >/dev/null 2>&1 || true
