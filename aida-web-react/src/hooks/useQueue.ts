import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import type { QueueEntry } from '@shared/types';
import { fetchQueue, addToQueue, removeFromQueue, reorderQueue } from '../api/queue';

// trace:STORY-0369 | ai:claude

const DEFAULT_USER = 'default';

export function useQueue(userId: string = DEFAULT_USER) {
  return useQuery<{ entries: QueueEntry[]; total: number }>({
    queryKey: ['queue', userId],
    queryFn: () => fetchQueue(userId),
    staleTime: 15_000,
  });
}

export function useAddToQueue(userId: string = DEFAULT_USER) {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (data: {
      requirement_id: string;
      position?: string;
      note?: string;
      added_by?: string;
    }) => addToQueue(userId, data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['queue'] });
    },
  });
}

export function useRemoveFromQueue(userId: string = DEFAULT_USER) {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (reqId: string) => removeFromQueue(userId, reqId),
    onMutate: async (reqId) => {
      await queryClient.cancelQueries({ queryKey: ['queue', userId] });
      const previous = queryClient.getQueryData<{
        entries: QueueEntry[];
        total: number;
      }>(['queue', userId]);

      queryClient.setQueryData<{ entries: QueueEntry[]; total: number }>(
        ['queue', userId],
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
        queryClient.setQueryData(['queue', userId], context.previous);
      }
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ['queue'] });
    },
  });
}

export function useReorderQueue(userId: string = DEFAULT_USER) {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (items: { requirement_id: string; position: number }[]) =>
      reorderQueue(userId, items),
    onMutate: async (items) => {
      await queryClient.cancelQueries({ queryKey: ['queue', userId] });
      const previous = queryClient.getQueryData<{
        entries: QueueEntry[];
        total: number;
      }>(['queue', userId]);

      // Optimistic: reorder entries based on new positions
      queryClient.setQueryData<{ entries: QueueEntry[]; total: number }>(
        ['queue', userId],
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
        queryClient.setQueryData(['queue', userId], context.previous);
      }
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ['queue'] });
    },
  });
}
