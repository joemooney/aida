---
name: aida-orchestrate
description: Drive the whole open spec list to merged as the operator's proxy — triage, sketch-gate, fan out implementers, fresh review per branch, batched integration, verified merge.
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
  - Edit
  - Write
  - Grep
  - Glob
  - Agent
  - SendMessage
  - TaskStop
  - PushNotification
---

# AIDA Orchestrate Skill

## Purpose

Run the **orchestrator seat as the operator's proxy**, from start to finish.
The objective is concrete: `aida list` becomes empty, with every item approved,
implemented, reviewed and merged to the default branch. Items that genuinely
need a human are reported, never faked, and deferring an item never counts as
finishing it.

You coordinate; you do not write the code. Specs are implemented by **separate
sessions in their own worktrees**. Every branch is checked by a **reviewer that
did not write it**. Approved branches land through **batched integration PRs**
that are merged on the exact commit and then verified. Each routine decision
the operator delegated to you is recorded on its spec as a `PROXY DECISION`
comment, so the audit trail stays in the substrate.

This skill composes existing surfaces rather than restating them:
- `aida list/add/edit/comment/defer/questions`
- `aida burndown plan` (the pickability gate)
- `aida worktree add` (takes the lease)
- `/aida-integrate` (the merge discipline)

## When to use

- The operator has **explicitly delegated decision authority** in their own
  words ("make decisions on my behalf", "be my proxy"). They then ask to drive
  the list to empty, work the backlog overnight, or set a `/goal` of "`aida list`
  is empty".
- Without that delegation, use `/aida-solo` or `/aida-triage` and ask.

## Skip if

- A warm interactive advisor/integrator loop with the operator in the room →
  `/aida-solo`.
- Fanning implementers out over the already-queued ready set, without triaging
  or approving drafts yourself → `/aida-burndown`.
- A headless, unattended `aida queue work --drain` from a `/goal` prompt →
  `/aida-drain-queue`.
- Only merging PRs that are already finished → `/aida-integrate`.
- Watching someone else's drain → `/aida-oversee`.
- One spec → `/aida-pickup`.

## Required capabilities

This skill needs four harness capabilities:
- a subagent launcher (Claude Code: `Agent`);
- a way to message a running subagent (`SendMessage`);
- Bash `run_in_background` jobs that notify when they exit;
- a way to stop a stuck job (`TaskStop`).

**Implementation and review must always run in separate sessions.** Without
subagents, launch them with `aida queue work <SPEC> --strict` (the spec must be
queued; `--strict` refuses to auto-queue an unsigned-off spec),
or with `aida agent new` told to push a branch and not open a PR. You can also
route review to another seat. If no second session is available, stop at
pushed branches and report. **Never review your own diff.**

## The loop

Repeat these steps until the list is empty or only human-gated items remain.

### 1. Read the board

```bash
aida session handoff --seat orchestrator --show   # resuming? read IN FLIGHT first
aida list --fields id,status,title
aida burndown plan --status approved --json       # ready / supervised / serialize_held / parked
aida awaiting
aida ps
```

Put each open item in one bucket:

| Bucket | Action |
|---|---|
| `draft` | Triage it (step 2). |
| `awaiting_signoff` in `burndown plan` (approved, not yet queued) | Sign off by queueing it (step 2), or run the sketch gate first if it is architecture-class. |
| `ready` in `burndown plan` | Dispatch an implementer (step 4), unless it is architecture-class and has no posted `ADVISOR SIGNOFF: APPROVED`. The sketch-gate row wins. |
| Approved but architecture, authority, autonomy or store-integrity class | Sketch gate first (step 3). |
| `supervised` / keystone | `/aida-guided-implement` with the operator, or report it. |
| `serialize_held`, `parked`, unmet blocked-by, pending decision | Report it. Don't dispatch. |
| `in-progress` | Check the handoff's IN FLIGHT list and `aida ps` **before** re-dispatching. Only an orphan with no live session is re-dispatched. |
| Needs humans, a live run, or an operator decision | Report it. Never fake it. |
| EPIC | A rollup; it closes when its children close. |

### 2. Triage drafts as the proxy

For each draft, decide, then record why with
`aida comment add <ID> "PROXY DECISION (orchestrator for <operator>): ..."`:

- **Concrete, testable, bounded** → `aida edit <ID> --status approved`, then
  `aida queue add <ID>`. Queueing is the sign-off that makes it `ready`; record
  both in the PROXY DECISION.
- **Exact duplicate** → `aida edit <ID> --status rejected`, and name the
  survivor in the comment.
- **Architecture-class** → approve it and send it through step 3 (the sketch
  gate). Queue it only after `ADVISOR SIGNOFF: APPROVED`. Never defer it just
  to clear it.
- **Needs something that hasn't happened yet** (real data over time, another
  spec merging first) → `aida defer <ID> --until "<concrete trigger>"`.
- **Changes authority** (roles, grants, human-at-TTY floors, merge-hold,
  unattended-run enablement, your own permissions) **or sets product direction**
  → leave it as a draft. Record the fork with
  `aida questions ask <ID> -q "<question>" -c "label|consequence|resolution" -c "..."`
  so it appears in `aida human` and `aida awaiting`, and report it.

If an approval is refused for lack of authority, don't work around it. Report
it.

### 3. Sketch gate (architecture-class specs)

An architecture change needs a sketch on the owning spec plus **independent**
advisor signoff before any code is written.

1. An architect session writes the sketch as a spec comment that begins
   `SKETCH (awaiting advisor signoff):`. It must cover:
   - owner and trigger;
   - surface;
   - races and fail-closed behaviour;
   - files, named by symbol;
   - named tests;
   - out-of-scope items;
   - open questions.

   No code at this stage.
2. A **different** advisor session verifies the sketch's claims against the
   code, answers the open questions, and posts either
   `ADVISOR SIGNOFF: APPROVED` (with binding amendments) or
   `CHANGES REQUESTED`.
3. The spec is implemented **as amended**; the advisor's text wins. If the
   advisor splits it into slices, build slice 1, file the rest as child tasks,
   and tag the parent `closure:pending`.

Advisors often find real bugs on main. File each one as its own spec, with
`blocked-by` edges.

### 4. Dispatch implementers (parallel)

Use one session per spec, each in a worktree that takes the implementer lease:

```bash
aida worktree add <SPEC> --path ../wt-<spec> --branch <agent>/<spec>
```

The implementer brief must include:
- Read `aida show <SPEC> --full` and the repo's CLAUDE.md. Work only in the
  worktree.
- Tests use fixtures only. Never touch the live store or real user files
  (`~/.aida`, the crontab, system timers); use a fake HOME.
- Put `trace:<SPEC> | ai:<tool>` markers in the language's plain line comment
  (`//`, `#`), **never** in a doc comment (`///`, docstrings), where it can
  leak into `--help` or generated docs. Keep spec IDs out of user-facing text.
- Run the project's full check set, the same checks CI runs, and list them
  explicitly.
- Wait for long commands in the foreground with a timeout. **Never** use
  `pgrep`/`until` polling loops.
- **Never run a `.py` file or Python snippet as a shell script.** Bash runs
  `import` as ImageMagick's screen-capture tool. Use `python3 file.py` or a
  `python3 -` heredoc.
- Commit as `[AI:<tool>] type(scope): summary (<SPEC>)`, push the branch, and
  set the spec to `done`. **Don't open a PR.**

For store, lock or authority code, use the strongest model available.

### 5. Fresh review of every branch

Every branch gets a reviewer that did not write it. Make the review **strict**
for:
- drain logic;
- merge-gate logic;
- store and cache writes;
- locks;
- authority and identity;
- anything that launches unattended work.

The review brief must include:
- the exact diff command;
- the spec and any signed-off sketch;
- the specific risks to probe, for example "can an agent run `<cmd>` and
  enable X by itself?" or "does a second `save()` drop edits?";
- to run the targeted tests in a scratch worktree, then remove it;
- the known flaky tests;
- to reply `VERDICT: APPROVE` or `VERDICT: REQUEST_CHANGES`. Only real
  correctness, integrity or safety problems count as blockers.

On `REQUEST_CHANGES`, send the findings back to the **same implementer** so it
keeps its context. Include a PROXY DECISION on any routine design question. Then
re-review with a **new** reviewer.

Any code you write yourself, even a small integration fix, is reviewed by
someone else.

On approval, run `aida comment add <SPEC> "Review: APPROVE (...)"` and queue the
spec for a batch. Non-blocking notes worth doing become approved follow-up
specs. Optional or data-gated notes become deferred specs with a trigger.

### 6. Batched integration

Put the approved branches into one integration branch based on
`origin/main`. Follow `/aida-integrate`'s merge discipline.

- **Stacked branches** (built on a parent that has since been squash-merged):
  first run `git rebase --onto origin/main <parent-sha> <branch>`. Afterwards,
  check that `git diff --stat origin/main HEAD` shows only that branch's own
  files, and that nothing already on main was reverted.
- Run the **project's full local check set** before pushing. For a failing
  test, decide whether it's known-flaky (re-run it single-threaded) or a real
  integration break.
- **Mechanical** integration fixes can be made inside the batch, for example a
  new struct field missing from a test fixture, or two branches bumping the
  same contract version. Each fix goes to a fresh reviewer before you push, and
  each is listed in the PR body.
- A conflict that turns on a **design choice** goes to the advisor or the
  operator. The proxy never resolves it.
- If main moved while the checks were running, merge main in and re-run the
  checks that could have broken.

### 7. PR, CI, verified merge

- Push the branch and open the PR with
  `gh pr create --base <default> --title "[AI:<tool>] chore(integrate): batch N - SPEC-A SPEC-B" --body "<spec list + each mechanical fix made in the batch>"`.
- Wait in a Bash `run_in_background` job, e.g.
  `gh pr checks N --required --watch --fail-fast` (in a repo with no required
  checks, use plain `--watch` and report that). Never poll from the model.
  On GitLab or other forges, use the equivalent, or `/aida-integrate`'s forge
  probes.
- Merge on the exact sha:
  `gh pr merge N --squash --match-head-commit <full-sha> --subject "... (#N)" --body "<one line per spec>"`.
  End a body line with `(SPEC-ID)` **only when this merge completes that spec**.
  For a slice, trailer the child task, never the parent.
- **Never** use `--admin`. Never disable or edit required checks or branch
  protection. If `merge-hold-gate` is red or a hold is set, stop and report.
- **Verify MERGED** with `gh pr view N --json state` before touching specs.
  After a network error, re-check instead of assuming.
- Run `aida pull` and confirm the specs auto-completed. `closure:pending` specs
  correctly stay at Done.
- Clean up the worktrees and branches. Keep any branch that a stacked
  follow-up still builds on.

### 8. Post-merge live actions

Some merges need a live action, such as a one-time data migration or repairing
an **already-enabled** scheduled job.

- If the action runs the project's own tool, rebuild or install the merged
  version first.
- Dry-run first, and proceed only if the result matches what reviewers
  predicted.
- Back up anything the user owns before touching it, and change only the lines
  your tool owns.
- Verify afterwards, and record the result on the spec.
- **Never** create or enable a scheduled or unattended run. Changes to
  user-owned files outside the repo need the operator's OK, unless the
  delegation covers them explicitly.

## Guardrails (non-negotiable)

- **Never merge as proxy a diff that changes authority.** That covers grants,
  roles, human-at-TTY floors, merge-hold, unattended-run enablement, and your
  own permissions. Push it, have it reviewed, and **leave the PR open for the
  operator**.
- **Never** force-push the default branch. Use `--force-with-lease` only on
  your own branches.
- **Never** bypass a human-at-TTY floor, and never use `--admin`. If an agent
  could flip a floor by itself, that is a **blocker bug**: file it.
- **Proxy authority is for routine calls, not a rubber stamp.** If a change has
  a high blast radius and the diff doesn't fully convince you, leave it
  unmerged and escalate.
- **Deferral never counts toward the goal.** List every deferred item and its
  trigger in each report.
- **No model-side polling.** A waiter running `pgrep -f "<pattern>"` can match
  its own command line and never exit. Stop stale waiters and read their
  output files directly.
- **Honest reporting.** Never fabricate data or mark undone work done. If an
  inherited artifact looks fabricated, reopen it with a CORRECTION comment.
- **Look before deleting.** Inspect a stray file first, and say why you removed
  it.
- **Exactly one orchestrator per repo.** If you find a second one, the later
  session stands down: it stops its duplicate agents and hands its items to the
  earlier one through the handoff and a direct message.

## Context and handoff

The loop is long. Keep subagent transcripts out of your context and rely on
their summaries. Refresh the handoff after every batch so a restart never loses
state:

```bash
aida session handoff --seat orchestrator --write - <<'EOF'
Goal; batch flow; MERGED; IN FLIGHT (spec, worktree, branch, head sha, review
state, PR/run ids); AWAITING OPERATOR; DEFERRED (+trigger); CLOSURE-PENDING
(+trigger); known flakes.
EOF
```

**Rotating out** (context ceiling or restart): dispatch nothing new; stop or let
finish every subagent, background shell and waiter until none is running; run
`/goal clear` if a goal is set; write the handoff **last**; then tell the
operator to exit with **Exit and stop tasks**, never "Move to background". A
backgrounded session keeps driving the loop. Find one with `claude agents`
and stop it with `claude stop <id>`.

**Taking over:** before acting on the handoff, check for a live predecessor
(`claude agents`, `ListAgents`, `aida ps`). If one is still running, message it
and ask the operator; do not start a second loop.

## Report to the operator

After each batch, report in plain language, leading with the outcome (use
`PushNotification`, if available, when the list drains or an item becomes
blocked on the operator):
- what merged and which specs completed;
- what's in review, and the real bugs reviewers found on main;
- the proxy decisions made;
- what was deferred, and why;
- what is blocked on the operator, and why.
