#!/bin/bash
# Black-box MCP stdio compatibility suite.
#
# Builds the AIDA CLI, initializes a temporary project, starts
# `aida mcp-serve`, and drives JSON-RPC requests like Codex or another local
# MCP client would.
#
# Default run validates the descriptor + CLI-to-MCP read direction (post-TASK-440
# outputSchema landing), resources, and MCP error envelopes. The agent-facing
# descriptor contract is behind `--require-agent-contract` while docs and
# implementation converge. The MCP-write -> CLI-read and coordination stages
# live behind `--require-mcp-write-roundtrip`; pass it through to the Python
# suite for the full check.
#
# trace:TASK-451 | ai:codex
# trace:BUG-310 | ai:codex
# trace:TASK-549 | ai:antigravity
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$PROJECT_ROOT"
cargo build -p aida-cli --quiet

TARGET_DIR=$(cargo metadata --no-deps --format-version 1 \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
AIDA_BIN="$TARGET_DIR/debug/aida"

PYTHONDONTWRITEBYTECODE=1 python3 "$SCRIPT_DIR/test_mcp_stdio.py" --aida "$AIDA_BIN" "$@"
