import type { Requirement, FieldChange, Comment } from '@shared/types';

export type TimelineEventType = 'Created' | 'Modified' | 'CommentAdded';

export interface TimelineEvent {
  id: string;
  timestamp: string;
  eventType: TimelineEventType;
  reqId: string;
  specId: string;
  reqTitle: string;
  author: string;
  description: string;
  changes: FieldChange[];
  commentContent?: string;
}

export interface DateGroup {
  dateKey: string;
  label: string;
  events: TimelineEvent[];
}

function flattenComments(comments: Comment[]): Comment[] {
  const result: Comment[] = [];
  for (const c of comments) {
    result.push(c);
    if (c.replies?.length) {
      result.push(...flattenComments(c.replies));
    }
  }
  return result;
}

export function buildTimelineEvents(requirements: Requirement[]): TimelineEvent[] {
  const events: TimelineEvent[] = [];

  for (const req of requirements) {
    const specId = req.spec_id ?? req.id;

    // Created event
    events.push({
      id: `${req.id}-created`,
      timestamp: req.created_at,
      eventType: 'Created',
      reqId: req.id,
      specId,
      reqTitle: req.title,
      author: req.created_by ?? req.owner,
      description: `Created ${specId}: ${req.title}`,
      changes: [],
    });

    // History entries → Modified events
    if (req.history) {
      for (const entry of req.history) {
        const changedFields = entry.changes.map((c) => c.field_name).join(', ');
        events.push({
          id: `${req.id}-history-${entry.id}`,
          timestamp: entry.timestamp,
          eventType: 'Modified',
          reqId: req.id,
          specId,
          reqTitle: req.title,
          author: entry.author,
          description: `Modified ${changedFields} on ${specId}`,
          changes: entry.changes,
        });
      }
    }

    // Comments → CommentAdded events (recursively flattened)
    if (req.comments) {
      const allComments = flattenComments(req.comments);
      for (const comment of allComments) {
        events.push({
          id: `${req.id}-comment-${comment.id}`,
          timestamp: comment.created_at,
          eventType: 'CommentAdded',
          reqId: req.id,
          specId,
          reqTitle: req.title,
          author: comment.author,
          description: `Commented on ${specId}`,
          changes: [],
          commentContent: comment.content,
        });
      }
    }
  }

  // Sort newest-first
  events.sort((a, b) => new Date(b.timestamp).getTime() - new Date(a.timestamp).getTime());
  return events;
}

export function filterTimelineEvents(
  events: TimelineEvent[],
  authorFilter: string,
  fieldFilter: string,
): TimelineEvent[] {
  let filtered = events;

  if (authorFilter) {
    const lower = authorFilter.toLowerCase();
    filtered = filtered.filter((e) => e.author.toLowerCase().includes(lower));
  }

  if (fieldFilter) {
    const lower = fieldFilter.toLowerCase();
    filtered = filtered.filter(
      (e) =>
        e.eventType === 'Modified' &&
        e.changes.some((c) => c.field_name.toLowerCase().includes(lower)),
    );
  }

  return filtered;
}

export function groupEventsByDate(events: TimelineEvent[]): DateGroup[] {
  const groups = new Map<string, TimelineEvent[]>();

  for (const event of events) {
    const date = new Date(event.timestamp);
    const dateKey = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`;
    const existing = groups.get(dateKey);
    if (existing) {
      existing.push(event);
    } else {
      groups.set(dateKey, [event]);
    }
  }

  const result: DateGroup[] = [];
  for (const [dateKey, groupEvents] of groups) {
    const date = new Date(dateKey + 'T12:00:00');
    const label = date.toLocaleDateString('en-US', {
      weekday: 'long',
      year: 'numeric',
      month: 'long',
      day: 'numeric',
    });
    result.push({ dateKey, label, events: groupEvents });
  }

  // Sort newest date first
  result.sort((a, b) => b.dateKey.localeCompare(a.dateKey));
  return result;
}
