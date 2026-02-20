import type { Requirement } from '@shared/types';
import { apiFetch } from './client';

export function assignToSprint(
  reqId: string,
  sprintId: string,
): Promise<Requirement> {
  return apiFetch<Requirement>(`/v2/requirements/${reqId}/sprint`, {
    method: 'PUT',
    body: JSON.stringify({ sprint_id: sprintId }),
  });
}

export function removeFromSprint(reqId: string): Promise<Requirement> {
  return apiFetch<Requirement>(`/v2/requirements/${reqId}/sprint`, {
    method: 'DELETE',
  });
}
