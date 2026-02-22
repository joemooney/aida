import type { StoredAiEvaluation } from '@shared/types';
import { apiFetch } from './client';

export function evaluateRequirement(id: string): Promise<StoredAiEvaluation> {
  return apiFetch<StoredAiEvaluation>(`/v2/requirements/${id}/evaluate`, { method: 'POST' });
}
