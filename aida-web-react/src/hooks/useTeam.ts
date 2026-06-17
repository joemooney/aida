import { useQuery } from '@tanstack/react-query';
import type { TeamMemberDto, CoordinationClaimDto } from '@shared/types';
import { fetchTeam, fetchCoordination } from '../api/team';

// trace:STORY-649 | ai:claude

export function useTeam() {
  return useQuery<{ members: TeamMemberDto[] }>({
    queryKey: ['team'],
    queryFn: fetchTeam,
    staleTime: 20_000,
  });
}

export function useCoordination() {
  return useQuery<{ claims: CoordinationClaimDto[] }>({
    queryKey: ['coordination'],
    queryFn: fetchCoordination,
    // Coordination claims (leases/drains) move faster than the roster.
    staleTime: 15_000,
  });
}
