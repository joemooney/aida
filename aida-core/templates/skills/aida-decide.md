---
name: aida-decide
description: The human-decision drain as a skill. Sweep the whole decision inbox (every pending DecisionRequest + every under-specified spec the questions sweep flags), then loop the operator through each one interactively — record the chosen resolution for specs with enumerated choices (`aida questions answer`), or drop into the acceptance-authoring loop for specs that need criteria written (`aida questions clarify` / `/aida-clarify`) — until the inbox is empty. The human-facing analog of `aida burndown run` (which drains the decision-FREE ready set with headless agents). Use when the operator says "drain my decisions", "answer the pending questions", "clear the decision inbox", or wants to batch-resolve everything blocking the backlog in one sitting.
allowed-tools:
  - Bash
  - Read
  - Grep
  - AskUserQuestion
---

# AIDA Decide Skill

## Purpose

Human intervention is the recurring burn-down bottleneck (STORY-522). The async
decision protocol already splits that intervention into two halves:

- **DETECTION** — `aida questions sweep` walks the backlog and attaches a
  `DecisionRequest` to every spec that needs a human decision before it's
  pickable.
- **RESOLUTION** — `aida questions answer` records a discrete choice (a pure
  operator data op, no LLM), and `aida questions clarify` / `/aida-clarify`
  interrogates the operator to author missing acceptance criteria.

What was missing is the **single orchestrating loop** that runs the operator
through the *whole* inbox in one sitting. That is this skill. `/aida-decide` is
the human-decision drain: sweep, then for each pending item either **record the
answer** or **author the acceptance**, looping until the inbox is empty.

It is the human-facing mirror of `aida burndown run`: burndown drains the
decision-FREE ready set with headless agents; `/aida-decide` drains the
decision-LADEN set with the operator at the keyboard. Run decide first to
unblock specs, then burndown to ship them.

## Distinct from `/aida-advise`

`/aida-advise` is the **headless advisor tier** (STORY-306) — a fresh `claude -p`
that the `--no-human=both` drain spawns to judge a punt, biased toward escalate.
Different actor, no human. `/aida-decide` is the opposite seat: a human IS at
the keyboard, and the job is to drain what advise escalated *to* them. Do not
conflate the two — advise produces decisions for the human; decide is the human
consuming them.

## When to use

- "Drain my decisions", "answer the pending questions", "clear the decision
  inbox", "what's blocking the backlog that needs me?"
- The operator wants to batch-resolve everything gating the ready set before
  kicking off a `burndown run`.

## Skip if

- There is no human at the keyboard. This is an **interactive** skill — it asks
  real questions and waits for real answers. Headless → stop.
- The operator wants a single named spec clarified → `/aida-clarify <SPEC>`.
- The operator wants to ship decision-free work, not make decisions →
  `/aida-burndown` (`aida burndown run`).

## Procedure

### 1. Refresh detection (optional sweep)

If `$ARGUMENTS` contains `--sweep` (or the operator asks to detect new
candidates first), run the detection pass so the inbox reflects the current
backlog:

```
aida questions sweep --apply           # attach DecisionRequests to newly-flagged specs
```

Without `--sweep`, **do not** auto-mutate — drain what is already in the inbox.
Echo the swept count if you ran it (`N specs newly flagged`).

### 2. Read the inbox

```
aida questions list
```

This lists every spec carrying a `DecisionRequest`, pending ones first. If the
inbox is empty, **stop** — say so plainly and do not fabricate decisions. Report
the count of pending items you are about to drain.

### 3. Loop the operator through each pending item

For each pending request, in the order `questions list` shows them, present the
distilled question to the operator and route it by what it asks for:

**a. Discrete-choice request → record the answer.** The `DecisionRequest`
carries a self-contained question and ≥2 enumerated choices (each maps to a
concrete resolution), often with a recommended default + rationale. Present it
with **AskUserQuestion**: the question verbatim, each choice as an option, and
mark the recommended default first with `(Recommended)`. The advisor already
distilled it to be self-contained — do **not** make the operator re-read the
spec. On their pick, record it non-interactively:

```
aida questions answer <SPEC> <CHOICE>     # CHOICE = 1-based number, or `default`
```

Answering APPLIES the resolution (binds the decision into `## Acceptance`,
clears the gate, or rejects) and auto-queues the now decision-free spec onto the
burndown ready set — so a recorded answer is also the hand-off to the drain.

**b. Under-specified spec → author the acceptance.** If the request is "this
spec has no usable acceptance criteria" (the resolution is *write them*, not
*pick one of N*), a discrete answer can't resolve it. Drop into the
acceptance-authoring loop for that one spec:

```
aida questions clarify <SPEC>             # launches the interactive /aida-clarify loop
```

or, since you are already an interactive session, invoke `/aida-clarify <SPEC>`
inline — interrogate with a small targeted question set, reconcile against the
spec body, draft `## Acceptance`, bind it, clear the gating tag, re-check
pickability. (See `aida-clarify.md` for the full loop.) When it returns,
continue the inbox loop.

**Batch shortcut.** If the operator says "just take all the recommended
defaults", honor it in one shot for the discrete-choice items:

```
aida questions answer --all-defaults
```

Then fall back to step 3b for any remaining under-specified specs (defaults
can't author acceptance).

### 4. Loop until the inbox is empty

After each answer or clarify, re-read `aida questions list`. Keep going until no
pending items remain. Resolving one spec can unblock or reveal another, so the
inbox is the loop condition — not a fixed list captured at the start.

### 5. Report and hand off

When the inbox is drained, give the operator an honest summary:

- how many decisions you recorded (and which specs), how many you clarified;
- anything you could **not** resolve and why (e.g. the operator deferred it, or
  a clarify revealed a deeper blocker) — name those specs so they aren't lost;
- the obvious next step: the just-unblocked specs are now on the burndown ready
  set, so offer `/aida-burndown` (or `aida burndown run`) to ship them.

## Notes

- **No fabrication.** Empty inbox → stop. Never invent a decision to look busy.
- **Operator's call, always.** You present and record; you never auto-pick a
  substantive choice on the operator's behalf. `--all-defaults` is the one
  exception, and only because the operator explicitly asked for it.
- **The answer is the data op.** Recording is `aida questions answer` — a pure,
  no-LLM store write. Your value is the orchestration: sweeping the whole inbox,
  presenting each item clearly, and routing answer-vs-clarify so the operator
  drains it all in one sitting instead of one-item-at-a-time babysitting.

<!-- trace:STORY-602 (parent STORY-522) -->
