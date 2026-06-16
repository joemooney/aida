---
name: aida-sprint
description: Sprint planning — select approved requirements, group by feature, create sprint containers, and set priorities for the iteration.
allowed-tools:
  - Bash
---
<!-- AIDA Generated: v2.0.0 | checksum:df533e59 | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


# AIDA Sprint Planning Skill

## Purpose

Plan a development sprint by selecting approved requirements, grouping them by feature area, estimating scope, and creating a sprint container to track progress.

## When to Use

Use this skill when:
- User wants to plan a sprint or iteration
- User asks "what should we work on next?"
- User wants to prioritize approved requirements
- At the start of a development cycle

## Available Work

!`aida list --status approved 2>/dev/null | head -20 || echo "none"`

## Workflow

### Step 1: Review Available Requirements

```bash
# All approved requirements by priority
aida list --status approved --priority high 2>/dev/null
aida list --status approved --priority medium 2>/dev/null

# In-progress work (carry-overs)
aida list --status in-progress 2>/dev/null
```

### Step 2: Group by Feature

```bash
# List features with requirement counts
aida list 2>/dev/null
```

Present requirements grouped by feature area, with priority indicators.

### Step 3: Select Sprint Scope

Ask user to select requirements for the sprint. Consider:
- **Priority**: High-priority items first
- **Dependencies**: Check for blocking relationships
- **Balance**: Mix of features, fixes, and technical debt
- **Capacity**: Reasonable amount for the sprint duration

### Step 4: Create Sprint Container

```bash
# Create sprint requirement as organizational container
aida add --title "Sprint <N>: <theme>" \
  --description "Sprint goals: ..." \
  --type sprint \
  --status in-progress

# Link selected requirements as children
aida rel add --from <SPRINT-ID> --to <REQ-ID> --type Parent
```

### Step 5: Update Statuses

```bash
# Mark sprint items as in-progress
aida edit <REQ-ID> --status in-progress
```

### Step 6: Present Sprint Plan

```
## Sprint Plan: <Sprint N>

### Goals
- <Primary objective>
- <Secondary objective>

### Selected Requirements (N items)

**High Priority**
- [SPEC-ID] Title (Feature: X)
- [SPEC-ID] Title (Feature: Y)

**Medium Priority**
- [SPEC-ID] Title
- [SPEC-ID] Title

### Carry-overs from Previous Sprint
- [SPEC-ID] Title (in-progress)

### Sprint Container: <SPRINT-ID>
```

## CLI Reference

```bash
aida list --status approved                       # Available work
aida list --status in-progress                    # Current work
aida list                                 # Feature areas
aida add --title "..." --type sprint              # Create sprint
aida rel add --from <A> --to <B> --type Parent    # Link to sprint
aida edit <SPEC-ID> --status in-progress          # Start work
```

## Best Practices

- Keep sprints focused — 5-10 requirements per sprint for a small team
- Always include carry-over items from the previous sprint
- Balance feature work with bug fixes and technical debt
- Use the sprint container to track overall progress