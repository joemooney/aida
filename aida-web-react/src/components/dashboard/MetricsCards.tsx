import { useNavigate } from 'react-router-dom';
import type { Requirement, RequirementStatus } from '@shared/types';
import { STATUS_CONFIG } from '../../lib/constants';

interface MetricsCardsProps {
  requirements: Requirement[];
}

export function MetricsCards({ requirements }: MetricsCardsProps) {
  const navigate = useNavigate();
  const total = requirements.length;
  const byStatus: Record<string, number> = {};
  for (const req of requirements) {
    byStatus[req.status] = (byStatus[req.status] ?? 0) + 1;
  }

  const cards: { label: string; count: number; dot: string; status: RequirementStatus | null }[] = [
    { label: 'Total', count: total, dot: 'bg-accent', status: null },
    ...(['InProgress', 'Approved', 'Completed', 'Draft'] as RequirementStatus[]).map((status) => ({
      label: STATUS_CONFIG[status].label,
      count: byStatus[status] ?? 0,
      dot: STATUS_CONFIG[status].dot,
      status,
    })),
  ];

  return (
    <div className="grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-5">
      {cards.map((card) => (
        <button
          key={card.label}
          type="button"
          onClick={() => navigate(card.status ? `/list?status=${card.status}` : '/list')}
          className="rounded-xl border border-edge bg-surface-alt p-4 transition-all hover:border-accent/50 hover:bg-surface-hover cursor-pointer text-left"
        >
          <div className="flex items-center gap-2 mb-2">
            <span className={`h-2 w-2 rounded-full ${card.dot}`} />
            <span className="text-xs font-medium uppercase tracking-wider text-content-muted">
              {card.label}
            </span>
          </div>
          <span className="text-2xl font-bold text-content">{card.count}</span>
        </button>
      ))}
    </div>
  );
}
