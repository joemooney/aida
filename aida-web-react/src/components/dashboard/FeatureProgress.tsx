import type { Requirement } from '@shared/types';

interface FeatureProgressProps {
  requirements: Requirement[];
}

export function FeatureProgress({ requirements }: FeatureProgressProps) {
  const features = new Map<string, { total: number; completed: number }>();

  for (const req of requirements) {
    const f = req.feature || 'Uncategorized';
    const entry = features.get(f) ?? { total: 0, completed: 0 };
    entry.total++;
    if (req.status === 'Completed') entry.completed++;
    features.set(f, entry);
  }

  const sorted = [...features.entries()].sort((a, b) => b[1].total - a[1].total);

  if (sorted.length === 0) return null;

  return (
    <div className="rounded-xl border border-edge bg-surface-alt p-6">
      <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-6">Feature Progress</h3>
      <div className="space-y-4">
        {sorted.map(([name, { total, completed }]) => {
          const pct = total > 0 ? Math.round((completed / total) * 100) : 0;
          return (
            <div key={name}>
              <div className="flex items-center justify-between mb-1.5">
                <span className="text-sm font-medium text-content">{name}</span>
                <span className="text-xs text-content-muted">{completed}/{total} ({pct}%)</span>
              </div>
              <div className="h-2 rounded-full bg-surface-hover overflow-hidden">
                <div
                  className="h-full rounded-full bg-accent transition-all duration-500"
                  style={{ width: `${pct}%` }}
                />
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
