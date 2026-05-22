#!/bin/bash
# Doc-vs-MCP consistency gate.
#
# Builds the AIDA CLI, then runs the Python harness that parses
# docs/agents/cross-agent-onboarding.md and asserts every documented tool +
# argument exists in `aida mcp-serve` tools/list.
#
# trace:TASK-452 | ai:claude
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$PROJECT_ROOT"
cargo build -p aida-cli --quiet

TARGET_DIR=$(cargo metadata --no-deps --format-version 1 \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
AIDA_BIN="$TARGET_DIR/debug/aida"

python3 "$SCRIPT_DIR/test_mcp_doc_consistency.py" --aida "$AIDA_BIN" "$@"
