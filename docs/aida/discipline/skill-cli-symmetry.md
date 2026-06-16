# Skill ↔ CLI-verb symmetry — when a slice ships, the skill *calls* it

A skill often starts as a prompt that walks an agent through some
deterministic work by hand — derive the ready set, assemble a review
checklist, compute a goal condition. When that deterministic slice is
worth hardening, it ships as a CLI verb: `aida burndown plan`,
`aida review prompt`, `aida goal`, `aida digest`, … The Rust verb is now
the single source of truth for that logic.

**The rule:** when a skill's deterministic slice ships as a CLI verb, the
parent skill must *call* that verb in the same PR rather than keep (or
re-derive) the logic inline. Rewrite the hand-walked section to invoke
the verb and use its output — e.g. "run `aida burndown plan --json` and
treat its `ready` set as law" instead of re-describing how to decide
which specs are ready.

This is the same anti-drift discipline as the **CLI ↔ MCP mirror**
(STORY-82): two surfaces describing the same behaviour drift the moment
one is edited and the other isn't. A skill that re-implements a verb's
logic in prose is a second copy that will silently disagree with the
Rust the next time the verb's behaviour changes — and the skill copy is
the one nobody runs tests against.

## Why call the verb instead of re-deriving

- **One source of truth.** The verb's behaviour is exercised by tests and
  by every other caller; the prose copy is exercised by nobody.
- **No silent drift.** Change the verb once and every caller — CLI, MCP,
  skill — moves together. A re-implemented skill section stays frozen at
  whatever the verb did the day the prose was written.
- **The skill stays thin.** Its job is to resolve parameters, sequence
  calls, handle autonomy modes, and narrate — not to own logic that has
  a hardened home.

## What this is NOT

- It is **not** a mandate to add a CLI verb for every skill. Pure-agentic
  skills (judgement, narration, design forks) have no deterministic slice
  to extract — leave them alone. The classification of a skill as
  slice-backed vs pure-agentic vs launcher is a separate decision (see
  `skill-prompt-kinds.md`).
- It is **not** a hard gate. The trip-point checklist line in
  `/aida-commit` and `/aida-pr` is a prompt-level nudge to a reading
  agent at ship time, not a lint or CI check.

## The trip point

When you ship a new CLI slice verb, before you open the PR: find the
skill(s) that previously did that work by hand and update them to call
the verb. The `/aida-commit` and `/aida-pr` checklists carry a one-line
reminder — *"Shipped a new CLI slice verb? Update its parent skill to
call it (no re-impl)."* — so the symmetry trips at the moment it's
cheapest to honour.

<!-- trace:TASK-736 -->
