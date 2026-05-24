# Backlog grooming

How to move work out of *Approved* and onto the *queue* without filling
the queue with noise.

## What "backlog" means in AIDA

AIDA's lifecycle has six interesting states for unfinished work:

| State | What it means |
|-------|---------------|
| **Draft** | Captured but not yet ready to schedule. |
| **Approved** | Ready to schedule. Nothing has claimed it yet. |
| **Planned** | Approved AND on the queue. (Manual transition or via `aida backlog groom`.) |
| **In Progress** | Picked up; someone is working on it now. |
| **Done** | Work finished on a branch; not yet merged. |
| **Completed** | Merged to the default branch. |

The **backlog** in AIDA is the *Approved-but-not-yet-queued* pile. Draft
items aren't ready; Planned and In Progress items are already moving;
Completed items are done. The pile that needs *grooming* — i.e. ongoing
human judgment about what's worth queueing next and what isn't —
is the Approved set.

Backlog grooming is the act of moving items from that pile onto the
queue with intent: pick the cheap, non-conflicting ones for a batch
drain, leave the structurally large ones for their own sessions.

## The two surfaces

AIDA ships two surfaces for grooming:

- **`aida backlog`** — the CLI verb. Three subcommands (`list`, `groom`,
  `analyze`) plus stable JSON output. Drives the actual queue insertions
  and tag applications.
- **`/aida-backlog-groom`** — the Claude Code skill. Wraps the CLI in an
  interactive conversation: surfaces candidates, calls `analyze` to spot
  file-overlap conflicts, uses `AskUserQuestion` for the multi-select.

The CLI is the substrate; the skill is the conversation. The skill never
does anything the CLI can't — that's deliberate, so the same workflow
runs unattended (CLI in scripts) and supervised (skill in a session).

## How risk is graded

Every backlog item gets one of four risk chips on `aida backlog list`:

| Chip | What it means | Heuristic |
|------|---------------|-----------|
| **low** | Cheap. Probably one file. Probably a one-PR drain. | Type ∈ {task, doc} AND priority == low AND has at least one of: `papercut`, `cosmetic`, `severity:cosmetic`, `lifecycle:trivial`, `docs-only`, `fmt` |
| **medium** | Has a known shape. Plan file owns it OR it's a real task/bug at medium priority. | Type ∈ {task, bug} AND priority == medium, OR `docs/plans/<date>-<slug>.md` owns the spec |
| **high** | Structural or load-bearing. Should not be bulk-queued. | Type ∈ {story, epic, spike} OR priority == high OR has a `BlockedBy` / `Child` relationship |
| **unknown** | No signals. The heuristic doesn't know. | Anything else |

**The chip is advisory, not gating.** `groom` will never refuse to enqueue
a spec because the heuristic painted it `high`; the operator decides. The
chip's job is to surface effort, not to block.

## How conflict detection works

`aida backlog analyze --specs <ids>` walks every pair of selected
candidates and asks: *would running these two in parallel on separate
branches conflict at merge time?*

The file set for each spec is the union of:

1. **Trace comments** — every source file that contains
   `// trace:<SPEC-ID>` (or `# trace:<SPEC-ID>` etc.) somewhere.
2. **Plan files** — the `## Critical Files` section of any
   `docs/plans/*.md` whose header line names the spec.

Verdicts:

- **safe-parallel** — disjoint file sets. Branch concurrently, ship
  independently.
- **serialize** — at least one shared file. The verdict lists the
  shared paths. Run one first; rebase the second after the first lands.
- **unknown** — at least one spec contributed *zero* files (no trace
  comments and no plan file owns it). Treat this as **serialize by
  default**: the absence of signals is not a green light, just an
  absence of evidence.

What this does **not** promise:

- It does not look at already-merged commits (only trace comments +
  plans). A spec that's "done in spirit but not yet flagged" can slip
  past the detector. This is a known gap; a future TASK adds `git log
  --grep "(SPEC-ID)"` as a third file-overlap source.
- It does not model logical conflicts (two specs both renaming the same
  function but in different files). The detector is *file-level*, not
  *semantic*.

## How `batch:NAME` composes

`aida backlog groom --specs A,B,C --batch overnight-2026-05-24` does two
things:

1. Adds A, B, C to the queue (via the same `storage.queue_add` path
   `aida queue add` uses).
2. Applies the tag `batch:overnight-2026-05-24` to each.

The tag is what `aida queue work --batch NAME` consumes — same surface,
different producer. Once the groom lands, you can:

```bash
aida queue work --batch overnight-2026-05-24 --auto-complete
```

…to drain the whole batch as one autonomous chain, one full
implementer→CI→reviewer→merge→pull→build lifecycle per member.

The batch name is a free-form string. Convention:

- `cleanup` / `low-risk-cleanup` — generic cleanup drain.
- `overnight-YYYY-MM-DD` — autonomous drain for a specific date.
- `<scope>-followups` — when a parent spec landed and you're grooming
  its filed follow-ups.

## A typical grooming session

```bash
# 1. See what's in the pile.
aida backlog list --risk low --limit 100

# 2. Spot a cluster of cheap, related items. Get their ids.
LOW_IDS=$(aida backlog list --risk low --json \
            | jq -r '.rows[].spec_id' | paste -sd,)

# 3. Check the cluster doesn't step on itself.
aida backlog analyze --specs "$LOW_IDS"

# 4. Preview the groom (no writes).
aida backlog groom --specs "$LOW_IDS" --batch low-risk --dry-run

# 5. Commit it for real.
aida backlog groom --specs "$LOW_IDS" --batch low-risk

# 6. Drain it.
aida queue work --batch low-risk --auto-complete
```

## What backlog grooming is *not*

- **Not auto-pickup.** `aida backlog groom` never enqueues something
  the operator did not explicitly select. There's no "groom everything
  that looks safe" path on purpose — autonomy here would bypass the one
  human-judgment step that matters.
- **Not cross-machine.** The groom acts on the local queue. Routing
  work to another machine is the `aida queue add --global` /
  `aida queue add --for-session` story, not this one.
- **Not a sprint planner.** Sprint planning is a separate concern
  (where work goes in *time*, not just *order*). A future
  cross-reference will compose the two via sprint tags; today the
  workflows are independent.
- **Not ML-driven.** The risk chip is six lines of `match`, not a
  prediction model. Better evidence-based heuristics will arrive when
  the calibration data from `STORY-439` lands; until then, the
  heuristic is honest about being a heuristic.

## When to skip grooming

- Your queue is already right-sized — adding more would be busywork.
- You want a single specific spec — `aida queue add <ID>` is more
  direct.
- You're triaging in `dialog` mode and the next step is *capture*, not
  *schedule* — file requirements first; groom them later.

---

*Companions: [`tag-conventions.md`](tag-conventions.md) for the
`batch:*` namespace, [`session-discipline.md`](session-discipline.md)
for what happens **after** a batch drains.*
