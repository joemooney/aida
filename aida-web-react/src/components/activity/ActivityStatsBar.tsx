import type { ActivityStats, StatusBreakdown } from '../../lib/activity-utils';

const breakdownItems: { key: keyof StatusBreakdown; label: string; color: string }[] = [
  { key: 'completed', label: 'Completed', color: 'bg-green-500/15 text-green-500' },
  { key: 'inProgress', label: 'In Progress', color: 'bg-blue-500/15 text-blue-500' },
  { key: 'approved', label: 'Approved', color: 'bg-violet-500/15 text-violet-500' },
  { key: 'created', label: 'Created', color: 'bg-emerald-500/15 text-emerald-500' },
  { key: 'commented', label: 'Commented', color: 'bg-amber-500/15 text-amber-500' },
  { key: 'other', label: 'Other', color: 'bg-slate-500/15 text-slate-400' },
];

interface ActivityStatsBarProps {
  stats: ActivityStats;
}

export function ActivityStatsBar({ stats }: ActivityStatsBarProps) {
  const bd = stats.statusBreakdown;

  return (
    <div className="flex gap-3">
      {/* Worked On — with breakdown */}
      <div className="flex-1 rounded-xl border border-edge bg-surface p-4">
        <div className="text-2xl font-bold tabular-nums text-blue-500">
          {stats.workedOn}
        </div>
        <div className="text-xs text-content-muted mt-1">Worked On</div>
        {stats.workedOn > 0 && (
          <div className="flex flex-wrap gap-1.5 mt-2">
            {breakdownItems.map(({ key, label, color }) =>
              bd[key] > 0 ? (
                <span key={key} className={`rounded-full px-1.5 py-0.5 text-[10px] font-medium ${color}`}>
                  {bd[key]} {label}
                </span>
              ) : null,
            )}
          </div>
        )}
      </div>

      {/* Completed — promoted to top-level */}
      <div className="flex-1 rounded-xl border border-edge bg-surface p-4">
        <div className="text-2xl font-bold tabular-nums text-green-500">
          {bd.completed}
        </div>
        <div className="text-xs text-content-muted mt-1">Completed</div>
      </div>

      {/* In Queue */}
      <div className="flex-1 rounded-xl border border-edge bg-surface p-4">
        <div className="text-2xl font-bold tabular-nums text-emerald-500">
          {stats.queueSize}
        </div>
        <div className="text-xs text-content-muted mt-1">In Queue</div>
      </div>

      {/* Unqueued Work */}
      <div className="flex-1 rounded-xl border border-edge bg-surface p-4">
        <div className="text-2xl font-bold tabular-nums text-amber-500">
          {stats.unqueuedWork}
        </div>
        <div className="text-xs text-content-muted mt-1">Unqueued Work</div>
      </div>

      {/* Queue Untouched */}
      <div className="flex-1 rounded-xl border border-edge bg-surface p-4">
        <div className="text-2xl font-bold tabular-nums text-slate-400">
          {stats.queueUntouched}
        </div>
        <div className="text-xs text-content-muted mt-1">Queue Untouched</div>
      </div>
    </div>
  );
}
