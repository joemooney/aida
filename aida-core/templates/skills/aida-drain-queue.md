---
name: aida-drain-queue
description: Assemble a safe, correctly-phrased /goal prompt for autonomously draining your role's work queue. Wraps the queue-drain pattern with templated /goal text that avoids the two recurring phrasing traps — fake command flags and the wrong mechanism clause. Use when the user asks to "drain the queue", "work everything queued", or run an autonomous multi-item session.
disable-model-invocation: true
allowed-tools:
  - Bash
---

# AIDA Drain-Queue Skill

## Purpose

Turn "drain my queue" into a correctly-phrased `/goal` autonomous loop —
*without* hand-rolling the `/goal` text, which has bitten two recurring
traps. This skill **is** the templating layer: it assembles the `/goal`
prompt from flags so the phrasing is correct by construction.

## When to use

- The user asks to drain / work-through / clear their role's queue.
- The user wants an autonomous multi-item session ("work all of
  EPIC-26's queued stories", "drain the tui-v1 batch").
- The EPIC-26 TUI's autonomous-mode panel dispatches here (STORY-136).

## Skip if

- There is a single specific item to do — use `/aida-pickup` instead.
- No role is active — run `eval "$(aida role enter <name>)"` first, so
  the queue filter has a target.

## The two traps this skill exists to eliminate

Hand-rolled `/goal` text repeatedly hit these (2026-05-15 drain). The
skill templates them away — read this section before editing the
template so the *why* survives.

### Trap 1 — fake command flags

`aida queue work --next` was hand-written as shorthand for "pick the
head item". **`--next` is not a real flag.** The Stop hook kept
re-checking the literal mechanism clause, the command kept erroring, and
the loop never declared completion — it needed a manual `/goal clear`.

**Rule: every command in the `/goal` text must be one you could paste
into a shell verbatim and have succeed.** The real commands are:

- `aida queue work` — bare, no id → picks up the **head** of the active
  role's queue. There is no `--next` and no `--head`.
- `aida queue list --role <role>` — the termination check.
- `aida queue done <id>` — runs *inside* `/aida-pickup` per item, not in
  the `/goal` text.

### Trap 2 — the mechanism clause shapes the workflow

`autonomous-merge each` was used in a drain meant to keep a reviewer in
the loop. Autonomous-merge **skips** the reviewer handoff — the
`Review PR-N` story is filed by `aida session end`, which an
autonomous-merge flow never reaches. Ten PRs merged; the reviewer queue
stayed empty.

**Rule: the mechanism clause must match the intended handoff.**

- `review` mode → each item ends with `aida session end`, which files a
  `Review PR-N` story for the `reviewer` role. The reviewer stays in the
  loop. **This is the default.**
- `merge` mode → each item autonomously merges its own PR. No reviewer
  checkpoint. Use only when you explicitly want no review.

## Flags

| Flag | Default | Meaning |
|------|---------|---------|
| `--mode review\|merge` | `review` | reviewer-in-the-loop vs. autonomous-merge |
| `--role <name>` | active role | which role's queue to drain |
| `--batch <tag>` | (none) | restrict to items tagged `batch:<tag>` |
| `--max <N>` | (unbounded) | stop after N items even if more remain |
| `--dry-run` | off | print the assembled `/goal` text, do not invoke |

## Workflow

### Step 1 — resolve the parameters

- `role`: `--role` flag → `$AIDA_SESSION_ROLE` → otherwise ask the user.
- `mode`: `--mode` flag → otherwise `review`.
- `batch`, `max`: from the flags if present.
- Confirm the queue is non-empty:
  `aida queue list --role <role>` (add `--batch <tag>` when set). If it
  is already empty, stop — there is nothing to drain.

### Step 2 — assemble the /goal text

Substitute the resolved values into the template below. The
`<batch suffix>` is ` --batch <tag>` when `--batch` is set, else empty.

Mechanism clause, by `mode`:

- **review**: `commit, push, open a PR, then run \`aida session end\`
  (which queues a Review PR-N story for the reviewer role)`
- **merge**: `commit, push, open a PR, and autonomously merge it`

`<max clause>`: `Stop after <N> items even if the queue is not empty.`
when `--max` is set, otherwise omitted.

Assembled `/goal` text:

> Autonomously drain the **<role>** work queue. Loop: run `aida queue
> work` with no id (it picks up the head of the <role> queue) to start
> the next item, follow the `/aida-pickup` workflow to implement it to
> completion, <mechanism clause>, then pick up the next item. <max
> clause> Completion condition: `aida queue list --role
> <role><batch suffix>` reports zero queued items — verify by running
> that exact command. Stop and report when it is empty.

### Step 3 — dry-run or invoke

- `--dry-run` (or the user asks to preview): print the assembled
  `/goal` text inside a fenced block and **stop**. Do not invoke it.
- Otherwise: issue the assembled text as a real `/goal <text>`
  invocation to start the autonomous loop.

### Step 4 — during the drain

Each item follows `/aida-pickup`: `aida queue work` → implement →
commit → the mode's mechanism clause → next. The loop's Stop hook
re-checks the termination command after each item and ends the loop
when the queue is empty (or `--max` is hit).

#### Waiting between items — event-driven, not timer-driven

For an **unattended / overnight** loop, do not sit on a blind timer
while an item runs. Launch the harness `Monitor` tool over the drain's
wake feed and let it wake you:

```
Monitor(command: "aida watch --emit-wakes", persistent: true)
```

`aida watch` tails the drain's event stream and prints a line **only**
on an actionable verb — a PR shipped or merged, a CI verdict, a punt, a
shelve, an escalation, the drain finished — staying silent through the
benign phase churn. The supervising session burns **zero tokens while
the item runs** and wakes exactly when there is something to do, so
supervision cost drops from O(time-elapsed) to O(actionable events).
`aida watch` also emits a single wake line and exits if the orchestrator
process dies, so an overnight loop never blocks forever on a corpse.

Keep a **long-interval** `ScheduleWakeup` (e.g. 30–60 min) as the
documented degenerate fallback: if no event stream is live, or the
watcher wedges, the timer still resurfaces the loop. **Correctness never
depends on the event path** — the termination check is still the exact
`aida queue list --role <role><batch suffix>` command from the `/goal`
text, re-run on every wake from either source. The event feed only
changes *when* you look, never *what* decides the loop is done.

## Worked examples

`/aida-drain-queue`
> review mode, active role. Each item → PR → `aida session end` → the
> reviewer queue grows. Loop ends when `aida queue list --role <role>`
> is empty.

`/aida-drain-queue --mode merge --batch tui-v1`
> Drains only `batch:tui-v1` items, autonomously merging each PR — no
> reviewer checkpoint. Ends when `aida queue list --role <role> --batch
> tui-v1` is empty.

`/aida-drain-queue --role reviewer --max 5`
> Drains up to 5 items from the reviewer queue with the review
> mechanism.

`/aida-drain-queue --dry-run`
> Prints the assembled `/goal` text without starting the loop — inspect
> the phrasing first.

## Related

- `/aida-pickup` — the single-item version; the per-item loop body here.
- `aida goal` (TASK-242) — derives machine-checkable completion
  conditions from spec metadata; this skill applies the same "real
  verification command, no vague conditions" discipline at the
  workflow level.
- STORY-136 — the EPIC-26 TUI autonomous-mode panel; its "Drain to
  review / Drain to merge" buttons dispatch to this skill.
- `/aida-burndown` — the parallel fan-out drain; it waits on the same
  `aida watch --emit-wakes` feed via `Monitor`, with the same
  long-interval timer fallback.

trace:TASK-995 | ai:claude
