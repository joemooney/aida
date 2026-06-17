import type { CoordinationClaimDto } from '@shared/types';
import { cn } from '../../lib/utils';
import { EmptyState } from '../ui/EmptyState';
import { Lock } from 'lucide-react';

// trace:STORY-649 | ai:claude

interface CoordinationPanelProps {
  claims: CoordinationClaimDto[];
}

// Render seconds as a compact relative age ("3m", "2h", "5d").
function formatAge(ageSecs: bigint): string {
  const secs = Number(ageSecs);
  if (!Number.isFinite(secs) || secs < 60) return `${Math.max(0, secs)}s`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(secs / 3600);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(secs / 86400);
  return `${days}d`;
}

function kindConfig(kind: string): { bg: string; color: string; label: string } {
  switch (kind.toLowerCase()) {
    case 'lease':
      return { bg: 'bg-blue-500/10', color: 'text-blue-300', label: 'Lease' };
    case 'drain':
      return { bg: 'bg-amber-500/10', color: 'text-amber-300', label: 'Drain' };
    case 'solo':
      return { bg: 'bg-violet-500/10', color: 'text-violet-300', label: 'Solo' };
    default:
      return { bg: 'bg-slate-500/10', color: 'text-slate-300', label: kind || 'Claim' };
  }
}

export function CoordinationPanel({ claims }: CoordinationPanelProps) {
  return (
    <div className="rounded-xl border border-edge bg-surface-alt p-6">
      <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-4">
        Who Holds What
      </h3>
      {claims.length === 0 ? (
        <EmptyState
          icon={<Lock className="h-8 w-8" />}
          title="No active claims"
          description="No team data yet — leases, drains and solo locks appear here while held (see `aida team`)."
        />
      ) : (
        <ul className="space-y-2">
          {claims.map((claim, idx) => {
            const kc = kindConfig(claim.kind);
            return (
              <li
                key={`${claim.holderUser}-${claim.kind}-${claim.scope ?? idx}`}
                className="rounded-lg border border-edge bg-surface-raised p-3"
              >
                <div className="flex items-center justify-between gap-2 mb-1">
                  <div className="flex items-center gap-2 min-w-0">
                    <span
                      className={cn(
                        'inline-flex items-center rounded-md px-2 py-0.5 text-[11px] font-medium shrink-0',
                        kc.bg,
                        kc.color,
                      )}
                    >
                      {kc.label}
                    </span>
                    <span className="text-sm font-medium text-content truncate">
                      {claim.scope ?? kc.label}
                    </span>
                  </div>
                  <div className="flex items-center gap-2 shrink-0">
                    {claim.stale && (
                      <span className="inline-flex items-center rounded-md bg-red-500/10 px-2 py-0.5 text-[11px] font-medium text-red-400">
                        Stale
                      </span>
                    )}
                    <span className="text-xs text-content-muted tabular-nums">
                      {formatAge(claim.ageSecs)}
                    </span>
                  </div>
                </div>
                <div className="text-xs text-content-secondary">
                  {claim.holderUser}
                  <span className="text-content-muted"> on {claim.host}</span>
                  {claim.agent && <span className="text-content-muted"> · {claim.agent}</span>}
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
