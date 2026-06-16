---
name: aida-backlog-groom
description: Guided burndown-prep — one advisor pass from approvable drafts and approved-but-not-queued specs to a blessed, drain-ready queue. Surfaces both buckets, keeps approval an explicit human judgment (never auto-approve), then grooms the chosen set onto the queue with risk + file-overlap heuristics.
allowed-tools:
  - Bash
  - Read
  - Grep
  - Glob
---
<!-- AIDA Generated: v2.0.0 | checksum:219daeb2 | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


# AIDA Backlog Groom Skill

## Purpose

Get from a raw backlog to a **blessed, drain-ready queue in one sitting**.
The pass starts one step earlier than plain grooming: it surfaces both the
*approvable drafts* (the triage inbox) and the *approved-but-not-queued*
specs, keeps **approval as an explicit advisor judgment** (NEVER
auto-approved — ADR-3 / STORY-546), then grooms the chosen approved set
onto the queue *with intent* — which items are safe to drain as a batch,
which would step on each other's toes in parallel, and which deserve a
single-spec session of their own.

The CLI (`aida list`, `aida burndown plan --candidates`,
`aida backlog list` / `analyze` / `groom`, `aida questions`) does the
heavy lifting. This skill drives the **interactive selection** — the
approve/defer judgment and the cluster shape — the part Claude is good at
and the CLI deliberately is not. It is **pure composition**: triage +
backlog candidates + the existing `aida backlog groom`. No new approval
path, no new queue path.

## When to use

- You're about to start a burndown / overnight drain and want to bless a
  set first — *"prep the backlog"*, *"set me up to burn down"*,
  *"what can I queue for tonight?"*.
- Drafts have piled up and you want one pass that both triages them and
  queues the ready work.
- The Approved pile has grown past what you can scan by eye (often
  ~10+ items).
- You want a `batch:<NAME>` to feed `aida queue work --batch NAME
  --auto-complete` (single drain, many specs).
- The user says *"groom the backlog"*, *"what should I queue?"*, or
  *"pick a few low-risk items"*.

## Skip if

- The queue is already what you intended — don't enqueue for its own sake.
- The user is asking about a specific spec (use `/aida-pickup` or
  `aida queue add <ID>` directly).
- The user only wants to *file* new requirements (use `/aida-req` —
  grooming is downstream of capture).

## Guided burndown-prep pass (start here)

This is the one-sitting flow: surface → approve → groom → name the drain.
Steps 1–6 below are the grooming machinery; this section frames the two
extra surfacing+approval steps that turn a raw backlog into a blessed
queue. Run it top-to-bottom for a full prep; jump straight to Step 1 if
everything you care about is already Approved.

### Prep-A: Surface both buckets in one view

Show the advisor *everything that could move forward*, in two clearly
labelled buckets:

```bash
# Bucket (a) — DRAFTS the advisor could approve (the triage inbox)
aida list draft

# Bucket (b) — APPROVED but not yet queued (ready vs awaiting sign-off)
aida burndown plan --candidates          # curation view: approved + pickable, not queued
aida backlog list                        # the groomable set, with risk chips
```

`aida burndown plan --candidates` is the "what could I bless next" aid —
read-only, never auto-queues. `aida backlog list` adds the risk chip per
spec. Render the two buckets as a short combined table (drafts first, then
approved-not-queued), not a wall of output.

### Prep-B: Approve / select per item — NEVER auto-approve

Approval is an **explicit advisor (human) judgment**, one item at a time
(or an explicit multi-select). Do **not** bulk-approve to clear the
inbox.

For each **draft** in bucket (a), decide:

- **Approvable now** — the spec is clear and pickable. Promote it
  explicitly:

  ```bash
  aida edit <DRAFT-ID> --status approved
  ```

- **Needs a human decision first** — there's a real fork, a missing
  acceptance criterion, or an open question. Do **not** approve it.
  Route it to the decision inbox instead so it's parked, visible, and
  answered outside any agent session:

  ```bash
  aida questions sweep --apply          # attach DecisionRequests to drafts that need one
  aida questions ask <DRAFT-ID> ...      # pose a specific structured question
  aida questions clarify <DRAFT-ID>      # interactive: author acceptance criteria
  ```

  Drafts parked in `aida questions` stay out of the queue until a human
  answers — they are **not** approved by this pass.

The output of Prep-B is a concrete set of **Approved** spec IDs you (the
advisor) chose to move forward. That set is what Step 1 onward grooms.

## Grooming workflow

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

### Step 6: Show what landed, then name the drain

```bash
aida queue list --batch overnight-2026-05-24
```

Tells the user what's now drain-ready. **Always end by naming the exact
drain command for the freshly-blessed set** — don't leave the operator to
reconstruct it. With a batch:

```bash
aida queue work --batch overnight-2026-05-24 --auto-complete
```

or, for an autonomous overnight run:

```bash
aida queue work --batch overnight-2026-05-24 --auto-complete --no-human
```

If you groomed without a `--batch` tag, name the per-spec drain or the
skill instead:

```bash
aida queue work <SPEC> --auto-complete    # one blessed spec
/aida-burndown                            # drive the whole blessed queue down
```

That naming is the hand-off — the prep pass is done the moment the
operator has a copy-paste drain command for exactly the set they just
blessed.

## Two traps to watch

- **Every command in this skill is a real shell line.** No invented
  flags. If you want a feature `aida backlog` doesn't expose yet, file
  a TASK — don't fake the flag in the prose.
- **Risk chips are advisory.** Do not refuse to groom a spec because
  the heuristic painted it `high` — the heuristic is wrong sometimes
  and the operator is the deciding party. Surface the chip, surface
  *why*, let them choose.

## Related skills / commands

- `/aida-burndown` — drive an autonomous burndown over the queue this
  pass just blessed (the natural next step)
- `/aida-clarify` — the interactive complement to `aida questions sweep`:
  author acceptance criteria for the under-specified drafts this pass
  parked instead of approving
- `/aida-pickup` — drain a single queued spec (the consumer side of this
  skill's output)
- `/aida-drain-queue` — drive an autonomous chain over what you just
  groomed
- `aida queue work --batch NAME` — the natural pairing on the
  consumer side
- `aida burndown plan --candidates` — the read-only "what could I bless
  next" view that opens the prep pass
- `docs/aida/discipline/backlog-grooming.md` — the discipline doc on
  what "backlog" means in AIDA and how the heuristics work

trace:STORY-558 | ai:claude