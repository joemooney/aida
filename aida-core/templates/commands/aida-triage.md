# Triage Bug

Systematically reproduce, narrow, root-cause, and plan a fix for a reported bug.

## Instructions

Follow the workflow in `.claude/skills/aida-triage.md`:

1. Read the bug requirement (`aida show <BUG-ID>`) and confirm reproduction steps
2. Narrow the failure surface — bisect, log, or add probes as needed
3. Identify root cause with file:line evidence
4. Assess impact (severity, blast radius, data risk)
5. Propose a fix strategy and record findings as comments on the requirement
6. Update status (in-progress, blocked, or ready-for-fix) before exiting

Use when a bug has been filed and needs a structured investigation.
