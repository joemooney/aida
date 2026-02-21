# Sprint Metrics Tab with Sprint Picker

**Date**: 2026-02-20
**Commit**: 8361b7b

## Status

Completed

## Context

Move sprint charts from the bottom of the Sprint Board into a dedicated Metrics tab. Users can switch between Board and Metrics tabs, and pick any sprint (including past/archived) to view burndown, burn-up, and velocity charts independently of the currently selected sprint on the Board tab.

## Phases

### Phase 1: Tab Toggle UI
- Added Board/Metrics tab toggle below the sprint selector strip
- Used `LayoutGrid` and `BarChart3` icons from lucide-react
- Tab state managed via `activeTab: 'board' | 'metrics'` local state
- Styled with accent-colored bottom border for active tab

### Phase 2: MetricsTab Component
- Created inline `MetricsTab` component within `SprintView.tsx`
- Props: `allSprints`, `sprintItemsMap`, `metricsSprintId`, `onSelectSprint`
- Defaults to currently selected sprint; maintains separate selection state (`metricsSprintId`)

### Phase 3: Sprint Picker
- Horizontal row of pill-shaped sprint buttons
- Each button shows sprint number or title
- Selected sprint highlighted with accent background and white text
- Archived sprints shown at reduced opacity
- Clicking a sprint loads its charts without affecting the Board tab selection

### Phase 4: Chart Integration
- Renders existing `SprintCharts` component (burndown, burn-up, velocity) for the picked sprint
- Shows `EmptyState` when no sprint data is available
- Charts receive items from the `sprintItemsMap` keyed by selected sprint ID

## Files Changed

| Action | File |
|--------|------|
| Modified | `aida-web-react/src/components/sprint/SprintView.tsx` |

1 file changed (127 insertions, 29 deletions).

## Related Requirements

None explicitly referenced.

## Design Decisions

- **Inline component**: `MetricsTab` is defined in the same file as `SprintView` since it is tightly coupled and relatively small
- **Independent sprint selection**: Metrics tab maintains its own `metricsSprintId` state so users can browse historical sprint data without changing the Board tab's active sprint
- **Reuses existing charts**: No new chart components were needed; the existing `SprintCharts`, `BurndownChart`, `BurnupChart`, and `VelocityChart` are rendered as-is
