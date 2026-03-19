import { apiFetch } from './client';

export interface JiraSyncDiff {
  field: string;
  aida_value: string;
  jira_value: string;
}

export interface JiraSyncItem {
  aida_id: string;
  aida_title: string;
  aida_status: string;
  jira_key: string;
  jira_status: string | null;
  jira_summary: string | null;
  sync_status: 'in_sync' | 'drifted' | 'error' | 'unchecked';
  diffs: JiraSyncDiff[];
}

export interface JiraSyncResponse {
  items: JiraSyncItem[];
  total: number;
  in_sync: number;
  drifted: number;
  errors: number;
}

export function fetchJiraSync(): Promise<JiraSyncResponse> {
  return apiFetch<JiraSyncResponse>('/v2/jira/sync');
}
