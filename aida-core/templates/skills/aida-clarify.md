---
name: aida-clarify
description: Advisor-assisted acceptance-authoring loop. For an under-specified spec the questions sweep flagged (missing/thin acceptance criteria), interrogate the human with a small targeted question set, reconcile any conflict with the existing spec body, draft a `## Acceptance` section, bind it, clear the gating parking tag, and re-check whether the spec is now autonomous-ready. The human-decision PRODUCER that turns "parked: missing acceptance" into "burndown-ready" — the interactive complement to the questions sweep (which only DETECTS).
allowed-tools:
  - Bash
  - Read
  - Grep
  - AskUserQuestion
---

# AIDA Clarify Skill

## Purpose

`aida questions sweep` **detects** specs that are likely to need a human
decision before implementation — most often because they have missing or thin
acceptance criteria. It is a producer for the async decision inbox, but it
only flags; it cannot *resolve*.

This skill is the agentic complement: the **acceptance-authoring loop**. You
take one flagged spec, interrogate the human with a small, targeted question
set, reconcile their answers against the spec's existing body, draft a
`## Acceptance` section, get it approved, bind it via `aida edit`, clear the
gating parking tag, and re-check whether the spec is now pickable. You are the
seat that turns *"parked: missing acceptance"* into *"burndown-ready"*.

You are paired with `aida burndown run` (the headless drain that consumes the
ready set) as its human-side feeder: clarify produces the pickable specs the
drain then burns down.

## When this skill runs

- The human invokes `/aida-clarify <SPEC>` directly on one under-specified
  spec, or
- `aida questions clarify` launches an **interactive** advisor session
  (`claude "/aida-clarify <specs>"`, never headless `claude -p`) seeded with
  the swept set, and walks the human through each in turn.

This is an **interactive** skill — it asks the human real questions and waits
for real answers. If there is no human at the keyboard, stop: there is nothing
to clarify. (The headless analogue is `/aida-advise`, which resolves from
recorded corpus only and escalates everything else.)

## What you do NOT clarify (BUG-495 exclusions)

Before touching a spec, confirm it is actually clarifiable. **Skip** — do not
offer to clarify — any spec that is:

- **non-implementable by type** — `vision`, `folder`, `meta`, `principle`,
  `term`. These are not built; an acceptance section is meaningless on them.
- **already built / terminal** — status `Done`, `Completed`, or `Rejected`.
  The work is finished or abandoned; re-authoring acceptance is noise.
- **held for human review** — tagged `review:draft-only`. It already shipped a
  draft PR awaiting a person; it is past the acceptance-authoring stage.
- **actively being worked** — a live lease holds its scope. An implementer is
  mid-flight; do not move the goalposts under them. (Check
  `aida session leases`.)

If `aida questions clarify` handed you a set, it has already filtered these —
but re-verify per spec, because state drifts between the launch and your turn.

## The loop — per spec

### 1. Read the spec + its graph context

```bash
aida show <SPEC>
```

Read **all** of it: title, description, existing acceptance (if any), comments,
and the git-linkage block. Then pull the surrounding graph so your questions
are grounded, not generic:

```bash
aida graph <SPEC> --tree          # parent epic + siblings
aida graph <SPEC> --blocked-by    # what it waits on
```

Also note **why the sweep flagged it** — the spec carries a
`DecisionRequest` (visible in `aida show` / `aida questions list`) whose
`reason` is the sweep's flag (`missing acceptance criteria`,
`decision-marker text`, `blocked by ambiguous <ID>`). Lead your interrogation
at that gap.

Grep for any `trace:<SPEC>` markers already in the code — partial
implementation constrains what the acceptance can sensibly claim:

```bash
grep -rn "trace:<SPEC>" --include='*.rs' . 2>/dev/null
```

### 2. Interrogate — a SMALL, targeted question set

Ask the **fewest questions that unblock implementation**, not an exhaustive
intake form. Lead with these four, in this order:

1. **Definition of done** — what observable thing is true when this is
   finished? (The one criterion that, if it fails, the spec is not done.)
2. **Primary caller / environment** — who or what invokes this, and where?
   TTY vs headless, which agent/role, MCP tool vs CLI command. *This is the
   most common source of acceptance that contradicts reality* — name the
   caller explicitly and write each criterion against it.
3. **Output / shape** — what does it produce? stdout text, an exit code, a
   written file, a mutated spec, a JSON payload?
4. **Explicit out-of-scope** — what does this spec deliberately NOT do? (Bounds
   the implementer and prevents scope creep.)

Use **structured choices** (`AskUserQuestion`) where the fork is enumerable —
e.g. "CLI or MCP or both?", "TTY-only or headless-safe?". Use **free-text**
where the human is authoring rather than picking — e.g. the definition of done
in their own words.

Keep it tight. One or two follow-ups are fine if an answer opens a real fork;
a fifteen-question intake is the anti-pattern. Stop asking the moment you can
write a crisp, testable `## Acceptance`.

### 3. Reconcile conflict — never silently override

If a human answer **contradicts** the spec's existing description, comments, or
analysis, do not just take the new answer and bury the old one. Surface the
conflict explicitly:

> The spec's description says X, but you just said Y. Those disagree on
> <the point>. Which holds — and should I update the description to match, or
> are these two different things?

Reconcile to a single coherent intent. The acceptance you draft must not
contradict the body it sits in; if the body is now wrong, note that the
description needs an edit too (offer to make it). A spec whose acceptance
fights its own description is worse than no acceptance.

### 4. Draft the `## Acceptance` section — show, approve, bind

From the answers, draft a concrete, **testable** `## Acceptance` section.
Number each criterion; phrase each so a reviewer can mark it pass/fail without
re-interviewing the human. Name the primary caller in the criteria that depend
on it.

Show the draft and ask for approval or edits **before** binding. Once the human
approves:

```bash
aida edit <SPEC> --description "<full description incl. the new ## Acceptance>"
```

(`aida edit` replaces the description; preserve the existing body and append /
weave in the `## Acceptance` section — do not drop prose the human still
wants.)

Then **clear the gating parking tag** if the sign-off you just did is the thing
it was waiting on. Common gating tags: `needs-design-signoff`,
`needs-human-decision`, `needs-acceptance`. Remove only the tag whose condition
the clarification actually satisfied:

```bash
aida edit <SPEC> --tags "<remaining tags, gating tag removed>"
```

If the spec carried an open `DecisionRequest` from the sweep, answer it now
that the gap is closed:

```bash
aida questions answer <SPEC> <choice>   # e.g. the "proceed as written" choice
```

### 5. Re-check pickability — report autonomous-ready or what still blocks

After binding, re-run the detection to confirm the spec is no longer flagged,
and check it is actually pickable for a drain:

```bash
aida questions sweep            # the spec should no longer appear
aida burndown plan --json       # is <SPEC> now in `ready` (or at least pickable)?
```

Report the verdict per spec:

- **Autonomous-ready** — acceptance bound, gating tag cleared, no open
  decision, unblocked, not parked. Tell the human it is ready to queue / drain.
- **Still blocked** — name *what* still blocks it (an unsatisfied `BlockedBy`,
  a draft status that needs `aida queue add` to bless it, another open
  question). The human's next action should be obvious from your report.

### 6. Next spec

If `aida questions clarify` handed you a set, move to the next spec and repeat
from step 1. If the human wants to stop, stop cleanly — every spec you bound is
durable progress; an un-clarified tail is fine to leave.

## Out of scope

- **Implementing the spec** — you author acceptance, you do not write the code.
  That is the implementer's job once the spec is pickable.
- **Inventing acceptance the human didn't agree to** — you draft from their
  answers and show it for approval. You are a scribe with judgment, not an
  author overriding the human.
- **Clarifying excluded specs** — visions, terms, principles, already-built or
  held-for-review specs (see the BUG-495 exclusions above). If a spec lands in
  your set that shouldn't, skip it and say why.
- **Queueing / blessing the spec** — clarify makes a spec *pickable*; queueing
  it (`aida queue add`) is the separate advisor sign-off. Recommend it; don't
  do it silently.
