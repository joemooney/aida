#!/bin/bash
# Black-box MCP stdio compatibility suite.
#
# Builds the AIDA CLI, initializes a temporary project, starts
# `aida mcp-serve`, and drives JSON-RPC requests like Codex or another local
# MCP client would.
#
# Validates the descriptor + CLI-to-MCP read direction (post-TASK-440
# outputSchema landing), resources, MCP error envelopes, AND the full
# MCP-write -> CLI-read + coordination round trips. The write-roundtrip stages
# were staged behind a gate while BUG-310 (MCP writes reporting success without
# persisting) was in flight; BUG-310 has shipped, so they run by default now.
# The agent-facing descriptor contract remains behind `--require-agent-contract`
# while docs and implementation converge.
#
# trace:TASK-451 | ai:codex
# trace:BUG-310 | ai:codex
# trace:TASK-453 | ai:claude
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
