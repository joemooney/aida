import type { Requirement } from '@shared/types';
import type { QueueEntry } from '@shared/types';
import {
  buildTimelineEvents,
  groupEventsByDate,
  type TimelineEvent,
  type DateGroup,
} from './timeline-utils';

export interface ActivityItem extends TimelineEvent {
  inQueue: boolean;
}

export interface StatusBreakdown {
  completed: number;
  inProgress: number;
  approved: number;
  created: number;
  commented: number;
  other: number;
}

export interface ActivityStats {
  workedOn: number;
  queueSize: number;
  unqueuedWork: number;
  queueUntouched: number;
  statusBreakdown: StatusBreakdown;
}

export type TimeRange = 'today' | 'week' | 'month' | 'all';

function getTimeRangeCutoff(range: TimeRange): Date | null {
  if (range === 'all') return null;
  const now = new Date();
  switch (range) {
    case 'today': {
      const start = new Date(now);
      start.setHours(0, 0, 0, 0);
      return start;
    }
    case 'week': {
      const start = new Date(now);
      start.setDate(start.getDate() - 7);
      return start;
    }
    case 'month': {
      const start = new Date(now);
      start.setMonth(start.getMonth() - 1);
      return start;
    }
  }
}

/** Check if an event author matches the given userId (handle).
 *  Handles variations: "@joe", "joe", "Joe Mooney" (matched via owner lookup). */
function authorMatches(author: string, userId: string, ownersByReq: Map<string, string>, reqId: string): boolean {
  const a = author.toLowerCase().replace(/^@/, '');
  const u = userId.toLowerCase();
  if (a === u) return true;
  // Also match if the requirement's owner is the userId (covers name-based authors)
  const owner = ownersByReq.get(reqId)?.toLowerCase();
  return owner === u;
}

export function buildUserActivity(
  requirements: Requirement[],
  queueEntries: QueueEntry[],
  userId: string,
  timeRange: TimeRange,
): ActivityItem[] {
  const allEvents = buildTimelineEvents(requirements);

  const queueReqIds = new Set(queueEntries.map((e) => e.requirementId));

  const cutoff = getTimeRangeCutoff(timeRange);

  const filterByAuthor = userId !== 'default';

  // Build owner lookup for fallback matching
  const ownersByReq = new Map(requirements.map((r) => [r.id, r.owner]));

  return allEvents
    .filter((e) => {
      if (filterByAuthor && !authorMatches(e.author, userId, ownersByReq, e.reqId)) return false;
      if (cutoff && new Date(e.timestamp) < cutoff) return false;
      return true;
    })
    .map((e) => ({
      ...e,
      inQueue: queueReqIds.has(e.reqId),
    }));
}

export function computeActivityStats(
  activityItems: ActivityItem[],
  queueEntries: QueueEntry[],
): ActivityStats {
  const touchedReqIds = new Set(activityItems.map((item) => item.reqId));
  const queueReqIds = new Set(queueEntries.map((e) => e.requirementId));

  const unqueuedWork = [...touchedReqIds].filter((id) => !queueReqIds.has(id)).length;
  const queueUntouched = [...queueReqIds].filter((id) => !touchedReqIds.has(id)).length;

  // Count status transitions and event types across activity
  const breakdown: StatusBreakdown = {
    completed: 0,
    inProgress: 0,
    approved: 0,
    created: 0,
    commented: 0,
    other: 0,
  };

  // Track unique reqs per category to avoid double-counting
  const completedReqs = new Set<string>();
  const inProgressReqs = new Set<string>();
  const approvedReqs = new Set<string>();
  const createdReqs = new Set<string>();
  const commentedReqs = new Set<string>();
  const otherReqs = new Set<string>();

  for (const item of activityItems) {
    if (item.eventType === 'Created') {
      createdReqs.add(item.reqId);
    } else if (item.eventType === 'CommentAdded') {
      commentedReqs.add(item.reqId);
    } else if (item.eventType === 'Modified') {
      const statusChange = item.changes.find((c) => c.field_name === 'status');
      if (statusChange) {
        const newStatus = statusChange.new_value.toLowerCase();
        if (newStatus === 'completed') {
          completedReqs.add(item.reqId);
        } else if (newStatus === 'inprogress' || newStatus === 'in-progress') {
          inProgressReqs.add(item.reqId);
        } else if (newStatus === 'approved') {
          approvedReqs.add(item.reqId);
        } else {
          otherReqs.add(item.reqId);
        }
      } else {
        otherReqs.add(item.reqId);
      }
    }
  }

  breakdown.completed = completedReqs.size;
  breakdown.inProgress = inProgressReqs.size;
  breakdown.approved = approvedReqs.size;
  breakdown.created = createdReqs.size;
  breakdown.commented = commentedReqs.size;
  breakdown.other = otherReqs.size;

  return {
    workedOn: touchedReqIds.size,
    queueSize: queueEntries.length,
    unqueuedWork,
    queueUntouched,
    statusBreakdown: breakdown,
  };
}

export function groupActivityByDate(items: ActivityItem[]): DateGroup[] {
  return groupEventsByDate(items);
}
