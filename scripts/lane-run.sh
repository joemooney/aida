#!/usr/bin/env bash
# trace:BUG-2648 | ai:codex
# Run from the primary checkout. AIDA owns admission, child exit and publication.
# Never follow this command with an unconditional ship --commit-pending.
set -euo pipefail
exec aida queue work "$@"
