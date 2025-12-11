# Sprint and Epic Planning Design

This document outlines the design for adding Sprint-based planning capabilities to AIDA, enabling Agile/Scrum workflows while maintaining the existing functional decomposition hierarchy.

## Problem Statement

AIDA currently supports:
- **Parent/Child relationships** for functional decomposition (Feature → Sub-features → Tasks)
- **Existing Agile types**: Epic, Story, Task, Spike

However, there's no way to:
- Assign requirements to time-boxed Sprints
- Track work across multiple Sprints
- View requirements by Sprint assignment (Planning View)
- Handle backlog items (unassigned work)

## Key Insight: Orthogonal Concerns

A single requirement can have multiple relationships:

| Concern | Relationship | Example |
|---------|--------------|---------|
| **Functional Decomposition** | Parent/Child | "User Auth" Feature contains "Login" and "Logout" sub-features |
| **Strategic Grouping** | EpicContains | "User Auth" Epic groups related stories |
| **Work Planning** | SprintAssignment | "Login Form" Story assigned to Sprint 2 |

These are **orthogonal** - a Story can be:
- A child of Feature "User Auth" (functional)
- Part of Epic "Security Improvements" (strategic)
- Assigned to Sprint 3 (temporal)

Overloading Parent/Child for all these would be problematic.

## Design

### 1. New Requirement Type: Sprint

Add `Sprint` to the `RequirementType` enum:

```rust
pub enum RequirementType {
    // ... existing types ...
    Epic,
    Story,
    Task,
    Spike,
    Sprint,  // NEW: Time-boxed iteration
    Folder,
}
```

**Benefits of Sprint as Requirement:**
- Gets a unique ID (e.g., `SPRINT-001`)
- Full history tracking
- Comments for retrospectives
- Custom fields for metadata
- Consistent with existing data model

### 2. Sprint-Specific Custom Fields

Define these as project defaults or per-sprint:

| Field | Type | Description |
|-------|------|-------------|
| `sprint_number` | Integer | Sprint sequence number |
| `start_date` | Date | Sprint start date |
| `end_date` | Date | Sprint end date |
| `sprint_goal` | Text | Sprint goal/theme |
| `velocity` | Integer | Planned story points |
| `actual_velocity` | Integer | Completed story points |

### 3. New Relationship Type: SprintAssignment

Add to built-in relationship definitions:

```yaml
relationship_definitions:
  - name: sprint_assignment
    display_name: Sprint Assignment
    description: Assigns a requirement to a Sprint for work tracking
    inverse: sprint_contains
    symmetric: false
    cardinality: many_to_one  # Each item in one Sprint at a time
    source_types: [Story, Task, Bug, Spike, Functional, NonFunctional]
    target_types: [Sprint]
    built_in: true

  - name: sprint_contains
    display_name: Sprint Contains
    description: Inverse of Sprint Assignment
    inverse: sprint_assignment
    symmetric: false
    cardinality: one_to_many
    source_types: [Sprint]
    target_types: [Story, Task, Bug, Spike, Functional, NonFunctional]
    built_in: true
```

**Cardinality: Many-to-One**
- A requirement can be assigned to **one** Sprint at a time
- When moved to a new Sprint, the old relationship is removed
- History captures the change (supports carry-over tracking)

### 4. Epic Containment (Optional Enhancement)

If not using Parent/Child for Epics, add a dedicated relationship:

```yaml
relationship_definitions:
  - name: epic_contains
    display_name: Epic Contains
    description: Groups requirements under a strategic Epic
    inverse: part_of_epic
    symmetric: false
    cardinality: one_to_many
    source_types: [Epic]
    target_types: [Story, Task, Bug, Spike, Functional, NonFunctional]
    built_in: true
```

**Note:** Epics can also use existing Parent/Child if preferred. This provides flexibility.

### 5. Backlog Handling

**Definition:** Requirements with no `SprintAssignment` relationship = Backlog

**No explicit Backlog entity needed.** The Planning View will show:
- Active/Future Sprints with assigned items
- "Backlog" section: items without Sprint assignment

### 6. Planning View (GUI)

New view type alongside List, Tree, KanBan, Timeline:

```
┌─────────────────────────────────────────────────────────────┐
│ Planning View                          [+ New Sprint]       │
├─────────────────────────────────────────────────────────────┤
│ ▼ Sprint 3 (Current) - Dec 2-15                            │
│   ├─ STORY-042: User login form           [In Progress]    │
│   ├─ STORY-043: Password reset            [Draft]          │
│   └─ BUG-015: Fix session timeout         [In Progress]    │
│                                                             │
│ ▼ Sprint 4 (Upcoming) - Dec 16-29                          │
│   ├─ STORY-044: Two-factor auth           [Draft]          │
│   └─ STORY-045: OAuth integration         [Draft]          │
│                                                             │
│ ▼ Backlog (12 items)                                       │
│   ├─ STORY-050: Admin dashboard           [Draft]          │
│   ├─ STORY-051: Reporting module          [Draft]          │
│   └─ ... (show more)                                        │
└─────────────────────────────────────────────────────────────┘
```

**Features:**
- Drag-and-drop between Sprints/Backlog
- Sprint progress indicators (story points, completion %)
- Filter by Epic, Type, Status
- Collapse/expand Sprint sections
- Quick "Assign to Sprint" context menu action

### 7. Sprint Lifecycle

```
┌──────────┐    ┌──────────┐    ┌───────────┐    ┌──────────┐
│  Draft   │───▶│  Active  │───▶│ Completed │───▶│ Archived │
└──────────┘    └──────────┘    └───────────┘    └──────────┘
     │               │                │
     │               │                │
     ▼               ▼                ▼
 Planning        Execution       Retrospective
```

Sprint status maps to existing `RequirementStatus`:
- `Draft` - Planning phase
- `In Progress` - Active Sprint
- `Completed` - Sprint finished
- `Archived` - Historical record

### 8. Carry-Over Handling

When a Sprint completes with unfinished work:

1. **Manual Move:** User moves items to next Sprint
2. **History Tracking:** Original Sprint assignment captured in history
3. **Metrics:** Can query "items carried over from Sprint X"

Example history entry:
```
Sprint Assignment changed from "Sprint 3" to "Sprint 4"
Reason: Carry-over (incomplete)
```

### 9. CLI Commands

```bash
# Create a Sprint
aida add --type Sprint --title "Sprint 4" \
    --field sprint_number=4 \
    --field start_date=2024-12-16 \
    --field end_date=2024-12-29 \
    --field sprint_goal="Complete authentication module"

# Assign to Sprint
aida rel add --from STORY-042 --to SPRINT-004 --type sprint_assignment

# Move to different Sprint (removes old, adds new)
aida sprint assign STORY-042 SPRINT-005

# List Sprint contents
aida sprint list SPRINT-004

# List backlog (items without sprint)
aida list --no-sprint

# Sprint summary
aida sprint summary SPRINT-004
```

### 10. GUI Actions

**Context Menu (on any assignable requirement):**
- "Assign to Sprint..." → Sprint picker dialog
- "Move to Backlog" → Remove Sprint assignment

**Sprint Actions:**
- "Start Sprint" → Set status to In Progress
- "Complete Sprint" → Set status to Completed, prompt for carry-over
- "View Sprint Report" → Summary of completed/incomplete items

**Toolbar:**
- Sprint filter dropdown in Planning View
- "Show Completed Sprints" toggle

## Implementation Phases

### Phase 1: Core Model
- [ ] Add `Sprint` to `RequirementType` enum
- [ ] Add `sprint_assignment` / `sprint_contains` to built-in relationships
- [ ] Add sprint-related custom field definitions as defaults
- [ ] Update validation to allow Sprint relationships

### Phase 2: CLI Support
- [ ] Add `aida sprint` subcommand
- [ ] Implement `sprint assign`, `sprint list`, `sprint summary`
- [ ] Add `--no-sprint` filter option to `list` command

### Phase 3: Planning View (GUI)
- [ ] Create new `View::Planning` variant
- [ ] Implement Sprint-grouped tree display
- [ ] Add Backlog section for unassigned items
- [ ] Implement drag-and-drop for Sprint assignment

### Phase 4: Enhanced Features
- [ ] Sprint progress indicators (velocity, burn-down data)
- [ ] Carry-over workflow on Sprint completion
- [ ] Sprint report generation
- [ ] Epic grouping layer in Planning View

## Data Model Changes

### RequirementsStore additions

```rust
impl RequirementsStore {
    /// Get all Sprints, sorted by sprint_number or start_date
    pub fn get_sprints(&self) -> Vec<&Requirement> {
        self.requirements
            .iter()
            .filter(|r| r.req_type == RequirementType::Sprint)
            .collect()
    }

    /// Get items assigned to a specific Sprint
    pub fn get_sprint_items(&self, sprint_id: &Uuid) -> Vec<&Requirement> {
        self.requirements
            .iter()
            .filter(|r| {
                r.relationships.iter().any(|rel|
                    rel.rel_type == RelationshipType::Custom("sprint_assignment".into())
                    && rel.target_id == *sprint_id
                )
            })
            .collect()
    }

    /// Get backlog items (no Sprint assignment)
    pub fn get_backlog(&self) -> Vec<&Requirement> {
        self.requirements
            .iter()
            .filter(|r| {
                r.req_type != RequirementType::Sprint
                && r.req_type != RequirementType::Folder
                && !r.relationships.iter().any(|rel|
                    rel.rel_type == RelationshipType::Custom("sprint_assignment".into())
                )
            })
            .collect()
    }
}
```

## Migration

No migration needed for existing data:
- `Sprint` is a new type
- `sprint_assignment` is a new relationship
- Existing requirements remain unchanged
- Users opt-in to Sprint planning by creating Sprints

## Future Considerations

1. **Velocity Tracking:** Auto-calculate from story points
2. **Burndown Charts:** Visual Sprint progress
3. **Sprint Templates:** Copy Sprint structure for recurring patterns
4. **Team Assignment:** Link Sprints to Teams (requires Team entity)
5. **Release Planning:** Group Sprints into Releases
6. **Capacity Planning:** Track team capacity per Sprint

## Questions Resolved

| Question | Decision |
|----------|----------|
| Should Sprints be requirements? | Yes - gets IDs, history, comments |
| Many-to-many Sprint assignment? | No - one Sprint at a time, history tracks moves |
| Explicit Backlog entity? | No - filter by "no Sprint assignment" |
| Use existing Epic type? | Yes - already exists, can use Parent/Child or new EpicContains |
