import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type { Requirement, TeamMemberDto, CoordinationClaimDto } from '@shared/types';
import {
  fetchTeam,
  fetchCoordination,
  setAssignee,
  setTeamRole,
  type SetRoleResponse,
} from '../api/team';
import { usePermissions, requireWrite } from './usePermissions';

// trace:STORY-649 | ai:claude

// Live-refresh cadence for the team dashboard. Background refetch keeps the
// board current as the team works without a foreground spinner (React Query
// serves cached data while it refetches). slice C2. trace:STORY-651 | ai:claude
const TEAM_REFETCH_MS = 12_000;

export function useTeam() {
  return useQuery<{ members: TeamMemberDto[] }>({
    queryKey: ['team'],
    queryFn: fetchTeam,
    staleTime: 20_000,
    refetchInterval: TEAM_REFETCH_MS,
    refetchIntervalInBackground: false,
  });
}

export function useCoordination() {
  return useQuery<{ claims: CoordinationClaimDto[] }>({
    queryKey: ['coordination'],
    queryFn: fetchCoordination,
    // Coordination claims (leases/drains) move faster than the roster.
    staleTime: 15_000,
    refetchInterval: TEAM_REFETCH_MS,
    refetchIntervalInBackground: false,
  });
}

/**
 * Drag-to-reassign mutation. Optimistically rewrites the spec's `assignee` in
 * the cached requirements list, rolls back on error, and invalidates both the
 * team and requirements queries on settle so the board + roster reconcile.
 * `assignee: null` moves the card to the Unassigned column. trace:STORY-651
 */
export function useReassign() {
  const { canWrite } = usePermissions();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ id, assignee }: { id: string; assignee: string | null }) => {
      requireWrite(canWrite);
      return setAssignee(id, assignee);
    },
    onMutate: async ({ id, assignee }) => {
      await queryClient.cancelQueries({ queryKey: ['requirements'] });
      const previous = queryClient.getQueryData<Requirement[]>(['requirements']);

      queryClient.setQueryData<Requirement[]>(['requirements'], (old) =>
        old?.map((req) =>
          req.id === id || req.spec_id === id ? { ...req, assignee } : req,
        ),
      );

      return { previous };
    },
    onError: (_err, _vars, context) => {
      if (context?.previous) {
        queryClient.setQueryData(['requirements'], context.previous);
      }
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ['requirements'] });
      queryClient.invalidateQueries({ queryKey: ['team'] });
    },
  });
}

/**
 * Set-role mutation for the roster. Resolves with the server's
 * {@link SetRoleResponse} so callers can surface the guardrail `caveat`.
 * Invalidates the team query on settle. trace:STORY-651 | ai:claude
 */
export function useSetRole() {
  const { canWrite } = usePermissions();
  const queryClient = useQueryClient();

  return useMutation<SetRoleResponse, Error, { user: string; role: string }>({
    mutationFn: ({ user, role }) => {
      requireWrite(canWrite);
      return setTeamRole(user, role);
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ['team'] });
    },
  });
}
