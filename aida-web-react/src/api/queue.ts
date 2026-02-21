import type { QueueEntry } from '@shared/types';
import { apiFetch } from './client';

// trace:STORY-0369 | ai:claude

export function fetchQueue(
  userId: string,
  includeCompleted = false,
): Promise<{ entries: QueueEntry[]; total: number }> {
  const params = includeCompleted ? '?includeCompleted=true' : '';
  return apiFetch(`/v2/queue/${encodeURIComponent(userId)}${params}`);
}

export function addToQueue(
  userId: string,
  data: {
    requirement_id: string;
    position?: string;
    note?: string;
    added_by?: string;
  },
): Promise<QueueEntry> {
  return apiFetch(`/v2/queue/${encodeURIComponent(userId)}`, {
    method: 'POST',
    body: JSON.stringify({
      requirementId: data.requirement_id,
      position: data.position,
      note: data.note,
      addedBy: data.added_by,
    }),
  });
}

export function removeFromQueue(
  userId: string,
  reqId: string,
): Promise<void> {
  return apiFetch(`/v2/queue/${encodeURIComponent(userId)}/${reqId}`, {
    method: 'DELETE',
  });
}

export function reorderQueue(
  userId: string,
  items: { requirement_id: string; position: number }[],
): Promise<void> {
  return apiFetch(`/v2/queue/${encodeURIComponent(userId)}/reorder`, {
    method: 'POST',
    body: JSON.stringify({
      items: items.map((i) => ({
        requirementId: i.requirement_id,
        position: i.position,
      })),
    }),
  });
}
