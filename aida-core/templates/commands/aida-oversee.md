---
description: Run /aida-oversee.
---
# AIDA Oversee

The oversight seat — the judgment layer over the `aida supervise watch` loop.
Keeps an operator out of babysitting the automation: it watches the drain,
realigns work to the objective, recovers transient trouble through the shipped
reflexes, and brings you only the things that genuinely need a human decision.

## Usage

```
/aida-oversee <OBJECTIVE>    Oversee a named objective (an epic id, e.g. EPIC-63).
                             Interactive; the loop underneath is substrate-first
                             and safe to run with no agent awake.
```

## Instructions

Follow the workflow in `.claude/skills/aida-oversee/SKILL.md`:

1. Run the mechanical pass — `aida supervise watch --objective <OBJECTIVE>`
   (add `--execute` to realign the queue + fire reflexes; `--interval <secs>`
   for a continuous watch).
2. Read the substrate for judgment — `aida awaiting`, `aida ps`,
   `.aida/events.jsonl`, `aida findings list`, `aida mailbox inbox`.
3. Classify every observation into one tier: **silent** (benign/transient,
   aligned, progress), **digest** (a quiet one-line summary of progress + what
   the loop did), or **escalate** (a genuine human-decision — surface via
   `aida notify`).
4. Coordinate through the substrate (mailbox + drain lock), never by relaying
   agent↔agent through the operator.
5. Never drive, review, or merge — escalate those. Authority per ADR-29:
   coordinate + realign + trigger reflexes + escalate.

A quiet pass is a good pass — if nothing needs a human, say nothing.
