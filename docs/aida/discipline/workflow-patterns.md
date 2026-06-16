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

## Planning-pass discipline (don't leave untracked plan files)

A "planning pass" — a loop of `/aida-plan <SPEC>` over several queue items, run
from the main repo — writes one plan file per spec. If those land directly in
`docs/plans/` as **untracked** files, a later implementer's PR that lands its own
plan at the same path makes `git pull --ff-only` abort:

    error: The following untracked working tree files would be overwritten by merge:
        docs/plans/2026-05-19-<spec>.md

**Rule:** a planning pass writes drafts to `docs/plans/_draft/` (gitignored —
scaffolded by `aida init`). Promote a draft to `docs/plans/<name>.md` only when
it's adopted (and commit it as part of that work). Never leave generated plans
untracked in `docs/plans/` — they become a merge landmine for every later PR.
trace:TASK-383 | ai:claude

## Recursive-failure-risk fixes use the keyboard, not the drain

A fix to the **autonomy machinery itself** — the orchestrator, lease
management, the reviewer/implementer phase enforcement, merge/CI/drain
plumbing — must NOT be shipped through an unsupervised `--auto-complete
--no-human` drain. The reason is recursive: the fix rides *through* the very
system it repairs, so if that system's current failure rate is non-trivial,
the fix gets caught in the same failure it's meant to remove — and a headless
drain has no human to recover it. You can spend a night watching a reliability
fix fail to merge because of the bug it fixes.

**Rule of thumb — sort the work by what it touches:**

- **Touches the drain's own correctness** (orchestrator, leases, phase
  transitions, merge/pull/build plumbing, anything whose failure would *abort
  or corrupt a drain*) → ship it **at the keyboard**, supervised, via
  `--zen --auto-complete` with a human (or live advisor) watching. The fix
  still exercises the real path (strongest validation — see
  `substrate-as-bouncer.md` on dogfooding the system you're fixing), but a
  human catches the recursive failure the first time it bites.
- **Touches anything else** (a CLI papercut, a display bug, docs, a new
  read-only surface, a self-contained feature with small blast radius) → fine
  to drain unsupervised.

This refines the general "dogfood your fix through the system it fixes"
instinct: dogfooding is right, but for the *recursive-failure-risk* subset you
do it **watched**, not overnight. The supervised loop still counts as
autonomous work — the only thing excluded is the *unattended* `--no-human`
drain of a fix to the unattended drain.
