# Timeline View for Web Dashboard

**Date**: 2026-02-20
**Commit**: a33cac0

## Status

Completed

## Context

Add a Timeline view showing a chronological event feed from requirement history, comments, and creation events. The view provides a two-column layout with a scrollable event list on the left and a detail panel on the right. All data is built client-side from the existing `useRequirements()` hook -- no backend changes were required.

## Phases

### Phase 1: Timeline Utility Functions
- Created `timeline-utils.ts` with functions to build, filter, and group timeline events from requirements data
- Event types: Created, Modified (field changes), CommentAdded
- Events are collected from requirement creation timestamps, history entries, and comments (recursive traversal)
- Sorted chronologically (newest first) and grouped by date

### Phase 2: Timeline Event Card
- Created `TimelineEventCard.tsx` -- single event row with icon, time, spec ID, title, and author avatar
- Event type icons differentiate creation, modification, and comment events

### Phase 3: Timeline Date Group
- Created `TimelineDateGroup.tsx` -- sticky date header with grouped event cards beneath
- Provides visual separation between days

### Phase 4: Timeline Detail Panel
- Created `TimelineDetailPanel.tsx` -- right-column detail showing event info, field change diffs, and comment content
- For modification events: displays old/new value pairs for each changed field
- For comment events: displays the comment content

### Phase 5: Timeline Filter Bar
- Created `TimelineFilterBar.tsx` -- author and field text filters with event count and clear button
- Allows narrowing the event feed by author name or specific field changes

### Phase 6: Timeline View (Top-Level)
- Created `TimelineView.tsx` -- top-level two-column layout with scrollable event list and detail panel
- Click an event to see details in the right panel

### Phase 7: Routing and Navigation
- Modified `App.tsx` to add `/timeline` route
- Modified `Sidebar.tsx` to add Timeline nav item with Clock icon between Sprints and Skills

## Files Changed

| Action | File |
|--------|------|
| Modified | `aida-web-react/src/App.tsx` |
| Modified | `aida-web-react/src/components/layout/Sidebar.tsx` |
| Created | `aida-web-react/src/lib/timeline-utils.ts` |
| Created | `aida-web-react/src/components/timeline/TimelineEventCard.tsx` |
| Created | `aida-web-react/src/components/timeline/TimelineDateGroup.tsx` |
| Created | `aida-web-react/src/components/timeline/TimelineDetailPanel.tsx` |
| Created | `aida-web-react/src/components/timeline/TimelineFilterBar.tsx` |
| Created | `aida-web-react/src/components/timeline/TimelineView.tsx` |

2 modified, 6 created (8 files total).

## Related Requirements

None explicitly referenced.

## Design Decisions

- **Client-side data derivation**: All timeline events are built from the existing requirements data fetched by `useRequirements()`, avoiding any new backend API endpoints
- **Event sources**: Three event types are extracted: requirement creation (from `created_at`), field modifications (from `history` entries with `FieldChange` details), and comments (recursive traversal of comment trees)
- **Sticky date headers**: Date groups use sticky positioning so the current date context remains visible while scrolling
- **No pagination**: All events are loaded and rendered; filtering reduces the visible set
