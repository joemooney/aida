# Workflow patterns

Two recurring patterns trip up AIDA sessions: how an autonomous-loop prompt
is phrased, and how "next steps" UI is shaped. Both are easy to get subtly
wrong.

## `/goal` prompt phrasing

A `/goal` autonomous-loop prompt has two failure modes, both phrasing bugs:

### Use real command flags only

The `/goal` completion evaluator may match literal command strings against
the session transcript. If the prompt names a flag that does not exist
(`aida queue work --next` — there is no `--next`; the no-arg form picks the
queue head), the evaluator keeps looking for a command that never runs and
refuses to declare the goal complete.

**Before writing a flag into a `/goal` prompt, verify it with
`aida <subcommand> --help`.**

### The mechanism clause shapes the workflow

The verbs in the prompt decide how handoffs route. Pick deliberately:

- **Reviewer-honoring drain** (implementer ships → reviewer reviews):
  `commit + push + open PR + aida session end` — `aida session end` queues
  the PR for the reviewer.
- **Self-merge drain** (no reviewer): `commit + push + PR + autonomous-merge
  each` — this bypasses the reviewer queue entirely.

Match the termination check to the mechanism: `until aida queue list shows
no items routed to implementer` works when items leave the queue via merge
or session-end.

Reference phrasing for a reviewer-honoring implementer drain:

```
/goal drain the implementer queue, one item per session via `aida queue work`
      (the no-arg form picks the queue head),
      commit + push + open PR + `aida session end` (which queues the PR for review),
      until `aida queue list` shows no items routed to implementer
```

## Parallel choices vs sequential steps

"Next steps" / "what to do next" UI splits into two shapes that need
different formats. Conflating them produces self-contradictory specs.

| Shape | The user… | Right format |
|-------|-----------|--------------|
| **Parallel choices** | picks ONE of N complete next-actions | a Path / What / Why **table** |
| **Sequential steps** | does ALL of them, in order | a **numbered list** with flow arrows |

The discriminator: *"if the user does nothing, does the workflow still
progress?"*

- Yes (passive flow) → sequential steps; numbered list.
- No (the user must choose) → parallel choices; table.

Examples — parallel: an end-of-session menu (continue / open PR / pause —
pick one). Sequential: a post-merge hint (`merge` → `pull` → `build` — do
all). Do not cross-reference one spec's format from another unless the
shapes genuinely match.
