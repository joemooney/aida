import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import type { QueueEntry } from '@shared/types';
import { fetchQueue, addToQueue, removeFromQueue, reorderQueue } from '../api/queue';
import { useAuth } from './useAuth';
import { requireWrite, usePermissions } from './usePermissions';

// trace:STORY-0369 | ai:claude

const DEFAULT_USER = 'default';

function resolveUserId(inputUserId: string, authEnabled: boolean, userHandle?: string): string {
  if (inputUserId !== DEFAULT_USER) return inputUserId;
  if (authEnabled && userHandle) return userHandle;
  return DEFAULT_USER;
}

export function useQueue(userId: string = DEFAULT_USER) {
  const { authEnabled, user } = useAuth();
  const resolvedUserId = resolveUserId(userId, authEnabled, user?.handle);

  return useQuery<{ entries: QueueEntry[]; total: number }>({
    queryKey: ['queue', resolvedUserId],
    queryFn: () => fetchQueue(resolvedUserId),
    staleTime: 15_000,
  });
}

export function useAddToQueue(userId: string = DEFAULT_USER) {
  const { authEnabled, user } = useAuth();
  const { canWrite } = usePermissions();
  const resolvedUserId = resolveUserId(userId, authEnabled, user?.handle);
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (data: {
      requirement_id: string;
      position?: string;
      note?: string;
      added_by?: string;
    }) => {
      requireWrite(canWrite);
      return addToQueue(resolvedUserId, data);
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['queue'] });
    },
  });
}

export function useRemoveFromQueue(userId: string = DEFAULT_USER) {
  const { authEnabled, user } = useAuth();
  const { canWrite } = usePermissions();
  const resolvedUserId = resolveUserId(userId, authEnabled, user?.handle);
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (reqId: string) => {
      requireWrite(canWrite);
      return removeFromQueue(resolvedUserId, reqId);
    },
    onMutate: async (reqId) => {
      await queryClient.cancelQueries({ queryKey: ['queue', resolvedUserId] });
      const previous = queryClient.getQueryData<{
        entries: QueueEntry[];
        total: number;
      }>(['queue', resolvedUserId]);

      queryClient.setQueryData<{ entries: QueueEntry[]; total: number }>(
        ['queue', resolvedUserId],
        (old) => {
          if (!old) return old;
          const entries = old.entries.filter((e) => e.requirementId !== reqId);
          return { entries, total: entries.length };
        },
      );

      return { previous };
    },
    onError: (_err, _vars, context) => {
      if (context?.previous) {
        queryClient.setQueryData(['queue', resolvedUserId], context.previous);
      }
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ['queue'] });
    },
  });
}

export function useReorderQueue(userId: string = DEFAULT_USER) {
  const { authEnabled, user } = useAuth();
  const { canWrite } = usePermissions();
  const resolvedUserId = resolveUserId(userId, authEnabled, user?.handle);
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (items: { requirement_id: string; position: number }[]) => {
      requireWrite(canWrite);
      return reorderQueue(resolvedUserId, items);
    },
    onMutate: async (items) => {
      await queryClient.cancelQueries({ queryKey: ['queue', resolvedUserId] });
      const previous = queryClient.getQueryData<{
        entries: QueueEntry[];
        total: number;
      }>(['queue', resolvedUserId]);

      // Optimistic: reorder entries based on new positions
      queryClient.setQueryData<{ entries: QueueEntry[]; total: number }>(
        ['queue', resolvedUserId],
        (old) => {
          if (!old) return old;
          const posMap = new Map(
            items.map((i) => [i.requirement_id, i.position]),
          );
          const entries = old.entries
            .map((e) => ({
              ...e,
              position: posMap.get(e.requirementId) ?? e.position,
            }))
            .sort((a, b) => a.position - b.position);
          return { entries, total: entries.length };
        },
      );

      return { previous };
    },
    onError: (_err, _vars, context) => {
      if (context?.previous) {
        queryClient.setQueryData(['queue', resolvedUserId], context.previous);
      }
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ['queue'] });
    },
  });
}
