---
name: aida-memory-query
description: Query AIDA as quiet project memory before changing code.
allowed-tools:
  - Bash
  - Grep
  - Read
---

# AIDA Memory Query

## Purpose

Read the project's AIDA memory before acting. This is the lightweight path
for ordinary chat sessions: CLI first, compact TOON output, no extra setup.

## When to Use

Use this skill when:
- Starting a meaningful change
- The user asks whether something already exists
- You need prior decisions, requirements, or implementation intent
- You see a trace comment and need the linked requirement

## Query Reflex

Start with the cheapest useful read:

```bash
AIDA_AGENT_OUTPUT=toon aida search "<topic>"
AIDA_AGENT_OUTPUT=toon aida list --status approved
AIDA_AGENT_OUTPUT=toon aida show <SPEC-ID>
```

If the result is old, treat `modified_at` as a freshness hint, not a verdict.
Verify against code or current docs before relying on stale memory.

## Code Correlation

When a spec looks relevant, check for traces:

```bash
rg "trace:<SPEC-ID>"
```

When code has a trace, read the spec:

```bash
AIDA_AGENT_OUTPUT=toon aida show <SPEC-ID>
```

## Response Shape

Report only what changes the work:
- Relevant spec IDs and titles
- The decision or requirement that matters
- Any freshness concern
- Any gap worth capturing with `$aida-memory-capture`
