---
description: Run /aida-orchestrate.
---
# Drive the Open List to Merged (Operator Proxy)

Act as the operator's proxy orchestrator. The goal is an empty `aida list`,
with every item approved, implemented, reviewed and merged. Items that
genuinely need a human are reported rather than faked. You coordinate; each
implementer runs as its own process in its own seat. Every branch gets a fresh reviewer, and approved work
lands through batched integration PRs that are merged on the exact commit and
then verified.

## Instructions

Follow the workflow in `.claude/skills/aida-orchestrate/SKILL.md`:

1. Read the board: `aida session handoff --seat orchestrator --show`, `aida list`,
   `aida burndown plan --status approved --json`, `aida awaiting` and `aida ps`.
   Dispatch only from the ready set. Check IN FLIGHT before you re-dispatch.
2. Triage drafts as the proxy and record each decision as a `PROXY DECISION`
   comment. Approve concrete work and sign it off with `aida queue add <ID>`, which
   makes it ready (architecture-class work only after the advisor signs off).
   Reject exact duplicates. Defer only
   items waiting on a real trigger; a deferral never counts toward the goal.
   Anything that touches authority or product direction stays a draft, recorded
   with `aida questions ask`.
3. Architecture, authority, autonomy and store-integrity specs go through a
   sketch gate: an architect session writes the sketch, a different advisor
   session signs it off, and only then is it implemented as amended.
4. Launch one implementer per spec as a separate process from Bash:
   `aida queue work <SPEC> --role implementer --no-human --vendor claude`. It
   takes the worktree and implementer lease (never your grant), opens its own
   PR and stops, and ends with one `implementer:` line whose exit code you
   branch on (0 PR opened, 31 no PR, 32 punted, 33 held, 34 cap reached). For
   an interactive lane you will steer, run
   `aida worktree add <SPEC> --role implementer` and start the session from
   inside that worktree. Never implement through the Agent tool. Tests use
   fixtures only, no polling loops, and Python never runs as a shell script.
   <!-- trace:STORY-1662 | ai:claude -->
   Keep at most 3 child sessions live at
   once, give each a wall-clock and token budget in its brief, and state the
   child's model explicitly; a child that runs out is stopped and re-dispatched
   once, never retried silently. <!-- trace:BUG-1816 | ai:claude -->
5. Give every branch a fresh reviewer as a separate process launched from
   Bash: `aida queue work PR-<n> --role reviewer --no-human --vendor claude`
   (under `AIDA_CONTAINED` the codex vendor refuses). Only when it exits 0
   with a `verdict:` line, read `.aida/review-verdicts/PR-<n>.json`; any
   non-zero exit (30 = no verified verdict) means launch a new reviewer. If
   it refuses with "branch pr-<n> is held by worktree ...", run
   `aida session end --spec PR-<n> --remove --skip-ci --yes` and
   `git branch -D pr-<n>`, then relaunch; if it refuses with "base is N
   commits behind ... overlaps", have the author merge main up first. Never review through the Agent tool,
   never write the reviewer's prompt, and never record a verdict yourself.
   Your seat must be entered with `--delegate-seat reviewer`. On
   RequestChanges, relaunch the implementer on its branch or mail its lane, then launch a
   new reviewer at the new head. <!-- trace:STORY-1654 | ai:claude -->
6. Batch approved branches on top of origin/main, and run the project's full
   local check set (the same checks CI runs). Fix only mechanical integration
   breaks, and have each fix reviewed. Design conflicts go to the advisor or
   the operator.
7. Push and open the PR. Wait on the required checks in a background job.
   Have a reviewer process approve the batch PR's head, then ship with
   `aida pr ship N`; never use `--admin` or `--override-stale-approval`. Verify the PR shows
   MERGED, run `aida pull`, then clean up. Never merge authority-changing diffs
   as proxy: leave them for the operator.
8. Refresh the handoff note after every batch
   (`aida session handoff --seat orchestrator --write -`). Report
   operator-blocked items plainly.

Guardrails:
- Implementation and review always run in separate sessions, and review runs
  in a separate process with its own reviewer seat.
- Never force-push the default branch.
- Never bypass a human-at-TTY floor.
- Never approve or merge changes to authority.
- Escalate when you're unsure.
- Never fabricate results.
