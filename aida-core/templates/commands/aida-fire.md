---
description: Run /aida-fire.
---
# Fire mode

Follow `.claude/skills/aida-fire/SKILL.md`:

1. Confirm the operator mandate (scope, merge authority, end time) in this
   session or the handoff, and record it. With no mandate, refuse to start.
2. Survey open PRs and queued work; drop anything with an existing branch,
   PR or claim.
3. Fan out fixers and independent reviewers in one message, about 10 agents
   at most, with the review tier scaled to risk.
4. Ship approved PRs with the FAQ-57 recipe, one at a time, each freshly
   updated from main. Cap review at two rounds; file follow-ups.
5. Watch CI in the background; stop shipping while main is red.
6. Route merge-hold clears to the operator, honour spec-level gates, capture,
   and hand off.

The authority model is `.aida/discipline/fire-mode.md`.

<!-- trace:STORY-1692 | ai:claude -->

ARGUMENTS: $ARGUMENTS
