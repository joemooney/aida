#!/bin/bash
# trace:STORY-1477 | ai:codex
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/lib/isolated_home.sh"
aida_isolate_home
cd "$SCRIPT_DIR/.."
# CI's gates and contracts lanes already built the workspace binary.
# trace:TASK-1979 | ai:claude
[ "${AIDA_CI_PREBUILT_CLI:-}" = 1 ] || cargo build -p aida-cli --quiet
TARGET_DIR=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
PYTHONDONTWRITEBYTECODE=1 python3 "$SCRIPT_DIR/test_history_templates.py" "$TARGET_DIR/debug/aida"
