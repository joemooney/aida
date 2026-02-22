import type { ActivityStats } from '../../lib/activity-utils';

const statCards = [
  { key: 'workedOn', label: 'Worked On', color: 'text-blue-500' },
  { key: 'queueSize', label: 'In Queue', color: 'text-green-500' },
  { key: 'unqueuedWork', label: 'Unqueued Work', color: 'text-amber-500' },
  { key: 'queueUntouched', label: 'Queue Untouched', color: 'text-slate-400' },
] as const;

interface ActivityStatsBarProps {
  stats: ActivityStats;
}

export function ActivityStatsBar({ stats }: ActivityStatsBarProps) {
  return (
    <div className="flex gap-3">
      {statCards.map(({ key, label, color }) => (
        <div
          key={key}
          className="flex-1 rounded-xl border border-edge bg-surface p-4"
        >
          <div className={`text-2xl font-bold tabular-nums ${color}`}>
            {stats[key]}
          </div>
          <div className="text-xs text-content-muted mt-1">{label}</div>
        </div>
      ))}
    </div>
  );
}
