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

export interface ActivityStats {
  workedOn: number;
  queueSize: number;
  unqueuedWork: number;
  queueUntouched: number;
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

  return allEvents
    .filter((e) => {
      if (filterByAuthor && e.author.toLowerCase() !== userId.toLowerCase()) return false;
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

  return {
    workedOn: touchedReqIds.size,
    queueSize: queueEntries.length,
    unqueuedWork,
    queueUntouched,
  };
}

export function groupActivityByDate(items: ActivityItem[]): DateGroup[] {
  return groupEventsByDate(items);
}
