#!/usr/bin/env bash
# Credentialed operator foreground loop. Host independently verifies every claim.
# trace:STORY-2695 | ai:codex
set -euo pipefail
if [ "$EUID" -eq 0 ] || [ "$#" -ne 1 ]; then
    echo 'Usage: rearm.sh s0|s1 (credentialed operator, not root)' >&2
    exit 2
fi
case "$1" in s0|s1) ;; *) exit 2;; esac
exec /usr/bin/python3 -I /usr/local/libexec/aida-ci-rearm.py "$1"
