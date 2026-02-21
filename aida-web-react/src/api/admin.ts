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
