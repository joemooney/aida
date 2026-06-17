import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import type { Requirement } from '@shared/types';
import {
  fetchRequirements,
  fetchRequirementSummaries,
  fetchRequirement,
  updateRequirement,
  createRequirement,
  setParent,
} from '../api/requirements';
import { usePermissions, requireWrite } from './usePermissions';

// BUG-571: cache keys for the two list flavors. The lightweight summary list
// (list/board/dashboard/team views) and the full list (Activity/Timeline/
// Sprint, which need history+comments) use DISTINCT keys so they don't fight
// over one cache entry when both are mounted (the Sidebar is always mounted).
// Mutations invalidate BOTH so edits reconcile everywhere. trace:BUG-571
export const REQUIREMENTS_KEY = ['requirements'] as const;
export const REQUIREMENT_SUMMARIES_KEY = ['requirements', 'summary'] as const;

/**
 * Full requirement list — includes the heavy nested arrays (comments, history,
 * processing records). Only the views that actually read those fields use it
 * (Activity, Timeline, Sprint). It does NOT poll on an interval, so it never
 * re-downloads the ~6.5MB blob in the background. trace:BUG-571 | ai:claude
 */
export function useRequirements() {
  return useQuery<Requirement[]>({
    queryKey: REQUIREMENTS_KEY,
    queryFn: fetchRequirements,
    staleTime: 30_000,
  });
}

/**
 * Lightweight summary list for the list / board / dashboard / team views. The
 * server omits comments/history/processing/description, so this payload is a
 * small fraction of the full blob. This is the hook that live-refreshes on an
 * interval (cheap to re-fetch). trace:STORY-651 | ai:claude trace:BUG-571
 */
export function useRequirementSummaries() {
  return useQuery<Requirement[]>({
    queryKey: REQUIREMENT_SUMMARIES_KEY,
    queryFn: fetchRequirementSummaries,
    staleTime: 30_000,
    // Live refresh: background refetch keeps the team dashboard's assignment
    // board (and other live views) current as the team works. React Query
    // serves cached data while it refetches, so there is no foreground flicker.
    // Now polls the cheap summary endpoint, never the 6.5MB full list.
    refetchInterval: 15_000,
    refetchIntervalInBackground: false,
  });
}

export function useRequirement(id: string | null) {
  return useQuery<Requirement>({
    queryKey: ['requirement', id],
    queryFn: () => fetchRequirement(id!),
    enabled: !!id,
  });
}

export function useUpdateRequirement() {
  const { canWrite } = usePermissions();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ id, data }: { id: string; data: Partial<Requirement> }) => {
      requireWrite(canWrite);
      return updateRequirement(id, data);
    },
    onMutate: async ({ id, data }) => {
      // Cancel + snapshot BOTH list flavors (full + summary). trace:BUG-571
      await queryClient.cancelQueries({ queryKey: REQUIREMENTS_KEY });
      await queryClient.cancelQueries({ queryKey: REQUIREMENT_SUMMARIES_KEY });
      await queryClient.cancelQueries({ queryKey: ['requirement', id] });
      const previous = queryClient.getQueryData<Requirement[]>(REQUIREMENTS_KEY);
      const previousSummaries = queryClient.getQueryData<Requirement[]>(REQUIREMENT_SUMMARIES_KEY);
      const previousSingle = queryClient.getQueryData<Requirement>(['requirement', id]);

      const patch = (old: Requirement[] | undefined) =>
        old?.map((req) =>
          req.id === id || req.spec_id === id ? { ...req, ...data } : req,
        );
      queryClient.setQueryData<Requirement[]>(REQUIREMENTS_KEY, patch);
      queryClient.setQueryData<Requirement[]>(REQUIREMENT_SUMMARIES_KEY, patch);
      if (previousSingle) {
        queryClient.setQueryData<Requirement>(['requirement', id], { ...previousSingle, ...data });
      }

      return { previous, previousSummaries, previousSingle };
    },
    onError: (_err, { id }, context) => {
      if (context?.previous) {
        queryClient.setQueryData(REQUIREMENTS_KEY, context.previous);
      }
      if (context?.previousSummaries) {
        queryClient.setQueryData(REQUIREMENT_SUMMARIES_KEY, context.previousSummaries);
      }
      if (context?.previousSingle) {
        queryClient.setQueryData(['requirement', id], context.previousSingle);
      }
    },
    onSettled: (_data, _err, { id }) => {
      // Partial-match invalidation covers both ['requirements'] and
      // ['requirements','summary']. trace:BUG-571
      queryClient.invalidateQueries({ queryKey: REQUIREMENTS_KEY });
      queryClient.invalidateQueries({ queryKey: ['requirement', id] });
    },
  });
}

export function useCreateRequirement() {
  const { canWrite } = usePermissions();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (data: Partial<Requirement>) => {
      requireWrite(canWrite);
      return createRequirement(data);
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['requirements'] });
    },
  });
}

export function useSetParent() {
  const { canWrite } = usePermissions();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, parentId }: { id: string; parentId: string | null }) => {
      requireWrite(canWrite);
      return setParent(id, parentId);
    },
    onSettled: (_data, _err, { id }) => {
      queryClient.invalidateQueries({ queryKey: ['requirements'] });
      queryClient.invalidateQueries({ queryKey: ['requirement', id] });
    },
  });
}
