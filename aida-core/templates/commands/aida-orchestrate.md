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

1. Read the board: `aida session handoff --seat orchestrator --show`, `aida list`,
   `aida burndown plan --status approved --json`, `aida awaiting` and `aida ps`.
   Dispatch only from the ready set. Check IN FLIGHT before you re-dispatch.
2. Triage drafts as the proxy and record each decision as a `PROXY DECISION`
   comment. Approve concrete work and reject exact duplicates. Defer only
   items waiting on a real trigger; a deferral never counts toward the goal.
   Anything that touches authority or product direction stays a draft, recorded
   with `aida questions ask`.
3. Architecture, authority, autonomy and store-integrity specs go through a
   sketch gate: an architect subagent writes the sketch, a separate advisor
   subagent signs it off, and only then is it implemented as amended.
4. Dispatch one implementer session per spec, each with its own
   `aida worktree add <SPEC>`, which takes the lease. Tests use fixtures only,
   no polling loops, and Python never runs as a shell script. The implementer
   pushes its branch and opens no PR.
5. Give every branch a fresh reviewer, and a strict one for drain, merge,
   store, lock, authority or unattended-launch code. On REQUEST_CHANGES,
   return the findings to the same implementer, then re-review with a new
   reviewer.
6. Batch approved branches on top of origin/main, and run the project's full
   local check set (the same checks CI runs). Fix only mechanical integration
   breaks, and have each fix reviewed. Design conflicts go to the advisor or
   the operator.
7. Push and open the PR. Wait on the required checks in a background job.
   Merge with `--match-head-commit`; never use `--admin`. Verify the PR shows
   MERGED, run `aida pull`, then clean up. Never merge authority-changing diffs
   as proxy: leave them for the operator.
8. Refresh the handoff note after every batch
   (`aida session handoff --seat orchestrator --write -`). Report
   operator-blocked items plainly.

Guardrails:
- Implementation and review always run in separate sessions.
- Never force-push the default branch.
- Never bypass a human-at-TTY floor.
- Never approve or merge changes to authority.
- Escalate when you're unsure.
- Never fabricate results.
