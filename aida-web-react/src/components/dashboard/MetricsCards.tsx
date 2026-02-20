import type { Requirement } from '@shared/types';
import { STATUS_CONFIG } from '../../lib/constants';
import type { RequirementStatus } from '@shared/types';

interface MetricsCardsProps {
  requirements: Requirement[];
}

export function MetricsCards({ requirements }: MetricsCardsProps) {
  const total = requirements.length;
  const byStatus: Record<string, number> = {};
  for (const req of requirements) {
    byStatus[req.status] = (byStatus[req.status] ?? 0) + 1;
  }

  const cards: { label: string; count: number; dot: string }[] = [
    { label: 'Total', count: total, dot: 'bg-accent' },
    ...(['InProgress', 'Approved', 'Completed', 'Draft'] as RequirementStatus[]).map((status) => ({
      label: STATUS_CONFIG[status].label,
      count: byStatus[status] ?? 0,
      dot: STATUS_CONFIG[status].dot,
    })),
  ];

  return (
    <div className="grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-5">
      {cards.map((card) => (
        <div
          key={card.label}
          className="rounded-xl border border-edge bg-surface-alt p-4 transition-all hover:border-edge-hover"
        >
          <div className="flex items-center gap-2 mb-2">
            <span className={`h-2 w-2 rounded-full ${card.dot}`} />
            <span className="text-xs font-medium uppercase tracking-wider text-content-muted">
              {card.label}
            </span>
          </div>
          <span className="text-2xl font-bold text-content">{card.count}</span>
        </div>
      ))}
    </div>
  );
}
