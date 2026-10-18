---
description: Run /aida-fire.
---
# Fire mode

Follow `.claude/skills/aida-fire/SKILL.md`:

1. Confirm the operator mandate (scope, merge authority, end time) in this
   session or the handoff, and record it. With no mandate, refuse to start.
2. Survey open PRs and queued work; drop anything with an existing branch,
   PR or claim.
3. Fan out fixers in one message, and review each PR in a separate reviewer
   process launched from Bash (`aida queue work PR-<n> --role reviewer --no-human`),
   never an Agent-tool reviewer; about 10 agents at most.
4. Ship PRs the reviewer process approved with `aida pr ship`, one at a time,
   each freshly updated from main; never record a verdict yourself. Cap review at two rounds; file follow-ups.
5. Watch CI in the background; stop shipping while main is red.
6. Route merge-hold clears to the operator, honour spec-level gates, capture,
   and hand off.

The authority model is `.aida/discipline/fire-mode.md`.

<!-- trace:STORY-1692 | ai:claude -->

ARGUMENTS: $ARGUMENTS
