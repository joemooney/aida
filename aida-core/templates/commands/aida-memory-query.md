---
description: "Query AIDA as quiet project memory before changing code."
---
# AIDA Memory Query

Read the project's AIDA memory before acting — the lightweight path for
ordinary chat sessions: CLI first, compact TOON output, no extra setup.

## Instructions

Follow the workflow in `.claude/skills/aida-memory-query.md`:

1. Start with the cheapest useful read:
   ```bash
   AIDA_AGENT_OUTPUT=toon aida search "<topic>"
   AIDA_AGENT_OUTPUT=toon aida list --status approved
   AIDA_AGENT_OUTPUT=toon aida show <SPEC-ID>
   ```
2. When a spec looks relevant, correlate with code (`rg "trace:<SPEC-ID>"`);
   when code carries a trace, read the linked spec.
3. Treat `modified_at` as a freshness hint, not a verdict — verify stale
   memory against code or current docs before relying on it.
4. Report only what changes the work: relevant spec IDs + titles, the
   decision or requirement that matters, freshness concerns, and any gap
   worth capturing with `/aida-memory-capture`.

Use at the start of a meaningful change or when checking whether a decision
already exists.
