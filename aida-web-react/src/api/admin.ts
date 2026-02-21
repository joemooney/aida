// trace:TASK-0373 | ai:claude
import { apiFetch } from './client';

export interface AdminStatus {
  devMode: boolean;
  version: string;
  uptimeSeconds: number;
  building: boolean;
}

export interface SseStatusEvent {
  phase: 'building' | 'success' | 'failed' | 'restarting';
  durationMs?: number;
  exitCode?: number;
}

export interface SseLogEvent {
  line: string;
  stream: 'stdout' | 'stderr';
}

export function fetchAdminStatus(): Promise<AdminStatus> {
  return apiFetch<AdminStatus>('/v2/admin/status');
}

// ============================================================================
// API Keys
// ============================================================================

export interface ApiKeyInfo {
  name: string;
  isSet: boolean;
  source: string;
  maskedValue: string;
}

export function fetchApiKeys(): Promise<ApiKeyInfo[]> {
  return apiFetch<ApiKeyInfo[]>('/v2/admin/api-keys');
}

export function setApiKey(name: string, value: string): Promise<ApiKeyInfo> {
  return apiFetch<ApiKeyInfo>(`/v2/admin/api-keys/${encodeURIComponent(name)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ value }),
  });
}

export function deleteApiKey(name: string): Promise<ApiKeyInfo> {
  return apiFetch<ApiKeyInfo>(`/v2/admin/api-keys/${encodeURIComponent(name)}`, {
    method: 'DELETE',
  });
}
