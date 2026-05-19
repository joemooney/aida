import type { Requirement, RequirementStatus } from '@shared/types';
import { STATUS_ORDER, STATUS_CONFIG } from '../../lib/constants';

interface StatusChartProps {
  requirements: Requirement[];
}

export function StatusChart({ requirements }: StatusChartProps) {
  const total = requirements.length;
  if (total === 0) return null;

  const counts: Record<string, number> = {};
  for (const req of requirements) {
    counts[req.status] = (counts[req.status] ?? 0) + 1;
  }

  // Build conic-gradient segments
  // STORY-86: Done (lime-500) reads as "almost shipped" against
  // Completed's emerald-500 ("merged to main") — same hue family,
  // clearly distinct slice in the chart.
  const statusColors: Record<RequirementStatus, string> = {
    Draft: '#6b7280',
    Approved: '#3b82f6',
    Planned: '#8b5cf6',
    InProgress: '#f59e0b',
    // STORY-332: NeedsAttention — fuchsia, mirroring the CLI punt palette.
    NeedsAttention: '#d946ef',
    Done: '#84cc16',
    Completed: '#10b981',
    Rejected: '#ef4444',
  };

  let cumulative = 0;
  const segments: string[] = [];
  for (const status of STATUS_ORDER) {
    const count = counts[status] ?? 0;
    if (count === 0) continue;
    const start = (cumulative / total) * 360;
    cumulative += count;
    const end = (cumulative / total) * 360;
    segments.push(`${statusColors[status]} ${start}deg ${end}deg`);
  }

  const gradient = `conic-gradient(${segments.join(', ')})`;

  return (
    <div className="rounded-xl border border-edge bg-surface-alt p-6">
      <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-6">Status Distribution</h3>
      <div className="flex items-center gap-8">
        {/* Donut chart */}
        <div
          className="relative h-36 w-36 shrink-0 rounded-full"
          style={{ background: gradient }}
        >
          <div className="absolute inset-4 rounded-full bg-surface-alt flex items-center justify-center">
            <span className="text-2xl font-bold text-content">{total}</span>
          </div>
        </div>

        {/* Legend */}
        <div className="space-y-2.5">
          {STATUS_ORDER.map((status) => {
            const count = counts[status] ?? 0;
            if (count === 0) return null;
            const config = STATUS_CONFIG[status];
            return (
              <div key={status} className="flex items-center gap-2.5">
                <span className={`h-2.5 w-2.5 rounded-full ${config.dot}`} />
                <span className="text-sm text-content-secondary min-w-[80px]">{config.label}</span>
                <span className="text-sm font-semibold text-content">{count}</span>
                <span className="text-xs text-content-muted">({Math.round((count / total) * 100)}%)</span>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
