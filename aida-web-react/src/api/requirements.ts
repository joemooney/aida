import type { Requirement, RequirementSummaryDto } from '@shared/types';
import { apiFetch } from './client';

export function fetchRequirements(): Promise<Requirement[]> {
  return apiFetch<Requirement[]>('/v2/requirements');
}

// BUG-571: the lightweight summary projection. The server omits the heavy
// nested arrays (comments / history / processing records) and the long
// description, so this payload is a small fraction of the full ~6.5MB blob.
// List / board / dashboard / team views fetch this; the single full record is
// loaded on demand when a spec is opened (fetchRequirement).
//
// The returned objects are a structural subset of `Requirement` (every field
// they carry has the same shape), so we type the result as `Requirement[]` for
// the views that only read summary fields — the omitted heavy fields read as
// `undefined`, which those views never touch. Views that DO need history /
// comments (Activity, Timeline, Sprint) keep using `fetchRequirements`.
export function fetchRequirementSummaries(): Promise<Requirement[]> {
  return apiFetch<RequirementSummaryDto[]>('/v2/requirements?view=summary') as Promise<
    Requirement[]
  >;
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
  return apiFetch<Requirement>('/v2/requirements', {
    method: 'POST',
    body: JSON.stringify(data),
  });
}

export function searchRequirements(query: string): Promise<Requirement[]> {
  return apiFetch<Requirement[]>(`/v2/search?q=${encodeURIComponent(query)}`);
}

export function reloadServer(): Promise<{ reloaded: boolean; requirements: number }> {
  return apiFetch('/v2/reload', { method: 'POST' });
}

export function setParent(id: string, parentId: string | null): Promise<Requirement> {
  return apiFetch<Requirement>(`/v2/requirements/${id}/parent`, {
    method: 'PUT',
    body: JSON.stringify({ parent_id: parentId }),
  });
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
