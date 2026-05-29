---
name: aida-punt
description: Punt a spec to Needs Attention when you hit a design-fork you cannot safely resolve. The honest alternative to guessing during an autonomous drain — pause the spec with a structured reason and return control so the orchestrator advances.
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
---

# AIDA Punt Skill

## Purpose

Pause a spec in the **Needs Attention** lifecycle status when, mid-work, you
hit a decision you cannot safely make on your own — a genuine design-fork,
an ambiguous spec, missing context, a blocking dependency.

Guessing past such a fork produces a *silent wrong implementation*: the code
compiles, the session ends green, and the wrong call is only discovered
later. Punting is the honest move — it records the fork, pauses the spec for
a human or advisor to decide, and lets an autonomous drain continue to the
next item instead of stalling or guessing.

## When to use

- You are working a spec (interactively or under a `--no-human` drain) and
  hit a **design-fork** — two or more genuinely valid implementations and
  the spec does not say which.
- The spec is **ambiguous or self-contradictory** and you cannot resolve it
  from its text, parent, or acceptance criteria.
- You are **missing context** the spec assumes — a decision, a file, an
  external fact you do not have.
- The spec is **blocked** by work that is not done.

## Skip if

- You can resolve the fork yourself from the spec, the codebase, or recorded
  project conventions — then just decide and proceed. Punting a fork you
  *could* have resolved adds triage noise.
- The spec is fine but a *test* fails or a *bug* surfaces — that is a
  finding (`aida add` a draft tagged `from-implementer:`), not a punt.
- You are in an interactive session and a human is right there — ask them.

## Workflow

### Step 1: Confirm it is a real fork

Re-read the spec, its `## Acceptance` section, its parent, and any plan that
owns it. A punt is for a decision you genuinely cannot make — not for the
first hard question. If you would punt, you should be able to state the fork
in one sentence and name the options.

### Step 2: Classify the obstacle

Pick the category that *observably* describes the obstacle — not a judgment
about your own competence:

| Category | Use when |
|----------|----------|
| `design-fork` | Multiple valid designs; the spec does not say which. |
| `ambiguous-spec` | The spec itself is unclear or self-contradictory. |
| `missing-context` | You need information or context you do not have. |
| `blocked-dependency` | You cannot proceed — depends on unfinished work. |
| `other` | A real obstacle that fits none of the above. |

### Step 3: Punt

```bash
aida punt <SPEC-ID> \
  --category <category> \
  --reason "<one or two sentences: the fork, and the options>" \
  --lean "<your best guess if forced to choose — optional>"
```

- `--reason` is the fork itself — what the decision is and what the options
  are. Write it for a human who has not seen the code.
- `--lean` is optional: if you *had* to pick, what would you pick and why.
  Recorded separately from the reason so triage can see your inclination
  without it being mistaken for a decision.

`aida punt` flips the spec to **Needs Attention**, records the structured
reason on the spec, and appends a record to the punt ledger
(`.aida/punts.jsonl`).

### Step 4: Return control

Once you have punted, **stop working that spec**. Do not commit a guess. The
spec is paused; an orchestrator will continue to the next item, and a human
or the advisor will triage the punt (`aida findings` surfaces it).

## What happens next

A Needs Attention spec is excluded from normal queue pickup but surfaces in
`aida findings` for triage. Whoever triages it resolves it with
`aida edit <SPEC> --status in-progress` (resume), `--status approved`, or
`--status rejected`.

## Notes

- `blocked-dependency` punts are worth a second action: file the blocking
  relationship so the dependency graph reflects it.
- A spec can only be punted while it is **In Progress** — punting is the
  "I was working this and hit a fork" move.
