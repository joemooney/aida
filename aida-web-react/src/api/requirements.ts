import type { Requirement } from '@shared/types';
import { apiFetch } from './client';

export function fetchRequirements(): Promise<Requirement[]> {
  return apiFetch<Requirement[]>('/v2/requirements');
}

export function fetchRequirement(id: string): Promise<Requirement> {
  return apiFetch<Requirement>(`/v2/requirements/${id}`);
}

export function updateRequirement(
  id: string,
  data: Partial<Requirement>,
): Promise<Requirement> {
  return apiFetch<Requirement>(`/v2/requirements/${id}`, {
    method: 'PUT',
    body: JSON.stringify(data),
  });
}

export function createRequirement(
  data: Partial<Requirement>,
): Promise<Requirement> {
  return apiFetch<Requirement>('/requirements', {
    method: 'POST',
    body: JSON.stringify(data),
  });
}

export function searchRequirements(query: string): Promise<Requirement[]> {
  return apiFetch<Requirement[]>(`/v2/search?q=${encodeURIComponent(query)}`);
}

export function addComment(
  id: string,
  content: string,
  author: string = 'web-user',
): Promise<void> {
  return apiFetch(`/requirements/${id}/comments`, {
    method: 'POST',
    body: JSON.stringify({ content, author }),
  });
}
