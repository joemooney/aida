---
name: aida-orchestrate
description: Drive the whole open spec list to merged as the operator's proxy — triage, sketch-gate, fan out implementers, fresh review per branch, batched integration, verified merge.
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
  - Edit
  - Write
  - Agent
  - SendMessage
  - TaskStop
---

# AIDA Orchestrate Skill

## Purpose

Run the **orchestrator seat as the operator's proxy**, from start to finish.
The objective is concrete: `aida list` becomes empty, and every item on it has
been approved, implemented, reviewed and merged to the default branch. The one
exception is an item that genuinely needs a human; those are reported, never
faked.

You coordinate; you do not write the code. Specs are implemented by **subagents
in their own worktrees**, each branch is checked by a **fresh reviewer that did
not write it**, and approved branches land through **batched integration PRs**
that pass exactly one CI run each. Decisions the operator delegated to you are
recorded on the spec as `PROXY DECISION` comments, so the audit trail stays in
the substrate.

This skill composes existing surfaces rather than re-implementing them:
`aida list/add/edit/comment/defer/rel`, `aida worktree`, the forge gates
(`merge-hold-gate`, required CI), and the integrator recipe in `/aida-integrate`.

## When to use

- "Drive the list to empty", "be my proxy overnight", "work the whole backlog",
  or a `/goal` whose condition is `aida list` being empty.
- The operator has explicitly delegated decision authority to you, in words.
  Examples: "make decisions on my behalf", "be my proxy". Without that
  delegation, use `/aida-triage` and ask.

## Skip if

- One spec only → `/aida-pickup` or `/aida-implement`.
- A headless, unattended drain with no interactive coordinator → `/aida-drain-queue`
  (it runs `aida queue work --drain` from a `/goal` prompt).
- Merging PRs that are already finished → `/aida-integrate`.
- Watching a drain someone else runs → `/aida-oversee`.

## Required capabilities

This skill needs four harness capabilities:
- a subagent launcher (Claude Code: `Agent`);
- a way to message a running subagent (`SendMessage`);
- background shell jobs that notify on exit (`run_in_background`);
- a way to stop a stuck job (`TaskStop`).

On a harness missing any of these, do the same loop sequentially yourself, one
spec at a time. The gates and guardrails below still apply unchanged.

## The loop

Repeat these steps until the list is empty or only human-gated items remain.

### 1. Read the board

```bash
aida list --fields id,status,title        # open lens
aida awaiting                              # what is waiting on you
aida ps                                    # live sessions / orphaned in-progress
```

Classify each open item into one of these buckets:

| Bucket | Action |
|---|---|
| `draft` | Triage it (step 2). |
| `approved`, small and bounded | Dispatch an implementer (step 4). |
| `approved`, architecture, authority, autonomy or store-integrity | Sketch gate first (step 3). |
| `in-progress` with no live session | Resume or re-dispatch it. |
| Needs humans, a live run, or an operator decision | Report it. Never fake it. |
| EPIC | A rollup; it closes when its children close. |

### 2. Triage drafts as the proxy

For each draft, decide, then record the reason with
`aida comment add <ID> "PROXY DECISION (orchestrator for <operator>): ..."`:

- **Concrete, testable, bounded** → `aida edit <ID> --status approved`.
- **Exact duplicate** → reject it and name the survivor.
- **Needs a design sketch, or its trigger hasn't happened yet** →
  `aida defer <ID> --until "<concrete trigger>"`. A deferral is only as good
  as its trigger, so name the event that would un-defer it.
- **Changes how the operator or agents hold authority** (roles, grants,
  floors), **or sets product direction** (vision, positioning) → leave it as
  `draft`, tag it `needs-operator`, and report it. Do not approve your own
  authority.

If an approval is refused for lack of authority, do not work around it. Leave
the item as it is and report it.

### 3. Sketch gate (architecture-class specs)

The rule is that an architecture change needs a sketch on the owning spec plus
**independent** advisor signoff before any code.

1. Dispatch an architect subagent. It writes the sketch as a spec comment that
   starts with `SKETCH (awaiting advisor signoff):` and covers:
   - owner and trigger
   - surface
   - races and fail-closed behaviour
   - files by symbol
   - named tests
   - out-of-scope items
   - open questions for the advisor

   It writes no code.
2. Dispatch a **separate** advisor subagent. It verifies the sketch's claims
   against the code, answers the open questions, and posts
   `ADVISOR SIGNOFF: APPROVED` (with binding amendments) or
   `CHANGES REQUESTED`.
3. The implementer then builds the sketch **as amended**; the advisor's text
   wins. If the advisor splits the work into slices, build slice 1, file the
   rest as child tasks, and tag the parent `closure:pending`.

Expect advisors to find real bugs on main. When they do, file each one as its
own spec and add `blocked-by` edges.

### 4. Dispatch implementers (parallel)

Give each spec one subagent and one worktree:

```bash
git fetch -q origin && git worktree add ../wt-<spec> -b <agent>/<spec> origin/main
aida edit <SPEC> --status in-progress
```

The implementer brief must include:
- Read `aida show <SPEC> --full` and CLAUDE.md. Work only in the worktree.
- Fixtures only in tests. Never touch the live store or real user files such as
  `~/.aida` or the crontab; use a fake HOME.
- Add `// trace:<SPEC> | ai:<tool>` markers in plain `//` comments, never `///`
  doc comments, which leak into `--help`. Keep SPEC IDs out of user-facing text.
- Run the project's full check set: fmt, lint, the test suites, docs and
  interface checks. List them explicitly in the brief.
- Wait in the foreground with a timeout. **No `pgrep`/`until` polling loops.**
- Run Python only as `python3 file.py` or a `python3 -` heredoc. **Never
  through bash**: `import math` would run ImageMagick and capture the screen.
- Commit as `[AI:<tool>] type(scope): summary (<SPEC>)`, push the branch, and
  set the spec to `done`. **Do not open a PR.**

For store, lock or authority code, use the strongest model available.

### 5. Fresh review of every branch

Give every branch a reviewer that did not write it. Use a **strict** reviewer
for:
- drain logic;
- merge-gate logic;
- store and cache writes;
- locks;
- authority and identity;
- anything that launches unattended work.

The review brief must include:
- The exact diff command.
- The spec and any signed-off sketch.
- The specific risks to probe. Be concrete, for example: "can an agent run
  `<cmd>` and enable X by itself?" or "does a second `save()` drop edits?"
- Instructions to run the targeted tests in a scratch worktree, then remove it.
- The known pre-existing flaky tests, so the reviewer can separate them from
  regressions.
- An instruction to reply `VERDICT: APPROVE` or `VERDICT: REQUEST_CHANGES`.
  Only real correctness, integrity or safety problems count as blockers.

On `REQUEST_CHANGES`, send the findings back to the **same implementer** with
`SendMessage`, which keeps its context. Include a PROXY DECISION for any open
design question. Re-review with a **new** reviewer.

If you write a fix yourself, for example a small integration fix, someone else
still reviews it.

When approved, record `aida comment add <SPEC> "Review: APPROVE (...)"` and
queue the spec for a batch. Non-blocking notes worth doing become approved
follow-up specs. Notes that are optional or data-gated become deferred specs
with triggers.

### 6. Batched integration

Put the approved branches into one integration branch, with one CI run per
batch:

```bash
git worktree add ../wt-intN -b <agent>/integrate-N origin/main
cd ../wt-intN && for b in ...; do git -c rerere.enabled=false merge --no-edit -q origin/<branch>; done
```

- Resolve conflicts hunk by hunk.
  - Test-module lists and changelog-style lists: keep both sides.
  - A branch built on a squash-merged parent: use `-X theirs` scoped to that
    merge, then **verify** that `git diff --stat origin/main HEAD` shows only
    the new branch's own files, and that nothing already on main was reverted.
- Run the project's full local preflight, the same checks CI runs.
  - For each failure, decide whether it is known-flaky (re-run it
    single-threaded) or a real integration break.
  - Semantic conflicts between separately approved branches are real. Examples:
    a new struct field that another branch's test fixture doesn't set, or two
    branches bumping the same contract version. Fix them in the batch and say so.
- If main moved during preflight, merge main in and re-run the checks that
  could break.

### 7. PR, CI and verified merge

```bash
git push -u origin <agent>/integrate-N
gh pr create --base main --head <agent>/integrate-N --title "... batch N - SPEC-A SPEC-B" --body "..."
```

- **Wait in a background shell** that polls the exact head sha's required
  checks (for example `gh api repos/O/R/commits/$sha/check-runs`) and exits on
  success, failure or cancellation. Guard against network errors. Never poll
  from the model.
- **Merge on the exact sha:**
  `gh pr merge N --squash --match-head-commit <full-sha> --subject "... (#N)"`,
  with one body line per spec that ends in `(SPEC-ID)`.
- **Verify MERGED** with `gh pr view N --json state` before touching any spec.
  After a network error, re-check the state. Never assume.
- Run `aida pull`, and confirm each spec auto-completed. `closure:pending` specs
  correctly stay at Done.
- Clean up: remove the worktrees, delete the local and remote branches, and keep
  any branch a stacked follow-up still depends on.

### 8. Post-merge operational steps

Some merges need a live action, for example a one-time data migration or
repairing an installed cron or timer.

- Rebuild the release binary.
- Dry-run first. Proceed only if the result matches what reviewers predicted.
- Back up anything user-owned before touching it, for example
  `crontab -l > backup`.
- Change only the lines your tool owns.
- Verify afterwards, then record what you did on the spec.

## Guardrails (non-negotiable)

- **Never** force-push the default branch. Only use `--force-with-lease` on
  your own branches.
- **Never** bypass a human-at-TTY floor: merge-hold clear, enabling unattended
  runs, issuing grants. If an agent could flip one of these itself, that is a
  **blocker bug**. File it.
- **Never** approve work that changes your own authority.
- **No model-side polling.** Use notify-on-exit background jobs. A waiter
  running `pgrep -f "<pattern>"` can match its own command line and never exit.
  Stop stale waiters (`TaskStop`) and read the outputs directly.
- **No mail storms** to agents that don't read the mailbox.
- **Honest reporting.** If a goal needs real participants, a live measurement,
  a time window, or an operator decision, say so plainly and list those items.
  Never fabricate data, and never mark work done that isn't done. If an
  inherited artifact looks fabricated, reopen it with a CORRECTION comment.
- **Check before deleting.** Look at a stray file before removing it, and say
  why you removed it.

## Context economy and handoff

This loop is long. Keep subagent results out of your own context by relying on
their summaries. At the seat's context ceiling, write a handoff note and let a
fresh session continue:

```bash
aida session handoff --seat orchestrator --write - <<'EOF'
Goal, batch flow, MERGED list, IN FLIGHT (worktree + branch + review state),
AWAITING OPERATOR, CLOSURE-PENDING (with the exact trigger), known flakes.
EOF
```

Refresh it after every batch, so a restart never loses the state.

## Report to the operator

After each batch, give a short plain-language update:
- what merged, and which specs completed;
- what's in review, and anything reviewers found. Name real bugs found on main;
- any proxy decisions made;
- what is blocked on the operator, and why.

Lead with the outcome, and use the operator's vocabulary rather than your
internal labels.
