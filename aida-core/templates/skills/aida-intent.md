---
name: aida-intent
description: Produce the plain-terms WHY-comprehension of a spec — read the spec plus its immediate graph neighborhood (parents, children, blockers, referenced specs, decisions, key comments) and distil its reason-for-being into two registers (layman prose + an LLM-dense brief). Write the result as a JSON sidecar the launcher folds onto the spec. Launched by `aida intent <spec>` (headless `claude -p`); not run by hand. trace:STORY-631 | ai:claude
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
  - Grep
---

# AIDA Intent Skill

## Purpose

Answer **why does this spec EXIST** — its intent, in plain terms — distilled
from the tangled web of parents, blockers, decisions, comments, and history that
a human finds overwhelming but an LLM can comprehend.

This is **not** `aida why`. `aida why` is a deterministic Rust classifier that
answers "why is this spec still *open*?" (status / blockers / leases). This
skill is the complementary **synthesis**: it reads the surrounding graph and
explains the spec's *reason-for-being* in prose. Two verbs, two kinds of work.

You are launched by `aida intent <spec>` as a headless `claude -p`. The launcher
has already resolved the spec id and the output path; your job is the
comprehension a CLI cannot produce.

## Inputs

The prompt names the target spec id and the exact output path. Read, in order:

1. **The spec itself** — `aida show <SPEC> -c` (title, description, status,
   tags, `## Acceptance`, comments).
2. **Its immediate graph neighborhood** — `aida graph <SPEC>` plus
   `aida show` on each immediate neighbor:
   - **parents / children** — what larger goal this serves, or what it rolls up.
   - **blockers / blocked** (`BlockedBy` / `Blocks`) — the hard dependencies that
     shape when and why this can move.
   - **referenced specs** (`References`) — sibling context.
   - **decisions** (ADR-* neighbors) — recorded rationale that explains *why this
     shape* was chosen.
   - **key comments** — operator/advisor notes that carry intent the title omits.

Stay within the **immediate** neighborhood — one hop. Do not crawl the whole
graph; the point is the local web that explains this spec.

## Output — TWO registers

Write a single JSON object to the path the prompt gives you, with exactly these
keys:

```json
{
  "layman": "<plain prose for a human skimmer>",
  "llm": "<denser, structured comprehension for an agent>",
  "model": "<your model id, e.g. claude-opus-4-8>"
}
```

- **`layman`** — 2-5 sentences of plain English a non-author could read and
  understand *why this spec matters and what problem it solves*. No jargon, no
  SPEC-IDs unless naming a concrete dependency. The "explain it to a teammate
  who just walked in" register.
- **`llm`** — a denser, structured brief an implementer agent loads *before*
  working the spec: the goal, the load-bearing constraints, the key
  relationships ("blocked by X because…", "serves EPIC-Y's…"), and any decision
  rationale that bounds the solution space. Bullet-dense is fine here.
- **`model`** — your own model identifier.

## Rules

- **Synthesize, do not transcribe.** Do not just paste the description back —
  explain the *why* the description assumes the reader already knows.
- **Ground it in the graph.** Name the actual neighbors that give the spec its
  reason-for-being; do not invent context.
- **This is AI-generated comprehension, not ground truth.** The launcher labels
  it as such on display. Write what the graph supports; flag uncertainty in
  prose rather than overstating.
- **Write ONLY the JSON object to the output path.** That sidecar is the
  contract; the launcher reads it, stamps it with a timestamp + drift hash, and
  stores it on the spec. Do not edit the spec yourself.
