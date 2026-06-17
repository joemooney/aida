import { Users } from 'lucide-react';
import { useTeam, useCoordination } from '../../hooks/useTeam';
import { useRequirements } from '../../hooks/useRequirements';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';
import { RosterPanel } from './RosterPanel';
import { AssignmentBoard } from './AssignmentBoard';
import { CoordinationPanel } from './CoordinationPanel';
import { TeamBurndown } from './TeamBurndown';

// trace:STORY-649 | ai:claude
// Slice B of the team dashboard (docs/plans/2026-06-17-team-dashboard.md):
// a read-only Team page over the slice-A /api/v2/team + /api/v2/coordination
// endpoints plus the existing requirements feed. Drag-to-reassign is slice C.

export function TeamPage() {
  const { data: teamData, isLoading: teamLoading, error: teamError } = useTeam();
  const { data: coordData, isLoading: coordLoading } = useCoordination();
  const { data: requirements, isLoading: reqLoading } = useRequirements();

  if (teamLoading || coordLoading || reqLoading) {
    return (
      <div className="flex items-center justify-center h-64">
        <Spinner size="lg" />
      </div>
    );
  }

  // A failed team fetch (e.g. endpoint unavailable) is treated as "no team
  // data yet" rather than a hard error, so the page degrades gracefully.
  const members = teamData?.members ?? [];
  const claims = coordData?.claims ?? [];
  const reqs = requirements ?? [];

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-xl font-semibold text-content">Team</h1>
        <p className="text-sm text-content-secondary mt-1">
          Who's on the team, who's assigned what, and who holds which claims.
        </p>
      </div>

      {teamError && members.length === 0 && claims.length === 0 && reqs.length === 0 ? (
        <EmptyState
          icon={<Users className="h-10 w-10" />}
          title="No team data yet"
          description="Register clones and roles, then assign work — see `aida team`."
        />
      ) : (
        <>
          <RosterPanel members={members} />

          <div className="grid grid-cols-1 gap-6 lg:grid-cols-2">
            <CoordinationPanel claims={claims} />
            <TeamBurndown requirements={reqs} />
          </div>

          <AssignmentBoard requirements={reqs} />
        </>
      )}
    </div>
  );
}
