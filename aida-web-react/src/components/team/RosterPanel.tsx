import type { TeamMemberDto } from '@shared/types';
import { formatRelativeDate } from '../../lib/utils';
import { EmptyState } from '../ui/EmptyState';
import { RoleBadge } from './RoleBadge';
import { Users } from 'lucide-react';

// trace:STORY-649 | ai:claude

interface RosterPanelProps {
  members: TeamMemberDto[];
}

export function RosterPanel({ members }: RosterPanelProps) {
  return (
    <div className="rounded-xl border border-edge bg-surface-alt p-6">
      <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-4">Roster</h3>
      {members.length === 0 ? (
        <EmptyState
          icon={<Users className="h-8 w-8" />}
          title="No team data yet"
          description="Register clones and roles with the CLI (see `aida team`) to populate the roster."
        />
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="text-left text-[11px] uppercase tracking-wider text-content-muted">
                <th className="pb-2 pr-4 font-medium">User</th>
                <th className="pb-2 pr-4 font-medium">Role</th>
                <th className="pb-2 pr-4 font-medium">Host(s)</th>
                <th className="pb-2 pr-4 font-medium">Last seen</th>
              </tr>
            </thead>
            <tbody>
              {members.map((member) => (
                <tr key={member.userId} className="border-t border-edge">
                  <td className="py-2.5 pr-4">
                    <div className="flex items-center gap-2">
                      {member.activeClaim ? (
                        <span
                          className="h-2 w-2 rounded-full bg-emerald-400 animate-pulse"
                          title={`Active now: ${member.activeClaim}`}
                        />
                      ) : (
                        <span className="h-2 w-2 rounded-full bg-gray-600" title="Idle" />
                      )}
                      <span className="font-medium text-content">{member.userId}</span>
                    </div>
                  </td>
                  <td className="py-2.5 pr-4">
                    <RoleBadge role={member.role} />
                  </td>
                  <td className="py-2.5 pr-4 text-content-secondary">
                    {member.hosts.length > 0 ? member.hosts.join(', ') : <span className="text-content-muted">—</span>}
                  </td>
                  <td className="py-2.5 pr-4 text-content-secondary">
                    {member.lastSeen ? (
                      formatRelativeDate(member.lastSeen)
                    ) : (
                      <span className="text-content-muted">—</span>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
