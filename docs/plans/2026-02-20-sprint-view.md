# Sprint View with Planning & Backlog

**Date**: 2026-02-20
**Commit**: b0e5025

## Status

Completed

## Context

Implement Sprint Planning view with drag-and-drop between backlog and sprints. Users can select a sprint from a horizontal card strip, then drag requirements between the backlog column and the sprint column to assign/unassign work.

## Phases

### Phase 1: REST API Endpoints (Rust)
- Added `PUT /api/v2/requirements/:id/sprint` to assign a requirement to a sprint
- Added `DELETE /api/v2/requirements/:id/sprint` to remove from sprint (back to backlog)
- Created `SprintAssignRequest` struct with `sprint_id` and optional `username`
- Handlers validate target is a Sprint type, call `store.assign_to_sprint()` / `store.remove_from_sprint()`

### Phase 2: TypeScript API, Hooks, and Utils
- Created `src/api/sprints.ts` with `assignToSprint()` and `removeFromSprint()` API functions
- Created `src/hooks/useSprints.ts` with `useAssignToSprint()` and `useRemoveFromSprint()` mutation hooks
- Created `src/lib/sprint-utils.ts` with sprint utility functions:
  - `isSprintAssignment()` -- type-checks `{ Custom: "sprint_assignment" }` relationship
  - `getSprintNumber()`, `getSprintGoal()`, `getSprintDates()` -- custom field accessors
  - `getSprintState()` -- returns `'active' | 'past' | 'future' | 'unknown'` based on dates
  - `computeSprintProgress()` -- calculates completion percentage and story points
  - `getSprintAssignmentTarget()` -- extracts sprint UUID from requirement relationships

### Phase 3: Sprint UI Components (7 files)
- `SprintView.tsx` -- Main page component at `/sprints` route, derives sprints/backlog from `useRequirements()`, DnD context
- `SprintSelector.tsx` -- Horizontal scrollable strip of sprint cards at top
- `SprintCard.tsx` -- Sprint card showing title, dates, state badge, progress bar
- `SprintBoard.tsx` -- Two-column layout (backlog + sprint items)
- `SprintColumn.tsx` -- Droppable column with header showing item count and story points
- `SprintItemCard.tsx` -- Draggable requirement card with spec_id, priority, type badge, story points
- `SprintProgressBar.tsx` -- Reusable progress bar with color-coded fill

### Phase 4: Integration
- Modified `App.tsx` to add `/sprints` route
- Modified `Sidebar.tsx` to add Sprints nav item with Zap icon

## DnD Logic

- Drag from backlog to sprint column: calls `assignToSprint(reqId, sprintId)`
- Drag from sprint to backlog: calls `removeFromSprint(reqId)`
- Same column drop: no-op
- Both mutations invalidate `['requirements']` query key

## Files Changed

| Action | File |
|--------|------|
| Modified | `aida-server/src/rest.rs` |
| Modified | `aida-web-react/src/App.tsx` |
| Modified | `aida-web-react/src/components/layout/Sidebar.tsx` |
| Created | `aida-web-react/src/api/sprints.ts` |
| Created | `aida-web-react/src/hooks/useSprints.ts` |
| Created | `aida-web-react/src/lib/sprint-utils.ts` |
| Created | `aida-web-react/src/components/sprint/SprintView.tsx` |
| Created | `aida-web-react/src/components/sprint/SprintSelector.tsx` |
| Created | `aida-web-react/src/components/sprint/SprintCard.tsx` |
| Created | `aida-web-react/src/components/sprint/SprintBoard.tsx` |
| Created | `aida-web-react/src/components/sprint/SprintColumn.tsx` |
| Created | `aida-web-react/src/components/sprint/SprintItemCard.tsx` |
| Created | `aida-web-react/src/components/sprint/SprintProgressBar.tsx` |

3 modified, 10 created (13 files total).

## Related Requirements

None explicitly referenced.

## Design Decisions

- **Data derivation**: Sprints and backlog are derived client-side from the existing `useRequirements()` query -- sprints are requirements with `req_type === 'Sprint'`, backlog items are non-sprint requirements without a sprint assignment relationship
- **Sprint state**: Computed from start/end date custom fields relative to current date
- **Story points**: Uses `weight` field on requirements
- **Build verification**: Both `cargo build -p aida-server` and `npm run build` passed (357KB JS, 31KB CSS)
