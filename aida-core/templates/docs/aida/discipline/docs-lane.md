# The docs lane (single-writer scope)

Documentation is the cleanest *naturally single-writer* scope: low-conflict,
high-overlap if many hands touch it, and easy to route. The **docs lane** is a
durable convention that gives that scope one owner — one agent drains all doc
work sequentially, every other agent FLAGS doc work instead of editing docs
directly. Other lanes (code subsystems, the merge cascade) run in parallel; the
docs lane never collides with them because, by topology, only one writer is ever
in `docs/`.

This is the MVP / proof-of-concept for **subsystem-scoped agents** (SPIKE-10):
prove the persistent single-writer scope-lane on the safest subsystem first,
then evaluate generalizing to a code subsystem.

## Single-writer is a CONVENTION, not a gate

There is no ownership registry and no enforcement code policing exclusivity.
Conflict-freedom is a *property of the topology* — if only one agent ever writes
`docs/`, doc edits never conflict. The discipline below is what makes that
property hold; the substrate does not check it. (This is deliberate: a
scope-ownership registry would be heavyweight machinery for a guarantee the
topology already gives. Revisit only if the convention proves insufficient — see
the revisit-trigger below.)

## The two rules

1. **One agent owns the docs lane.** It wears the `docs` role
   (`aida role enter docs`) and is the only writer in `docs/`. It drains doc
   work sequentially within the scope.
2. **Every other agent FLAGS, never edits.** When a non-docs agent notices doc
   work — a stale guide, a missing section, a design decision worth capturing —
   it does **not** edit `docs/` itself. It routes the work to the docs lane via
   the `needs-docs` routing primitive (below) and keeps draining its own lane.

The flag-not-edit rule is the whole point: it keeps `docs/` single-writer so
the docs agent can drain without rebase churn, and it means a code agent never
context-switches into prose work mid-flow.

## The `needs-docs` routing primitive (the doc inbox)

`needs-docs` is the canonical, AIDA-wide flag any agent uses to route doc work
into the docs lane's pickable set. It is a flat behavior/routing tag (see
[`tag-conventions.md`](tag-conventions.md) rule 2 — it describes a
characteristic, not a CLI surface, so no `aida:` prefix).

Three equivalent ways to file into the doc inbox, in increasing weight:

- **Tag an existing spec** that needs documentation follow-up:

  ```bash
  aida edit STORY-123 --tags needs-docs
  ```

- **File a finding** when the doc work is an observation, not yet a spec — the
  capture-doc-seeds discipline ([`observation-discipline.md`](observation-discipline.md)):

  ```bash
  aida findings add "BUG-456 fix changed the pull contract — docs/lifecycle.md needs a note" --tags needs-docs
  ```

- **Route a brief / queue item directly to the lane** when the doc work is
  concrete and ready to pick up:

  ```bash
  aida queue add --for docs STORY-123
  aida brief docs STORY-123 --note "auto-bump section is now wrong after BUG-456"
  ```

  (`docs` is just a role name; the queue's `--for <role>` routing and
  `aida brief <role>` already accept any role identifier — no new surface is
  needed. `aida role enter docs` activates the lane; `aida role list` shows it.)

The push-feed (other agents filing `needs-docs`) and the pull-feed (the docs
agent's periodic sweep) both converge on the same pickable set: anything tagged
`needs-docs` or routed `--for docs`.

## How the docs agent drains its lane

The docs owner runs two complementary loops:

- **Drain the inbox** — `/aida-burndown` (or `aida queue work`) filtered to the
  docs lane: the `needs-docs` tag plus anything routed `--for docs`.

  ```bash
  aida queue work --for docs --auto-complete          # drain the routed queue
  aida list --tags needs-docs                         # the flagged-but-unqueued backlog
  ```

  Drain sequentially within the scope (single writer) — there is no fan-out
  inside the docs lane the way there is for parallel code implementers.

- **Sweep for gaps** — a periodic `/aida-docs-review` sweep on a `/loop`, to
  catch doc drift that no agent flagged. The sweep is the pull-feed; it
  complements the `needs-docs` push-feed so gaps surface even when the
  originating agent forgot to flag.

  ```
  /loop 1d /aida-docs-review
  ```

## Revisit-trigger — when to promote SPIKE-10 proper

The docs lane is a deliberate MVP. After it has run cleanly for **N drain
cycles** (suggested: ~5 sweeps with no single-writer collision and a steadily
draining `needs-docs` backlog), evaluate generalizing the pattern to a *code*
subsystem — a second persistent scope-lane with its own owner and inbox. That
generalization is SPIKE-10 proper (subsystem-scoped agents). Until the docs lane
has demonstrably earned it, do **not** build the generalized scope-ownership
machinery — the convention is the proof, the machinery is the payoff.

<!-- trace:STORY-540 -->
