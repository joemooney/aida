# My Activity: Reconcile Planned vs. Actual Work

**Date:** 2026-02-22

## Context

The Queue page shows what you *plan* to work on. But in practice, you work on other things — urgent bugs, features that came up in conversation, etc. There's no visibility into that gap. **My Activity** bridges it by showing a user's actual work (requirement changes, comments, creations) cross-referenced against their queue, surfacing the delta as actionable signal.

This is a **frontend-only v1** — no new backend endpoints. All data is derived client-side from `useRequirements()` and `useQueue()`.

## Approach

Reuse the existing `buildTimelineEvents()` utility from `timeline-utils.ts` to extract per-user activity from requirement history/comments. Cross-reference against queue entries to tag each activity item as "in queue" or "not in queue". Display summary stats highlighting the gap.

Same `?user=` URL param pattern as QueuePage for user scoping.

## Files Created (5)

| File | Description |
|------|-------------|
| `src/lib/activity-utils.ts` | Activity data transformation + stats computation |
| `src/components/activity/ActivityPage.tsx` | Main page with user scoping, time filter, two-column layout |
| `src/components/activity/ActivityStatsBar.tsx` | 5 stat cards (Worked On with breakdown, Completed, In Queue, Unqueued Work, Queue Untouched) |
| `src/components/activity/ActivityItemCard.tsx` | Single activity event with queue badge |
| `src/components/activity/ActivityDateGroup.tsx` | Date-grouped event list |

## Files Modified (3)

| File | Description |
|------|-------------|
| `src/App.tsx` | Add `/activity` route |
| `src/components/layout/Sidebar.tsx` | Add "My Activity" nav item with Activity icon |
| `src/hooks/useGlobalHotkeys.ts` | Add `g+a` chord shortcut |

## Data Flow

```
useRequirements() ──┐
                    ├──→ buildUserActivity(reqs, queue, userId, range)
useQueue(userId) ───┘         │
                              ├──→ ActivityItem[] (timeline events + inQueue flag)
                              │       │
                              │       ├──→ computeActivityStats() → ActivityStatsBar
                              │       └──→ groupActivityByDate()  → ActivityDateGroup[]
                              │                                         └── ActivityItemCard[]
                              └──→ detail panel (reuse TimelineDetailPanel)
```

## Stats Bar

5 stat cards:
- **Worked On** (blue) — unique requirements touched, with breakdown pills (Completed, In Progress, Approved, Created, Commented, Other)
- **Completed** (green) — requirements moved to Completed status in time range
- **In Queue** (emerald) — current queue size
- **Unqueued Work** (amber) — touched but not in queue (the interesting signal)
- **Queue Untouched** (slate) — in queue but no recent activity

## Related Requirements

- STORY-0369 (My Queue)
- STORY-0375 (Keyboard shortcuts)

## Status

Completed
