import type { TeamMemberDto, CoordinationClaimDto, Requirement } from '@shared/types';
import { apiFetch } from './client';

// trace:STORY-649 | ai:claude

export function fetchTeam(): Promise<{ members: TeamMemberDto[] }> {
  return apiFetch('/v2/team');
}

export function fetchCoordination(): Promise<{ claims: CoordinationClaimDto[] }> {
  return apiFetch('/v2/coordination');
}

// Slice C2 — write surfaces over the slice-C1 endpoints. trace:STORY-651 | ai:claude

/**
 * Set (or clear) a spec's durable assignee. The server also routes the spec
 * into the target user's queue and sends the assignment mailbox notice. Pass
 * `assignee: null` to unassign. Returns the updated requirement.
 * Mirrors `PUT /api/v2/requirements/:id/assignee`. trace:STORY-651 | ai:claude
 */
export function setAssignee(id: string, assignee: string | null): Promise<Requirement> {
  return apiFetch(`/v2/requirements/${encodeURIComponent(id)}/assignee`, {
    method: 'PUT',
    body: JSON.stringify({ assignee }),
  });
}

/** Shape of `PUT /api/v2/team/:user/role`'s response (not ts-rs exported). */
export interface SetRoleResponse {
  user: string;
  role: string;
  /** Guardrail-not-security framing the UI surfaces after a role change. */
  caveat: string;
}

/**
 * Set a team member's role. Returns the canonicalized role plus the guardrail
 * caveat the UI surfaces. Mirrors `PUT /api/v2/team/:user/role`.
 * trace:STORY-651 | ai:claude
 */
export function setTeamRole(user: string, role: string): Promise<SetRoleResponse> {
  return apiFetch(`/v2/team/${encodeURIComponent(user)}/role`, {
    method: 'PUT',
    body: JSON.stringify({ role }),
  });
}
