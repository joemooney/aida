---
name: aida-decompose
description: Break a large requirement into vertical slice child requirements. Each slice cuts through all layers (DB, API, UI) and is independently deliverable.
allowed-tools:
  - Bash
  - Read
  - Glob
  - Grep
---

# AIDA Decompose Skill

## Purpose

Break a large requirement into thin, vertical slice child requirements. Each slice cuts end-to-end through all relevant layers (database, API, UI, etc.) and is independently deliverable and testable.

This is NOT horizontal decomposition (build all the DB first, then all the API, then all the UI). Vertical slices let you ship working increments and get feedback early.

## When to Use

- Before implementing an epic, story, or large functional requirement
- When a requirement touches multiple layers and feels too big for a single PR
- When you want to parallelize work across agents or developers
- When the user says "break this down", "slice this", "decompose"
- Before `/aida-sprint` to create right-sized work items

## Workflow

### Step 1: Load the Requirement

```bash
aida show <SPEC-ID>
```

Understand the full scope: title, description, acceptance criteria, tags, and any existing children.

```bash
aida list --parent <SPEC-ID>
```

### Step 2: Identify the Layers Involved

Explore the codebase to understand which layers this requirement touches:

```bash
# Understand project structure
ls -d */
```

Common layers to look for:
- **Database**: schema changes, migrations, storage logic
- **API/Backend**: endpoints, service logic, validation
- **UI/Frontend**: components, views, user interactions
- **Infrastructure**: configuration, deployment, monitoring
- **Tests**: unit, integration, end-to-end

List the layers explicitly before proceeding.

### Step 3: Find Thin End-to-End Cuts

For each potential slice, ask:
- Does it deliver user-visible value (even if minimal)?
- Does it cut through all relevant layers?
- Can it be implemented and tested independently?
- Is it small enough for a single PR / work session?

**Think tracer bullet first.** The first slice should prove the architecture works end-to-end with the simplest possible case.

Example decomposition:
```
Epic: "User can manage project tags"

Slice 1 (tracer bullet): Add a single hardcoded tag to a project [AFK]
  - DB: tags table + project_tags join table
  - API: POST /projects/:id/tags endpoint
  - UI: "Add tag" button with hardcoded value
  - Proves: schema, API, UI wiring all work

Slice 2: Create/delete tags with user input [AFK]
  - DB: (reuse schema from slice 1)
  - API: full CRUD for tags
  - UI: tag input field, delete button

Slice 3: Tag autocomplete and search [AFK]
  - API: GET /tags?q= search endpoint
  - UI: autocomplete dropdown component

Slice 4: Bulk tag management [HITL]
  - UI: multi-select, bulk assign/remove
  - Needs UX review for interaction design
```

### Step 4: Create Child Requirements

For each slice, create a child requirement with clear acceptance criteria:

```bash
aida add \
  --title "Slice N: <concise description>" \
  --description "## Acceptance Criteria
- [ ] <criterion 1>
- [ ] <criterion 2>
- [ ] <criterion 3>

## Layers
- DB: <what changes>
- API: <what changes>
- UI: <what changes>

## Notes
- [AFK|HITL]: <reason>
- Depends on: <earlier slice if any>" \
  --type task \
  --status draft \
  --tags "vertical-slice"
```

### Step 5: Create Parent Relationships

Link each child to the parent requirement:

```bash
aida rel add --from <PARENT-SPEC-ID> --to <CHILD-SPEC-ID> --type Parent
```

### Step 6: Order and Tag Slices

Order slices so that:
1. **First slice = tracer bullet** (proves the architecture, smallest possible end-to-end)
2. **Core slices** (deliver the main value proposition)
3. **Enhancement slices** (polish, edge cases, advanced features)

Tag each slice:
- **[AFK]** — Agent can implement autonomously with clear acceptance criteria
- **[HITL]** — Needs human-in-the-loop (UX decisions, ambiguous requirements, external dependencies)

```bash
aida comment add <CHILD-SPEC-ID> "Slice order: N of M. [AFK|HITL]. <rationale>"
```

### Step 7: Record Rationale on Parent

```bash
aida comment add <PARENT-SPEC-ID> "Decomposed into N vertical slices. Tracer bullet: <CHILD-ID>. See children for full breakdown."
```

### Step 8: Present the Breakdown

Format as a numbered list:

```
## Decomposition of <SPEC-ID>: <title>

### Layers Identified
- DB, API, UI (list the actual layers)

### Slices (N total)

1. **<CHILD-ID>: <title>** [AFK] (tracer bullet)
   - DB: <change>  |  API: <change>  |  UI: <change>
   - Acceptance: <1-line summary>

2. **<CHILD-ID>: <title>** [AFK]
   - DB: <change>  |  API: <change>  |  UI: <change>
   - Acceptance: <1-line summary>
   - Depends on: Slice 1

3. **<CHILD-ID>: <title>** [HITL]
   - Reason for HITL: <why human needed>
   - ...

### Dependency Graph
Slice 1 -> Slice 2 -> Slice 3
                    -> Slice 4 (independent)

### Estimated Effort
- AFK slices: N (can be parallelized after tracer bullet)
- HITL slices: M (need human review)
```

## Anti-Patterns to Avoid

- **Horizontal slicing**: "First build the whole database, then the whole API" defeats the purpose. Every slice must be vertical.
- **Too-large slices**: If a slice takes more than one session, it needs further decomposition.
- **Missing the tracer bullet**: Always start with the thinnest possible slice that proves the architecture.
- **Ignoring dependencies**: Be explicit about which slices depend on which.
- **All AFK**: If every slice is AFK, you may be avoiding the hard design questions. Some slices genuinely need human judgment.

## CLI Reference

```bash
aida show <SPEC-ID>                    # Load the parent requirement
aida list --parent <SPEC-ID>           # Check for existing children
aida add --title "..." --type task     # Create child slice
aida rel add --from <ID> --to <ID> --type Parent  # Link child to parent
aida comment add <SPEC-ID> "..."       # Record decomposition rationale
aida edit <SPEC-ID> --status in-progress  # Update parent status
```
