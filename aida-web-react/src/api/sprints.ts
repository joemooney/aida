import type { Requirement } from '@shared/types';
import { apiFetch } from './client';

export interface CreateSprintData {
  title: string;
  sprint_number: string;
  start_date: string;
  end_date: string;
  sprint_goal?: string;
  planned_velocity?: string;
}

export function createSprint(data: CreateSprintData): Promise<Requirement> {
  const { title, sprint_number, start_date, end_date, sprint_goal, planned_velocity } = data;
  const custom_fields: Record<string, string> = {
    sprint_number,
    start_date,
    end_date,
  };
  if (sprint_goal) custom_fields.sprint_goal = sprint_goal;
  if (planned_velocity) custom_fields.planned_velocity = planned_velocity;

  return apiFetch<Requirement>('/v2/requirements', {
    method: 'POST',
    body: JSON.stringify({
      title,
      req_type: 'sprint',
      status: 'approved',
      custom_fields,
    }),
  });
}

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
