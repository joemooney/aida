---
name: aida-doc
description: Proactive living-documentation capture. Prompts the user for the WHY behind recently-touched specs (scenario, motivation, alternatives, example) and writes the answers to the store via `aida doc add`. Use at natural checkpoints — after /aida-pickup ships, after /aida-review merges, at session end, or manually when the user says "this needs docs."
allowed-tools:
  - Bash
  - Read
  - Grep
  - Edit
  - Write
---

# AIDA Doc Capture Skill

## Purpose

Catch the rationale, scenarios, and gotchas behind a piece of work
*while* the context is still fresh and turn them into structured `Doc`
requirements (see STORY-104). Specs in the store answer "what" — Doc
entries answer "why," "when," and "instead of what."

Without this skill, the rich context generated during work (design
debates, edge-case discoveries, alternative approaches considered)
evaporates after the PR ships. The acceptance section captures the
contract but not the journey.

## When to use

- **After `/aida-pickup` ships an item** — the implementer has the
  cleanest mental model right now. Capture before the context window
  rotates.
- **After `/aida-review` finishes** — review comments often surface
  rationale that should be documented (why a guard exists, why an
  alternative was rejected).
- **At session end** — last-chance prompt for the day's work; pairs
  well with `/aida-capture` (which catches missed *requirements*; this
  catches missed *narrative*).
- **Manually**, when the user says "this needs docs," "we should
  document the why," or "future-me won't remember this."

## Skip if

- The work in the session was pure refactor / rename / formatting —
  no narrative worth capturing.
- The spec already has a thorough description plus comments covering
  the WHY (run `aida show <id> --comments` to check before prompting).
- The user explicitly says "don't capture" or "skip docs for this."

## Recent activity (auto-rendered)

!`aida role show 2>/dev/null | sed -n '/Recent activity/,/^$/p' | head -15`

## Existing docs in this store (auto-rendered)

!`aida doc list 2>/dev/null | head -10 || echo "(none yet — this would be the first DOC entry)"`

## Workflow

### Step 1: Pick the candidate spec(s)

The skill accepts an optional spec id argument:

- `/aida-doc TASK-81` — focus on a specific spec the user named.
- `/aida-doc` (no arg) — scan the active role's recent activity
  (`aida role show`) for specs touched in the last few minutes
  whose status flipped to Completed or that had an `add`/`edit`
  event. Present the candidate list to the user; let them pick one
  or "skip all."

If the candidate already has Doc entries, surface them up front so
the user can decide whether to add a new perspective or amend an
existing one (`aida edit DOC-N` plus `aida comment add DOC-N`).

```bash
aida doc show <SPEC-ID>     # docs about <SPEC-ID>, if any
aida show <SPEC-ID> --comments   # full spec + existing narrative
```

### Step 2: Seed questions

For the chosen spec, ask the user **at most three** of the questions
below — whichever look most load-bearing for this kind of work. Don't
ask all four; the friction-to-value ratio matters. Bias toward
questions whose answers aren't already in the spec description.

1. **Scenario** — "When is this useful?" (One-line label, e.g.
   `"muddle recovery"`, `"first-time setup"`, `"offline workflow"`.)
2. **Motivation** — "What does this replace or improve? What used to
   happen before this shipped?" (One paragraph.)
3. **Alternatives** — "What did you consider and reject? When would
   you reach for a different tool instead?" (Brief decision tree.)
4. **Example** — "What's the canonical one-liner / minimal recipe?"
   (Copy-pasteable snippet.)

For each answer, offer the user a chance to revise: "Here's what I
captured: <draft>. Edit, accept, or skip?"

### Step 3: Draft the Doc body

Compose the answers into a short Markdown body. Recommended sections
(only include those the user actually answered):

```
## When to use
<scenario / motivation text>

## Alternatives
<alternatives text>

## Example
<code block>

## See also
- <parent EPIC / sibling stories>
- <related Doc entries from Step 1>
```

Don't pad with section headers the user didn't speak to — empty
"## TBD" boilerplate is worse than a shorter, true document.

### Step 4: Capture via `aida doc add`

Write the entry to the store:

```bash
aida doc add \
  --title "<one-line title — make it a question or a recipe name>" \
  --about <SPEC-ID> [--about <OTHER-SPEC-ID> ...] \
  --scenario "<scenario label from Step 2>" \
  --audience <user|agent|developer>[,<other>] \
  --description-from-file <tmpfile> \
  --tags <comma,separated,optional>
```

For long-form description bodies write the draft to a tempfile and
use `--description-from-file` (shell quoting on multi-paragraph
inline `--description "..."` is a footgun — `aida add` covers the
same ground via the same flag for the same reason, see BUG-17).

Each `--about` id is resolved against the store before the entry
lands; a phantom id fails fast with a clear error rather than
producing an orphaned doc.

### Step 5: Confirm + offer follow-ups

After the entry is written, surface:

- The new `DOC-N` id and one-line summary.
- A pointer back to the captured spec (`aida doc show <SPEC-ID>`).
- An offer to capture another candidate from Step 1, or stop.

Don't auto-loop — capture should feel like a deliberate moment, not
a treadmill.

## Title conventions

Good titles are searchable, scannable, and reused later as
book-chapter or tutorial-section headings:

- **Question form** for explanatory entries —
  `"When to use aida queue work --steal"`,
  `"Why doc entries are a top-level type, not a Meta subtype"`.
- **Recipe form** for how-tos —
  `"Recipe: drain a queued cluster with auto-first"`.
- **Avoid** bug-report-style titles —
  `"Fix for missing FR-1-042 panic"` belongs in commit messages, not
  Doc entries.

## Audience conventions

`--audience` accepts any comma-separated tags; the conventional set is:

- `user` — end-user-facing doc (how to drive AIDA from the CLI).
- `agent` — agent-facing doc (when an AI agent should reach for this
  surface vs. a different one).
- `developer` — contributor-facing doc (internal rationale for the
  next person editing this code).

Multi-tag is fine and common — `--audience user,agent` is the typical
default since most doc entries serve both.

## Pairs with

- **`/aida-pickup`** — after the implementer marks a spec done, this
  is the natural moment to capture the WHY. The pickup skill may
  surface `/aida-doc` as a "want to capture rationale?" prompt
  before stepping to the next queue item.
- **`/aida-review`** — review surfaces rationale ("why this guard,"
  "why this exception"). Promote the load-bearing review notes to
  Doc entries instead of letting them rot in the PR thread.
- **`/aida-capture`** — captures missed *requirements*. `/aida-doc`
  captures missed *narrative*. Either or both at session end.
- **STORY-104** — the `aida doc` data model + add/list/show surface
  this skill drives.
- **EPIC-24** — the living-documentation vision this fits into.

## CLI reference

```bash
# Capture a new entry
aida doc add --title "..." --about <SPEC> --scenario "..." \
             --audience user,agent --description "..."

# Browse + filter
aida doc list                       # all doc entries
aida doc list --about <SPEC>        # docs about <SPEC>
aida doc list --scenario "..."      # docs in a specific scenario
aida doc list --audience agent      # docs tagged for agents

# Show a single entry or every doc about a spec
aida doc show DOC-N                 # full detail for one entry
aida doc show <SPEC-ID>             # all docs about <SPEC-ID>

# Editing an existing entry uses the generic surface
aida edit DOC-N --status approved
aida comment add DOC-N "Follow-up: ..."
```
