// trace:TASK-0373 | ai:claude
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useCallback, useRef, useState } from 'react';
import { fetchAdminStatus, fetchApiKeys, setApiKey, deleteApiKey } from '../api/admin';
import type { AdminStatus, ApiKeyInfo, SseStatusEvent, SseLogEvent } from '../api/admin';
import { getAuthToken } from '../api/client';
import { requireAdmin, usePermissions } from './usePermissions';

export function useAdminStatus() {
  const { canAdmin } = usePermissions();
  return useQuery<AdminStatus>({
    queryKey: ['admin', 'status'],
    queryFn: fetchAdminStatus,
    staleTime: 10_000,
    enabled: canAdmin,
  });
}

export type BuildPhase = 'idle' | 'building' | 'success' | 'failed' | 'restarting' | 'reconnecting';

export interface LogEntry {
  line: string;
  stream: 'stdout' | 'stderr';
  timestamp: number;
}

export function useRebuild() {
  const { canAdmin } = usePermissions();
  const [phase, setPhase] = useState<BuildPhase>('idle');
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [durationMs, setDurationMs] = useState<number | null>(null);
  const [exitCode, setExitCode] = useState<number | null>(null);
  const eventSourceRef = useRef<EventSource | null>(null);

  const startBuild = useCallback((restart: boolean) => {
    requireAdmin(canAdmin);
    // Close any existing connection
    eventSourceRef.current?.close();

    // Reset state
    setLogs([]);
    setPhase('building');
    setDurationMs(null);
    setExitCode(null);

    const params = new URLSearchParams();
    if (restart) params.set('restart', 'true');

    const token = getAuthToken();
    if (token) params.set('session_token', token);
    const es = new EventSource(`/api/v2/admin/rebuild?${params.toString()}`);
    eventSourceRef.current = es;

    es.addEventListener('log', (e: MessageEvent) => {
      const data: SseLogEvent = JSON.parse(e.data);
      setLogs((prev) => [...prev, { ...data, timestamp: Date.now() }]);
    });

    es.addEventListener('status', (e: MessageEvent) => {
      const data: SseStatusEvent = JSON.parse(e.data);
      setPhase(data.phase);
      if (data.durationMs != null) setDurationMs(data.durationMs);
      if (data.exitCode != null) setExitCode(data.exitCode);

      if (data.phase === 'restarting') {
        es.close();
        eventSourceRef.current = null;
        setPhase('reconnecting');
        pollForRestart();
      }

      if (data.phase === 'success' || data.phase === 'failed') {
        es.close();
        eventSourceRef.current = null;
      }
    });

    es.onerror = () => {
      // If we were building, the server may have gone away for restart
      if (phase === 'building') {
        es.close();
        eventSourceRef.current = null;
        setPhase('reconnecting');
        pollForRestart();
      }
    };
  }, [canAdmin, phase]);

  const pollForRestart = useCallback(() => {
    let attempts = 0;
    const maxAttempts = 40; // ~60 seconds

    const poll = () => {
      attempts++;
      if (attempts > maxAttempts) {
        setPhase('failed');
        return;
      }

      const token = getAuthToken();
      const statusUrl = token
        ? `/api/v2/admin/status?session_token=${encodeURIComponent(token)}`
        : '/api/v2/admin/status';
      fetch(statusUrl)
        .then((res) => {
          if (res.ok) {
            // Server is back
            window.location.reload();
          } else {
            setTimeout(poll, 1500);
          }
        })
        .catch(() => {
          setTimeout(poll, 1500);
        });
    };

    // Wait a moment before first poll (server needs time to exit)
    setTimeout(poll, 2000);
  }, []);

  return {
    phase,
    logs,
    durationMs,
    exitCode,
    startBuild,
    isBuilding: phase === 'building' || phase === 'restarting' || phase === 'reconnecting',
  };
}

// ============================================================================
// API Keys hooks
// ============================================================================

export function useApiKeys() {
  const { canAdmin } = usePermissions();
  return useQuery<ApiKeyInfo[]>({
    queryKey: ['admin', 'api-keys'],
    queryFn: fetchApiKeys,
    staleTime: 30_000,
    enabled: canAdmin,
  });
}

export function useSetApiKey() {
  const { canAdmin } = usePermissions();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ name, value }: { name: string; value: string }) =>
      {
        requireAdmin(canAdmin);
        return setApiKey(name, value);
      },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['admin', 'api-keys'] });
      queryClient.invalidateQueries({ queryKey: ['chat-status'] });
    },
  });
}

export function useDeleteApiKey() {
  const { canAdmin } = usePermissions();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => {
      requireAdmin(canAdmin);
      return deleteApiKey(name);
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['admin', 'api-keys'] });
      queryClient.invalidateQueries({ queryKey: ['chat-status'] });
    },
  });
}
