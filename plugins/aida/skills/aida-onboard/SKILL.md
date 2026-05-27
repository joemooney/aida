---
description: Orient Claude Code to an AIDA project and verify MCP-backed spec graph access.
disable-model-invocation: true
---

You are operating inside an AIDA project or preparing to initialize one.

Use this checklist:

1. Read `CLAUDE.md` for project conventions.
2. Read `docs/agents/cross-agent-onboarding.md` for the shared MCP operating model.
3. Run `aida status` to inspect active work, queue state, agents, and hygiene findings.
4. If MCP tools are available, verify `show_requirement` or `list_requirements` works before making spec-graph changes.
5. If you implement work, use AIDA lifecycle commands and include trailing spec IDs in commits and PR titles.

Do not treat chat memory as the source of truth. Use AIDA's spec graph, leases, briefs, findings, punts, status, and doctor surfaces for durable coordination.
