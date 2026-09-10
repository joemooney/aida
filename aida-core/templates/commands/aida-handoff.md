---
description: Capture conversation residue, write a durable handoff, and recommend fresh session vs compact.
argument-hint: [SPEC-ID]
allowed-tools: Bash, Read, Grep, Edit, Write
---

# AIDA Handoff

Codex prompt arguments: `$ARGUMENTS`

Follow the workflow in `.claude/skills/aida-handoff.md`:

1. Capture conversation-only residue using the `/aida-capture` discipline.
2. Run `aida session handoff --check` and inspect drains, leases, briefs, and open PR/review state.
3. Write a durable handoff note or brief for the next session. If `$ARGUMENTS` names a spec, use that as the focus.
4. End with one decision line: `Recommendation: START FRESH` or `Recommendation: COMPACT`, listing every pin when compaction is required.
