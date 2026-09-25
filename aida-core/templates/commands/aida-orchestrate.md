---
description: Run /aida-orchestrate.
---
# Drive the Open List to Merged (Operator Proxy)

Act as the operator's proxy orchestrator. The goal is an empty `aida list`,
with every item approved, implemented, reviewed and merged. Items that
genuinely need a human are reported rather than faked. You coordinate and
subagents implement. Every branch gets a fresh reviewer, and approved work
lands through batched integration PRs that are merged on the exact commit and
then verified.

## Instructions

Follow the workflow in `.claude/skills/aida-orchestrate/SKILL.md`:

1. Read the board with `aida list`, `aida awaiting` and `aida ps`. Put each
   item in one bucket: draft, small approved, architecture-class approved,
   orphaned in-progress, human-gated, or epic rollup.
2. Triage drafts as the proxy and record each decision as a `PROXY DECISION`
   comment. Approve concrete work, reject exact duplicates, and defer items
   with a concrete trigger. Leave anything that touches authority or product
   direction as a draft for the operator.
3. Architecture, authority, autonomy and store-integrity specs go through a
   sketch gate: an architect subagent writes the sketch, a separate advisor
   subagent signs it off, and only then is it implemented as amended.
4. Dispatch one implementer subagent per spec, each in its own worktree. They
   must use fixtures only, no polling loops, and run Python only via python3.
   They push their branch but open no PR.
5. Give every branch a fresh reviewer, and a strict one for drain, merge,
   store, lock, authority or unattended-launch code. On REQUEST_CHANGES,
   return the findings to the same implementer, then re-review with a new
   reviewer.
6. Batch approved branches on top of origin/main and run the full local
   preflight. Fix semantic conflicts between approved branches inside the
   batch.
7. Push and open the PR. Wait on the exact commit's required checks in a
   background shell. Merge with `--match-head-commit`, verify the PR shows
   MERGED, run `aida pull`, then clean up.
8. Refresh the handoff note after every batch
   (`aida session handoff --seat orchestrator --write -`). Report
   operator-blocked items plainly.

Guardrails: never force-push the default branch, never bypass a human-at-TTY
floor, never approve changes to your own authority, and never fabricate
results.
