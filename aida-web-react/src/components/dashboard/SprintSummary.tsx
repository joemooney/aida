// trace:TASK-0052 | ai:claude
import { useNavigate } from 'react-router-dom';
import { CalendarDays } from 'lucide-react';
import type { Requirement, RequirementStatus } from '@shared/types';
import { STATUS_CONFIG } from '../../lib/constants';
import {
  getSprintNumber,
  getSprintDates,
  computeSprintProgress,
} from '../../lib/sprint-utils';

interface SprintSummaryProps {
  sprint: Requirement;
  items: Requirement[];
}

function buildSprintFilterUrl(sprintId: string, status?: RequirementStatus): string {
  const rules: { field: string; operator: string; value: string }[] = [
    { field: '_sprint', operator: '=', value: sprintId },
  ];
  if (status) {
    rules.push({ field: 'status', operator: '=', value: status });
  }
  const query = { combinator: 'and', rules };
  const aq = btoa(JSON.stringify(query));
  return `/list?aq=${aq}`;
}

export function SprintSummary({ sprint, items }: SprintSummaryProps) {
  const navigate = useNavigate();
  const num = getSprintNumber(sprint);
  const { start, end } = getSprintDates(sprint);
  const progress = computeSprintProgress(items);
  const sprintId = sprint.spec_id ?? sprint.id;

  let daysLeft: number | null = null;
  if (end) {
    const diff = Math.ceil((new Date(end).getTime() - Date.now()) / (1000 * 60 * 60 * 24));
    daysLeft = Math.max(0, diff);
  }

  const byStatus: Record<string, number> = {};
  for (const item of items) {
    byStatus[item.status] = (byStatus[item.status] ?? 0) + 1;
  }

  const cards: { label: string; count: number; dot: string; status: RequirementStatus | null }[] = [
    { label: 'Total', count: items.length, dot: 'bg-accent', status: null },
    ...(['InProgress', 'Approved', 'Completed', 'Draft'] as RequirementStatus[]).map((status) => ({
      label: STATUS_CONFIG[status].label,
      count: byStatus[status] ?? 0,
      dot: STATUS_CONFIG[status].dot,
      status,
    })),
  ];

  const sprintLabel = num != null ? `Sprint ${num}` : sprint.title;

  const formatDate = (d: string) =>
    new Date(d).toLocaleDateString('en-US', { month: 'short', day: 'numeric' });

  return (
    <div className="rounded-xl border border-edge bg-surface-alt p-5 space-y-4">
      {/* Header */}
      <div className="flex items-center justify-between flex-wrap gap-2">
        <div className="flex items-center gap-3">
          <h2 className="text-base font-semibold text-content">{sprintLabel}</h2>
          {start && end && (
            <span className="flex items-center gap-1.5 text-xs text-content-muted">
              <CalendarDays className="h-3.5 w-3.5" />
              {formatDate(start)} – {formatDate(end)}
            </span>
          )}
        </div>
        {daysLeft != null && (
          <span className={`text-xs font-medium px-2 py-0.5 rounded-full ${
            daysLeft <= 2 ? 'bg-red-500/10 text-red-400' : 'bg-accent/10 text-accent'
          }`}>
            {daysLeft === 0 ? 'Ends today' : `${daysLeft}d left`}
          </span>
        )}
      </div>

      {/* Status cards */}
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-5">
        {cards.map((card) => (
          <button
            key={card.label}
            type="button"
            onClick={() => navigate(buildSprintFilterUrl(sprintId, card.status ?? undefined))}
            className="rounded-lg border border-edge bg-surface p-3 transition-all hover:border-accent/50 hover:bg-surface-hover cursor-pointer text-left"
          >
            <div className="flex items-center gap-1.5 mb-1">
              <span className={`h-1.5 w-1.5 rounded-full ${card.dot}`} />
              <span className="text-[11px] font-medium uppercase tracking-wider text-content-muted">
                {card.label}
              </span>
            </div>
            <span className="text-xl font-bold text-content">{card.count}</span>
          </button>
        ))}
      </div>

      {/* Progress bar */}
      <div className="flex items-center gap-4">
        <div className="flex-1">
          <div className="h-2 rounded-full bg-surface-hover overflow-hidden">
            <div
              className="h-full rounded-full bg-accent transition-all"
              style={{ width: `${progress.percentage}%` }}
            />
          </div>
        </div>
        <span className="text-sm font-medium text-content tabular-nums shrink-0">
          {progress.percentage}%
        </span>
        {progress.totalPoints > 0 && (
          <span className="text-xs text-content-muted tabular-nums shrink-0">
            {progress.completedPoints}/{progress.totalPoints} pts
          </span>
        )}
      </div>
    </div>
  );
}
