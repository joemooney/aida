# AIDA Rebase

Detect, classify, and (optionally) execute a rebase of the current
branch onto its upstream.

## Usage

```
/aida-rebase
```

## Instructions

Follow the workflow in `.claude/skills/aida-rebase.md`:

1. Probe with `aida rebase --dry-run --json` — fetches, classifies, no
   side effects
2. Surface the classification (clean / ahead-only / behind-only /
   diverged-safe / diverged-risky) in natural language
3. Execute (`aida rebase --auto`) or defer based on the user's decision
4. On conflict, fall back to a manual `git rebase`
