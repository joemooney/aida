---
name: aida-backlog-groom
description: Curate Approved-but-not-queued work into the queue, surfacing risk chips and pairwise file-overlap so low-risk non-conflicting items can drain in parallel.
allowed-tools:
  - Bash
  - Read
  - Grep
  - Glob
---

# AIDA Backlog Groom Skill

## Purpose

Move work out of the Approved backlog and onto the queue *with intent* —
which items are safe to drain as a batch, which would step on each
other's toes if run in parallel, and which deserve a single-spec session
of their own.

The CLI (`aida backlog list` / `analyze` / `groom`) does the heavy
lifting. This skill drives the **interactive selection** — the part Claude
is good at and the CLI deliberately is not.

## When to use

- The Approved pile has grown past what you can scan by eye (often
  ~10+ items).
- You want a `batch:<NAME>` to feed `aida queue work --batch NAME
  --auto-complete` (single drain, many specs).
- You're spinning up an overnight drain and need to triage what's
  safe to chain.
- The user says *"groom the backlog"*, *"what should I queue?"*, or
  *"pick a few low-risk items"*.

## Skip if

- The queue is already what you intended — don't enqueue for its own sake.
- The user is asking about a specific spec (use `/aida-pickup` or
  `aida queue add <ID>` directly).
- The user is in `advisor` mode and only wants to *file* requirements
  (use `/aida-req` — grooming is downstream of capture).

## Workflow

### Step 1: Inventory the backlog

```bash
aida backlog list --json
```

The default view returns up to 50 Approved-but-not-queued specs, sorted
by priority desc then created_at asc, each with:

- `spec_id`, `title`, `req_type`, `priority`
- `risk`: one of `low` / `medium` / `high` / `unknown` (advisory only —
  the heuristic is hints, not gates)
- `tags`: every tag set on the spec

Useful filters (each maps directly to a CLI flag — none are skill-only):

```bash
aida backlog list --risk low                # cheapest pile
aida backlog list --type task --tag papercut
aida backlog list --tag-prefix lifecycle:   # everything trivial-tagged
aida backlog list --limit 100               # raise the cap for big backlogs
```

### Step 2: Spot the cluster shape

Read the candidate list. Look for:

- **Clusters of low-risk task/doc items** that *probably* don't conflict
  — these are the natural cheap batch.
- **High-priority or `BlockedBy`-marked items** — these belong in their
  own session, not bulk-queued.
- **Plan-owned items** (`risk: medium` because a `docs/plans/` file owns
  them) — the plan already names the blast radius; treat them as
  intentional pickups, not batch fodder.

Risk chips are heuristic — don't refuse a spec because it's `unknown`.
The chip is a *hint about effort*; the operator decides.

### Step 3: Analyze pairwise file overlap

Once you have a short-list of, say, 5-12 candidates:

```bash
aida backlog analyze --specs SPEC-A,SPEC-B,SPEC-C,... --json
```

The output is a `pairs[]` array with `a`, `b`, `verdict`, and
`shared_files`. Verdicts:

- **`safe-parallel`** — disjoint trace-comment + plan-file sets;
  safe to run on parallel branches.
- **`serialize`** — at least one file is shared (`shared_files` lists
  them); merging both at once would conflict. Run one, ship, then the
  other.
- **`unknown`** — neither spec has trace comments or a plan file.
  Treat as **serialize-by-default** — the absence of signals is not a
  green light.

A clean `safe-parallel` cluster is the textbook batch candidate.

### Step 4: Present the selection to the user

Render a short table — *not* a wall of JSON — calling out:

- The cluster you're proposing
- Each item's risk chip + one-line title
- The pairwise verdicts (`safe-parallel` ⨯ N, `serialize` ⨯ M)
- Any `unknown` items you're treating as serialize

Then use `AskUserQuestion` to let the user multi-select which items to
groom and (separately) what `batch:NAME` tag to apply.

Defaults that usually fit:

- `batch:low-risk-cleanup` — generic cleanup drain
- `batch:overnight-YYYY-MM-DD` — date-stamped autonomous drain
- *(no batch)* — items keep their identity; the operator drains them
  individually with `aida queue work <SPEC>`

### Step 5: Groom

```bash
aida backlog groom --specs SPEC-A,SPEC-B,... --batch overnight-2026-05-24
```

`--dry-run` is your friend before the real run — it prints the would-be
queue insertions and tag applications without writing.

The CLI **refuses** a groom that touches a spec that is:

- Not Approved (you'd have to `aida edit <ID> --status approved` first)
- Already on someone's queue
- Archived

Refusal is the whole list at once — no half-applied state.

### Step 6: Show what landed

```bash
aida queue list --batch overnight-2026-05-24
```

Tells the user what's now drain-ready. The natural follow-up:

```bash
aida queue work --batch overnight-2026-05-24 --auto-complete
```

or, for an autonomous overnight run:

```bash
aida queue work --batch overnight-2026-05-24 --auto-complete --no-human
```

## Two traps to watch

- **Every command in this skill is a real shell line.** No invented
  flags. If you want a feature `aida backlog` doesn't expose yet, file
  a TASK — don't fake the flag in the prose.
- **Risk chips are advisory.** Do not refuse to groom a spec because
  the heuristic painted it `high` — the heuristic is wrong sometimes
  and the operator is the deciding party. Surface the chip, surface
  *why*, let them choose.

## Related skills / commands

- `/aida-pickup` — drain a queued spec (the consumer side of this
  skill's output)
- `/aida-drain-queue` — drive an autonomous chain over what you just
  groomed
- `aida queue work --batch NAME` — the natural pairing on the
  consumer side
- `docs/aida/discipline/backlog-grooming.md` — the discipline doc on
  what "backlog" means in AIDA and how the heuristics work
