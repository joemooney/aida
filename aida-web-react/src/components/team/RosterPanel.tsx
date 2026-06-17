import { useState } from 'react';
import type { TeamMemberDto } from '@shared/types';
import { formatRelativeDate } from '../../lib/utils';
import { EmptyState } from '../ui/EmptyState';
import { RoleBadge } from './RoleBadge';
import { Users, Info, X } from 'lucide-react';
import { useSetRole } from '../../hooks/useTeam';
import { usePermissions } from '../../hooks/usePermissions';

// trace:STORY-649 | ai:claude
// trace:STORY-651 | ai:claude
// Slice C2: the roster gains an inline role editor. Selecting a new role for a
// member calls PUT /api/v2/team/:user/role; the response's guardrail `caveat`
// is surfaced as an inline note. The control is gated behind write permission
// (the server enforces the real guardrail).

// Mirrors aida_core::team::core_role_names() — the roles a write is always
// allowed to record. trace:STORY-651 | ai:claude
const ROLE_OPTIONS = ['advisor', 'implementer', 'human'] as const;

interface RosterPanelProps {
  members: TeamMemberDto[];
}

interface RoleEditorProps {
  member: TeamMemberDto;
  canEdit: boolean;
  onCaveat: (caveat: string) => void;
}

function RoleEditor({ member, canEdit, onCaveat }: RoleEditorProps) {
  const setRole = useSetRole();

  if (!canEdit) {
    return <RoleBadge role={member.role} />;
  }

  const current = (member.role ?? '').trim().toLowerCase();
  const value = ROLE_OPTIONS.includes(current as (typeof ROLE_OPTIONS)[number])
    ? current
    : '';

  return (
    <div className="flex items-center gap-2">
      <select
        aria-label={`Role for ${member.displayLabel}`}
        value={value}
        disabled={setRole.isPending}
        onChange={(e) => {
          const role = e.target.value;
          if (!role) return;
          setRole.mutate(
            { user: member.userId, role },
            { onSuccess: (res) => onCaveat(res.caveat) },
          );
        }}
        className="rounded-md border border-edge bg-surface px-2 py-1 text-xs text-content focus:border-accent focus:outline-none disabled:opacity-50"
      >
        {value === '' && <option value="">No role</option>}
        {ROLE_OPTIONS.map((r) => (
          <option key={r} value={r}>
            {r.charAt(0).toUpperCase() + r.slice(1)}
          </option>
        ))}
      </select>
      {setRole.isError && (
        <span className="text-[11px] text-red-400" title={String(setRole.error)}>
          failed
        </span>
      )}
    </div>
  );
}

export function RosterPanel({ members }: RosterPanelProps) {
  const { canWrite } = usePermissions();
  const [caveat, setCaveat] = useState<string | null>(null);

  return (
    <div className="rounded-xl border border-edge bg-surface-alt p-6">
      <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-4">Roster</h3>

      {caveat && (
        <div className="mb-4 flex items-start gap-2 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-200">
          <Info className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <p className="flex-1">{caveat}</p>
          <button
            onClick={() => setCaveat(null)}
            className="shrink-0 rounded p-0.5 text-amber-300/70 hover:text-amber-200"
            aria-label="Dismiss"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
      )}

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
                      <div className="min-w-0">
                        <span
                          className="block truncate font-medium text-content"
                          title={member.userId}
                        >
                          {member.displayLabel}
                        </span>
                        {member.nodeNames.length > 0 && (
                          <span
                            className="block truncate text-[11px] text-content-muted"
                            title={member.nodeNames.join(', ')}
                          >
                            {member.nodeNames.join(', ')}
                          </span>
                        )}
                      </div>
                    </div>
                  </td>
                  <td className="py-2.5 pr-4">
                    <RoleEditor member={member} canEdit={canWrite} onCaveat={setCaveat} />
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
