---
name: aida-orchestrate
description: Drive the whole open spec list to merged as the operator's proxy — triage, sketch-gate, dispatch implementers as separate AIDA sessions, fresh review per branch, batched integration, verified merge. Vendor-neutral (Codex, Antigravity, any harness with a shell).
disable-model-invocation: true
---

# AIDA Orchestrate Skill (vendor-neutral)

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

This is the vendor-neutral form of the skill. It keeps the same loop, gates and
guardrails as the Claude Code version, but it drives every other session
through AIDA's own CLI, so it works from Codex, Antigravity, or any harness that
can run shell commands. It needs no subagent tool from the harness.

This skill composes existing surfaces rather than restating them:
- `aida list/add/edit/comment/defer/questions`
- `aida burndown plan` (the pickability gate)
- `aida worktree add` (takes the lease)
- `aida queue work`, `aida agent new`, `aida ps`, `aida tail`, `aida brief`,
  `aida mailbox`, `aida session send` (sessions and messages)
- `.aida/discipline/integrator-role.md` and
  `.aida/discipline/git-sync-and-review.md` (the merge discipline)

## When to use

- The operator has **explicitly delegated decision authority** in their own
  words ("make decisions on my behalf", "be my proxy"). They then ask to drive
  the list to empty, work the backlog overnight, or set a goal of "`aida list`
  is empty".
- Without that delegation, triage with the operator and ask before deciding.

The `disable-model-invocation` line in this file's frontmatter is honoured only
by Claude Code. In Codex and Antigravity nothing stops this skill from being
loaded, so **the delegation rule above is the real gate**: without the
operator's explicit delegation in their own words, do not act as proxy.

## Skip if

- Fanning implementers out over the already-queued ready set, without triaging
  or approving drafts yourself → `aida queue work --drain`.
- Only merging PRs that are already finished → `aida queue integrate`.
- One spec → `aida queue work <SPEC>`.

## Session mechanics (how this harness does the work)

Implementation and review **always run in separate sessions**. You never
implement a spec in your own session, and **you never review your own diff**.
AIDA provides every mechanic you need.

| Need | Command |
|---|---|
| Start an implementer headless (no terminal needed; the spec must be queued) | `aida queue work <SPEC> --vendor <claude\|codex\|agy> --no-human --strict` |
| Start an interactive seat in another terminal | `aida agent new <claude\|codex\|antigravity> --role <implementer\|reviewer\|advisor> --spec <SPEC> --prompt "<brief>"` |
| Start a reviewer that holds no lease | `aida agent new <claude\|codex\|antigravity> --role reviewer --prompt "<review brief>"` |
| Hand a session its brief or findings | `aida brief <agent> <SPEC> --note - --notify` (the note is read from stdin) |
| Message a named agent | `aida mailbox send --to <agent> --subject "<SPEC> findings" --stdin` |
| Type into a live interactive session | `aida session send <session-or-SPEC> "<text>" --enter` |
| See what is running | `aida ps` (add `--json` for machine output), `aida agent ls` |
| Read what one session is doing | `aida tail <SPEC-or-session-id> --no-follow -n 40` |
| Stop a stuck session | `aida agent stop <name>`, else the harness's own job control or `kill <pid>` from `aida ps` |
| Close a finished session's worktree and lease | `aida session end <id> -y` |

Rules for these mechanics:

- **Background waits.** Run long commands (a headless `aida queue work`,
  `gh pr checks --watch`) as the harness's background jobs when it has jobs
  that report on exit. Otherwise use a **bounded foreground wait**, for
  example `timeout 540 gh pr checks N --required --watch --fail-fast`, and run
  it again if it times out (exit 124).
- **No model-side polling.** Never loop `sleep` + `aida ps`, or `pgrep` +
  `until`, from the model. A `pgrep -f "<pattern>"` waiter can match its own
  command line and never exit. Wait on the job, or read its output once it ends.
- **Several implementers at once.** A headless `aida queue work` blocks until
  its session exits. To run several in parallel without harness jobs, detach
  each one with its own log, for example
  `nohup aida queue work <SPEC> --vendor codex --no-human > .aida/orchestrate-<SPEC>.log 2>&1 &`,
  then follow progress with `aida ps` and `aida tail <SPEC>`.
- **Leases.** `aida agent new --spec <SPEC>` refuses while another session
  holds that spec's lease. Launch a reviewer **without** `--spec` (it holds no
  lease) and have it check the branch out in a scratch worktree, or end the
  implementer session first with `aida session end <id> -y`.
- **Ending sessions.** `aida session end` needs `-y` when there is no
  terminal to confirm on. It refuses while agent processes are still running
  inside the worktree, and while the worktree has uncommitted changes. Stop the
  session first (`aida agent stop <name>`); use `--force` only after you have
  checked that nothing in the worktree needs keeping.
- **Roles.** `aida agent new --role` accepts only `implementer`, `advisor`,
  `reviewer` and `integrator` (see `aida agent list-roles`).
- **Session context.** Only Claude sessions resume a conversation
  (`aida queue work <SPEC> --resume`). For Codex and Antigravity, a session
  that exited is re-started with the findings in its brief:
  `aida agent new <claude|codex|antigravity> --resume latest` reopens the latest ended matching
  session, where the vendor supports it.

### Vendor limits (state them honestly)

- **Antigravity (`agy`) refuses interactive launches from `aida queue work`
  and `aida review`.** `aida queue work --vendor agy` must be given
  `--no-human`, which runs it headless. Otherwise the launch stops before any
  lease or worktree is created. Route an interactive reviewer to `claude` or
  `codex` instead.
- **Antigravity has no guided mode.** `aida queue work --guided` refuses agy:
  by dispatch policy agy is fenced to draft-for-review, mechanical and bounded
  work. Send keystone, store, lock and authority specs, and every strict
  review, to `claude` or `codex`.
- **`aida agent new antigravity`** does start agy interactively, but in the
  terminal it runs in. From a harness shell with no terminal, use headless
  `aida queue work --no-human` instead, or ask the operator to run the command
  in another terminal.
- **Only Claude can detach an interactive seat** (`aida agent new claude --bg`).
  An `aida agent new codex` or `aida agent new antigravity` seat takes over the
  terminal it starts in.
- **`aida session send`** needs the target session to be in a supported
  terminal. If it reports no terminal adapter, use `aida brief ... --notify` or
  the mailbox.
- **Headless `aida queue work` always opens a PR.** It runs the standard pickup
  flow, which ends with the PR step, so the "don't open a PR" line in the brief
  does not hold for these sessions. Treat that PR as follows:
  - The branch still gets a fresh review (step 5). An implementer's PR is
    never evidence of review.
  - When the branch is folded into the integration branch, close the
    implementer's PR with a note that links the batch PR
    (`gh pr close N --comment "Folded into batch PR #M"`). Never merge it on
    its own.
  - If you use that PR as the batch PR instead, every step 7 gate still
    applies: required checks, exact-sha merge, no `--admin`, verify MERGED.
  - Do not run `aida queue integrate` alongside this orchestrator on those
    specs. It would drive the same PRs to merge on its own, outside your
    batches and review gates.
- If only one session is available (no second vendor, no second terminal, no
  headless launch), stop at pushed branches and report. Never review your own
  diff.

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
| `supervised` / keystone | A guided session with the operator (`aida queue work <SPEC> --guided --vendor claude\|codex`), or report it. |
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

1. A sketch-author session writes the sketch as a spec comment that begins
   `SKETCH (awaiting advisor signoff):`. Launch it as its own session, for
   example `aida agent new codex --role advisor --prompt "<sketch brief: sketch only, no code>"`.
   The sketch must cover:
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
   `CHANGES REQUESTED`. Launch it as a new session (a different vendor is
   better), never the sketch author and never yourself.
3. The spec is implemented **as amended**; the advisor's text wins. If the
   advisor splits it into slices, build slice 1, file the rest as child tasks,
   and tag the parent `closure:pending`.

Advisors often find real bugs on main. File each one as its own spec, with
`blocked-by` edges.

### 4. Dispatch implementers (parallel)

Use one session per spec, each in its own worktree with the implementer lease.
First write the brief so the session reads it on pickup:

```bash
aida brief <agent> <SPEC> --note - <<'EOF'
<implementer brief, below>
EOF
```

Then launch the session. Pick the vendor per spec.

`aida queue work` works only on a **queued** spec. In this skill, queueing is
the sign-off (step 2), so dispatch only specs you have queued with
`aida queue add <SPEC>`. Without `--strict`, `aida queue work` silently queues
an approved but unqueued spec itself, which would skip that sign-off, so
always pass `--strict`.

```bash
# headless, no terminal needed (the spec must already be queued)
aida queue work <SPEC> --vendor codex --no-human --strict
# or an interactive seat in another terminal; --spec creates the worktree and lease
aida agent new claude --role implementer --spec <SPEC> --prompt "<short pickup line>"
```

When you need the worktree before a session exists, `aida worktree add <SPEC>
--path ../wt-<spec> --branch <agent>/<spec>` takes the lease directly.

The implementer brief must include:
- Read `aida show <SPEC> --full` and the repo's agent instructions (AGENTS.md
  or CLAUDE.md). Work only in the worktree.
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

For store, lock or authority code, use the strongest model and vendor
available (`--model` on `aida queue work`), never agy.

Watch the fleet with `aida ps` and `aida tail <SPEC> --no-follow -n 40`. A
session that makes no progress is stopped with `aida agent stop <name>` or the
harness's job control, and the spec is re-dispatched only after `aida ps`
shows no live session for it.

### 5. Fresh review of every branch

Every branch gets a reviewer that did not write it: a **new** session, never
the implementer and never you. Launch it without `--spec`, so it does not
collide with the implementer's lease:

```bash
aida agent new codex --role reviewer --prompt "$(cat review-brief.md)"
```

Make the review **strict** for:
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
- to reply `VERDICT: APPROVE` or `VERDICT: REQUEST_CHANGES`, and to record it
  with `aida review record <SPEC> --verdict approved|request-changes --sha <full-sha> --branch <branch> --summary "<one line>"`.
  Only real correctness, integrity or safety problems count as blockers.

On `REQUEST_CHANGES`, send the findings back to the implementer. Include a
PROXY DECISION on any routine design question. Then re-review with a **new**
reviewer.

- **Implementer session still live:** `aida session send <SPEC> "<findings>" --enter`,
  or `aida brief <agent> <SPEC> --note - --notify`, or
  `aida mailbox send --to <agent> --stdin`. It keeps its context.
- **Implementer session has exited** (always the case for headless runs):
  re-drive the spec on the same branch.

  ```bash
  aida queue rework <SPEC> --reason "<findings>"   # records the findings, re-queues the spec
  aida queue work <SPEC> --vendor codex --no-human --strict --branch <implementer-branch>
  ```

  Use `--vendor codex` or `--vendor agy` (agy only for bounded, non-strict
  work). Pass `--branch` with the implementer's existing branch. Without it
  the new session does **not** reliably reuse that branch: `aida queue work`
  reuses an open PR's branch only when the spec is still In Progress or Done,
  and `aida queue rework` moves a Done spec back to Approved. The new session
  would otherwise fork a fresh branch from `origin/main` without the earlier
  commits. The new session starts cold; the findings reach it through the
  spec comment that `--reason` writes. For Claude only, `--resume` continues
  the earlier conversation instead.

Any code you write yourself, even a small integration fix, is reviewed by
someone else.

On approval, run `aida comment add <SPEC> "Review: APPROVE (...)"` and queue the
spec for a batch. Non-blocking notes worth doing become approved follow-up
specs. Optional or data-gated notes become deferred specs with a trigger.

### 6. Batched integration

Put the approved branches into one integration branch based on
`origin/main`. Follow the merge discipline in
`.aida/discipline/integrator-role.md`.

- **Stacked branches** (built on a parent that has since been squash-merged):
  first run `git rebase --onto origin/main <parent-sha> <branch>`. Afterwards,
  check that `git diff --stat origin/main HEAD` shows only that branch's own
  files, and that nothing already on main was reverted.
- Run the **project's full local check set** before pushing. For a failing
  test, decide whether it's known-flaky (re-run it single-threaded) or a real
  integration break.
- **Mechanical** integration fixes can be made inside the batch, for example a
  new struct field missing from a test fixture, or two branches bumping the
  same contract version. Each fix goes to a fresh reviewer session before you
  push, and each is listed in the PR body.
- A conflict that turns on a **design choice** goes to the advisor or the
  operator. The proxy never resolves it.
- If main moved while the checks were running, merge main in and re-run the
  checks that could have broken.

### 7. PR, CI, verified merge

- Push the branch and open the PR with
  `gh pr create --base <default> --title "[AI:<tool>] chore(integrate): batch N - SPEC-A SPEC-B" --body "<spec list + each mechanical fix made in the batch>"`.
- Wait for CI as a harness background job, or as a bounded foreground wait:
  `timeout 540 gh pr checks N --required --watch --fail-fast`, re-run on exit
  124. In a repo with no required checks, use plain `--watch` and report that.
  Never poll from the model. On GitLab or other forges, use the equivalent
  forge CLI.
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
- Clean up with `aida session end <id> -y` for each finished session, and remove
  merged branches. Keep any branch that a stacked follow-up still builds on.

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

- **Implementation and review always run in separate sessions. Never review
  your own diff.**
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
  State vendor limits as they are; never claim a session ran when it was
  refused.
- **Look before deleting.** Inspect a stray file first, and say why you removed
  it.

## Context and handoff

The loop is long. Keep other sessions' transcripts out of your context: read
`aida tail <SPEC> --no-follow -n 40` or their final report, not the whole log.
Refresh the handoff after every batch so a restart never loses state:

```bash
aida session handoff --seat orchestrator --write - <<'EOF'
Goal; batch flow; MERGED; IN FLIGHT (spec, worktree, branch, session, vendor, review state);
AWAITING OPERATOR; DEFERRED (+trigger); CLOSURE-PENDING (+trigger); known flakes.
EOF
```

## Report to the operator

After each batch, report in plain language, leading with the outcome (use the
harness's notification, if it has one, when the list drains or an item becomes
blocked on the operator):
- what merged and which specs completed;
- what's in review, and the real bugs reviewers found on main;
- the proxy decisions made;
- what was deferred, and why;
- what is blocked on the operator, and why;
- any vendor limit that changed the plan (for example an agy launch refused
  interactively and re-routed).
