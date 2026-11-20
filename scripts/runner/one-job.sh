#!/usr/bin/env bash
# Operator-only, root-owned installed entry point. No paths/commands from caller.
# trace:STORY-2695 | ai:codex
set -euo pipefail
if [ "$EUID" -ne 0 ] || [ "$#" -ne 1 ]; then
    echo 'Usage: admission/JIT envelope on stdin | sudo aida-ci-one-job s0|s1' >&2
    exit 2
fi
case "$1" in s0|s1) ;; *) echo 'Unknown runner slot' >&2; exit 2;; esac
exec /usr/bin/python3 -I /usr/local/libexec/aida-ci-controller.py "$1"
