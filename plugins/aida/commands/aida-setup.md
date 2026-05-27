---
description: Verify local AIDA CLI, MCP, and project setup for Claude Code.
---

# AIDA Setup

Verify this Claude Code session is attached to an AIDA project:

1. Run `aida --version`.
2. If the project is not initialized, run `aida init`.
3. Confirm the MCP server is configured as `aida mcp-serve`.
4. Read `CLAUDE.md` and `docs/agents/cross-agent-onboarding.md`.
5. Use AIDA lifecycle commands for work: `aida queue work`, `aida pr ship`, `aida status`, and `aida doctor`.

If `aida` is not on `PATH`, install the CLI or configure Claude Code's MCP command with the absolute binary path.
