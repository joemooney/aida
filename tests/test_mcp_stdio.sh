#!/bin/bash
# Black-box MCP stdio compatibility suite.
#
# Builds the AIDA CLI, initializes a temporary project, starts
# `aida mcp-serve`, and drives JSON-RPC requests like Codex or another local
# MCP client would. Doc-vs-descriptor contract drift is covered separately by
# tests/test_mcp_doc_consistency.sh.
#
# trace:SPEC-398 | ai:codex
# trace:BUG-310 | ai:codex
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$PROJECT_ROOT"
cargo build -p aida-cli --quiet

TARGET_DIR=$(cargo metadata --no-deps --format-version 1 \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
AIDA_BIN="$TARGET_DIR/debug/aida"

PYTHONDONTWRITEBYTECODE=1 python3 "$SCRIPT_DIR/test_mcp_stdio.py" --aida "$AIDA_BIN" "$@"
