import { useMemo } from 'react';
import type { Requirement, RequirementStatus } from '@shared/types';
import { STATUS_CONFIG } from '../../lib/constants';
import { cn } from '../../lib/utils';
import { StatusChart } from '../dashboard/StatusChart';
import { EmptyState } from '../ui/EmptyState';
import { BarChart3 } from 'lucide-react';

// trace:STORY-649 | ai:claude

// Statuses that count as "done" for the per-assignee progress bar.
const DONE_STATUSES: RequirementStatus[] = ['Done', 'Completed'];

interface TeamBurndownProps {
  requirements: Requirement[];
}

export function TeamBurndown({ requirements }: TeamBurndownProps) {
  const stateful = useMemo(
    () =>
      requirements.filter(
        (r) => r.req_type !== 'Folder' && r.req_type !== 'Meta' && !r.archived,
      ),
    [requirements],
  );

  // Per-assignee completed/total roll-up for the assigned subset.
  const perAssignee = useMemo(() => {
    const groups = new Map<string, { total: number; done: number }>();
    for (const req of stateful) {
      if (!req.assignee || !req.assignee.trim()) continue;
      const entry = groups.get(req.assignee) ?? { total: 0, done: 0 };
      entry.total += 1;
      if (DONE_STATUSES.includes(req.status)) entry.done += 1;
      groups.set(req.assignee, entry);
    }
    return Array.from(groups.entries())
      .map(([assignee, v]) => ({ assignee, ...v }))
      .sort((a, b) => a.assignee.localeCompare(b.assignee));
  }, [stateful]);

  if (stateful.length === 0) {
    return (
      <div className="rounded-xl border border-edge bg-surface-alt p-6">
        <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-4">
          Team Burndown
        </h3>
        <EmptyState
          icon={<BarChart3 className="h-8 w-8" />}
          title="No requirements yet"
          description="Add requirements via the CLI to see the team's status breakdown."
        />
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <StatusChart requirements={stateful} />

      {perAssignee.length > 0 && (
        <div className="rounded-xl border border-edge bg-surface-alt p-6">
          <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-4">
            Progress by Assignee
          </h3>
          <div className="space-y-3">
            {perAssignee.map(({ assignee, total, done }) => {
              const pct = total > 0 ? Math.round((done / total) * 100) : 0;
              return (
                <div key={assignee}>
                  <div className="flex items-center justify-between mb-1 text-sm">
                    <span className="text-content truncate">{assignee}</span>
                    <span className="text-xs text-content-muted tabular-nums">
                      {done}/{total}
                    </span>
                  </div>
                  <div className="h-1.5 rounded-full bg-surface-hover overflow-hidden">
                    <div
                      className={cn('h-full rounded-full transition-all', STATUS_CONFIG.Completed.dot)}
                      style={{ width: `${pct}%` }}
                    />
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}
