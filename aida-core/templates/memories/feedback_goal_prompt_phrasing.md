---
name: goal-prompt-phrasing
description: /goal autonomous-loop prompts — use only real command flags, and pick mechanism clauses that route handoffs as intended.
propagation: scaffolding-pack
metadata:
  type: feedback
---

A `/goal` autonomous-loop prompt has two failure modes, both phrasing bugs.

**1. Use real command flags only.** The `/goal` completion evaluator may match literal command strings against the session transcript. A flag that does not exist (e.g. `aida queue work --next` — there is no `--next`; the no-arg form picks the queue head) makes the evaluator wait forever for a command that never runs, refusing to declare the goal complete.

**2. The mechanism clause shapes the workflow.** The verbs in the prompt decide how handoffs route:

- Reviewer-honoring drain: `commit + push + open PR + aida session end` — session-end queues the PR for the reviewer.
- Self-merge drain (no reviewer): `commit + push + PR + autonomous-merge each` — bypasses the reviewer queue.

**Why:** Both failure modes look like the loop "broke" but are purely phrasing — the loop did exactly what the words said.

**How to apply:** Before writing a flag into a `/goal` prompt, verify it with `aida <subcommand> --help`. Pick the mechanism clause that routes handoffs the way you want, and match the termination check to it (e.g. `until aida queue list shows no items routed to implementer`).

Reference phrasing for a reviewer-honoring implementer drain:

```
/goal drain the implementer queue, one item per session via `aida queue work`
      (the no-arg form picks the queue head),
      commit + push + open PR + `aida session end` (queues the PR for review),
      until `aida queue list` shows no items routed to implementer
```

Composes with [[verify-before-filing]] (check queue state before assuming the loop failed) and [[run-help-before-suggesting-flags]].
