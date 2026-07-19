# Building your first project with AIDA

<!-- trace:TASK-277 | ai:claude -->

A follow-along walkthrough. We take a small **TODO CLI** from a blank repo to a
merged, traceable feature graph — six specs filed, two driven all the way to
*Completed*, in a single sitting.

> **Time:** ~15 min to read · ~1–2 hours to follow along on your own machine.
> **You'll need:** the `aida` binary on your PATH ([install](getting-started.md)),
> `git`, and [Claude Code](https://claude.com/claude-code).
>
> No Claude Code? You can still run every `aida` command in this doc — just
> write the code by hand at the two points where the walkthrough hands off to a
> Claude session, and commit with the [commit format](#a-note-on-commits) yourself.

## Why this doc exists

[`getting-started.md`](getting-started.md) installs AIDA and files one
requirement. The README's [5-minute walkthrough](../README.md#getting-started-in-5-minutes)
carries *one* spec end to end so you can see the lifecycle.

This doc is the next rung up: a **whole tiny project**. Not one spec — a real
parent/child graph of six, with two features driven through implement → PR →
review → merge → Completed. The point is to see what AIDA is *for*, not just
what each command does. The commands are simple; the value is in what they
accumulate.

The project is a **TODO CLI**. Pick whatever language you like — the `aida`
side behaves identically whether Claude writes Rust, Python, or Go. AIDA never
touches your source except for the one-line `trace:` comments you'll see below.

## The shape of the project

Six specs, one epic:

```
EPIC-1   TODO CLI
├── STORY-1   Add and list tasks
├── STORY-2   Mark a task done
├── STORY-3   Persist tasks to a JSON file
├── STORY-4   Filter tasks by status and tag
└── BUG-1     Listing crashes on an empty data file   ── references ──▶ STORY-3
```

We file all six up front, then drive **STORY-1** and **STORY-2** through the
full lifecycle. STORY-3, STORY-4, and BUG-1 stay queued — that's your project
to finish after the walkthrough ends.

> **A note on the ids you'll see.** AIDA numbers specs from a single shared
> sequence, not per-type — so when you file the epic first and four stories next,
> your real ids come out `EPIC-1, STORY-2, STORY-3, STORY-4, STORY-5`, and the bug
> is `BUG-6`. The diagram above uses the tidy `STORY-1..4 / BUG-1` names for
> readability; substitute your own ids as you follow along (the *shape* of the
> graph is identical either way).

---

## Step 0 — Initialize

Make a repo and turn on AIDA.

```
$ mkdir todo-cli && cd todo-cli
$ git init
Initialized empty Git repository in ~/todo-cli/.git/

$ aida init
✓ orphan branch aida-store + worktree .aida-store/
✓ .aida/config.toml · .aida/cache.db
✓ seeded META requirements
✓ scaffolded CLAUDE.md · AGENTS.md · .claude/ · .mcp.json · docs/plans/
AIDA initialized — distributed git-canonical mode.
```

One command set up a git-canonical requirements store (one YAML file per spec
on the orphan `aida-store` branch), a SQLite read cache, the MCP server config,
and the Claude Code skills. The full inventory of what `aida init` writes is in
[`getting-started.md`](getting-started.md#step-2-initialize-your-project).

---

<!-- trace:BUG-597 -->

> **A note on roles before you start.** A fresh `aida init` seats you as the
> **implementer** role — the seat that files and *implements* work. Two things in
> this walkthrough — promoting a spec to *Approved* (Steps 1–3) and routing it to
> a queue (Step 5) — are gated to the **advisor** role (or an interactive
> session); the implementer can't do them. Since you're driving this whole
> project solo, you wear both hats: prefix those commands with
> `AIDA_SESSION_ROLE=advisor` and they go through. Every command below that needs
> it is shown with the prefix already in place. (The prefix also prints a
> one-line `ℹ You're operating as advisor…` reminder — harmless, just
> informational; it's elided from the sample output below.)

## Step 1 — File the epic

Everything in AIDA starts as a spec. The epic is the umbrella the stories hang
off.

```
$ AIDA_SESSION_ROLE=advisor aida add --title "TODO CLI" \
           --type epic --status approved --feature todo
Added: EPIC-1 - TODO CLI
```

`aida add` prints one line: the new spec id and its title. `--status approved`
skips *Draft*: you've already decided this project should exist. `--feature todo`
tags it so every spec in this project shares one feature name — we'll query on
that in Step 4. (Filing as the advisor is what lets `--status approved` stick;
as the default implementer role it would land in *Draft* with a note that
approving needs advisor authority.)

---

## Step 2 — File the stories

Four stories, each filed as a **child of EPIC-1** in the same command via
`--parent`:

```
$ AIDA_SESSION_ROLE=advisor aida add --title "Add and list tasks" --type story --status approved \
           --feature todo --parent EPIC-1
Added: STORY-1 - Add and list tasks
  Linked: EPIC-1 → parent of STORY-1

$ AIDA_SESSION_ROLE=advisor aida add --title "Mark a task done" --type story --status approved \
           --feature todo --parent EPIC-1
Added: STORY-2 - Mark a task done
  Linked: EPIC-1 → parent of STORY-2

$ AIDA_SESSION_ROLE=advisor aida add --title "Persist tasks to a JSON file" --type story --status approved \
           --feature todo --parent EPIC-1
Added: STORY-3 - Persist tasks to a JSON file
  Linked: EPIC-1 → parent of STORY-3

$ AIDA_SESSION_ROLE=advisor aida add --title "Filter tasks by status and tag" --type story --status approved \
           --feature todo --parent EPIC-1
Added: STORY-4 - Filter tasks by status and tag
  Linked: EPIC-1 → parent of STORY-4
```

The `--parent EPIC-1` flag did two things in one call: created the story **and**
the typed `child → parent` edge between it and the epic. That edge is a real
relationship in the graph — not a string in a description field.

> **Without AIDA** — These four stories would be markdown headings under an
> "Epic: TODO CLI" heading, or four tickets in a tracker with the epic named in
> a free-text "relates to" field. The link is *there*, but it isn't *typed* and
> it isn't *queryable*. AIDA's edge has a type (`child`) and a direction, so
> "what's under this epic?" is one command, not a scroll.

---

## Step 3 — File the bug, and add a cross-link by hand

File a bug, then link it to the story it touches with a `references` edge — the
second way to create relationships, after `--parent`:

```
$ AIDA_SESSION_ROLE=advisor aida add --title "Listing crashes on an empty data file" \
           --type bug --priority high --status approved \
           --feature todo --parent EPIC-1
Added: BUG-1 - Listing crashes on an empty data file
  Linked: EPIC-1 → parent of BUG-1

$ aida rel add BUG-1 STORY-3 --type references
Added relationship: BUG-1 --[References]--> STORY-3
```

`aida rel add` works on specs that already exist — use it whenever the
relationship wasn't obvious at creation time. The relationship vocabulary is
small and typed: `parent`, `child`, `references`, `duplicate`, `verifies`,
`verified-by`. BUG-1 `references` STORY-3 because the empty-file crash lives in
the persistence story's code.

---

## Step 4 — See the graph

You've filed six specs. Look at what you built.

```
$ aida list --parent EPIC-1
ID             Type         Status     Priority   Title
──────────────────────────────────────────────────────────────────────────
STORY-1        Story        Approved   Medium     Add and list tasks
STORY-2        Story        Approved   Medium     Mark a task done
STORY-3        Story        Approved   Medium     Persist tasks to a JSON file
STORY-4        Story        Approved   Medium     Filter tasks by status and tag
BUG-1          Bug          Approved   High       Listing crashes on an empty data file
```

`--parent EPIC-1` restricts the listing to direct children of the epic. Now the
bug's cross-link:

```
$ aida rel list BUG-1
FROM   TYPE        TO       TITLE
  BUG-1  child       EPIC-1   TODO CLI
  BUG-1  references  STORY-3  Persist tasks to a JSON file

2 edges
```

This is the **graph**: six specs, six typed edges, all queryable. It cost you
six `aida add` calls and one `aida rel add`. From here on, AIDA does the
remembering.

---

## Step 5 — Route work to the queue

A spec being *Approved* means it's agreed — not that anyone is doing it. Work
gets *routed* by putting it on a role's queue. Queue STORY-1 and STORY-2 for the
**implementer** role (routing work to a queue is an advisor act, so the
`AIDA_SESSION_ROLE=advisor` prefix from Step 1 is back):

```
$ AIDA_SESSION_ROLE=advisor aida queue add STORY-1 --for implementer
✓ Added STORY-1 (Add and list tasks) to queue [for:implementer]

$ AIDA_SESSION_ROLE=advisor aida queue add STORY-2 --for implementer
✓ Added STORY-2 (Mark a task done) to queue [for:implementer]
```

Check the queue:

```
$ aida queue list
My Queue (2 items)
────────────────────────────────────────────────────────────────────────────────
  1. STORY-1 Add and list tasks  [▸ Approved]  [for:implementer]  [@EPIC-1*]
  2. STORY-2 Mark a task done  [▸ Approved]  [for:implementer]  [@EPIC-1*]
```

Each line carries the spec's status, the role it's routed to (`[for:implementer]`),
and a dimmed `[@EPIC-1*]` — the parent epic, derived from the graph so you can
see what a queued item belongs to without opening it.

> **Without AIDA** — "What should I pick up next?" is a question you answer in
> stand-up, in Slack, or by re-reading the board. The queue *is* the answer: an
> ordered, per-role list. When you (or an agent) finish one item, the next is
> already named.

---

## Step 6 — Pick up STORY-1 and implement it

`aida queue work` collapses pull + worktree + branch + session + role into one
command:

```
$ aida queue work STORY-1
✓ pulled aida-store (up to date)
✓ worktree   ../todo-cli-story-1   ·   branch story-1
✓ session    019e4b22 · role implementer · scope STORY-1
↳ launching Claude Code…
```

You're now in a Claude Code session, in a **fresh git worktree** — your main
checkout is untouched, so an in-flight feature never blocks another. Inside the
session, run the pickup skill:

```
> /aida-pickup
```

Claude reads STORY-1, writes the TODO-CLI code for "add and list tasks", and —
the part that matters — drops a one-line `trace:` comment on each function it
creates:

```rust
// trace:STORY-1 | ai:claude
fn cmd_add(task: &str) -> Result<()> { ... }
```

```rust
// trace:STORY-1 | ai:claude
fn cmd_list() -> Result<()> { ... }
```

Then it commits, using AIDA's commit format (the `(STORY-1)` suffix is the link
back to the spec):

```
[AI:claude] feat(cli): add and list tasks (STORY-1)
```

Filing STORY-1 with `aida queue work` flipped its status to **In Progress**
automatically.

> **Without AIDA** — Six months from now someone opens `src/commands.rs` and
> finds `cmd_add`. *Why does this exist? What was it supposed to do?* The answer
> lived in a ticket whose ID nobody wrote down, or a commit message buried under
> 400 others. The `// trace:STORY-1` comment makes that link survive: it's right
> there in the code, and `aida show STORY-1` walks back the other way.

---

## Step 7 — Ship STORY-1

Still inside the session, open the PR:

```
> /aida-pr
✓ pushed story-1 → origin
✓ opened PR #1 — https://github.com/you/todo-cli/pull/1
✓ queued a reviewer for PR #1
```

STORY-1 is now **Done** — *work finished on a branch*, PR open, awaiting review.
Done is not merged; the precise distinction is in
[`lifecycle.md`](lifecycle.md). Notice `/aida-pr` also queued a **reviewer** —
the next role in the loop.

Review is its own session. Pick up the reviewer item the same way (no argument
= head of your queue):

```
$ aida queue work
✓ session    019e4b9c · role reviewer · scope PR-1
↳ launching Claude Code…

> /aida-review
✓ reviewed PR #1 — verdict: approve
  trace comments present · covers the add/list path · no findings
```

Merge it, then pull:

```
$ gh pr merge 1 --squash
✓ Squashed and merged pull request #1

$ aida pull
✓ code    fast-forwarded main (1 commit)
✓ store   up to date
↳ auto-bumps STORY-1 → Completed
```

You never typed `--status completed`. `aida pull` saw a commit referencing
STORY-1 land on `main` and bumped it for you. STORY-1 is **Completed**.

---

## Step 8 — Do it again: STORY-2

Same loop, second story. This is the rhythm — once it's familiar it barely
registers as steps:

```
$ aida queue work STORY-2          # head of the queue is STORY-2 now
↳ launching Claude Code…
> /aida-pickup                     # Claude implements "mark a task done"
> /aida-pr                         # opens PR #2, queues a reviewer

$ aida queue work                  # pick up the reviewer item
> /aida-review                     # verdict: approve

$ gh pr merge 2 --squash
$ aida pull
↳ auto-bumps STORY-2 → Completed
```

Two features merged. The mechanics never changed between STORY-1 and STORY-2 —
that's the point. The loop is the same shape every time.

---

## Step 9 — Look back at what accumulated

This is where the filing from Steps 1–4 pays off. Ask the graph what happened.

**The epic's progress** — `aida list` hides Completed work by default so the
day-to-day view stays actionable; `--all` shows the archive too:

```
$ aida list --parent EPIC-1 --all
ID             Type         Status     Priority   Title
──────────────────────────────────────────────────────────────────────────
STORY-1        Story        Completed  Medium     Add and list tasks
STORY-2        Story        Completed  Medium     Mark a task done
STORY-3        Story        Approved   Medium     Persist tasks to a JSON file
STORY-4        Story        Approved   Medium     Filter tasks by status and tag
BUG-1          Bug          Approved   High       Listing crashes on an empty data file
```

Two of five done, at a glance — no spreadsheet.

**The code↔spec linkage** — `aida show` includes a git-linkage section,
populated automatically from the trace comments and the commit's `(STORY-1)`
reference:

```
$ aida show STORY-1
ID: STORY-1
Title: Add and list tasks
Type: Story
Status: ✓ Completed
...

Git linkage:
  Branch     merged to main
  PR         PR-1
  Commits (1)
    a3f9c1e2 [AI:claude] feat(cli): add and list tasks (STORY-1)
  Files traced (2)
    src/commands.rs — cmd_add
    src/commands.rs — cmd_list
```

That **Git linkage** block is the answer to "what code implements this spec?" —
and it assembled itself. You filed a spec, Claude wrote `// trace:STORY-1`, the
commit said `(STORY-1)`, and AIDA stitched the three together.

> **Without AIDA** — "What code implements the TODO CLI epic?" means
> `git log --grep`, hoping every commit named the right ticket, hoping the
> ticket IDs are stable, and reading 400 messages to be sure. Here it's
> `aida show` per story. The graph answered a question you'd otherwise answer
> by archaeology.

(An epic has no commits of its *own* — `aida show EPIC-1` won't list code
directly. The linkage lives on the stories that did the work; the epic is the
index that ties them together.)

---

## Step 10 *(optional)* — Plan a design-heavier spec

STORY-4 ("Filter tasks by status and tag") has real design choices — filter
syntax, AND/OR semantics, how tags compose. For specs like that, hand
`/ultraplan` a fully-contextualized prompt instead of a one-liner:

```
$ aida ultraplan STORY-4
✓ assembled planning prompt for STORY-4 (description + acceptance + graph context)
✓ copied to clipboard — paste into /ultraplan
```

`aida ultraplan` reads STORY-4's description, its place in the graph (parent
EPIC-1, siblings STORY-1..3), and the AIDA 11-section plan template, and builds
a prompt the planner can anchor on. Save the result under `docs/plans/` with
`/aida-import-plan`. Cheap specs don't need this; design-forked ones do.

---

## Step 11 *(optional)* — Drain the rest autonomously

You drove STORY-1 and STORY-2 by hand so you could see each stage. STORY-3 and
BUG-1 are mechanical enough to run unattended. `--auto-complete` runs the whole
implement → CI → review → merge → pull chain from one command:

```
$ aida queue work STORY-3 --auto-complete
```

It drives the implementer session, waits for CI, runs the reviewer, merges, and
bumps STORY-3 to *Completed* — no further input. The trade-off is real:
**interactive = better design decisions, autonomous = better throughput**. Run
the loop by hand until the rhythm is familiar, then reach for `--auto-complete`
on the mechanical batches. Full guidance: [`autonomous-drain.md`](autonomous-drain.md).

---

## What just happened

You ran maybe twenty commands. Here's what you actually built:

| You did | AIDA kept |
|---------|-----------|
| `aida add … --parent EPIC-1` ×5, `aida rel add` ×1 | A six-spec graph with six typed, queryable edges |
| `aida queue add … --for implementer` | An ordered, per-role work queue — "what's next" is answered |
| `aida queue work` | Each feature in its own worktree + branch + session, no cross-contamination |
| Let Claude write `// trace:STORY-1` | A code→spec link that survives in the source itself |
| Committed `… (STORY-1)` and merged | `aida show` auto-assembled the spec→commit→PR→files linkage |
| `aida pull` after each merge | Status bumped *Done → Completed* on its own |

None of these is impressive in isolation — *"I could do this in 20 lines of
bash"* is a fair first reaction. The value isn't any one command; it's that
six months from now `aida show STORY-1` still answers *why does this code
exist*, `aida list --parent EPIC-1` still shows project shape, and a coding
agent starting cold can query the whole graph through MCP instead of
re-deriving it. The walkthrough is small on purpose. The graph it leaves behind
is the product.

## A note on commits

If you're following along without Claude Code, commit the implement steps
yourself in AIDA's format so the linkage still assembles:

```
[AI:tool] type(scope): description (REQ-ID)

e.g.   feat(cli): add and list tasks (STORY-1)
```

`type` is one of `feat fix docs style refactor perf test build ci chore revert`;
the `(REQ-ID)` suffix is what `aida pull` scans for to auto-bump status. Drop
the `[AI:tool]` prefix when no AI wrote the code. Full rules: the **Commit
message format** section of [`CLAUDE.md`](../CLAUDE.md).

## Where to go next

- **[Spec lifecycle](lifecycle.md)** — the full Draft → Released state machine,
  the verb vocabulary, and the edge cases (cluster PRs, parallel pipelining).
- **[Getting started](getting-started.md)** — install options and the command
  quick-reference.
- **[How AIDA compares](../README.md#how-aida-compares)** — the
  *"why AIDA instead of X?"* question, one neighbor tool at a time.
- **[Why AIDA?](WHY-AIDA.md)** — the problem statement behind all of it.

Now finish the TODO CLI: STORY-3, STORY-4, and BUG-1 are still queued.
