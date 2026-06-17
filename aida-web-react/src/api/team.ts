import type { TeamMemberDto, CoordinationClaimDto } from '@shared/types';
import { apiFetch } from './client';

// trace:STORY-649 | ai:claude

export function fetchTeam(): Promise<{ members: TeamMemberDto[] }> {
  return apiFetch('/v2/team');
}

export function fetchCoordination(): Promise<{ claims: CoordinationClaimDto[] }> {
  return apiFetch('/v2/coordination');
}
