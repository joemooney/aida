---
name: aida-plan
description: Plan the implementation of a requirement before coding. Use when user wants to decompose, design, or plan a requirement's implementation.
allowed-tools:
  - Bash
  - Read
  - Glob
  - Grep
---
<!-- AIDA Generated: v2.0.0 | checksum:52467c63 | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


# AIDA Planning Skill

## Purpose

Plan the implementation of an approved requirement before coding begins. This ensures implementation is well-thought-out, decomposed into manageable pieces, and the plan is recorded in the requirements database.

## When to Use

Use this skill when:
- User says "plan <SPEC-ID>" or "plan implementation of <requirement>"
- A requirement is in `Approved` status and needs planning before implementation
- `/aida-implement` is invoked on a requirement that hasn't been planned yet
- User wants to decompose a large requirement into child requirements

## Core Principles

Planning separates design decisions from implementation, allowing you to review the approach before committing effort, identify risks early, and create a clear implementation roadmap. All planning decisions should be captured in the requirements database as child requirements, comments for design decisions, and a status transition to `Planned` when complete.

### Vertical Slices, Not Horizontal Layers

**This is the most important principle.** Decompose work into thin end-to-end slices through all layers, NOT into horizontal layers.

**Wrong (horizontal):**
1. Build the database schema
2. Build the API layer
3. Build the UI

**Right (vertical slices):**
1. Slice 1: User can create an account (DB + API + UI for signup only)
2. Slice 2: User can log in (DB + API + UI for login only)
3. Slice 3: User can reset password (DB + API + UI for reset only)

Each slice is:
- **Deployable independently** — delivers user-visible value
- **Testable end-to-end** — can verify the whole flow works
- **Small enough for one session** — keeps focus and momentum
- **Demonstrates the pattern** — later slices are faster because the first slice proves the architecture

When decomposing, ask: "Can this child requirement be demo'd to a user?" If not, it's probably a horizontal layer, not a vertical slice.

## Workflow

### Step 1: Load Requirement Context

```bash
aida show <SPEC-ID>
```

Display to user: SPEC-ID, title, description, status, priority, type, related requirements, and existing comments.

Verify the requirement is in `Approved` status. If not, inform the user:
- `Draft`: Needs approval first
- `Planned`: Already planned, proceed to `/aida-implement`
- `In Progress` or `Completed`: Already being/been implemented

### Step 2: Analyze Scope

Examine the requirement to understand:
1. What files will need to be created or modified?
2. What external dependencies are involved?
3. Are there any architectural decisions to make?
4. What are the edge cases and error scenarios?
5. Are there any unknowns or risks?

For each significant unknown, note it as a question to resolve during planning.

### Step 3: Decompose into Child Requirements

If the requirement is complex, break it into child requirements:

```bash
aida add --title "Component: User input validation" \
  --description "Validate user input for..." --type task --status draft
aida rel add --from <PARENT-ID> --to <CHILD-ID> --type Parent
```

Guidelines for decomposition:
- **Vertical slices**: each child delivers end-to-end value through all layers
- Each child should be implementable in a focused session
- Children should have clear boundaries
- Avoid too many children (3-7 is usually good)
- Order children so the first slice proves the architecture (the "tracer bullet")
- Tag each child as `[HITL]` (needs human review/decision) or `[AFK]` (agent can do autonomously)

### Step 4: Document Design Decisions

Record significant design decisions and identify affected files:

```bash
aida comment add <SPEC-ID> "Design: Using async/await pattern because..."
aida comment add <SPEC-ID> "Risk: External API rate limiting may need handling"
aida comment add <SPEC-ID> "Files to modify:
- src/models.rs: Add new struct
- src/handlers.rs: Add endpoint
- src/tests/mod.rs: Add unit tests"
```

### Step 5: Archive the plan to docs/plans/

For non-trivial work, write the full plan to `docs/plans/YYYY-MM-DD-<slug>.md`
using the structure in `docs/plans/_TEMPLATE.md` (scaffolded by `aida init`).
The template's 11 sections — Approach + diagram, Decisions, Files (in
build-order), Critical Files, Reusable helpers, Risks + gotchas, Tests
(named), Verification (executable), Followups, Related, plus a Date / Specs
/ Status / Complexity header — are what distinguish a plan from a wishlist.

Two conventions worth honoring:

- **Symbol refs over line refs.** Cite `fn handle_pull_command` not
  `main.rs:19713`. Symbol refs survive edits; line refs go stale fast.
- **Reusable helpers section.** Enumerate the existing helpers the
  implementer should call rather than re-invent (e.g.
  `extract_spec_ids_from_commit`, `git_ops::head_sha`,
  `Storage::update_atomically`). This is the highest-leverage section —
  it prevents accidental reimplementation. Seed it from the trace graph:

  ```bash
  aida plan helpers <SPEC-ID>                          # print the section
  aida plan helpers <SPEC-ID> --append docs/plans/<file>.md   # write it in
  ```

  `aida plan helpers` walks sibling / tag-mate / same-feature specs and
  harvests their `// trace:` comments — review the output and prune it to
  the helpers that actually matter for this plan.

Worked example: `docs/plans/2026-05-13-story-86-done-status.md`.

After writing the plan, lint it:

```bash
aida plan verify docs/plans/YYYY-MM-DD-<slug>.md
```

`aida plan verify` reports drifted `path:line` refs (with the corrected
line located by symbol name), missing files, and absent required sections.
It exits non-zero on any missing file or section — usable as a pre-commit
hook on `docs/plans/`. Pass `--fix` to rewrite drifted refs in place.

### Step 6: Mark as Planned

When planning is complete:

```bash
aida edit <SPEC-ID> --status planned
aida comment add <SPEC-ID> "Planning complete. Ready for implementation."
```

If child requirements were created, approve them:

```bash
aida edit <CHILD-ID> --status approved
```

### Step 7: Present Plan to User

Summarize for the user:
1. Overview of implementation approach
2. List of child requirements as vertical slices (numbered in implementation order)
3. Which slice is the "tracer bullet" (proves the architecture end-to-end)
4. Key design decisions made
5. Which items are `[HITL]` vs `[AFK]`
6. Any risks or unknowns identified

Ask if they want to proceed to implementation with `/aida-implement`.

## Status Transitions

During planning, requirements transition:
- **Approved** -> **Planned** (when planning is complete)

Child requirements created during planning start as:
- **Draft** -> **Approved** (when ready for implementation)

## Integration with /aida-implement

When `/aida-implement` is invoked on a requirement:
1. Check the status
2. If `Approved` (not `Planned`), suggest running `/aida-plan` first
3. If `Planned`, proceed with implementation

## CLI Reference

```bash
aida show <SPEC-ID>                              # Show requirement details
aida grep "keyword" -f description               # Search descriptions
aida grep -i "auth" --status approved            # Case insensitive, filter by status
aida grep -E "TODO|FIXME" -f comments            # Regex in comments
aida grep -l "database"                          # List matching SPEC-IDs only
aida add --title "..." --description "..." --type task --status draft
aida rel add --from <PARENT-ID> --to <CHILD-ID> --type Parent
aida comment add <SPEC-ID> "Design: ..."         # Add design comment
aida edit <SPEC-ID> --status planned             # Mark as planned
aida edit <CHILD-ID> --status approved           # Approve child requirements
```