---
name: aida-human-audit
description: Audit the coordination inbox — reconcile every `aida human` item so the "does this need me?" report is true from the graph. Walk each surfaced item and triage it into needs-you / fix-state / file-tool-bug, act on it, and hand back a ledger. Never narrate an item away in chat when the fix is a substrate edit. Invocable by `aida human audit`.
allowed-tools:
  - Bash
  - Read
  - Grep
  - Glob
---

# AIDA Human-Audit Skill

## Purpose

When the operator asks *"does anything in `aida human` need me?"*, the answer
must be **true from the graph** — not require the advisor to explain items away
in chat. This pass walks every item the coordination inbox surfaces and either
confirms it genuinely needs the human, **fixes the spec state** so it
self-corrects, or **files the reporter bug** when the report itself is wrong.

Pairs with the CLI: `aida human` *shows* the inbox, `aida human review <spec>`
looks at one item, and **`aida human audit`** runs this whole reconcile pass
(the CLI verb triggers this skill).

## The principle it enforces

Ephemeral narration that should have been a substrate edit is an **opacity
failure**. If an item "doesn't need you," make the graph *say* so
(defer / archive / status / relationship) or fix the reporter — don't just
explain it, or the next `aida human` re-surfaces it and the operator re-asks.
This is [[substrate-as-bouncer]] applied to the coordination inbox: encode the
truth, don't police it in prose.

## Instructions

1. **Enumerate.** Run `aida human` and `aida awaiting --json` (the structured
   view). List every surfaced item across all buckets — reviews-awaiting,
   needs-attention, findings, unread mail, reviewer verdicts, escalations.

2. **Triage each item into exactly one of three, and act:**

   - **A — Genuinely needs the human.** A real decision / review / merge gate.
     *Keep it.* State it crisply: what's being asked, your recommendation, the
     consequence of each choice. Do **not** mutate state to make it disappear.

   - **B — Stale spec state.** The item is real but its state is wrong or
     imprecise. *Fix the spec* so it self-corrects or surfaces with the right
     reason: `aida edit <ID> --status …`, `aida defer <ID> --until "<trigger>"`,
     `aida archive <ID>`, or add a `blocked-by` / `parent` relationship. The
     `--until` trigger is what distinguishes "parked, returns on X" from
     "filed away."

   - **C — Tool false-positive.** The state is already correct but `aida human`
     mis-reports it (surfaces the store branch, a deferred / archived / completed
     spec, a stale agent branch, …). Do **not** edit the spec to paper over it —
     *file or link the reporter bug* (e.g. BUG-722). The fix belongs in the
     reporter, not in a spec edit that distorts real state.

3. **Verify, don't assume.** After every bucket-B edit, re-run `aida human` and
   confirm the item actually dropped or changed. **If it didn't, the item is
   really bucket C** — reclassify and file the tool bug. (This is exactly how
   BUG-722 was found: deferring the specs did *not* drop them, proving the
   detector ignores view-state.)

4. **Report a ledger.** One row per item → verdict (A / B / C) → action taken →
   resulting `aida human` delta. **Lead with the bucket-A items** — the things
   that genuinely need the operator — then the B / C reconciliations.

5. **Sync.** `aida push` any spec edits so the store is current.

## Guardrails

- **Never auto-approve or auto-merge** to make an item disappear — an approval
  or a merge is an explicit human judgment (that item is bucket A).
- Deferring / archiving is a **view-level park, not deletion** — the YAML, audit
  trail, and graph survive. Use `--until` triggers so deferred work returns.
- If an item needs an operator decision you can't make (architecture, priority,
  a keystone merge), it is **bucket A** — surface it, don't force it into B.

## Delineation

- **`/aida-human-audit`** (`aida human audit`) reconciles the *coordination
  inbox* (`aida human` / `aida awaiting`) so its report is true.
- **`/aida-backlog-groom`** reconciles the *backlog* (drafts +
  approved-not-queued) into a drain-ready queue.
- **`/aida-digest`** narrates *what happened*; this pass fixes *what's surfaced*.

Related: **BUG-722** — the reviews-awaiting precision fixes (exclude the store
branch, skip deferred/archived, split draft WIP branches into a `wip-branches`
label) that shrink how many items land in bucket C over time.
